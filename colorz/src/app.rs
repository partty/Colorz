//! The GApplication: single instance, command line, actions and lifetime.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use colorz_core::color::Color;
use colorz_core::format::{Format, Formatted};
use colorz_core::palette::PaletteColor;
use colorz_core::palette_ops::{color_name, palette_display_name};
use colorz_core::parse::{ParseContext, parse_color};
use colorz_core::storage::Paths;
use gtk::{gio, glib};

use crate::APP_ID;
use crate::platform::tray::{self, TrayCommand, TrayEvent, TrayHandle, TrayModel};
use crate::platform::{CAN_PICK, autostart, clipboard, notify, pick, shortcuts};
use crate::state::{AppState, ColorSource, PickPurpose, PickRoute, StateEvent, pick_route};
use crate::ui::palette_actions;
use crate::ui::palettes_page::PalettesPage;
use crate::ui::pick_page::PickPage;
use crate::ui::settings::{ShortcutCell, ShortcutStatus};
use crate::ui::wheel_page::WheelPage;
use crate::ui::window::ColorzWindow;
use crate::ui::{self, open_image_dialog_from_texture};

const UNDO_FAILED: &str = "Can't undo: the palette has changed";
/// The toast for a replaced row, named as it was before the replacement.
fn replaced_message(before: &PaletteColor) -> String {
    format!("Replaced {}", color_name(before))
}

/// After this long, a pick the portal never answered no longer blocks a
/// new one.
const PICK_STUCK_AFTER: Duration = Duration::from_secs(60);
/// How long a paste waits for the window to get focus before reading the
/// clipboard anyway.
const PASTE_FOCUS_TIMEOUT: Duration = Duration::from_secs(1);
/// Shown for the pick shortcut when the portal does not describe it.
const DEFAULT_SHORTCUT_LABEL: &str = "Super+Shift+C";

/// Everything owned by the primary instance.
struct Colorz {
    state: AppState,
    window: ColorzWindow,
    /// Own the pages' state bindings.
    _pick_page: PickPage,
    _wheel_page: WheelPage,
    _palettes_page: PalettesPage,
    /// When the screen pick in progress started.
    picking: Cell<Option<Instant>>,
    tray: TrayHandle,
    /// The recent picks the tray menu was last given, so that a click
    /// copies the color it showed.
    tray_recent: RefCell<Vec<Color>>,
    /// How the global pick shortcut is triggered; empty when not bound.
    shortcut_label: RefCell<String>,
    /// How binding the global shortcut went, for Settings.
    shortcut_status: Rc<ShortcutCell>,
    /// Keeps the app running while no window is shown.
    _hold: gio::ApplicationHoldGuard,
}

impl Colorz {
    /// Whether the user is looking at the window. A shown window may still
    /// be minimized, buried or on another workspace, so visibility is not
    /// enough.
    fn window_focused(&self) -> bool {
        let w = self.window.window();
        w.is_visible() && w.is_active()
    }

    /// Shows `msg` as a toast when `in_window`, else as a notification.
    fn report(&self, app: &adw::Application, id: &str, msg: &str, in_window: bool) {
        if in_window {
            self.window.show_toast(msg);
        } else {
            notify::notify(app, id, msg);
        }
    }

    /// Picks a color from the screen, makes it current and copies it.
    fn pick(self: &Rc<Self>, app: &adw::Application) {
        let started = Instant::now();
        if let Some(prev) = self.picking.get()
            && started.duration_since(prev) < PICK_STUCK_AFTER
        {
            // A token passed for this no-op would be stale by the next copy.
            clipboard::set_activation_token(None);
            return;
        }
        self.picking.set(Some(started));
        // Decided before the picker opens, which may take focus.
        let in_window = self.window_focused();
        let (c, app) = (self.clone(), app.clone());
        glib::spawn_future_local(async move {
            let main = c.window.window();
            let parent = main
                .is_visible()
                .then(|| main.clone().upcast::<gtk::Window>());
            let result = pick::pick_color(parent.as_ref()).await;
            if c.picking.get() == Some(started) {
                c.picking.set(None);
            }
            match result {
                Ok(Some(rgb)) => c.picked(&app, rgb, in_window),
                Ok(None) => {
                    c.state.take_pick_purpose();
                    clipboard::set_activation_token(None);
                }
                Err(msg) => {
                    c.state.take_pick_purpose();
                    clipboard::set_activation_token(None);
                    let msg = format!("Pick failed: {msg}");
                    c.report(&app, "pick-failed", &msg, in_window);
                }
            }
        });
    }

    /// `in_window`: the window had focus when the pick started.
    fn picked(&self, app: &adw::Application, rgb: [f64; 3], in_window: bool) {
        let config = self.state.config();
        let color = pick::pick_to_color(rgb, config.chrome_correction);
        // The purpose is consumed by whichever pick finishes next. If the
        // portal never answered an earlier pick, a new pick (started after
        // the PICK_STUCK_AFTER guard) has already set its own purpose, so
        // the stale one is simply overwritten; nothing else expires it.
        let purpose = self.state.take_pick_purpose();
        let active = self.state.active_palette().map(|p| p.id);
        let route = pick_route(
            purpose,
            self.state.editing().is_some(),
            config.auto_add_picks,
            active.as_deref(),
        );
        self.state.push_history(color);
        // Never `None` in practice: the default format falls back to HEX.
        let Some(formatted) = self.state.default_text(&color) else {
            clipboard::set_activation_token(None);
            let msg = "Pick failed: the color cannot be shown in the default format";
            self.report(app, "pick-failed", msg, in_window);
            return;
        };
        let routed = match route {
            PickRoute::Purpose(PickPurpose::AddTo { palette_id }) => {
                self.pick_add(app, &palette_id, color, in_window)
            }
            PickRoute::Purpose(PickPurpose::Replace {
                palette_id,
                index,
                original,
            }) => self.pick_replace(app, &palette_id, index, &original, color, in_window),
            PickRoute::Edit => {
                self.state.set_current(color, ColorSource::Pick);
                false
            }
            PickRoute::AutoAdd(id) => {
                self.state.set_current(color, ColorSource::Pick);
                if in_window {
                    self.window.set_page("pick");
                }
                self.pick_add(app, &id, color, in_window)
            }
            PickRoute::Normal => {
                self.state.set_current(color, ColorSource::Pick);
                if in_window {
                    self.window.set_page("pick");
                }
                false
            }
        };
        // One toast per pick: a palette toast replaces "Copied".
        self.copy(app, &color, &formatted, in_window, !(routed && in_window));
    }

    /// Adds a pick to palette `id`, reporting it. Returns whether a
    /// message was shown.
    fn pick_add(&self, app: &adw::Application, id: &str, color: Color, in_window: bool) -> bool {
        let Some(pal) = self.state.add_to_palette(id, &[color]) else {
            self.report(app, "palette-add", "That color no longer exists", in_window);
            return true;
        };
        let msg = palette_actions::added_message(1, &palette_display_name(&pal));
        if in_window {
            let (state, window) = (self.state.clone(), self.window.clone());
            let id = id.to_owned();
            self.window.toast_with_undo(&msg, move || {
                if !state.undo_add(&id, &[color]) {
                    window.show_toast(UNDO_FAILED);
                }
            });
        } else {
            notify::notify(app, "palette-add", &msg);
        }
        true
    }

    /// Replaces the color at `index` of palette `id` with a pick, if it
    /// still holds `original`. Returns whether a message was shown.
    fn pick_replace(
        &self,
        app: &adw::Application,
        id: &str,
        index: usize,
        original: &Color,
        color: Color,
        in_window: bool,
    ) -> bool {
        let before = self
            .state
            .palette(id)
            .and_then(|p| p.colors.get(index).cloned())
            .filter(|pc| pc.color == *original);
        let old = before
            .as_ref()
            .and_then(|_| self.state.replace_in_palette(id, index, color));
        let (Some(before), Some(old)) = (before, old) else {
            self.report(
                app,
                "palette-replace",
                "That color no longer exists",
                in_window,
            );
            return true;
        };
        let msg = replaced_message(&before);
        if in_window {
            let (state, window) = (self.state.clone(), self.window.clone());
            let id = id.to_owned();
            self.window.toast_with_undo(&msg, move || {
                if !state.undo_replace(&id, index, old, color) {
                    window.show_toast(UNDO_FAILED);
                }
            });
        } else {
            notify::notify(app, "palette-replace", &msg);
        }
        true
    }

    /// Copies recent pick `i` of the tray menu in the default format.
    fn copy_recent(&self, app: &adw::Application, i: usize) {
        let Some(color) = self.tray_recent.borrow().get(i).copied() else {
            clipboard::set_activation_token(None);
            return;
        };
        let in_window = self.window_focused();
        match self.state.default_text(&color) {
            Some(formatted) => self.copy(app, &color, &formatted, in_window, true),
            None => {
                clipboard::set_activation_token(None);
                let msg = "Copy failed: the color cannot be shown in the default format";
                self.report(app, "copy-failed", msg, in_window);
            }
        }
    }

    /// Puts `formatted` (`color` in the default format) on the clipboard
    /// and says so in a toast when `in_window`, else in a notification.
    fn copy(
        &self,
        app: &adw::Application,
        color: &Color,
        formatted: &Formatted,
        in_window: bool,
        say_so: bool,
    ) {
        let text = &formatted.text;
        clipboard::copy_text(app, &self.window, text);
        if in_window {
            if say_so {
                self.window.show_toast(&format!("Copied {text}"));
            }
        } else {
            // OKLCH and OKLab do not depend on the "copy as" target.
            let format = self.state.default_format();
            let uses_target = !matches!(format, Format::Oklch | Format::Oklab);
            let target = self
                .state
                .with_target(|t| (uses_target && !t.is_srgb()).then(|| t.name()));
            let oog = formatted.out_of_gamut;
            notify::notify_copied(app, text, color, target.as_deref(), oog);
        }
    }

    /// Pastes the clipboard: an image opens the image dialog, text is
    /// parsed as a color and becomes the current one.
    fn paste(self: &Rc<Self>) {
        // Shown first so that a toast or the new color is seen.
        self.window.present();
        let c = self.clone();
        glib::spawn_future_local(async move {
            // On Wayland the selection is only readable once focused.
            let main: gtk::Window = c.window.window().clone().upcast();
            let (tx, rx) = async_channel::bounded::<()>(1);
            let tx2 = tx.clone();
            crate::platform::focus::when_active(
                &main,
                PASTE_FOCUS_TIMEOUT,
                move || {
                    let _ = tx.try_send(());
                },
                move || {
                    let _ = tx2.try_send(());
                },
            );
            let _ = rx.recv().await;
            let clipboard = c.window.window().clipboard();
            let has_image = clipboard
                .formats()
                .contains_type(gtk::gdk::Texture::static_type());
            if has_image && let Ok(Some(texture)) = clipboard.read_texture_future().await {
                open_image_dialog_from_texture(&c.window, &c.state, texture);
                return;
            }
            let text = clipboard.read_text_future().await.ok().flatten();
            let parsed = text.and_then(|text| {
                let default_format = c.state.config().default_format;
                c.state.with_target(|target| {
                    parse_color(
                        &text,
                        &ParseContext {
                            default_format,
                            target,
                        },
                    )
                    .ok()
                })
            });
            match parsed {
                Some(color) => {
                    c.state.set_current(color, ColorSource::Paste);
                    c.window.set_page("pick");
                }
                None => c.window.show_toast("No color found in clipboard"),
            }
        });
    }

    fn refresh_tray(&self) {
        let (model, recent) = tray_snapshot(&self.state, &self.shortcut_label.borrow());
        self.tray_recent.replace(recent);
        self.tray.update(model);
    }
}

/// What the tray menu shows, plus the colors of its recent picks: entry
/// `i` of the menu is color `i`.
fn tray_snapshot(state: &AppState, shortcut_label: &str) -> (TrayModel, Vec<Color>) {
    let colors = state.history().entries;
    let recent = colors.iter().map(|c| {
        let [r, g, b] = c.to_srgb8();
        (format!("#{r:02X}{g:02X}{b:02X}"), [r, g, b])
    });
    let model = TrayModel {
        recent: recent.collect(),
        chrome: state.config().chrome_correction,
        shortcut_label: shortcut_label.to_owned(),
    };
    (model, colors)
}

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    if CAN_PICK {
        app.add_main_option(
            "pick",
            glib::Char::from(0),
            glib::OptionFlags::NONE,
            glib::OptionArg::None,
            "Pick a color from the screen",
            None,
        );
    }
    app.add_main_option(
        "background",
        glib::Char::from(0),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Start without opening the window",
        None,
    );

    let ctx: Rc<OnceCell<Rc<Colorz>>> = Rc::new(OnceCell::new());

    app.connect_startup({
        let ctx = ctx.clone();
        move |app| {
            ui::load_icons();
            gtk::Window::set_default_icon_name(APP_ID);
            ui::load_css();
            let state = AppState::new(Paths::user());
            let (model, recent) = tray_snapshot(&state, "");
            let (events, tray_events) = async_channel::unbounded();
            let tray = tray::spawn(events, model);
            let window = ColorzWindow::new(app);
            let pick_page = PickPage::new(&state, &window);
            window.set_pick_page(pick_page.widget());
            let wheel_page = WheelPage::new(&state, &window);
            window.set_wheel_page(wheel_page.widget());
            let palettes_page = PalettesPage::new(&state, &window);
            window.set_palettes_page(palettes_page.widget());
            let colorz = Rc::new(Colorz {
                state,
                window,
                _pick_page: pick_page,
                _wheel_page: wheel_page,
                _palettes_page: palettes_page,
                picking: Cell::new(None),
                tray,
                tray_recent: RefCell::new(recent),
                shortcut_label: RefCell::new(String::new()),
                shortcut_status: Rc::new(ShortcutCell::default()),
                _hold: app.hold(),
            });
            install_actions(app, &colorz);
            ui::image_dialog::install_drop_target(&colorz.window, &colorz.state);
            install_paste_shortcut(app, &colorz);
            report_save_failures(app, &colorz);
            handle_tray_events(app, &colorz, tray_events);
            follow_state(&colorz);
            if CAN_PICK {
                register_shortcut(app, &colorz);
            }
            ui::debug_snapshot(app, &colorz.window, &colorz.state);
            if ctx.set(colorz).is_err() {
                unreachable!("startup runs once");
            }
        }
    });

    // Runs in the primary instance for every launch, including remote ones.
    app.connect_command_line({
        let ctx = ctx.clone();
        move |app, cmdline| {
            let opts = cmdline.options_dict();
            if opts.contains("pick") {
                app.activate_action("pick", None);
            } else if !opts.contains("background")
                && let Some(c) = ctx.get()
            {
                c.window.present();
            }
            glib::ExitCode::SUCCESS
        }
    });

    // D-Bus activation (e.g. clicking a notification).
    app.connect_activate(move |_| {
        if let Some(c) = ctx.get() {
            c.window.present();
        }
    });

    app.run()
}

fn add_action(app: &adw::Application, name: &str, f: impl Fn() + 'static) {
    let action = gio::SimpleAction::new(name, None);
    action.connect_activate(move |_, _| f());
    app.add_action(&action);
}

fn install_actions(app: &adw::Application, colorz: &Rc<Colorz>) {
    let c = colorz.clone();
    add_action(app, "preferences", move || {
        ui::settings::show_settings(&c.window, &c.state, &c.shortcut_status);
    });
    let c = colorz.clone();
    add_action(app, "shortcuts", move || {
        let status = c.shortcut_status.get();
        let trigger = match &status {
            ShortcutStatus::Bound(t) => Some(t.as_str()),
            _ => None,
        };
        ui::settings::show_shortcuts(&c.window, trigger);
    });
    let c = colorz.clone();
    add_action(app, "about", move || ui::settings::show_about(&c.window));
    let c = colorz.clone();
    add_action(app, "import-palette", move || {
        ui::css_dialogs::import_palette_file(&c.window, &c.state);
    });
    let c = colorz.clone();
    add_action(app, "palette-from-image", move || {
        ui::image_dialog::pick_image_file(&c.window, &c.state);
    });
    let (c, weak) = (colorz.clone(), app.downgrade());
    add_action(app, "pick", move || {
        if let Some(app) = weak.upgrade() {
            c.pick(&app);
        }
    });
    let c = colorz.clone();
    add_action(app, "paste", move || c.paste());
    let c = colorz.clone();
    add_action(app, "show", move || c.window.present());
    let weak = app.downgrade();
    add_action(app, "quit", move || {
        if let Some(app) = weak.upgrade() {
            app.quit();
        }
    });

    app.set_accels_for_action("app.quit", &["<Ctrl>q"]);
    app.set_accels_for_action("app.preferences", &["<Ctrl>comma"]);
}

/// Ctrl+V pastes a color, except where a text field has the focus and
/// pastes text.
fn install_paste_shortcut(app: &adw::Application, colorz: &Rc<Colorz>) {
    let weak = app.downgrade();
    let action = gtk::CallbackAction::new(move |widget, _| {
        let in_text_field = widget
            .root()
            .and_then(|root| root.focus())
            .is_some_and(|focus| {
                std::iter::successors(Some(focus), |w| w.parent())
                    .any(|w| w.is::<gtk::Editable>() || w.is::<gtk::TextView>())
            });
        let dialog_open = widget
            .root()
            .and_downcast::<adw::ApplicationWindow>()
            .is_some_and(|w| w.visible_dialog().is_some());
        if in_text_field || dialog_open {
            return glib::Propagation::Proceed;
        }
        if let Some(app) = weak.upgrade() {
            app.activate_action("paste", None);
        }
        glib::Propagation::Stop
    });
    let shortcut = gtk::Shortcut::new(gtk::ShortcutTrigger::parse_string("<Ctrl>v"), Some(action));
    let controller = gtk::ShortcutController::new();
    controller.add_shortcut(shortcut);
    colorz.window.window().add_controller(controller);
}

fn report_save_failures(app: &adw::Application, colorz: &Rc<Colorz>) {
    let (c, weak) = (Rc::downgrade(colorz), app.downgrade());
    colorz.state.connect(move |e| {
        if let (StateEvent::SaveFailed(msg), Some(c), Some(app)) = (e, c.upgrade(), weak.upgrade())
        {
            c.report(&app, "save-failed", msg, c.window_focused());
        } else if let (StateEvent::EditingLost, Some(c)) = (e, c.upgrade()) {
            c.window.show_toast("Palette changed — editing stopped");
        }
    });
}

fn handle_tray_events(
    app: &adw::Application,
    colorz: &Rc<Colorz>,
    events: async_channel::Receiver<TrayEvent>,
) {
    let (c, weak) = (colorz.clone(), app.downgrade());
    glib::spawn_future_local(async move {
        while let Ok(TrayEvent { command, token }) = events.recv().await {
            let Some(app) = weak.upgrade() else { break };
            // On Wayland a window only gets focus with the click's token.
            let give_window_token = || {
                if let Some(token) = &token {
                    c.window.window().set_startup_id(token);
                }
            };
            match command {
                TrayCommand::Open => {
                    give_window_token();
                    c.window.present();
                }
                TrayCommand::Paste => {
                    give_window_token();
                    app.activate_action("paste", None);
                }
                TrayCommand::Pick => {
                    clipboard::set_activation_token(token);
                    app.activate_action("pick", None);
                }
                TrayCommand::CopyRecent(i) => {
                    clipboard::set_activation_token(token);
                    c.copy_recent(&app, i);
                }
                TrayCommand::ToggleChrome => c
                    .state
                    .update_config(|cfg| cfg.chrome_correction = !cfg.chrome_correction),
                TrayCommand::Quit => app.quit(),
            }
        }
    });
}

/// Whether to sync the autostart entry now: from an installed build only,
/// so development builds never touch it or use up the setup. Ubuntu does it
/// once; Windows (`every_start`) at every start.
fn should_sync_autostart(installed: bool, initialized: bool, every_start: bool) -> bool {
    installed && (every_start || !initialized)
}

/// Keeps the tray menu in step with the state, and syncs autostart.
fn follow_state(colorz: &Rc<Colorz>) {
    // Ubuntu: only the first installed run writes the autostart entry;
    // afterwards the Settings toggle does, so a choice made in GNOME Startup
    // Apps stays. Windows re-applies it from the config at every start.
    let cfg = colorz.state.config();
    if should_sync_autostart(
        autostart::running_installed(),
        cfg.autostart_initialized,
        autostart::SYNC_EVERY_START,
    ) {
        if let Err(e) = autostart::sync(cfg.start_at_login) {
            eprintln!("colorz: cannot update the autostart entry: {e}");
        } else {
            colorz
                .state
                .update_config(|c| c.autostart_initialized = true);
        }
    }
    let c = Rc::downgrade(colorz);
    colorz.state.connect(move |e| {
        let Some(c) = c.upgrade() else { return };
        match e {
            StateEvent::History => c.refresh_tray(),
            StateEvent::Config => c.refresh_tray(),
            _ => {}
        }
    });
}

/// Binds the global pick shortcut; without it, the user can set up the
/// fallback keybinding in Settings.
fn register_shortcut(app: &adw::Application, colorz: &Rc<Colorz>) {
    let (c, app) = (colorz.clone(), app.clone());
    glib::spawn_future_local(async move {
        match shortcuts::register(&app).await {
            Ok(trigger) => {
                let label = if trigger.is_empty() {
                    DEFAULT_SHORTCUT_LABEL
                } else {
                    trigger.as_str()
                };
                c.shortcut_label.replace(label.to_owned());
                c.shortcut_status.set(ShortcutStatus::Bound(trigger));
                c.refresh_tray();
            }
            Err(e) => {
                eprintln!("colorz: no global shortcut: {e}");
                c.shortcut_status.set(ShortcutStatus::Unavailable);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaced_message_names_old_row() {
        let mut pc = PaletteColor {
            color: Color::from_srgb8([0x12, 0x34, 0x56]),
            label: String::new(),
            var_name: String::new(),
        };
        assert_eq!(replaced_message(&pc), "Replaced #123456");
        pc.label = "Sky".into();
        assert_eq!(replaced_message(&pc), "Replaced Sky");
    }

    #[test]
    fn autostart_sync_rules() {
        // Ubuntu: once, from an installed build.
        assert!(should_sync_autostart(true, false, false));
        assert!(!should_sync_autostart(true, true, false));
        assert!(!should_sync_autostart(false, false, false));
        // Windows: every start of an installed build, never from target\.
        assert!(should_sync_autostart(true, true, true));
        assert!(should_sync_autostart(true, false, true));
        assert!(!should_sync_autostart(false, true, true));
    }

    #[test]
    fn tray_snapshot_keeps_what_the_menu_shows() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(Paths::in_dir(dir.path()));
        for rgb in [[255, 128, 1], [0, 0, 0], [0x34, 0x78, 0xF6]] {
            state.push_history(Color::from_srgb8(rgb));
        }
        let (model, colors) = tray_snapshot(&state, "Super+Shift+C");
        let hexes: Vec<_> = model.recent.iter().map(|(hex, _)| hex.as_str()).collect();
        assert_eq!(hexes, ["#3478F6", "#000000", "#FF8001"]);
        for (i, (_, rgb)) in model.recent.iter().enumerate() {
            assert_eq!(colors[i].to_srgb8(), *rgb);
        }
        assert_eq!(model.shortcut_label, "Super+Shift+C");
        assert!(!model.chrome);
        // A later pick shifts the history but not the snapshot the menu
        // was built from, so clicking entry 0 still copies #3478F6.
        state.push_history(Color::from_srgb8([9, 9, 9]));
        assert_eq!(
            colors.first().map(Color::to_srgb8),
            Some([0x34, 0x78, 0xF6])
        );
        assert_eq!(colors.get(3), None);
    }
}
