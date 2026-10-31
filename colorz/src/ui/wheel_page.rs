//! The Wheel tab: a large wheel in one of three color models, a slider per
//! channel, a hex entry, copy buttons, a palette bar and harmonies
//! (markers on the wheel, and a card per harmony color in place of the hex
//! entry).

use std::cell::{Cell, RefCell};
use std::f64::consts::{FRAC_PI_2, PI};
use std::rc::Rc;

use adw::prelude::*;
use colorz_core::color::{Color, RgbSpace};
use colorz_core::format::{Form, Format};
use colorz_core::palette_ops::EditSession;
use colorz_core::parse::{ParseContext, parse_color};
use gtk::{glib, pango};

use super::channel_slider::ChannelSlider;
use super::copy_pair::CopyPair;
use super::palette_actions::add_colors;
use super::palette_bar::PaletteBar;
use super::pick_page::{hex_of, swatch_area};
use super::wheel_channels::{
    WheelMode, channel_specs, channels_from_color, color_from_channels, wrap_hue,
};
use super::wheel_widgets::WheelView;
use super::window::ColorzWindow;
use crate::state::{AppState, ColorSource, StateEvent};

/// Width of each half of the before/after swatch; its height follows the
/// harmony cards, which share its row.
const SWATCH_WIDTH: i32 = 44;
const SWATCH_RADIUS: f64 = 8.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Harmony {
    #[default]
    None,
    Complementary,
    Analogous,
    Triad,
    SplitComplementary,
}

impl Harmony {
    const ALL: [Harmony; 5] = [
        Harmony::None,
        Harmony::Complementary,
        Harmony::Analogous,
        Harmony::Triad,
        Harmony::SplitComplementary,
    ];

    fn label(self) -> &'static str {
        match self {
            Harmony::None => "None",
            Harmony::Complementary => "Complementary",
            Harmony::Analogous => "Analogous",
            Harmony::Triad => "Triad",
            Harmony::SplitComplementary => "Split-complementary",
        }
    }
}

/// The most colors a harmony has, the current one included.
const MAX_HARMONY_COLORS: usize = 3;

/// The color the swatch's "before" half shows: the edited color's original
/// during an edit session, else the color when the page was entered.
fn before_color(session: Option<&EditSession>, entered: Color) -> Color {
    session.map_or(entered, |s| s.original)
}

/// The hue offsets of a `kind` harmony's colors in the order they are shown
/// and added to palettes; `0` is the current color.
fn harmony_offsets(kind: Harmony) -> &'static [f64] {
    match kind {
        Harmony::None => &[0.0],
        Harmony::Complementary => &[0.0, 180.0],
        Harmony::Analogous => &[-30.0, 0.0, 30.0],
        Harmony::Triad => &[0.0, 120.0, 240.0],
        Harmony::SplitComplementary => &[0.0, 150.0, 210.0],
    }
}

/// The hues, in `[0, 360)`, that go with hue `h` in a `kind` harmony.
pub fn harmony_hues(kind: Harmony, h: f64) -> Vec<f64> {
    harmony_offsets(kind)
        .iter()
        .filter(|o| **o != 0.0)
        .map(|o| wrap_hue(h + o))
        .collect()
}

/// A `kind` harmony's colors in card order: `current` itself, and the
/// others as the wheel's channels `ch` turned to their hue.
fn harmony_colors(kind: Harmony, mode: WheelMode, ch: [f64; 3], current: Color) -> Vec<Color> {
    let hue = mode.hue_index();
    harmony_offsets(kind)
        .iter()
        .map(|&o| {
            if o == 0.0 {
                current
            } else {
                let mut c = ch;
                c[hue] = wrap_hue(ch[hue] + o);
                color_from_channels(mode, c)
            }
        })
        .collect()
}

/// Height of the harmony cards' color area.
const CARD_SWATCH_HEIGHT: i32 = 32;

/// One harmony color: a swatch with its hex on it, over the copy buttons.
struct HarmonyCard {
    root: gtk::Box,
    area: gtk::DrawingArea,
    rgb: Rc<Cell<[u8; 3]>>,
    label: gtk::Label,
    copy: CopyPair,
}

impl HarmonyCard {
    fn new(window: &ColorzWindow) -> HarmonyCard {
        let rgb = Rc::new(Cell::new([0; 3]));
        // The card clips the swatch to its rounded corners.
        let area = swatch_area(None, 0.0, rgb.clone());
        area.set_content_height(CARD_SWATCH_HEIGHT);
        area.set_hexpand(true);
        let label = gtk::Label::builder()
            .css_classes(["mono-text", "caption"])
            .justify(gtk::Justification::Center)
            .ellipsize(pango::EllipsizeMode::End)
            .can_target(false)
            .build();
        let overlay = gtk::Overlay::builder().child(&area).build();
        overlay.add_overlay(&label);
        // The target badge does not fit three cards wide; the target's
        // name shows under the hex instead.
        let copy = CopyPair::new();
        copy.connect_copy({
            let window = window.clone();
            move |text| super::copy_with_toast(&window, text)
        });
        let bar = gtk::Box::builder()
            .spacing(2)
            .halign(gtk::Align::Center)
            .margin_top(2)
            .margin_bottom(2)
            .build();
        let [_badge, warning, full, numbers] = copy.widgets();
        for w in [warning, full, numbers] {
            bar.append(&w);
        }
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .overflow(gtk::Overflow::Hidden)
            .css_classes(["card", "harmony-card"])
            .build();
        root.append(&overlay);
        root.append(&bar);
        HarmonyCard {
            root,
            area,
            rgb,
            label,
            copy,
        }
    }

    fn show(&self, state: &AppState, c: Color, current: bool) {
        self.rgb.set(c.to_srgb8());
        self.area.queue_draw();
        let hex = state
            .format(&c, Format::Hex, Form::Full)
            .map_or_else(|| hex_of(&c), |f| f.text);
        let (target, is_srgb) = state.with_target(|t| (t.name(), t.is_srgb()));
        // Dark text on light colors.
        let ink = if c.l > 0.65 { "#000000cc" } else { "#ffffffe6" };
        let mut markup = format!(
            "<span foreground=\"{ink}\">{}",
            glib::markup_escape_text(&hex)
        );
        if !is_srgb {
            markup.push_str(&format!(
                "\n<small>{}</small>",
                glib::markup_escape_text(&target)
            ));
        }
        markup.push_str("</span>");
        self.label.set_markup(&markup);
        let what = if current {
            "Current color"
        } else {
            "Harmony color"
        };
        self.root.set_tooltip_text(Some(what));
        self.copy.update(state, &c, state.default_format());
        if current {
            self.root.add_css_class("current");
        } else {
            self.root.remove_css_class("current");
        }
    }
}

fn mode_name(mode: WheelMode) -> &'static str {
    match mode {
        WheelMode::Ring => "ring",
        WheelMode::Oklch => "oklch",
        WheelMode::Okhsl => "okhsl",
    }
}

/// Paints one half of the before/after swatch, rounded on its outer side.
fn draw_half(cr: &gtk::cairo::Context, w: i32, h: i32, rgb: [u8; 3], left: bool) {
    let (w, h) = (f64::from(w), f64::from(h));
    let r = SWATCH_RADIUS.min(h / 2.0).min(w);
    cr.new_sub_path();
    if left {
        cr.arc(r, r, r, PI, 3.0 * FRAC_PI_2);
        cr.line_to(w, 0.0);
        cr.line_to(w, h);
        cr.arc(r, h - r, r, FRAC_PI_2, PI);
    } else {
        cr.move_to(0.0, 0.0);
        cr.arc(w - r, r, r, -FRAC_PI_2, 0.0);
        cr.arc(w - r, h - r, r, 0.0, FRAC_PI_2);
        cr.line_to(0.0, h);
    }
    cr.close_path();
    let [red, green, blue] = rgb.map(|v| f64::from(v) / 255.0);
    cr.set_source_rgb(red, green, blue);
    let _ = cr.fill();
}

fn half_swatch(rgb: Rc<Cell<[u8; 3]>>, left: bool) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(SWATCH_WIDTH)
        .build();
    area.set_draw_func(move |_, cr, w, h| draw_half(cr, w, h, rgb.get(), left));
    area
}

/// List items as plain labels; `ellipsize` lets the drop-down button
/// shrink in a narrow window.
fn label_factory(ellipsize: bool) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let label = gtk::Label::builder().xalign(0.0).build();
            if ellipsize {
                label.set_ellipsize(pango::EllipsizeMode::End);
            }
            item.set_child(Some(&label));
        }
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = item.child().and_downcast::<gtk::Label>();
        let text = item.item().and_downcast::<gtk::StringObject>();
        if let (Some(label), Some(text)) = (label, text) {
            label.set_label(&text.string());
        }
    });
    factory
}

struct Inner {
    state: AppState,
    window: ColorzWindow,
    view: WheelView,
    mode: Cell<WheelMode>,
    /// Channels per mode (indexed by `WheelMode as usize`), so each mode
    /// keeps its own hue through grays.
    channels: Cell<[[f64; 3]; 3]>,
    sliders: [[ChannelSlider; 3]; 3],
    slider_boxes: [gtk::Box; 3],
    harmony: Cell<Harmony>,
    /// The color when the page was last entered or set from elsewhere.
    before: Cell<Color>,
    before_rgb: Rc<Cell<[u8; 3]>>,
    after_rgb: Rc<Cell<[u8; 3]>>,
    before_area: gtk::DrawingArea,
    after_area: gtk::DrawingArea,
    hex: gtk::Entry,
    /// Copies the default format; its badge also labels the hex entry.
    copy: CopyPair,
    out_of_gamut: gtk::Box,
    bar: PaletteBar,
    /// Shows the hex row, or the harmony cards while a harmony is chosen.
    value_stack: gtk::Stack,
    cards: Vec<HarmonyCard>,
    /// The colors the cards show, in order (empty with no harmony).
    card_colors: RefCell<Vec<Color>>,
}

/// The page widget plus the state that drives it. Keep the `WheelPage`
/// alive as long as the page is shown: its listeners hold it weakly.
pub struct WheelPage {
    widget: gtk::Widget,
    _inner: Rc<Inner>,
}

impl WheelPage {
    pub fn widget(&self) -> &gtk::Widget {
        &self.widget
    }

    pub fn new(state: &AppState, window: &ColorzWindow) -> WheelPage {
        let toggles = adw::ToggleGroup::new();
        for (mode, label) in WheelMode::ALL.into_iter().zip(["Ring", "OKLCH", "OKHSL"]) {
            toggles.add(
                adw::Toggle::builder()
                    .name(mode_name(mode))
                    .label(label)
                    .build(),
            );
        }
        toggles.set_active_name(Some(mode_name(WheelMode::Ring)));
        toggles.set_halign(gtk::Align::Center);
        toggles.set_margin_top(12);

        let view = WheelView::new();
        // Floats over the wheel's top-left corner, so showing it never
        // moves or resizes anything while a drag crosses the gamut edge.
        let out_of_gamut = gtk::Box::builder()
            .spacing(4)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .css_classes(["warning", "oog-indicator"])
            .tooltip_text("Outside the sRGB gamut \u{2014} shown mapped to sRGB")
            .visible(false)
            .build();
        out_of_gamut.append(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
        out_of_gamut.append(&gtk::Label::new(Some("Outside sRGB")));
        let wheel_area = gtk::Overlay::builder()
            .child(&view)
            .margin_top(6)
            .margin_start(12)
            .margin_end(12)
            .build();
        wheel_area.add_overlay(&out_of_gamut);

        let current = state.current();
        let before_rgb = Rc::new(Cell::new(current.to_srgb8()));
        let after_rgb = Rc::new(Cell::new(current.to_srgb8()));
        let before_area = half_swatch(before_rgb.clone(), true);
        let after_area = half_swatch(after_rgb.clone(), false);
        let before_button = gtk::Button::builder()
            .child(&before_area)
            .css_classes(["flat", "before-swatch"])
            .tooltip_text("Restore previous color")
            .build();
        before_button.set_cursor_from_name(Some("pointer"));
        before_button
            .update_property(&[gtk::accessible::Property::Label("Restore previous color")]);
        // The gap tells the "before" half from the "after" half.
        let swatches = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        swatches.set_valign(gtk::Align::Fill);
        swatches.append(&before_button);
        swatches.append(&after_area);
        let hex = gtk::Entry::builder()
            .hexpand(true)
            .width_chars(9)
            .css_classes(["mono-entry"])
            .valign(gtk::Align::Center)
            .build();
        hex.update_property(&[gtk::accessible::Property::Label("Hex color")]);
        let copy = CopyPair::new().labels_hex();
        let value_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        value_row.append(&swatches);
        value_row.append(&hex);
        for w in copy.widgets() {
            value_row.append(&w);
        }
        let cards: Vec<HarmonyCard> = (0..MAX_HARMONY_COLORS)
            .map(|_| HarmonyCard::new(window))
            .collect();
        let cards_row = gtk::Box::builder()
            .spacing(6)
            .homogeneous(true)
            .valign(gtk::Align::Center)
            // Room for the current card's outline, which the stack would
            // otherwise clip.
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(4)
            .margin_end(4)
            .build();
        for card in &cards {
            cards_row.append(&card.root);
        }
        let value_stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        value_stack.add_named(&value_row, Some("single"));
        value_stack.add_named(&cards_row, Some("cards"));

        let sliders = WheelMode::ALL.map(|mode| {
            channel_specs(mode).map(|s| ChannelSlider::new(s.name, s.min, s.max, s.step, s.digits))
        });
        let slider_boxes = sliders.each_ref().map(|row| {
            let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
            for s in row {
                b.append(s.widget());
            }
            b
        });
        let sliders_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        for b in &slider_boxes {
            sliders_box.append(b);
        }

        let bar = PaletteBar::new(state, window, false);
        let labels: Vec<&str> = Harmony::ALL.iter().map(|h| h.label()).collect();
        let harmony = gtk::DropDown::from_strings(&labels);
        harmony.set_factory(Some(&label_factory(true)));
        harmony.set_list_factory(Some(&label_factory(false)));
        harmony.set_tooltip_text(Some("Harmony"));
        harmony.update_property(&[gtk::accessible::Property::Label("Harmony")]);
        let harmony_label = gtk::Label::builder()
            .label("Harmony")
            .css_classes(["dim-label"])
            .hexpand(true)
            .xalign(1.0)
            .build();
        let harmony_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        harmony_row.append(&harmony_label);
        harmony_row.append(&harmony);

        let controls = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(6)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        controls.append(&value_stack);
        controls.append(&sliders_box);
        controls.append(bar.widget());
        controls.append(&harmony_row);
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.append(&toggles);
        page.append(&wheel_area);
        page.append(&controls);
        // Scrolls when the window is too short for everything; the wheel
        // still takes the free space when there is room.
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&page)
            .build();

        let inner = Rc::new(Inner {
            state: state.clone(),
            window: window.clone(),
            view: view.clone(),
            mode: Cell::new(WheelMode::Ring),
            channels: Cell::new([[0.0; 3]; 3]),
            sliders,
            slider_boxes,
            harmony: Cell::new(Harmony::None),
            before: Cell::new(current),
            before_rgb,
            after_rgb,
            before_area,
            after_area,
            hex: hex.clone(),
            copy: copy.clone(),
            out_of_gamut,
            bar,
            value_stack,
            cards,
            card_colors: RefCell::default(),
        });

        let weak = Rc::downgrade(&inner);
        let with = move |f: fn(&Inner)| {
            let weak = weak.clone();
            move || {
                if let Some(i) = weak.upgrade() {
                    f(&i);
                }
            }
        };
        let weak = Rc::downgrade(&inner);
        toggles.connect_active_name_notify({
            let weak = weak.clone();
            move |t| {
                let name = t.active_name();
                let mode = WheelMode::ALL
                    .into_iter()
                    .find(|m| name.as_deref() == Some(mode_name(*m)));
                if let (Some(i), Some(mode)) = (weak.upgrade(), mode) {
                    i.set_mode(mode);
                }
            }
        });
        view.connect_changed({
            let weak = weak.clone();
            move |ch| {
                if let Some(i) = weak.upgrade() {
                    i.edit(ch);
                }
            }
        });
        for (mode, row) in WheelMode::ALL.into_iter().zip(&inner.sliders) {
            for (idx, slider) in row.iter().enumerate() {
                let weak = weak.clone();
                slider.connect_changed(move |v| {
                    if let Some(i) = weak.upgrade() {
                        i.edit_channel(mode, idx, v);
                    }
                });
            }
        }
        hex.connect_activate({
            let f = with(Inner::hex_entered);
            move |_| f()
        });
        hex.connect_changed(|e| e.remove_css_class("error"));
        // Text that was typed but not entered is dropped on focus-out.
        let hex_focus = gtk::EventControllerFocus::new();
        hex_focus.connect_leave({
            let f = with(Inner::refresh_outputs);
            move |_| f()
        });
        hex.add_controller(hex_focus);
        copy.connect_copy({
            let window = window.clone();
            move |text| super::copy_with_toast(&window, text)
        });
        before_button.connect_clicked({
            let f = with(Inner::restore_before);
            move |_| f()
        });
        harmony.connect_selected_notify({
            let weak = weak.clone();
            move |d| {
                let kind = Harmony::ALL.get(d.selected() as usize).copied();
                if let (Some(i), Some(kind)) = (weak.upgrade(), kind) {
                    i.set_harmony(kind);
                }
            }
        });
        window.stack().connect_visible_child_name_notify({
            let f = with(Inner::entered);
            move |s| {
                if s.visible_child_name().as_deref() == Some("wheel") {
                    f();
                }
            }
        });

        inner.bar.connect_add({
            let weak = weak.clone();
            move || {
                if let Some(i) = weak.upgrade() {
                    add_colors(&i.state, &i.window, i.palette_colors());
                }
            }
        });

        for (i, b) in inner.slider_boxes.iter().enumerate() {
            b.set_visible(i == WheelMode::Ring as usize);
        }
        inner.load_current();
        inner.refresh_outputs();
        state.connect(move |e| {
            if let Some(i) = weak.upgrade() {
                i.on_event(e);
            }
        });
        WheelPage {
            widget: scroller.upcast(),
            _inner: inner,
        }
    }
}

impl Inner {
    fn on_event(&self, e: &StateEvent) {
        match e {
            // Our own edit: the channels are already up to date.
            StateEvent::Color(ColorSource::Wheel) => self.refresh_outputs(),
            StateEvent::Color(source) => {
                // The hex entry is an edit on this page, not a new start.
                if *source != ColorSource::Hex {
                    self.before.set(self.state.current());
                }
                self.load_current();
                self.refresh_outputs();
            }
            StateEvent::Config => self.refresh_outputs(),
            StateEvent::Editing | StateEvent::EditingLost => self.refresh_outputs(),
            StateEvent::Palettes | StateEvent::History | StateEvent::SaveFailed(_) => {}
        }
    }

    fn channels(&self) -> [f64; 3] {
        self.channels.get()[self.mode.get() as usize]
    }

    fn set_mode(&self, mode: WheelMode) {
        if self.mode.replace(mode) == mode {
            return;
        }
        self.view.set_mode(mode);
        for (i, b) in self.slider_boxes.iter().enumerate() {
            b.set_visible(i == mode as usize);
        }
        self.load_current();
        self.refresh_outputs();
    }

    /// Takes the channels from the current color.
    fn load_current(&self) {
        let mode = self.mode.get();
        self.show_channels(channels_from_color(
            mode,
            &self.state.current(),
            self.channels(),
        ));
    }

    /// Stores `ch` for the active mode and shows it on the wheel and the
    /// sliders.
    fn show_channels(&self, ch: [f64; 3]) {
        let mode = self.mode.get();
        let mut all = self.channels.get();
        all[mode as usize] = ch;
        self.channels.set(all);
        self.view.set_channels(ch);
        for (i, slider) in self.sliders[mode as usize].iter().enumerate() {
            slider.set_value(ch[i]);
            slider.set_gradient(move |v| {
                let mut c = ch;
                c[i] = v;
                color_from_channels(mode, c).to_srgb8()
            });
        }
        self.view
            .set_harmony_hues(harmony_hues(self.harmony.get(), ch[mode.hue_index()]));
    }

    fn edit(&self, ch: [f64; 3]) {
        self.show_channels(ch);
        let color = color_from_channels(self.mode.get(), ch);
        self.state.set_current(color, ColorSource::Wheel);
    }

    fn edit_channel(&self, mode: WheelMode, idx: usize, v: f64) {
        if mode != self.mode.get() {
            return;
        }
        let mut ch = self.channels();
        ch[idx] = v;
        self.edit(ch);
    }

    /// Updates everything that shows the current color but not the
    /// channels.
    fn refresh_outputs(&self) {
        let color = self.state.current();
        let before = before_color(self.state.editing().as_ref(), self.before.get());
        self.before_rgb.set(before.to_srgb8());
        self.after_rgb.set(color.to_srgb8());
        self.before_area.queue_draw();
        self.after_area.queue_draw();

        let hex = self
            .state
            .format(&color, Format::Hex, Form::Full)
            .map_or_else(|| hex_of(&color), |f| f.text);
        if self.hex.text() != hex {
            self.hex.set_text(&hex);
        }
        self.hex.remove_css_class("error");

        self.copy
            .update(&self.state, &color, self.state.default_format());

        let outside = self.mode.get() == WheelMode::Oklch && !color.in_gamut(RgbSpace::Srgb);
        self.out_of_gamut.set_visible(outside);
        self.refresh_cards();
    }

    /// Shows a card per harmony color, or the hex row with no harmony.
    fn refresh_cards(&self) {
        let kind = self.harmony.get();
        if kind == Harmony::None {
            self.card_colors.borrow_mut().clear();
            self.value_stack.set_visible_child_name("single");
            return;
        }
        let current = self.state.current();
        let colors = harmony_colors(kind, self.mode.get(), self.channels(), current);
        for (i, card) in self.cards.iter().enumerate() {
            match colors.get(i) {
                Some(c) => {
                    let is_current = harmony_offsets(kind)[i] == 0.0;
                    card.show(&self.state, *c, is_current);
                    card.root.set_visible(true);
                }
                None => card.root.set_visible(false),
            }
        }
        *self.card_colors.borrow_mut() = colors;
        self.value_stack.set_visible_child_name("cards");
    }

    fn entered(&self) {
        self.before.set(self.state.current());
        self.refresh_outputs();
    }

    fn restore_before(&self) {
        let before = before_color(self.state.editing().as_ref(), self.before.get());
        let mode = self.mode.get();
        self.show_channels(channels_from_color(mode, &before, self.channels()));
        self.state.set_current(before, ColorSource::Wheel);
    }

    fn hex_entered(&self) {
        let text = self.hex.text();
        let default_format = self.state.config().default_format;
        let parsed = self.state.with_target(|target| {
            parse_color(
                &text,
                &ParseContext {
                    default_format,
                    target,
                },
            )
        });
        match parsed {
            Ok(color) => self.state.set_current(color, ColorSource::Hex),
            Err(_) => self.hex.add_css_class("error"),
        }
    }

    fn set_harmony(&self, kind: Harmony) {
        self.harmony.set(kind);
        let ch = self.channels();
        self.view
            .set_harmony_hues(harmony_hues(kind, ch[self.mode.get().hue_index()]));
        self.refresh_cards();
    }

    /// What the bar's Add adds: the harmony's colors, or the current color
    /// with no harmony.
    fn palette_colors(&self) -> Vec<Color> {
        let cards = self.card_colors.borrow();
        if cards.is_empty() {
            vec![self.state.current()]
        } else {
            cards.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use colorz_core::palette_ops::EditSession;

    #[test]
    fn harmony_hues_wrap() {
        assert_eq!(harmony_hues(Harmony::Complementary, 300.0), [120.0]);
        assert!(harmony_hues(Harmony::None, 10.0).is_empty());
        assert_eq!(harmony_hues(Harmony::Analogous, 10.0), [340.0, 40.0]);
        assert_eq!(harmony_hues(Harmony::Triad, 300.0), [60.0, 180.0]);
        assert_eq!(
            harmony_hues(Harmony::SplitComplementary, 300.0),
            [90.0, 150.0]
        );
        // Never 360.
        assert_eq!(harmony_hues(Harmony::Complementary, 180.0), [0.0]);
    }

    #[test]
    fn before_is_original_while_editing() {
        let entered = Color::from_srgb8([1, 2, 3]);
        let original = Color::from_srgb8([9, 8, 7]);
        let session = EditSession {
            palette_id: "p".into(),
            index: 0,
            original,
        };
        assert_eq!(before_color(Some(&session), entered), original);
        assert_eq!(before_color(None, entered), entered);
    }

    #[test]
    fn harmony_colors_in_card_order() {
        let current = Color::from_srgb8([0x34, 0x78, 0xF6]);
        let mode = WheelMode::Okhsl;
        let ch = channels_from_color(mode, &current, [0.0; 3]);
        let hue_of = |c: &Color| channels_from_color(mode, c, ch)[mode.hue_index()];
        let near = |a: f64, b: f64| (wrap_hue(a - b + 180.0) - 180.0).abs() < 0.5;
        for kind in Harmony::ALL {
            let colors = harmony_colors(kind, mode, ch, current);
            assert_eq!(colors.len(), harmony_offsets(kind).len());
            assert!(colors.len() <= MAX_HARMONY_COLORS);
            for (c, o) in colors.iter().zip(harmony_offsets(kind)) {
                if *o == 0.0 {
                    assert_eq!(*c, current, "{kind:?}");
                } else {
                    assert!(near(hue_of(c), ch[mode.hue_index()] + o), "{kind:?} {o}");
                }
            }
        }
        // Analogous puts the current color between its neighbors.
        assert_eq!(
            harmony_colors(Harmony::Analogous, mode, ch, current)[1],
            current
        );
        assert_eq!(harmony_colors(Harmony::None, mode, ch, current), [current]);
    }
}
