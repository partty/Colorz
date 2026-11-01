//! The palette bar: active palette, its colors as chips, and Add.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use colorz_core::color::Color;
use colorz_core::palette::Palette;
use colorz_core::palette_ops::{EditSession, color_name, palette_display_name};
use gtk::{gio, glib};

use super::NEW_PALETTE_NAME;
use super::palette_actions::{auto_add_label, edit_on_wheel, menu_label, palettes_last_used_first};
use super::pick_page::swatch_area;
use super::window::ColorzWindow;
use crate::platform::CAN_PICK;
use crate::state::{AppState, StateEvent};

const CHIP_SIZE: i32 = 30;
const CHIP_RADIUS: f64 = 7.0;
const CHIP_SPACING: i32 = 6;
/// Room under the chips for the overlay scrollbar, so it never covers them
/// and the bar keeps one height whether or not the chips overflow.
const SCROLLBAR_ROOM: i32 = 12;
/// How far one mouse-wheel notch moves the chip line: two chips.
const WHEEL_NOTCH_PX: f64 = 2.0 * (CHIP_SIZE + CHIP_SPACING) as f64;

/// Horizontal pixels for a vertical scroll over the chip line: a wheel moves
/// by notches, a touchpad by its own pixels.
fn chip_scroll_px(wheel: bool, dy: f64) -> f64 {
    if wheel { dy * WHEEL_NOTCH_PX } else { dy }
}

/// The palette's colors as chips show them: stored values, except the edited
/// slot, which previews `current` while a session is on this palette.
pub fn chip_colors_preview_edited_value(
    palette: &Palette,
    session: Option<&EditSession>,
    current: Color,
) -> Vec<Color> {
    let mut colors: Vec<Color> = palette.colors.iter().map(|c| c.color).collect();
    if let Some(s) = session.filter(|s| s.palette_id == palette.id)
        && let Some(slot) = colors.get_mut(s.index)
    {
        *slot = current;
    }
    colors
}

/// The paint cell and area of the edited chip.
type EditedChip = (Rc<Cell<[u8; 3]>>, gtk::DrawingArea);

/// What the chip line was last built from, to skip identical rebuilds.
#[derive(PartialEq)]
struct ChipSig {
    id: Option<String>,
    names: Vec<(Color, String)>,
    edited: Option<usize>,
}

struct Inner {
    state: AppState,
    window: ColorzWindow,
    show_count: bool,
    name_label: gtk::Label,
    menu: gio::Menu,
    count: gtk::Label,
    badge: gtk::Label,
    add_button: gtk::Button,
    save_button: gtk::Button,
    chips: gtk::Box,
    edited_chip: RefCell<Option<EditedChip>>,
    sig: RefCell<Option<ChipSig>>,
    on_add: RefCell<Option<Rc<dyn Fn()>>>,
}

/// The bar widget. Keep it alive as long as it is shown: the state listener
/// holds it weakly.
pub struct PaletteBar {
    widget: gtk::Widget,
    inner: Rc<Inner>,
}

impl PaletteBar {
    pub fn widget(&self) -> &gtk::Widget {
        &self.widget
    }

    /// Called by the Add button. The owner decides which colors to add.
    pub fn connect_add(&self, f: impl Fn() + 'static) {
        *self.inner.on_add.borrow_mut() = Some(Rc::new(f));
    }

    pub fn new(state: &AppState, window: &ColorzWindow, show_count: bool) -> PaletteBar {
        let menu = gio::Menu::new();
        let name_label = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(20)
            .xalign(0.0)
            .build();
        let name_button = gtk::MenuButton::builder()
            .menu_model(&menu)
            .child(&name_label)
            .always_show_arrow(true)
            .css_classes(["flat"])
            .build();
        let count = gtk::Label::builder()
            .css_classes(["dim-label"])
            .visible(false)
            .build();
        let badge = gtk::Label::builder()
            .label("Auto-add")
            .css_classes(["badge"])
            .visible(false)
            .build();
        let spacer = gtk::Box::builder().hexpand(true).build();
        let add_button = gtk::Button::builder()
            .label("Add")
            .css_classes(["suggested-action"])
            .build();
        // Only while a palette color is being edited.
        let save_button = gtk::Button::builder()
            .label("Save")
            .tooltip_text("Save the edited color to the palette")
            .css_classes(["suggested-action"])
            .visible(false)
            .build();
        let top = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        top.append(&name_button);
        top.append(&count);
        top.append(&badge);
        top.append(&spacer);
        top.append(&add_button);
        top.append(&save_button);

        let chips = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(CHIP_SPACING)
            .height_request(CHIP_SIZE)
            .margin_top(5)
            .margin_bottom(5 + SCROLLBAR_ROOM)
            .margin_start(5)
            .margin_end(5)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&chips)
            .build();
        // A vertical wheel scrolls the chip line sideways. Capture phase, so
        // the page's own scroller doesn't take it while the chips overflow.
        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
        wheel.connect_scroll({
            let adj = scroller.hadjustment();
            move |ctrl, _, dy| {
                if dy == 0.0 || adj.upper() - adj.page_size() <= 0.0 {
                    return glib::Propagation::Proceed;
                }
                let px = chip_scroll_px(ctrl.unit() == gtk::gdk::ScrollUnit::Wheel, dy);
                adj.set_value(adj.value() + px);
                glib::Propagation::Stop
            }
        });
        scroller.add_controller(wheel);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .css_classes(["palette-bar"])
            .build();
        root.append(&top);
        root.append(&scroller);

        // Plain action, not a check item: a checkable item makes the popover
        // reserve an indicator column for every row.
        let auto_action = gio::SimpleAction::new("auto", None);
        let select = gio::SimpleAction::new("select", Some(glib::VariantTy::STRING));
        let new = gio::SimpleAction::new("new", None);
        let group = gio::SimpleActionGroup::new();
        group.add_action(&auto_action);
        group.add_action(&select);
        group.add_action(&new);
        root.insert_action_group("pbar", Some(&group));

        let inner = Rc::new(Inner {
            state: state.clone(),
            window: window.clone(),
            show_count,
            name_label,
            menu,
            count,
            badge,
            add_button: add_button.clone(),
            save_button: save_button.clone(),
            chips,
            edited_chip: RefCell::new(None),
            sig: RefCell::new(None),
            on_add: RefCell::new(None),
        });

        auto_action.connect_activate({
            let state = state.clone();
            move |_, _| state.update_config(|c| c.auto_add_picks = !c.auto_add_picks)
        });
        select.connect_activate({
            let state = state.clone();
            move |_, param| {
                if let Some(id) = param.and_then(|p| p.get::<String>()) {
                    state.set_active_palette(&id);
                }
            }
        });
        new.connect_activate({
            let (state, window) = (state.clone(), window.clone());
            move |_, _| {
                let state = state.clone();
                super::prompt_name(
                    &window,
                    "New Palette",
                    NEW_PALETTE_NAME,
                    "_Create",
                    move |name| {
                        let pal = Palette::new(&name);
                        let id = pal.id.clone();
                        state.upsert_palette(pal);
                        state.set_active_palette(&id);
                    },
                );
            }
        });
        save_button.connect_clicked({
            let state = state.clone();
            move |_| {
                // A lost session already toasts through `EditingLost`.
                let _ = state.save_editing();
            }
        });
        add_button.connect_clicked({
            let weak = Rc::downgrade(&inner);
            move |_| {
                if let Some(i) = weak.upgrade() {
                    i.fire_add();
                }
            }
        });

        inner.refresh();
        state.connect({
            let weak = Rc::downgrade(&inner);
            move |e| {
                if let Some(inner) = weak.upgrade() {
                    inner.on_event(e);
                }
            }
        });
        PaletteBar {
            widget: root.upcast(),
            inner,
        }
    }
}

impl Inner {
    fn on_event(&self, e: &StateEvent) {
        match e {
            StateEvent::Palettes | StateEvent::Config | StateEvent::Editing => self.refresh(),
            StateEvent::Color(_) => self.refresh_edited_chip(),
            StateEvent::History | StateEvent::EditingLost | StateEvent::SaveFailed(_) => {}
        }
    }

    fn fire_add(&self) {
        let f = self.on_add.borrow().clone();
        if let Some(f) = f {
            f();
        }
    }

    fn refresh(&self) {
        let cfg = self.state.config();
        self.badge.set_visible(CAN_PICK && cfg.auto_add_picks);
        let editing = self.state.editing().is_some();
        self.add_button
            .set_label(if editing { "Add as new" } else { "Add" });
        // One accent button at a time: Save while editing, else Add.
        if editing {
            self.add_button.remove_css_class("suggested-action");
        } else {
            self.add_button.add_css_class("suggested-action");
        }
        self.save_button.set_visible(editing);
        let pal = self.state.active_palette();
        self.name_label.set_label(
            &pal.as_ref()
                .map_or_else(|| "No palette".to_owned(), palette_display_name),
        );
        match &pal {
            Some(p) if self.show_count => {
                let n = p.colors.len();
                self.count.set_label(&if n == 1 {
                    "1 color".to_owned()
                } else {
                    format!("{n} colors")
                });
                self.count.set_visible(true);
            }
            _ => self.count.set_visible(false),
        }
        self.rebuild_menu(cfg.last_palette.as_deref(), cfg.auto_add_picks);
        self.rebuild_chips(pal.as_ref());
    }

    fn rebuild_menu(&self, last: Option<&str>, auto_add: bool) {
        let list = gio::Menu::new();
        for p in palettes_last_used_first(self.state.palettes(), last) {
            let item = gio::MenuItem::new(Some(&menu_label(&p.name)), None);
            item.set_action_and_target_value(Some("pbar.select"), Some(&p.id.to_variant()));
            list.append_item(&item);
        }
        let new = gio::Menu::new();
        new.append(Some("New palette\u{2026}"), Some("pbar.new"));
        let auto = gio::Menu::new();
        if CAN_PICK {
            auto.append(Some(auto_add_label(auto_add)), Some("pbar.auto"));
        }
        self.menu.remove_all();
        self.menu.append_section(None, &list);
        self.menu.append_section(None, &new);
        self.menu.append_section(None, &auto);
    }

    fn rebuild_chips(&self, pal: Option<&Palette>) {
        let session = self.state.editing();
        let edited = pal.and_then(|p| {
            session
                .as_ref()
                .filter(|s| s.palette_id == p.id && s.index < p.colors.len())
                .map(|s| s.index)
        });
        let sig = ChipSig {
            id: pal.map(|p| p.id.clone()),
            names: pal.map_or_else(Vec::new, |p| {
                p.colors.iter().map(|c| (c.color, color_name(c))).collect()
            }),
            edited,
        };
        if self.sig.borrow().as_ref() == Some(&sig) {
            return;
        }
        *self.sig.borrow_mut() = Some(sig);
        *self.edited_chip.borrow_mut() = None;
        while let Some(child) = self.chips.first_child() {
            self.chips.remove(&child);
        }
        let Some(pal) = pal else {
            self.chips.append(
                &gtk::Label::builder()
                    .label("Add creates a palette")
                    .css_classes(["dim-label"])
                    .build(),
            );
            return;
        };
        let colors = chip_colors_preview_edited_value(pal, session.as_ref(), self.state.current());
        for (i, (pc, color)) in pal.colors.iter().zip(colors).enumerate() {
            let rgb = Rc::new(Cell::new(color.to_srgb8()));
            let area = swatch_area(Some((CHIP_SIZE, CHIP_SIZE)), CHIP_RADIUS, rgb.clone());
            let name = chip_tooltip(pc);
            let button = gtk::Button::builder()
                .child(&area)
                .tooltip_text(&name)
                .css_classes(["palette-chip"])
                .build();
            button.update_property(&[gtk::accessible::Property::Label(&name)]);
            if edited == Some(i) {
                button.add_css_class("current");
                *self.edited_chip.borrow_mut() = Some((rgb, area));
            }
            let (state, window, id) = (self.state.clone(), self.window.clone(), pal.id.clone());
            button.connect_clicked(move |_| {
                // Clicking the chip being edited again stops editing without
                // saving; any other chip drops the edit and starts a new one.
                let this_one = state
                    .editing()
                    .is_some_and(|s| s.palette_id == id && s.index == i);
                if this_one {
                    state.cancel_editing();
                } else {
                    edit_on_wheel(&state, &window, &id, i);
                }
            });
            self.chips.append(&button);
        }
    }

    fn refresh_edited_chip(&self) {
        if let Some((rgb, area)) = &*self.edited_chip.borrow() {
            rgb.set(self.state.current().to_srgb8());
            area.queue_draw();
        }
    }
}

/// "Primary · #3478F6" when labeled, else just the hex.
fn chip_tooltip(pc: &colorz_core::palette::PaletteColor) -> String {
    let [r, g, b] = pc.color.to_srgb8();
    let hex = format!("#{r:02X}{g:02X}{b:02X}");
    if pc.label.trim().is_empty() {
        hex
    } else {
        format!("{} \u{b7} {hex}", pc.label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_notch_moves_two_chips_touchpad_moves_pixels() {
        assert_eq!(chip_scroll_px(true, 1.0), 72.0);
        assert_eq!(chip_scroll_px(true, -1.0), -72.0);
        assert_eq!(chip_scroll_px(false, 15.5), 15.5);
    }

    #[test]
    fn chip_tooltip_has_label_and_hex() {
        let mut pc = colorz_core::palette::PaletteColor {
            color: Color::from_srgb8([0x34, 0x78, 0xF6]),
            label: String::new(),
            var_name: String::new(),
        };
        assert_eq!(chip_tooltip(&pc), "#3478F6");
        pc.label = "Primary".into();
        assert_eq!(chip_tooltip(&pc), "Primary \u{b7} #3478F6");
    }

    fn pal() -> Palette {
        let mut p = Palette::new("p");
        colorz_core::palette_ops::append_colors(
            &mut p,
            &[
                Color::from_srgb8([1, 2, 3]),
                Color::from_srgb8([4, 5, 6]),
                Color::from_srgb8([7, 8, 9]),
            ],
        );
        p
    }

    #[test]
    fn edited_chip_previews_current() {
        let p = pal();
        let cur = Color::from_srgb8([200, 100, 50]);
        let s = EditSession {
            palette_id: p.id.clone(),
            index: 1,
            original: p.colors[1].color,
        };
        let got = chip_colors_preview_edited_value(&p, Some(&s), cur);
        assert_eq!(got, [p.colors[0].color, cur, p.colors[2].color]);
    }

    #[test]
    fn no_preview_without_or_for_other_palette() {
        let p = pal();
        let cur = Color::from_srgb8([200, 100, 50]);
        let stored: Vec<Color> = p.colors.iter().map(|c| c.color).collect();
        assert_eq!(chip_colors_preview_edited_value(&p, None, cur), stored);
        let other = EditSession {
            palette_id: "other".into(),
            index: 1,
            original: stored[1],
        };
        assert_eq!(
            chip_colors_preview_edited_value(&p, Some(&other), cur),
            stored
        );
    }

    #[test]
    fn out_of_range_index_is_ignored() {
        let p = pal();
        let s = EditSession {
            palette_id: p.id.clone(),
            index: 9,
            original: p.colors[0].color,
        };
        let stored: Vec<Color> = p.colors.iter().map(|c| c.color).collect();
        assert_eq!(
            chip_colors_preview_edited_value(&p, Some(&s), Color::from_srgb8([0, 0, 0])),
            stored
        );
    }
}
