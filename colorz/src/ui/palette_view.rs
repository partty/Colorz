//! The palette view of the Palettes tab: one palette's colors as rows
//! that can be reordered, edited inline, copied and deleted. Every edit is
//! saved at once.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use colorz_core::color::RgbSpace;
use colorz_core::format::{Form, Format, FormatOptions, format_color};
use colorz_core::palette::{Palette, PaletteColor, VarNameError, normalize_var_name};
use colorz_core::target::BuiltinTarget;
use gtk::{gdk, gio, glib};

use super::copy_pair::CopyPair;
use super::palette_actions::edit_on_wheel;
use super::pick_page::swatch_area;
use super::window::ColorzWindow;
use super::{copy_text, copy_with_toast, css_dialogs, prompt_name};
use crate::state::{AppState, ColorSource, PickPurpose, StateEvent};

const SWATCH_SIZE: i32 = 28;
const SWATCH_RADIUS: f64 = 6.0;
/// How long to wait before rebuilding the rows while a label is edited.
const EDIT_RETRY: Duration = Duration::from_millis(500);

/// One `#RRGGBB` per line, in sRGB whatever the "copy as" target is: the
/// hex list is palette export, like CSS and `.gpl`, so the target never
/// applies to it.
pub fn hex_list(p: &Palette, opts: &FormatOptions) -> String {
    let srgb = BuiltinTarget(RgbSpace::Srgb);
    p.colors
        .iter()
        .filter_map(|c| format_color(&c.color, Format::Hex, Form::Full, opts, &srgb))
        .map(|f| f.text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The pick purpose that replaces color `index` of `p`, or `None` when
/// there is no such color.
fn replace_purpose(p: &Palette, index: usize) -> Option<PickPurpose> {
    let original = p.colors.get(index)?.color;
    Some(PickPurpose::Replace {
        palette_id: p.id.clone(),
        index,
        original,
    })
}

/// A flat icon-only button with a tooltip and the same accessible label.
fn icon_button(icon: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tip)
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .build();
    b.update_property(&[gtk::accessible::Property::Label(tip)]);
    b
}

fn start_pick(window: &ColorzWindow) {
    let _ = WidgetExt::activate_action(window.window(), "app.pick", None);
}

/// How a variable name is shown: with its `--`, or empty.
fn var_display(name: &str) -> String {
    if name.is_empty() {
        String::new()
    } else {
        format!("--{name}")
    }
}

/// An inline-editable label that shows `placeholder` while empty.
fn editable(text: &str, placeholder: &str, css: &[&str]) -> (gtk::Overlay, gtk::EditableLabel) {
    let label = gtk::EditableLabel::builder()
        .text(text)
        .hexpand(true)
        .width_chars(4)
        .max_width_chars(16)
        .css_classes(css)
        .tooltip_text("Double-click to edit")
        .build();
    let hint = gtk::Label::builder()
        .label(placeholder)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Center)
        .can_target(false)
        .css_classes(["dim-label"])
        .visible(text.is_empty())
        .build();
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&label));
    overlay.add_overlay(&hint);
    label.connect_text_notify(move |l| hint.set_visible(l.text().is_empty()));
    (overlay, label)
}

fn row_menu() -> gio::Menu {
    let copy = gio::Menu::new();
    for (n, f) in Format::ALL.iter().enumerate() {
        let item = gio::MenuItem::new(Some(f.label()), None);
        item.set_action_and_target_value(Some("row.copy-as"), Some(&(n as u32).to_variant()));
        copy.append_item(&item);
    }
    let menu = gio::Menu::new();
    menu.append_submenu(Some("Copy in Format"), &copy);
    let delete = gio::Menu::new();
    delete.append(Some("Delete"), Some("row.delete"));
    menu.append_section(None, &delete);
    menu
}

fn header_menu() -> gio::Menu {
    let top = gio::Menu::new();
    top.append(Some("Rename\u{2026}"), Some("palette.rename"));
    top.append(Some("Duplicate"), Some("palette.duplicate"));
    let files = gio::Menu::new();
    files.append(
        Some("Export as CSS Variables\u{2026}"),
        Some("palette.export-css"),
    );
    files.append(
        Some("Import CSS Variables Into This Palette\u{2026}"),
        Some("palette.import-css"),
    );
    files.append(
        Some("Export as GIMP Palette\u{2026}"),
        Some("palette.export-gpl"),
    );
    files.append(Some("Copy All as Hex List"), Some("palette.copy-hex"));
    let delete = gio::Menu::new();
    delete.append(Some("Delete\u{2026}"), Some("palette.delete"));
    let menu = gio::Menu::new();
    menu.append_section(None, &top);
    menu.append_section(None, &files);
    menu.append_section(None, &delete);
    menu
}

struct Inner {
    state: AppState,
    window: ColorzWindow,
    nav: adw::NavigationView,
    page: adw::NavigationPage,
    stack: gtk::Stack,
    list: gtk::ListBox,
    /// The palette being shown.
    id: RefCell<Option<String>>,
    /// The palette as the rows were last built (or edited) to show it.
    shown: RefCell<Option<Palette>>,
    labels: RefCell<Vec<gtk::EditableLabel>>,
    rebuild_queued: Cell<bool>,
    weak_self: Weak<Inner>,
}

/// The palette page. Keep it alive as long as the page is shown: the state
/// listener holds it weakly.
pub struct PaletteView(Rc<Inner>);

impl PaletteView {
    pub fn new(state: &AppState, window: &ColorzWindow, nav: &adw::NavigationView) -> PaletteView {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .valign(gtk::Align::Start)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("color-select-symbolic")
            .title("No colors yet")
            .description("Add the current color with the + button.")
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&scrolled, Some("colors"));

        let add = gtk::Button::builder()
            .tooltip_text("Add the Current Color to This Palette")
            .action_name("palette.add-current")
            .icon_name("list-add-symbolic")
            .build();
        add.update_property(&[gtk::accessible::Property::Label(
            "Add the Current Color to This Palette",
        )]);
        let menu = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("Palette Menu")
            .menu_model(&header_menu())
            .build();
        let pick_into = icon_button("color-select-symbolic", "Pick into");
        let open_wheel = icon_button("applications-graphics-symbolic", "Open on wheel");
        let header = adw::HeaderBar::builder()
            .show_start_title_buttons(false)
            .show_end_title_buttons(false)
            .build();
        header.pack_end(&menu);
        header.pack_end(&add);
        header.pack_end(&pick_into);
        header.pack_end(&open_wheel);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&stack));
        let page = adw::NavigationPage::builder()
            .tag("palette")
            .title("Palette")
            .child(&toolbar)
            .build();

        let inner = Rc::new_cyclic(|weak_self| Inner {
            state: state.clone(),
            window: window.clone(),
            nav: nav.clone(),
            page,
            stack,
            list: list.clone(),
            id: RefCell::new(None),
            shown: RefCell::new(None),
            labels: RefCell::default(),
            rebuild_queued: Cell::new(false),
            weak_self: weak_self.clone(),
        });
        inner.install_actions();
        let weak = Rc::downgrade(&inner);
        pick_into.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(i) = weak.upgrade()
                    && let Some(palette_id) = i.id.borrow().clone()
                {
                    i.state
                        .set_pick_purpose(Some(PickPurpose::AddTo { palette_id }));
                    start_pick(&i.window);
                }
            }
        });
        open_wheel.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(i) = weak.upgrade()
                    && let Some(id) = i.id.borrow().clone()
                {
                    i.state.set_active_palette(&id);
                    i.window.set_page("wheel");
                }
            }
        });
        list.connect_row_activated({
            let weak = weak.clone();
            move |_, row| {
                if let Some(i) = weak.upgrade() {
                    i.row_activated(row.index());
                }
            }
        });
        nav.connect_popped({
            let weak = weak.clone();
            move |_, page| {
                if let Some(i) = weak.upgrade()
                    && *page == i.page
                {
                    i.id.take();
                    i.shown.take();
                }
            }
        });
        state.connect(move |e| {
            if let Some(i) = weak.upgrade() {
                i.on_event(e);
            }
        });
        PaletteView(inner)
    }

    /// Shows palette `id` and navigates to it.
    pub fn open(&self, id: &str) {
        self.0.open(id);
    }
}

impl Inner {
    fn open(&self, id: &str) {
        if self.state.palette(id).is_none() {
            return;
        }
        *self.id.borrow_mut() = Some(id.to_owned());
        self.rebuild();
        if self.nav.visible_page().as_ref() != Some(&self.page) {
            self.nav.push(&self.page);
        }
    }

    fn is_visible(&self) -> bool {
        self.id.borrow().is_some() && self.nav.visible_page().as_ref() == Some(&self.page)
    }

    fn on_event(&self, e: &StateEvent) {
        let Some(id) = self.id.borrow().clone() else {
            return;
        };
        match e {
            StateEvent::Palettes => match self.state.palette(&id) {
                None => {
                    let visible = self.is_visible();
                    self.id.take();
                    self.shown.take();
                    if visible {
                        self.nav.pop();
                    }
                }
                Some(p) => {
                    if self.shown.borrow().as_ref() != Some(&p) {
                        self.queue_rebuild();
                    }
                }
            },
            // The format or the target may have changed the value texts.
            StateEvent::Config if self.is_visible() => {
                self.queue_rebuild();
            }
            _ => {}
        }
    }

    /// Rebuilds the rows soon, once, and not while a label is edited.
    fn queue_rebuild(&self) {
        if self.rebuild_queued.replace(true) {
            return;
        }
        let weak = self.weak_self.clone();
        glib::idle_add_local_once(move || {
            if let Some(i) = weak.upgrade() {
                i.rebuild_queued.set(false);
                i.rebuild_unless_editing();
            }
        });
    }

    fn rebuild_unless_editing(&self) {
        if self.id.borrow().is_none() {
            return;
        }
        if self.labels.borrow().iter().any(|l| l.is_editing()) {
            self.rebuild_queued.set(true);
            let weak = self.weak_self.clone();
            glib::timeout_add_local_once(EDIT_RETRY, move || {
                if let Some(i) = weak.upgrade() {
                    i.rebuild_queued.set(false);
                    i.rebuild_unless_editing();
                }
            });
        } else {
            self.rebuild();
        }
    }

    fn rebuild(&self) {
        let Some(id) = self.id.borrow().clone() else {
            return;
        };
        let Some(pal) = self.state.palette(&id) else {
            return;
        };
        self.page.set_title(&pal.name);
        self.list.remove_all();
        self.labels.borrow_mut().clear();
        for (i, pc) in pal.colors.iter().enumerate() {
            let row = self.build_row(i, pc);
            self.list.append(&row);
        }
        let child = if pal.colors.is_empty() {
            "empty"
        } else {
            "colors"
        };
        self.stack.set_visible_child_name(child);
        *self.shown.borrow_mut() = Some(pal);
    }

    /// Applies `f` to the shown palette and saves it when `f` returns true.
    /// `structural` edits (and renames) make the rows rebuild; others are
    /// already visible in the edited widget.
    fn edit(&self, structural: bool, f: impl FnOnce(&mut Palette) -> bool) {
        let Some(id) = self.id.borrow().clone() else {
            return;
        };
        let Some(before) = self.state.palette(&id) else {
            return;
        };
        let mut pal = before.clone();
        if !f(&mut pal) {
            return;
        }
        if !structural && self.shown.borrow().as_ref() == Some(&before) {
            *self.shown.borrow_mut() = Some(pal.clone());
        }
        self.state.upsert_palette(pal);
    }

    fn current_palette(&self) -> Option<Palette> {
        let id = self.id.borrow().clone()?;
        self.state.palette(&id)
    }

    /// Whether the rows still show the stored palette. Rows address colors
    /// by index, so when they do not (a rebuild is waiting for a label edit
    /// to end) the action is dropped and the rows are refreshed.
    fn rows_current(&self) -> bool {
        let current = self.current_palette();
        if current.is_some() && current == *self.shown.borrow() {
            return true;
        }
        self.queue_rebuild();
        false
    }

    fn row_activated(&self, index: i32) {
        if !self.rows_current() {
            return;
        }
        let color = usize::try_from(index)
            .ok()
            .and_then(|i| Some(self.current_palette()?.colors.get(i)?.color));
        if let Some(c) = color {
            self.state.set_current(c, ColorSource::Palette);
        }
    }

    // --------------------------------------------------------------- rows

    fn build_row(&self, i: usize, pc: &PaletteColor) -> gtk::ListBoxRow {
        let weak = self.weak_self.clone();
        let handle = gtk::Image::builder()
            .icon_name("list-drag-handle-symbolic")
            .css_classes(["dim-label"])
            .tooltip_text("Drag to reorder")
            .build();
        handle.set_cursor_from_name(Some("grab"));
        let swatch = swatch_area(
            Some((SWATCH_SIZE, SWATCH_SIZE)),
            SWATCH_RADIUS,
            Rc::new(Cell::new(pc.color.to_srgb8())),
        );
        swatch.set_valign(gtk::Align::Center);

        let (label_box, label) = editable(&pc.label, "Label", &[]);
        let (var_box, var) = editable(
            &var_display(&pc.var_name),
            "--variable",
            &["mono-text", "caption"],
        );
        var.set_opacity(0.8);
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .overflow(gtk::Overflow::Hidden)
            .valign(gtk::Align::Center)
            .build();
        text.append(&label_box);

        let copy = CopyPair::new();
        let value = copy
            .update(&self.state, &pc.color, self.state.default_format())
            .unwrap_or_else(|| "\u{2014}".to_owned());
        let value = gtk::Label::builder()
            .label(value)
            .selectable(false)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(10)
            .css_classes(["mono-text", "caption"])
            .valign(gtk::Align::Center)
            .build();
        value.set_opacity(0.8);
        let second = gtk::Box::builder().spacing(8).build();
        second.append(&var_box);
        second.append(&value);
        text.append(&second);

        let content = gtk::Box::builder()
            .spacing(8)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(8)
            .margin_end(6)
            .build();
        for w in [
            handle.upcast_ref::<gtk::Widget>(),
            swatch.upcast_ref(),
            text.upcast_ref(),
        ] {
            content.append(w);
        }
        let edit = icon_button("applications-graphics-symbolic", "Edit on wheel");
        let replace = icon_button("color-select-symbolic", "Pick replacement");
        content.append(&edit);
        content.append(&replace);
        for w in copy.widgets() {
            content.append(&w);
        }
        edit.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(this) = weak.upgrade()
                    && this.rows_current()
                    && let Some(id) = this.id.borrow().clone()
                {
                    edit_on_wheel(&this.state, &this.window, &id, i);
                }
            }
        });
        replace.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(this) = weak.upgrade()
                    && this.rows_current()
                    && let Some(purpose) =
                        this.current_palette().and_then(|p| replace_purpose(&p, i))
                {
                    this.state.set_pick_purpose(Some(purpose));
                    start_pick(&this.window);
                }
            }
        });
        let row = gtk::ListBoxRow::builder().child(&content).build();

        label.connect_editing_notify({
            let weak = weak.clone();
            move |l| {
                if let (false, Some(this)) = (l.is_editing(), weak.upgrade()) {
                    this.label_edited(i, l);
                }
            }
        });
        var.connect_editing_notify({
            let weak = weak.clone();
            move |l| {
                if let (false, Some(this)) = (l.is_editing(), weak.upgrade()) {
                    this.var_name_edited(i, l);
                }
            }
        });
        copy.connect_copy({
            let weak = weak.clone();
            move |text| {
                if let Some(this) = weak.upgrade()
                    && this.rows_current()
                {
                    copy_with_toast(&this.window, text);
                }
            }
        });
        self.labels.borrow_mut().extend([label, var]);
        self.install_row_actions(&row, i);
        self.install_drag_and_drop(&row, &handle, i);
        row
    }

    fn label_edited(&self, i: usize, widget: &gtk::EditableLabel) {
        if !self.rows_current() {
            return;
        }
        let text = widget.text().trim().to_owned();
        if widget.text() != text {
            widget.set_text(&text);
        }
        self.edit(false, |p| match p.colors.get_mut(i) {
            Some(c) if c.label != text => {
                c.label = text;
                true
            }
            _ => false,
        });
    }

    fn var_name_edited(&self, i: usize, widget: &gtk::EditableLabel) {
        if !self.rows_current() {
            return;
        }
        let raw = widget.text();
        let mut error = None;
        self.edit(false, |p| {
            if i >= p.colors.len() || normalize_var_name(&raw) == p.colors[i].var_name {
                return false;
            }
            match p.set_var_name(i, &raw) {
                Ok(()) => true,
                Err(e) => {
                    error = Some(e);
                    false
                }
            }
        });
        if let Some(e) = error {
            self.window.show_toast(match e {
                VarNameError::Invalid => "Invalid variable name",
                VarNameError::Duplicate => "Name already used",
            });
        }
        // Show the stored name: normalized, or the old one after an error.
        let stored = self
            .current_palette()
            .and_then(|p| p.colors.get(i).map(|c| var_display(&c.var_name)))
            .unwrap_or_default();
        if widget.text() != stored {
            widget.set_text(&stored);
        }
    }

    fn copy_color_as(&self, i: usize, format: Format, form: Form) {
        if !self.rows_current() {
            return;
        }
        let Some(color) = self
            .current_palette()
            .and_then(|p| Some(p.colors.get(i)?.color))
        else {
            return;
        };
        match self.state.format(&color, format, form) {
            Some(f) => copy_with_toast(&self.window, &f.text),
            None => self
                .window
                .show_toast("This color cannot be shown in that format"),
        }
    }

    fn delete_color(&self, i: usize) {
        if !self.rows_current() {
            return;
        }
        let Some(id) = self.id.borrow().clone() else {
            return;
        };
        let mut removed = None;
        self.edit(true, |p| {
            if i < p.colors.len() {
                removed = Some(p.remove(i));
            }
            removed.is_some()
        });
        let Some(color) = removed else { return };
        let state = self.state.clone();
        self.window.toast_with_undo("Color deleted", move || {
            if let Some(mut p) = state.palette(&id) {
                let mut color = color.clone();
                // Another color may have taken the name since.
                if p.index_of_var(&color.var_name).is_some() {
                    color.var_name.clear();
                }
                p.insert(i.min(p.colors.len()), color);
                state.upsert_palette(p);
            }
        });
    }

    fn install_row_actions(&self, row: &gtk::ListBoxRow, i: usize) {
        let weak = self.weak_self.clone();
        let actions = gio::SimpleActionGroup::new();
        let copy_as = gio::SimpleAction::new("copy-as", Some(glib::VariantTy::UINT32));
        copy_as.connect_activate({
            let weak = weak.clone();
            move |_, param| {
                let format = param
                    .and_then(|p| p.get::<u32>())
                    .and_then(|n| Format::ALL.get(n as usize).copied());
                if let (Some(this), Some(format)) = (weak.upgrade(), format) {
                    this.copy_color_as(i, format, Form::Full);
                }
            }
        });
        actions.add_action(&copy_as);
        let delete = gio::SimpleAction::new("delete", None);
        delete.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.delete_color(i);
            }
        });
        actions.add_action(&delete);
        row.insert_action_group("row", Some(&actions));

        let click = gtk::GestureClick::builder().button(3).build();
        click.connect_pressed({
            let row = row.downgrade();
            move |_, _, x, y| {
                let Some(row) = row.upgrade() else { return };
                let popover = gtk::PopoverMenu::from_model(Some(&row_menu()));
                popover.set_parent(&row);
                popover.set_has_arrow(false);
                popover.set_halign(gtk::Align::Start);
                popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                popover.connect_closed(|p| {
                    let p = p.clone();
                    glib::idle_add_local_once(move || p.unparent());
                });
                popover.popup();
            }
        });
        row.add_controller(click);
    }

    fn install_drag_and_drop(&self, row: &gtk::ListBoxRow, handle: &gtk::Image, i: usize) {
        let source = gtk::DragSource::builder()
            .actions(gdk::DragAction::MOVE)
            .content(&gdk::ContentProvider::for_value(&(i as u32).to_value()))
            .build();
        source.connect_drag_begin({
            let row = row.downgrade();
            move |source, _| {
                if let Some(row) = row.upgrade() {
                    let icon = gtk::WidgetPaintable::new(Some(&row));
                    source.set_icon(Some(&icon), 0, 0);
                }
            }
        });
        handle.add_controller(source);

        let target = gtk::DropTarget::new(u32::static_type(), gdk::DragAction::MOVE);
        let weak = self.weak_self.clone();
        target.connect_drop(move |_, value, _, _| {
            let (Ok(from), Some(this)) = (value.get::<u32>(), weak.upgrade()) else {
                return false;
            };
            this.move_color(from as usize, i);
            true
        });
        row.add_controller(target);
    }

    fn move_color(&self, from: usize, to: usize) {
        if from == to || !self.rows_current() {
            return;
        }
        self.edit(true, |p| {
            let n = p.colors.len();
            if from >= n || to >= n {
                return false;
            }
            p.move_color(from, to);
            true
        });
    }

    // ------------------------------------------------------ header actions

    fn install_actions(&self) {
        let actions = gio::SimpleActionGroup::new();
        let add = |name: &str, f: fn(&Inner)| {
            let weak = self.weak_self.clone();
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(move |_, _| {
                if let Some(this) = weak.upgrade() {
                    f(&this);
                }
            });
            actions.add_action(&action);
        };
        add("add-current", Inner::add_current);
        add("rename", Inner::rename);
        add("duplicate", Inner::duplicate);
        add("export-css", |t| {
            if let Some(id) = t.id.borrow().as_deref() {
                css_dialogs::export_css_dialog(&t.window, &t.state, id);
            }
        });
        add("import-css", |t| {
            let id = t.id.borrow().clone();
            css_dialogs::import_css_dialog(&t.window, &t.state, id);
        });
        add("export-gpl", |t| {
            if let Some(id) = t.id.borrow().as_deref() {
                css_dialogs::export_gpl_dialog(&t.window, &t.state, id);
            }
        });
        add("copy-hex", Inner::copy_hex_list);
        add("delete", Inner::confirm_delete);
        self.page.insert_action_group("palette", Some(&actions));
    }

    fn add_current(&self) {
        let color = self.state.current();
        self.edit(true, |p| {
            p.push(PaletteColor {
                color,
                label: String::new(),
                var_name: String::new(),
            });
            true
        });
    }

    fn rename(&self) {
        let Some(pal) = self.current_palette() else {
            return;
        };
        let weak = self.weak_self.clone();
        prompt_name(
            &self.window,
            "Rename Palette",
            &pal.name,
            "_Rename",
            move |name| {
                if let Some(this) = weak.upgrade() {
                    this.edit(true, |p| {
                        p.name = name;
                        true
                    });
                }
            },
        );
    }

    fn duplicate(&self) {
        let Some(pal) = self.current_palette() else {
            return;
        };
        let copy = pal.duplicate(&format!("{} copy", pal.name));
        let id = copy.id.clone();
        self.state.upsert_palette(copy);
        self.open(&id);
    }

    fn copy_hex_list(&self) {
        let Some(pal) = self.current_palette() else {
            return;
        };
        if copy_text(&self.window, &hex_list(&pal, &self.state.format_options())) {
            self.window.show_toast("Copied hex list");
        }
    }

    fn confirm_delete(&self) {
        let Some(pal) = self.current_palette() else {
            return;
        };
        let dialog = adw::AlertDialog::builder()
            .heading("Delete Palette?")
            .body_use_markup(false)
            .body(format!("\u{201c}{}\u{201d} will be deleted.", pal.name))
            .default_response("cancel")
            .close_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "_Cancel"), ("delete", "_Delete")]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        let (state, window) = (self.state.clone(), self.window.clone());
        dialog.connect_response(Some("delete"), move |_, _| {
            // The state event closes the view.
            if let Some(removed) = state.delete_palette(&pal.id) {
                let state = state.clone();
                window.toast_with_undo("Palette deleted", move || {
                    state.upsert_palette(removed.clone());
                });
            }
        });
        dialog.present(Some(self.window.window()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use colorz_core::color::Color;

    #[test]
    fn replace_purpose_captures_original() {
        let mut p = Palette::new("p");
        let c = Color::from_srgb8([9, 8, 7]);
        p.push(PaletteColor {
            color: c,
            label: String::new(),
            var_name: String::new(),
        });
        assert_eq!(
            replace_purpose(&p, 0),
            Some(PickPurpose::Replace {
                palette_id: p.id.clone(),
                index: 0,
                original: c
            })
        );
        assert_eq!(replace_purpose(&p, 1), None);
    }

    #[test]
    fn hex_list_lines() {
        let mut p = Palette::new("p");
        for rgb in [[0x34, 0x78, 0xF6], [0, 0, 0], [255, 128, 1]] {
            p.push(PaletteColor {
                color: Color::from_srgb8(rgb),
                label: "x".into(),
                var_name: String::new(),
            });
        }
        let opts = FormatOptions::default();
        assert_eq!(hex_list(&p, &opts), "#3478F6\n#000000\n#FF8001");
        assert_eq!(hex_list(&Palette::new("e"), &opts), "");
    }

    #[test]
    fn var_names_show_with_dashes() {
        assert_eq!(var_display(""), "");
        assert_eq!(var_display("primary"), "--primary");
    }
}
