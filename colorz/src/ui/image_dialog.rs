//! Palette from an image: the dialog, its entry points (file chooser,
//! drop, paste) and the contain-fit geometry of its preview.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use colorz_core::color::Color;
use colorz_core::extract::{
    ExtractError, ExtractedColor, Preview, SampleImage, SortBy, extract, from_rgba,
    load_image_and_preview,
};
use colorz_core::palette::{Palette, PaletteColor};
use gtk::{gdk, gio, glib};

use super::copy_pair::CopyPair;
use super::copy_text;
use super::css_dialogs::open_palette;
use super::pick_page::swatch_area;
use super::window::ColorzWindow;
use crate::state::AppState;

const PREVIEW_MAX_SIDE: u32 = 1024;
const MIN_COLORS: f64 = 3.0;
const MAX_COLORS: f64 = 16.0;
const DEFAULT_COLORS: f64 = 6.0;
const SORTS: [(&str, SortBy); 3] = [
    ("Amount", SortBy::Amount),
    ("Lightness", SortBy::Lightness),
    ("Hue", SortBy::Hue),
];
/// Slider changes within this time make a single extraction.
const DEBOUNCE: Duration = Duration::from_millis(80);
const SWATCH_SIZE: i32 = 40;
const PASTED_NAME: &str = "Pasted image";
const READ_FAILED: &str = "Could not read image";

/// Where an image of `img_w` x `img_h` is drawn when fitted inside a
/// `widget_w` x `widget_h` widget without cropping or scaling past the
/// widget: `(x, y, width, height)`, centered.
pub fn picture_content_rect(
    widget_w: f64,
    widget_h: f64,
    img_w: f64,
    img_h: f64,
) -> (f64, f64, f64, f64) {
    if widget_w <= 0.0 || widget_h <= 0.0 || img_w <= 0.0 || img_h <= 0.0 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let scale = (widget_w / img_w).min(widget_h / img_h);
    let (w, h) = (img_w * scale, img_h * scale);
    ((widget_w - w) / 2.0, (widget_h - h) / 2.0, w, h)
}

/// One extracted color shown as a dot on the preview.
type Dot = ((f64, f64), [u8; 3]);

struct Imp {
    window: ColorzWindow,
    state: AppState,
    dialog: glib::WeakRef<adw::Dialog>,
    sample: Arc<SampleImage>,
    /// Bumped by every extraction request; older results are dropped.
    generation: Cell<u64>,
    results: RefCell<Vec<ExtractedColor>>,
    dots: Rc<RefCell<Vec<Dot>>>,
    dots_area: gtk::DrawingArea,
    scale: gtk::Scale,
    sort: adw::ToggleGroup,
    name_row: adw::EntryRow,
    flow: gtk::FlowBox,
    /// Copies are confirmed here: window toasts would sit under the dialog.
    toasts: adw::ToastOverlay,
    create: gtk::Button,
    /// Signalled once, when the first extraction is shown.
    ready: RefCell<Option<async_channel::Sender<()>>>,
}

impl Imp {
    fn sort_by(&self) -> SortBy {
        SORTS
            .get(self.sort.active() as usize)
            .map_or(SortBy::Amount, |s| s.1)
    }

    /// Re-runs the extraction after `delay`, unless a newer request
    /// arrives first.
    fn schedule(self: &Rc<Self>, delay: Duration) {
        let id = self.generation.get() + 1;
        self.generation.set(id);
        let k = self.scale.value().round().clamp(MIN_COLORS, MAX_COLORS) as usize;
        let sort = self.sort_by();
        let sample = self.sample.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            if !delay.is_zero() {
                glib::timeout_future(delay).await;
            }
            if weak.upgrade().is_none_or(|i| i.generation.get() != id) {
                return;
            }
            let Ok(colors) = gio::spawn_blocking(move || extract(&sample, k, sort)).await else {
                return;
            };
            if let Some(i) = weak.upgrade()
                && i.generation.get() == id
            {
                i.show(colors);
            }
        });
    }

    fn show(&self, colors: Vec<ExtractedColor>) {
        *self.dots.borrow_mut() = colors
            .iter()
            .map(|c| (c.position, c.color.to_srgb8()))
            .collect();
        self.dots_area.queue_draw();
        self.flow.remove_all();
        for c in &colors {
            self.flow.append(&self.swatch_row(&c.color));
        }
        self.create
            .set_sensitive(!colors.is_empty() && self.named());
        *self.results.borrow_mut() = colors;
        if let Some(tx) = self.ready.take() {
            let _ = tx.try_send(());
        }
    }

    fn swatch_row(&self, color: &Color) -> gtk::Box {
        let rgb = Rc::new(Cell::new(color.to_srgb8()));
        let swatch = swatch_area(Some((SWATCH_SIZE, SWATCH_SIZE)), 8.0, rgb);
        swatch.set_valign(gtk::Align::Center);
        let copy = CopyPair::new();
        let full = copy.update(&self.state, color, self.state.default_format());
        let label = gtk::Label::builder()
            .label(full.as_deref().unwrap_or_default())
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["mono-text"])
            .build();
        copy.connect_copy({
            let (window, toasts) = (self.window.clone(), self.toasts.downgrade());
            move |text| {
                if copy_text(&window, text)
                    && let Some(toasts) = toasts.upgrade()
                {
                    toasts.add_toast(adw::Toast::new(&format!("Copied {text}")));
                }
            }
        });
        let row = gtk::Box::builder()
            .spacing(8)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(4)
            .margin_end(4)
            .build();
        for w in [swatch.upcast_ref::<gtk::Widget>(), label.upcast_ref()] {
            row.append(w);
        }
        for w in copy.widgets() {
            row.append(&w);
        }
        row
    }

    fn named(&self) -> bool {
        !self.name_row.text().trim().is_empty()
    }

    fn create(&self) {
        let name = self.name_row.text();
        let name = name.trim();
        let results = self.results.borrow();
        if name.is_empty() || results.is_empty() {
            return;
        }
        let mut pal = Palette::new(name);
        for c in results.iter() {
            pal.push(PaletteColor {
                color: c.color,
                label: String::new(),
                var_name: String::new(),
            });
        }
        let id = pal.id.clone();
        self.state.upsert_palette(pal);
        if let Some(d) = self.dialog.upgrade() {
            d.close();
        }
        open_palette(&self.window, &id);
        self.window.show_toast("Palette created");
    }
}

fn draw_dots(cr: &gtk::cairo::Context, w: i32, h: i32, img: (u32, u32), dots: &[Dot]) {
    let (rx, ry, rw, rh) = picture_content_rect(
        f64::from(w),
        f64::from(h),
        f64::from(img.0),
        f64::from(img.1),
    );
    for ((px, py), rgb) in dots {
        let (x, y) = (rx + px * rw, ry + py * rh);
        cr.arc(x, y, 7.0, 0.0, std::f64::consts::TAU);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
        cr.set_line_width(5.0);
        let _ = cr.stroke_preserve();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_line_width(3.0);
        let _ = cr.stroke_preserve();
        let [r, g, b] = rgb.map(|c| f64::from(c) / 255.0);
        cr.set_source_rgb(r, g, b);
        let _ = cr.fill();
    }
}

fn texture_from_preview(p: &Preview) -> gdk::Texture {
    let bytes = glib::Bytes::from(&p.rgba[..]);
    gdk::MemoryTexture::new(
        p.width as i32,
        p.height as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        p.width as usize * 4,
    )
    .upcast()
}

fn build_dialog(
    window: &ColorzWindow,
    state: &AppState,
    sample: SampleImage,
    preview: &Preview,
    name: &str,
    ready: Option<async_channel::Sender<()>>,
) {
    let picture = gtk::Picture::builder()
        .paintable(&texture_from_preview(preview))
        .content_fit(gtk::ContentFit::Contain)
        .can_shrink(true)
        .hexpand(true)
        .vexpand(true)
        .build();
    picture.update_property(&[gtk::accessible::Property::Label("Image preview")]);
    let dots: Rc<RefCell<Vec<Dot>>> = Rc::default();
    let dots_area = gtk::DrawingArea::builder().can_target(false).build();
    let img_size = (preview.width, preview.height);
    dots_area.set_draw_func({
        let dots = dots.clone();
        move |_, cr, w, h| draw_dots(cr, w, h, img_size, &dots.borrow())
    });
    let overlay = gtk::Overlay::builder()
        .child(&picture)
        .height_request(200)
        .vexpand(true)
        .css_classes(["card"])
        .build();
    overlay.add_overlay(&dots_area);

    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, MIN_COLORS, MAX_COLORS, 1.0);
    scale.set_digits(0);
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    scale.set_value(DEFAULT_COLORS);
    scale.set_hexpand(true);
    scale.set_width_request(220);
    scale.update_property(&[gtk::accessible::Property::Label("Colors")]);
    let colors_row = adw::ActionRow::builder().title("Colors").build();
    colors_row.add_suffix(&scale);

    let sort = adw::ToggleGroup::builder()
        .valign(gtk::Align::Center)
        .build();
    for (label, _) in SORTS {
        sort.add(adw::Toggle::builder().label(label).build());
    }
    sort.set_active(0);
    let sort_row = adw::ActionRow::builder().title("Sort by").build();
    sort_row.add_suffix(&sort);

    let name_row = adw::EntryRow::builder().title("Name").text(name).build();
    let group = adw::PreferencesGroup::new();
    group.add(&colors_row);
    group.add(&sort_row);
    group.add(&name_row);

    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .max_children_per_line(2)
        .min_children_per_line(1)
        .homogeneous(true)
        .row_spacing(2)
        .column_spacing(8)
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&flow)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(120)
        .max_content_height(220)
        .propagate_natural_height(true)
        .build();

    let close = gtk::Button::builder()
        .label("_Close")
        .use_underline(true)
        .build();
    let create = gtk::Button::builder()
        .label("C_reate")
        .use_underline(true)
        .sensitive(false)
        .css_classes(["suggested-action"])
        .build();
    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .build();
    header.pack_start(&close);
    header.pack_end(&create);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&overlay);
    content.append(&group);
    content.append(&scrolled);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&toolbar));
    let dialog = adw::Dialog::builder()
        .title("Palette From Image")
        .content_width(640)
        .content_height(560)
        .child(&toasts)
        .default_widget(&create)
        .build();

    let imp = Rc::new(Imp {
        window: window.clone(),
        state: state.clone(),
        dialog: dialog.downgrade(),
        sample: Arc::new(sample),
        generation: Cell::new(0),
        results: RefCell::default(),
        dots,
        dots_area,
        scale: scale.clone(),
        sort: sort.clone(),
        name_row: name_row.clone(),
        flow,
        toasts,
        create: create.clone(),
        ready: RefCell::new(ready),
    });
    // The handlers hold `imp` weakly except for this one, which keeps it
    // alive until the dialog is gone.
    dialog.connect_closed({
        let keep = imp.clone();
        move |_| {
            // Results still in flight are dropped.
            keep.generation.set(keep.generation.get() + 1);
        }
    });
    let weak: Weak<Imp> = Rc::downgrade(&imp);

    close.connect_clicked({
        let dialog = dialog.downgrade();
        move |_| {
            if let Some(d) = dialog.upgrade() {
                d.close();
            }
        }
    });
    create.connect_clicked({
        let weak = weak.clone();
        move |_| {
            if let Some(i) = weak.upgrade() {
                i.create();
            }
        }
    });
    scale.connect_value_changed({
        let weak = weak.clone();
        move |_| {
            if let Some(i) = weak.upgrade() {
                i.schedule(DEBOUNCE);
            }
        }
    });
    sort.connect_active_notify({
        let weak = weak.clone();
        move |_| {
            if let Some(i) = weak.upgrade() {
                i.schedule(Duration::ZERO);
            }
        }
    });
    name_row.connect_changed({
        let weak = weak.clone();
        move |_| {
            if let Some(i) = weak.upgrade() {
                i.create
                    .set_sensitive(i.named() && !i.results.borrow().is_empty());
            }
        }
    });

    imp.schedule(Duration::ZERO);
    dialog.present(Some(window.window()));
}

type Decoded = Result<(SampleImage, Preview), ExtractError>;

/// Decodes on a worker thread, then opens the dialog; an unreadable image
/// only shows a toast.
fn open_with(
    window: &ColorzWindow,
    state: &AppState,
    name: String,
    ready: Option<async_channel::Sender<()>>,
    decode: impl FnOnce() -> Decoded + Send + 'static,
) {
    let (window, state) = (window.clone(), state.clone());
    glib::spawn_future_local(async move {
        match gio::spawn_blocking(decode).await {
            Ok(Ok((sample, preview))) => {
                build_dialog(&window, &state, sample, &preview, &name, ready);
            }
            _ => window.show_toast(READ_FAILED),
        }
    });
}

/// Opens the dialog for the image in `bytes`; `name` is the palette name
/// offered. An unreadable image only shows a toast.
pub fn open_image_dialog(window: &ColorzWindow, state: &AppState, bytes: Vec<u8>, name: String) {
    open_with(window, state, name, None, move || {
        load_image_and_preview(&bytes, PREVIEW_MAX_SIDE)
    });
}

/// Opens the dialog for a pasted image.
pub fn open_image_dialog_from_texture(
    window: &ColorzWindow,
    state: &AppState,
    texture: gdk::Texture,
) {
    let (width, height) = (texture.width(), texture.height());
    let mut downloader = gdk::TextureDownloader::new(&texture);
    downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = downloader.download_bytes();
    let data = bytes.to_vec();
    open_with(window, state, PASTED_NAME.to_owned(), None, move || {
        let (w, h) = (width.max(0) as usize, height.max(0) as usize);
        let row = w * 4;
        if stride < row || data.len() < stride * h.saturating_sub(1) + row {
            return Err(ExtractError::Empty);
        }
        let mut rgba = Vec::with_capacity(row * h);
        for y in 0..h {
            rgba.extend_from_slice(&data[y * stride..y * stride + row]);
        }
        from_rgba(w as u32, h as u32, rgba, PREVIEW_MAX_SIDE)
    });
}

/// Development aid: opens the dialog for `path` and waits (at most
/// `timeout`) until its first extraction is shown.
pub async fn debug_open(
    window: &ColorzWindow,
    state: &AppState,
    path: &std::path::Path,
    timeout: Duration,
) {
    let Ok(bytes) = std::fs::read(path) else {
        eprintln!("colorz: snapshot: cannot read {}", path.display());
        return;
    };
    let (tx, rx) = async_channel::bounded::<()>(1);
    let name = path
        .file_stem()
        .map_or_else(|| "Image".into(), |s| s.to_string_lossy().into_owned());
    open_with(window, state, name, Some(tx), move || {
        load_image_and_preview(&bytes, PREVIEW_MAX_SIDE)
    });
    let recv = std::pin::pin!(rx.recv());
    let timer = std::pin::pin!(glib::timeout_future(timeout));
    futures_util::future::select(recv, timer).await;
}

fn file_stem(file: &gio::File) -> Option<String> {
    let stem = file.basename()?.file_stem()?.to_string_lossy().into_owned();
    (!stem.trim().is_empty()).then_some(stem)
}

/// Reads `file` and opens the dialog for it.
async fn open_image_file(window: &ColorzWindow, state: &AppState, file: &gio::File) {
    let is_image = match file
        .query_info_future(
            "standard::content-type",
            gio::FileQueryInfoFlags::NONE,
            glib::Priority::DEFAULT,
        )
        .await
    {
        Ok(info) => info.content_type().is_some_and(|t| t.starts_with("image/")),
        // Let the decoder decide when the type is unknown.
        Err(_) => true,
    };
    if !is_image {
        window.show_toast(READ_FAILED);
        return;
    }
    match file.load_bytes_future().await {
        Ok((bytes, _)) => open_image_dialog(
            window,
            state,
            bytes.to_vec(),
            file_stem(file).unwrap_or_else(|| "Image".to_owned()),
        ),
        Err(_) => window.show_toast(READ_FAILED),
    }
}

/// Asks for an image file and opens the dialog for it.
pub fn pick_image_file(window: &ColorzWindow, state: &AppState) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Images"));
    filter.add_mime_type("image/*");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder()
        .title("Open Image")
        .modal(true)
        .filters(&filters)
        .default_filter(&filter)
        .build();
    let (window, state) = (window.clone(), state.clone());
    glib::spawn_future_local(async move {
        match dialog.open_future(Some(window.window())).await {
            Ok(file) => open_image_file(&window, &state, &file).await,
            Err(e) => {
                if !e.matches(gtk::DialogError::Dismissed)
                    && !e.matches(gio::IOErrorEnum::Cancelled)
                {
                    window.show_toast("Could not open the file chooser");
                }
            }
        }
    });
}

/// Makes dropping an image file on the window open the dialog.
pub fn install_drop_target(window: &ColorzWindow, state: &AppState) {
    let target = gtk::DropTarget::new(glib::Type::INVALID, gdk::DragAction::COPY);
    target.set_types(&[gdk::FileList::static_type(), gio::File::static_type()]);
    let (win, state) = (window.clone(), state.clone());
    target.connect_drop(move |_, value, _, _| {
        let Some(file) = value
            .get::<gdk::FileList>()
            .ok()
            .and_then(|l| l.files().into_iter().next())
            .or_else(|| value.get::<gio::File>().ok())
        else {
            return false;
        };
        let (win, state) = (win.clone(), state.clone());
        glib::spawn_future_local(async move {
            open_image_file(&win, &state, &file).await;
        });
        true
    });
    window.window().add_controller(target);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_rect_letterbox() {
        // A wide image in a square widget: bars above and below.
        let (x, y, w, h) = picture_content_rect(100.0, 100.0, 200.0, 100.0);
        assert_eq!((x, y, w, h), (0.0, 25.0, 100.0, 50.0));
        // A tall image in a wide widget: bars left and right.
        let (x, y, w, h) = picture_content_rect(200.0, 100.0, 50.0, 100.0);
        assert_eq!((x, y, w, h), (75.0, 0.0, 50.0, 100.0));
        // Small images are scaled up to fit.
        let (x, y, w, h) = picture_content_rect(100.0, 100.0, 10.0, 10.0);
        assert_eq!((x, y, w, h), (0.0, 0.0, 100.0, 100.0));
        // Degenerate sizes give an empty rect.
        assert_eq!(
            picture_content_rect(0.0, 10.0, 5.0, 5.0),
            (0.0, 0.0, 0.0, 0.0)
        );
    }
}
