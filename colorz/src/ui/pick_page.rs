//! The Pick tab: swatch, "copy as" target, format rows and recent picks.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use colorz_core::color::Color;
use colorz_core::format::Format;
use colorz_core::storage::{Config, TargetChoice};

use super::format_row::FormatRow;
use super::palette_actions::add_colors;
use super::palette_bar::PaletteBar;
use super::targets::{target_choices, target_labels};
use super::window::ColorzWindow;
use crate::state::{AppState, ColorSource, StateEvent};

/// The recent-picks strip never shows more than this many swatches.
const MAX_RECENT_SHOWN: usize = 20;
const SWATCH_RADIUS: f64 = 12.0;
const CHIP_SIZE: i32 = 28;

/// The formats to show rows for, in display order. The `color()` row needs a
/// target with a CSS name, which a custom ICC profile lacks.
fn visible_rows(cfg: &Config, target_has_css_name: bool) -> Vec<Format> {
    Format::ALL
        .into_iter()
        .filter(|f| cfg.visible_formats.contains(f))
        .filter(|f| *f != Format::CssColor || target_has_css_name)
        .collect()
}

pub fn hex_of(color: &Color) -> String {
    let [r, g, b] = color.to_srgb8();
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// Paints `rgb` as a rounded rectangle filling the area.
fn draw_swatch(cr: &gtk::cairo::Context, w: i32, h: i32, radius: f64, rgb: [u8; 3]) {
    use std::f64::consts::{FRAC_PI_2, PI};
    let (w, h) = (f64::from(w), f64::from(h));
    let r = radius.min(w / 2.0).min(h / 2.0);
    cr.new_sub_path();
    cr.arc(w - r, r, r, -FRAC_PI_2, 0.0);
    cr.arc(w - r, h - r, r, 0.0, FRAC_PI_2);
    cr.arc(r, h - r, r, FRAC_PI_2, PI);
    cr.arc(r, r, r, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
    let [red, green, blue] = rgb.map(|v| f64::from(v) / 255.0);
    cr.set_source_rgb(red, green, blue);
    let _ = cr.fill();
}

pub(super) fn swatch_area(
    size: Option<(i32, i32)>,
    radius: f64,
    rgb: Rc<Cell<[u8; 3]>>,
) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    if let Some((w, h)) = size {
        area.set_content_width(w);
        area.set_content_height(h);
    }
    area.set_draw_func(move |_, cr, w, h| draw_swatch(cr, w, h, radius, rgb.get()));
    area
}

struct Inner {
    state: AppState,
    window: ColorzWindow,
    swatch: gtk::DrawingArea,
    swatch_rgb: Rc<Cell<[u8; 3]>>,
    banner: adw::Banner,
    target_row: adw::ComboRow,
    target_choices: RefCell<Vec<TargetChoice>>,
    /// Set while the combo row is being updated from the config.
    syncing_target: Cell<bool>,
    last_target_error: RefCell<Option<String>>,
    rows: Vec<FormatRow>,
    recent_box: gtk::Box,
    recent: gtk::FlowBox,
    /// The history size the recent strip was last built for.
    recent_size: Cell<usize>,
    _bar: PaletteBar,
    weak_self: std::rc::Weak<Inner>,
}

/// The page widget plus the state that drives it. Keep the `PickPage`
/// alive as long as the page is shown: the state listener holds it weakly.
pub struct PickPage {
    widget: gtk::Widget,
    _inner: Rc<Inner>,
}

impl PickPage {
    pub fn widget(&self) -> &gtk::Widget {
        &self.widget
    }

    pub fn new(state: &AppState, window: &ColorzWindow) -> PickPage {
        let swatch_rgb = Rc::new(Cell::new(state.current().to_srgb8()));
        let swatch = swatch_area(None, SWATCH_RADIUS, swatch_rgb.clone());
        swatch.set_content_height(120);
        swatch.set_hexpand(true);

        let banner = adw::Banner::builder()
            .title("Correcting picks from Chrome")
            .button_label("Turn off")
            .build();
        let target_row = adw::ComboRow::builder()
            .title("Copy as")
            .subtitle("Color space for HEX, RGB, HSL and HSV")
            .build();
        let rows: Vec<FormatRow> = Format::ALL.into_iter().map(FormatRow::new).collect();
        let recent = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(1)
            .max_children_per_line(MAX_RECENT_SHOWN as u32)
            .row_spacing(4)
            .column_spacing(4)
            .build();
        let recent_title = gtk::Label::builder()
            .label("Recent picks")
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let recent_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        recent_box.append(&recent_title);
        recent_box.append(&recent);

        let bar = PaletteBar::new(state, window, true);
        bar.connect_add({
            let (state, window) = (state.clone(), window.clone());
            move || add_colors(&state, &window, vec![state.current()])
        });
        let bar_widget = bar.widget().clone();

        let inner = Rc::new_cyclic(|weak_self| Inner {
            state: state.clone(),
            window: window.clone(),
            swatch: swatch.clone(),
            swatch_rgb,
            banner: banner.clone(),
            target_row: target_row.clone(),
            target_choices: RefCell::default(),
            syncing_target: Cell::new(false),
            last_target_error: RefCell::default(),
            rows: rows.clone(),
            recent_box: recent_box.clone(),
            recent,
            recent_size: Cell::new(0),
            _bar: bar,
            weak_self: weak_self.clone(),
        });

        let weak = Rc::downgrade(&inner);
        banner.connect_button_clicked({
            let state = state.clone();
            move |_| state.update_config(|c| c.chrome_correction = false)
        });
        target_row.connect_selected_notify({
            let weak = weak.clone();
            move |_| {
                if let Some(i) = weak.upgrade() {
                    i.target_selected();
                }
            }
        });
        for row in &rows {
            let weak = weak.clone();
            row.connect_copy(move |text| {
                if let Some(i) = weak.upgrade() {
                    i.copy(text);
                }
            });
        }

        let formats = adw::PreferencesGroup::new();
        for row in &rows {
            formats.add(row.widget());
        }
        let target_group = adw::PreferencesGroup::new();
        target_group.add(&target_row);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        content.append(&swatch);
        content.append(&recent_box);
        content.append(&bar_widget);
        content.append(&target_group);
        content.append(&formats);
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&content)
            .build();
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.append(&banner);
        page.append(&scrolled);

        inner.refresh_all();
        inner.report_target_error();
        state.connect({
            let weak = Rc::downgrade(&inner);
            move |e| {
                if let Some(inner) = weak.upgrade() {
                    inner.on_event(e);
                }
            }
        });
        PickPage {
            widget: page.upcast(),
            _inner: inner,
        }
    }
}

impl Inner {
    fn on_event(&self, e: &StateEvent) {
        match e {
            StateEvent::Color(_) => self.refresh_color(),
            StateEvent::History => self.refresh_recent(),
            StateEvent::Config => {
                self.refresh_all();
                self.report_target_error();
            }
            StateEvent::Palettes
            | StateEvent::SaveFailed(_)
            | StateEvent::Editing
            | StateEvent::EditingLost => {}
        }
    }

    fn refresh_all(&self) {
        let cfg = self.state.config();
        self.banner.set_revealed(cfg.chrome_correction);
        self.refresh_target(&cfg);
        self.refresh_color();
        if self.recent_size.get() != cfg.history_size {
            self.refresh_recent();
        }
    }

    fn refresh_target(&self, cfg: &Config) {
        let choices = target_choices(cfg);
        self.syncing_target.set(true);
        if *self.target_choices.borrow() != choices {
            let labels = target_labels(&choices);
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            self.target_row
                .set_model(Some(&gtk::StringList::new(&labels)));
        }
        let selected = choices.iter().position(|c| *c == cfg.target).unwrap_or(0);
        self.target_row.set_selected(selected as u32);
        *self.target_choices.borrow_mut() = choices;
        self.syncing_target.set(false);
    }

    fn target_selected(&self) {
        if self.syncing_target.get() {
            return;
        }
        let choice = self
            .target_choices
            .borrow()
            .get(self.target_row.selected() as usize)
            .cloned();
        if let Some(choice) = choice
            && self.state.config().target != choice
        {
            self.state.update_config(|c| c.target = choice);
        }
    }

    /// Toasts a failure to load the ICC target, once per failure.
    fn report_target_error(&self) {
        let error = self.state.target_error();
        let changed = *self.last_target_error.borrow() != error;
        if changed && let Some(msg) = &error {
            self.window
                .show_toast(&format!("Cannot use ICC profile, using sRGB: {msg}"));
        }
        *self.last_target_error.borrow_mut() = error;
    }

    fn refresh_color(&self) {
        let color = self.state.current();
        self.swatch_rgb.set(color.to_srgb8());
        self.swatch.queue_draw();
        let cfg = self.state.config();
        let has_css_name = self.state.with_target(|t| t.css_name().is_some());
        let shown = visible_rows(&cfg, has_css_name);
        for row in &self.rows {
            let available = row.update(&self.state, &color);
            row.widget()
                .set_visible(available && shown.contains(&row.format()));
        }
    }

    fn refresh_recent(&self) {
        let cfg = self.state.config();
        let entries = self.state.history().entries;
        self.recent_size.set(cfg.history_size);
        let shown = entries.len().min(cfg.history_size).min(MAX_RECENT_SHOWN);
        while let Some(child) = self.recent.first_child() {
            self.recent.remove(&child);
        }
        for color in entries.into_iter().take(shown) {
            let rgb = Rc::new(Cell::new(color.to_srgb8()));
            let chip = swatch_area(Some((CHIP_SIZE, CHIP_SIZE)), 6.0, rgb);
            let button = gtk::Button::builder()
                .child(&chip)
                .css_classes(["swatch-chip"])
                .tooltip_text(hex_of(&color))
                .build();
            button.update_property(&[gtk::accessible::Property::Label(&format!(
                "Use {}",
                hex_of(&color)
            ))]);
            let weak = self.weak_self.clone();
            button.connect_clicked(move |_| {
                if let Some(i) = weak.upgrade() {
                    i.state.set_current(color, ColorSource::History);
                }
            });
            self.recent.append(&button);
        }
        self.recent_box.set_visible(shown > 0);
    }

    fn copy(&self, text: &str) {
        super::copy_with_toast(&self.window, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_visibility() {
        let cfg = Config {
            visible_formats: vec![Format::Hex, Format::CssColor, Format::Oklch],
            ..Config::default()
        };
        assert_eq!(
            visible_rows(&cfg, true),
            [Format::Hex, Format::Oklch, Format::CssColor]
        );
        // A custom ICC target has no CSS name, so color() is hidden.
        assert_eq!(visible_rows(&cfg, false), [Format::Hex, Format::Oklch]);
        let none = Config {
            visible_formats: vec![],
            ..Config::default()
        };
        assert!(visible_rows(&none, true).is_empty());
    }
}
