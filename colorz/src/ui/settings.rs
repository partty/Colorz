//! The Settings dialog, the Keyboard Shortcuts dialog and About.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use colorz_core::format::{CssSyntax, Form, Format, HexCase, MAX_DECIMALS, MIN_DECIMALS};
use colorz_core::icc::IccTarget;
use colorz_core::storage::{Config, TargetChoice};
use gtk::{gio, glib};

use super::targets::{target_choices, target_labels};
use super::window::ColorzWindow;
use crate::platform::{autostart, shortcuts};
use crate::state::{AppState, StateEvent};

/// The key binding offered when the portal shortcut is not available.
const FALLBACK_BINDING: &str = "<Super><Shift>c";
/// Shown for the pick shortcut when no trigger is known.
const DEFAULT_PICK_SHORTCUT: &str = "Super+Shift+C";

/// How binding the global pick shortcut went.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ShortcutStatus {
    /// Still being registered with the portal.
    #[default]
    Pending,
    /// Bound; holds the trigger as described by the portal.
    Bound(String),
    Unavailable,
}

type Listener = Rc<dyn Fn()>;

/// The shortcut status, observable so that an open Settings dialog
/// follows the registration.
#[derive(Default)]
pub struct ShortcutCell {
    status: RefCell<ShortcutStatus>,
    listeners: RefCell<Vec<(usize, Listener)>>,
    next_id: Cell<usize>,
}

impl ShortcutCell {
    pub fn get(&self) -> ShortcutStatus {
        self.status.borrow().clone()
    }

    pub fn set(&self, status: ShortcutStatus) {
        self.status.replace(status);
        let listeners: Vec<_> = self
            .listeners
            .borrow()
            .iter()
            .map(|(_, f)| f.clone())
            .collect();
        for f in listeners {
            f();
        }
    }

    fn connect(&self, f: impl Fn() + 'static) -> usize {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.listeners.borrow_mut().push((id, Rc::new(f)));
        id
    }

    fn disconnect(&self, id: usize) {
        self.listeners.borrow_mut().retain(|(i, _)| *i != id);
    }
}

const FORMS: [(Form, &str); 2] = [(Form::Full, "Full"), (Form::Numbers, "Numbers only")];
const HEX_CASES: [(HexCase, &str); 2] =
    [(HexCase::Upper, "Uppercase"), (HexCase::Lower, "Lowercase")];
const SYNTAXES: [(CssSyntax, &str); 2] =
    [(CssSyntax::Modern, "Modern"), (CssSyntax::Legacy, "Legacy")];

fn combo(title: &str, labels: &[&str]) -> adw::ComboRow {
    adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(labels))
        .build()
}

fn labels<T>(items: &[(T, &'static str)]) -> Vec<&'static str> {
    items.iter().map(|(_, l)| *l).collect()
}

fn index_of<T: PartialEq + Copy>(items: &[(T, &str)], value: T) -> u32 {
    items.iter().position(|(v, _)| *v == value).unwrap_or(0) as u32
}

struct Ui {
    dialog: adw::PreferencesDialog,
    state: AppState,
    shortcut: Rc<ShortcutCell>,
    /// Set while the controls are being updated from the config.
    syncing: Cell<bool>,
    default_format: adw::ComboRow,
    form: adw::ComboRow,
    hex_case: adw::ComboRow,
    css_syntax: adw::ComboRow,
    decimals: adw::SpinRow,
    visible: Vec<(Format, adw::SwitchRow)>,
    target_row: adw::ComboRow,
    target_choices: RefCell<Vec<TargetChoice>>,
    icc_group: adw::PreferencesGroup,
    icc_rows: RefCell<Vec<adw::ActionRow>>,
    chrome: adw::SwitchRow,
    autostart: adw::SwitchRow,
    history: adw::SpinRow,
    shortcut_row: adw::ActionRow,
    fallback_button: gtk::Button,
}

impl Ui {
    fn toast(&self, msg: &str) {
        self.dialog
            .add_toast(adw::Toast::builder().title(msg).use_markup(false).build());
    }

    fn refresh(self: &Rc<Self>) {
        let cfg = self.state.config();
        self.syncing.set(true);
        let format_index = Format::ALL
            .iter()
            .position(|f| *f == cfg.default_format)
            .unwrap_or(0);
        self.default_format.set_selected(format_index as u32);
        self.form.set_selected(index_of(&FORMS, cfg.default_form));
        self.hex_case
            .set_selected(index_of(&HEX_CASES, cfg.hex_case));
        self.css_syntax
            .set_selected(index_of(&SYNTAXES, cfg.css_syntax));
        self.decimals
            .set_value(cfg.decimals.clamp(MIN_DECIMALS, MAX_DECIMALS) as f64);
        for (format, row) in &self.visible {
            let is_default = *format == cfg.default_format;
            row.set_active(is_default || cfg.visible_formats.contains(format));
            row.set_sensitive(!is_default);
            row.set_subtitle(if is_default { "Default format" } else { "" });
        }
        self.refresh_target(&cfg);
        self.refresh_icc_list(&cfg);
        self.chrome.set_active(cfg.chrome_correction);
        self.autostart.set_active(cfg.start_at_login);
        self.history.set_value(cfg.history_size as f64);
        self.refresh_shortcut(&cfg);
        self.syncing.set(false);
    }

    fn refresh_target(&self, cfg: &Config) {
        let choices = target_choices(cfg);
        if *self.target_choices.borrow() != choices {
            let labels = target_labels(&choices);
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            self.target_row
                .set_model(Some(&gtk::StringList::new(&labels)));
        }
        let selected = choices.iter().position(|c| *c == cfg.target).unwrap_or(0);
        self.target_row.set_selected(selected as u32);
        *self.target_choices.borrow_mut() = choices;
    }

    fn refresh_icc_list(self: &Rc<Self>, cfg: &Config) {
        for row in self.icc_rows.borrow_mut().drain(..) {
            self.icc_group.remove(&row);
        }
        let mut rows = Vec::new();
        for path in &cfg.custom_icc {
            let row = adw::ActionRow::builder()
                .title(path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into(),
                ))
                .subtitle(
                    path.parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                )
                .use_markup(false)
                .build();
            let remove = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Remove profile")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let (weak, path) = (Rc::downgrade(self), path.clone());
            remove.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.remove_icc(&path);
                }
            });
            row.add_suffix(&remove);
            self.icc_group.add(&row);
            rows.push(row);
        }
        *self.icc_rows.borrow_mut() = rows;
    }

    fn refresh_shortcut(&self, cfg: &Config) {
        let (subtitle, button) = match &self.shortcut.get() {
            ShortcutStatus::Pending => ("Registering…".to_owned(), false),
            ShortcutStatus::Bound(trigger) if trigger.is_empty() => {
                (DEFAULT_PICK_SHORTCUT.to_owned(), false)
            }
            ShortcutStatus::Bound(trigger) => (trigger.clone(), false),
            ShortcutStatus::Unavailable if cfg.shortcut_fallback_installed => (
                format!("GNOME keyboard shortcut: {DEFAULT_PICK_SHORTCUT}"),
                false,
            ),
            ShortcutStatus::Unavailable => ("Not available".to_owned(), true),
        };
        self.shortcut_row.set_subtitle(&subtitle);
        self.fallback_button.set_visible(button);
    }

    fn remove_icc(&self, path: &PathBuf) {
        self.state.update_config(|c| {
            c.custom_icc.retain(|p| p != path);
            if c.target == (TargetChoice::Icc { path: path.clone() }) {
                c.target = TargetChoice::Srgb;
            }
        });
    }

    fn add_icc_file(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let parent = self.dialog.root().and_downcast::<gtk::Window>();
        glib::spawn_future_local(async move {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("ICC profiles"));
            filter.add_pattern("*.icc");
            filter.add_pattern("*.icm");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let dialog = gtk::FileDialog::builder()
                .title("Add ICC profile")
                .modal(true)
                .filters(&filters)
                .default_filter(&filter)
                .build();
            let Ok(file) = dialog.open_future(parent.as_ref()).await else {
                return;
            };
            // Settings may have been closed while choosing.
            let Some(ui) = weak.upgrade() else { return };
            let Some(path) = file.path() else {
                ui.toast("Not an RGB ICC profile");
                return;
            };
            if let Err(e) = IccTarget::load(&path) {
                eprintln!("colorz: {}: {e}", path.display());
                ui.toast("Not an RGB ICC profile");
                return;
            }
            if ui.state.config().custom_icc.contains(&path) {
                ui.toast("Profile already added");
                return;
            }
            ui.state.update_config(|c| c.custom_icc.push(path));
        });
    }

    fn install_fallback(&self) {
        match shortcuts::install_gsettings_fallback(FALLBACK_BINDING) {
            Ok(()) => {
                self.state
                    .update_config(|c| c.shortcut_fallback_installed = true);
                self.toast(&format!(
                    "Added the GNOME keyboard shortcut {DEFAULT_PICK_SHORTCUT}"
                ));
            }
            Err(e) => self.toast(&format!("Could not add the shortcut: {e}")),
        }
    }

    fn set_autostart(&self, on: bool) {
        self.state.update_config(|c| c.start_at_login = on);
        if !autostart::running_installed() {
            self.toast("Autostart applies to the installed app");
        } else if let Err(e) = autostart::sync(on) {
            self.state.update_config(|c| c.start_at_login = !on);
            self.toast(&format!("Could not update autostart: {e}"));
        }
    }
}

/// Runs `f` on the dialog's state unless the controls are being synced.
fn bind<A>(ui: &Rc<Ui>, f: impl Fn(&Rc<Ui>) + 'static) -> impl Fn(&A) + 'static {
    let weak: Weak<Ui> = Rc::downgrade(ui);
    move |_| {
        if let Some(ui) = weak.upgrade()
            && !ui.syncing.get()
        {
            f(&ui);
        }
    }
}

pub fn show_settings(window: &ColorzWindow, state: &AppState, shortcut: &Rc<ShortcutCell>) {
    let format_labels: Vec<&str> = Format::ALL.iter().map(|f| f.label()).collect();
    let default_format = combo("Default format", &format_labels);
    let form = combo("Default form", &labels(&FORMS));
    let hex_case = combo("Hex letters", &labels(&HEX_CASES));
    let css_syntax = combo("CSS syntax", &labels(&SYNTAXES));
    let decimals = adw::SpinRow::with_range(MIN_DECIMALS as f64, MAX_DECIMALS as f64, 1.0);
    decimals.set_title("Decimal places");
    decimals.set_subtitle("OKLCH and OKLab; HSL, HSV and color() follow");
    let visible: Vec<(Format, adw::SwitchRow)> = Format::ALL
        .iter()
        .map(|f| (*f, adw::SwitchRow::builder().title(f.label()).build()))
        .collect();

    let formats_group = adw::PreferencesGroup::builder().title("Copy").build();
    for row in [&default_format, &form, &hex_case, &css_syntax] {
        formats_group.add(row);
    }
    formats_group.add(&decimals);
    let visible_group = adw::PreferencesGroup::builder()
        .title("Shown formats")
        .description("Formats listed on the Pick tab")
        .build();
    for (_, row) in &visible {
        visible_group.add(row);
    }
    let formats_page = adw::PreferencesPage::builder()
        .title("Formats")
        .icon_name("edit-copy-symbolic")
        .build();
    formats_page.add(&formats_group);
    formats_page.add(&visible_group);

    let target_row = adw::ComboRow::builder()
        .title("Copy as")
        .subtitle("Color space for HEX, RGB, HSL and HSV")
        .model(&gtk::StringList::new(&[]))
        .build();
    let chrome = adw::SwitchRow::builder()
        .title("Correct picks from Chrome")
        .subtitle("Only for colors picked from Chrome. Turn off for other apps.")
        .build();
    let space_group = adw::PreferencesGroup::builder()
        .title("Color space")
        .build();
    space_group.add(&target_row);
    space_group.add(&chrome);
    let icc_group = adw::PreferencesGroup::builder()
        .title("Custom ICC profiles")
        .build();
    let add_icc = gtk::Button::builder()
        .label("Add ICC profile…")
        .halign(gtk::Align::Start)
        .css_classes(["pill"])
        .margin_top(6)
        .build();
    let icc_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    icc_box.append(&add_icc);
    let add_group = adw::PreferencesGroup::new();
    add_group.add(&icc_box);
    let color_page = adw::PreferencesPage::builder()
        .title("Color management")
        .icon_name("applications-graphics-symbolic")
        .build();
    color_page.add(&space_group);
    color_page.add(&icc_group);
    color_page.add(&add_group);

    let autostart_row = adw::SwitchRow::builder().title("Start at login").build();
    let history = adw::SpinRow::with_range(5.0, 100.0, 1.0);
    history.set_title("Recent picks kept");
    let fallback_button = gtk::Button::builder()
        .label("Use GNOME keyboard shortcut instead")
        .valign(gtk::Align::Center)
        .build();
    let shortcut_row = adw::ActionRow::builder()
        .title("Global shortcut")
        .use_markup(false)
        .build();
    shortcut_row.add_suffix(&fallback_button);
    let general_group = adw::PreferencesGroup::new();
    general_group.add(&autostart_row);
    general_group.add(&history);
    general_group.add(&shortcut_row);
    let general_page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();
    general_page.add(&general_group);

    let dialog = adw::PreferencesDialog::builder().title("Settings").build();
    dialog.add(&formats_page);
    dialog.add(&color_page);
    dialog.add(&general_page);

    let ui = Rc::new(Ui {
        dialog: dialog.clone(),
        state: state.clone(),
        shortcut: shortcut.clone(),
        syncing: Cell::new(true),
        default_format: default_format.clone(),
        form: form.clone(),
        hex_case: hex_case.clone(),
        css_syntax: css_syntax.clone(),
        decimals: decimals.clone(),
        visible: visible.clone(),
        target_row: target_row.clone(),
        target_choices: RefCell::default(),
        icc_group,
        icc_rows: RefCell::default(),
        chrome: chrome.clone(),
        autostart: autostart_row.clone(),
        history: history.clone(),
        shortcut_row,
        fallback_button: fallback_button.clone(),
    });

    default_format.connect_selected_notify(bind(&ui, |ui| {
        if let Some(f) = Format::ALL.get(ui.default_format.selected() as usize) {
            // The default format is always shown.
            ui.state.update_config(|c| {
                c.default_format = *f;
                if !c.visible_formats.contains(f) {
                    c.visible_formats.push(*f);
                }
            });
        }
    }));
    form.connect_selected_notify(bind(&ui, |ui| {
        if let Some((v, _)) = FORMS.get(ui.form.selected() as usize) {
            ui.state.update_config(|c| c.default_form = *v);
        }
    }));
    hex_case.connect_selected_notify(bind(&ui, |ui| {
        if let Some((v, _)) = HEX_CASES.get(ui.hex_case.selected() as usize) {
            ui.state.update_config(|c| c.hex_case = *v);
        }
    }));
    css_syntax.connect_selected_notify(bind(&ui, |ui| {
        if let Some((v, _)) = SYNTAXES.get(ui.css_syntax.selected() as usize) {
            ui.state.update_config(|c| c.css_syntax = *v);
        }
    }));
    decimals.connect_value_notify(bind(&ui, |ui| {
        let n = ui.decimals.value().round() as usize;
        ui.state.update_config(|c| c.decimals = n);
    }));
    for (format, row) in &visible {
        let format = *format;
        let row2 = row.clone();
        row.connect_active_notify(bind(&ui, move |ui| {
            let on = row2.is_active();
            ui.state.update_config(|c| {
                if c.default_format == format {
                    return;
                }
                // Keep the display order of `Format::ALL`.
                c.visible_formats = Format::ALL
                    .into_iter()
                    .filter(|f| {
                        if *f == format {
                            on
                        } else {
                            c.visible_formats.contains(f)
                        }
                    })
                    .collect();
            });
        }));
    }
    target_row.connect_selected_notify(bind(&ui, |ui| {
        let choice = ui
            .target_choices
            .borrow()
            .get(ui.target_row.selected() as usize)
            .cloned();
        if let Some(choice) = choice
            && ui.state.config().target != choice
        {
            ui.state.update_config(|c| c.target = choice);
        }
    }));
    chrome.connect_active_notify(bind(&ui, |ui| {
        let on = ui.chrome.is_active();
        ui.state.update_config(|c| c.chrome_correction = on);
    }));
    autostart_row.connect_active_notify(bind(&ui, |ui| {
        ui.set_autostart(ui.autostart.is_active());
    }));
    history.connect_value_notify(bind(&ui, |ui| {
        let n = ui.history.value().round() as usize;
        ui.state.update_config(|c| c.history_size = n);
    }));
    add_icc.connect_clicked(bind(&ui, |ui| ui.add_icc_file()));
    fallback_button.connect_clicked(bind(&ui, |ui| ui.install_fallback()));

    ui.refresh();
    let weak = Rc::downgrade(&ui);
    let id = state.connect(move |e| {
        if matches!(e, StateEvent::Config)
            && let Some(ui) = weak.upgrade()
        {
            ui.refresh();
        }
    });
    let weak = Rc::downgrade(&ui);
    let shortcut_id = shortcut.connect(move || {
        if let Some(ui) = weak.upgrade() {
            ui.refresh_shortcut(&ui.state.config());
        }
    });
    let (state2, shortcut2) = (state.clone(), shortcut.clone());
    dialog.connect_closed(move |_| {
        state2.disconnect(id);
        shortcut2.disconnect(shortcut_id);
    });
    // The dialog owns the controls and their handlers; keep the Ui alive
    // exactly as long as the dialog is.
    let keep = RefCell::new(Some(ui));
    dialog.connect_closed(move |_| drop(keep.take()));
    dialog.present(Some(window.window()));
}

pub fn show_shortcuts(window: &ColorzWindow, pick_trigger: Option<&str>) {
    let dialog = adw::ShortcutsDialog::new();

    let general = adw::ShortcutsSection::new(Some("General"));
    let pick = adw::ShortcutsItem::new("Pick a color from the screen", "<Super><Shift>c");
    if let Some(trigger) = pick_trigger.filter(|t| !t.is_empty() && *t != DEFAULT_PICK_SHORTCUT) {
        pick.set_subtitle(&format!("Currently {trigger}"));
    }
    general.add(pick);
    general.add(adw::ShortcutsItem::new("Paste a color", "<Control>v"));
    general.add(adw::ShortcutsItem::new("Settings", "<Control>comma"));
    general.add(adw::ShortcutsItem::new("Quit", "<Control>q"));
    dialog.add(general);

    let wheel = adw::ShortcutsSection::new(Some("Color wheel"));
    let nudge = adw::ShortcutsItem::new("Nudge the color", "Left Right Up Down");
    nudge.set_subtitle("In the Ring layout, Left and Right change saturation, Up and Down value");
    wheel.add(nudge);
    wheel.add(adw::ShortcutsItem::new(
        "Nudge in larger steps",
        "<Shift>Left <Shift>Right <Shift>Up <Shift>Down",
    ));
    wheel.add(adw::ShortcutsItem::new("Turn the hue", "Page_Up Page_Down"));
    wheel.add(adw::ShortcutsItem::new(
        "Turn the hue ten times as far",
        "<Shift>Page_Up <Shift>Page_Down",
    ));
    let fine = adw::ShortcutsItem::new("Fine adjustment", "");
    fine.set_subtitle("Shift + drag");
    wheel.add(fine);
    dialog.add(wheel);

    dialog.present(Some(window.window()));
}

pub fn show_about(window: &ColorzWindow) {
    let dialog = adw::AboutDialog::new();
    dialog.set_application_name("Colorz");
    dialog.set_application_icon(crate::APP_ID);
    dialog.set_version(env!("CARGO_PKG_VERSION"));
    dialog.set_license_type(gtk::License::Gpl30);
    dialog.set_developer_name("Ogi Tsonev");
    dialog.set_copyright("© 2026 Ogi Tsonev");
    dialog.set_website(env!("CARGO_PKG_REPOSITORY"));
    dialog.set_issue_url(concat!(env!("CARGO_PKG_REPOSITORY"), "/issues"));
    dialog.set_comments("Color picker and palette tool for GNOME");
    dialog.present(Some(window.window()));
}
