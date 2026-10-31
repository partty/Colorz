//! Widgets and windows.

pub mod channel_slider;
pub mod copy_pair;
pub mod css_dialogs;
pub mod format_row;
pub mod image_dialog;
pub mod palette_actions;
pub mod palette_bar;
pub mod palette_view;
pub mod palettes_page;
pub mod pick_page;
pub mod settings;
pub mod targets;
pub mod wheel_channels;
pub mod wheel_page;
pub mod wheel_widgets;
pub mod window;

use std::cell::Cell;
use std::path::PathBuf;
use std::pin::pin;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use futures_util::future::select;
use gtk::{gdk, gio, glib};

use crate::state::AppState;
pub use image_dialog::open_image_dialog_from_texture;
use window::ColorzWindow;

const CSS: &str = "
.badge, .default-pill {
    font-size: 0.8em;
    font-weight: bold;
    padding: 1px 8px;
    border-radius: 99px;
}
.badge {
    background: alpha(@warning_bg_color, 0.25);
    color: @warning_color;
}
.default-pill {
    background: alpha(@accent_bg_color, 0.2);
    color: @accent_color;
}
row.default-format {
    box-shadow: inset 3px 0 0 @accent_bg_color;
}
row.mono-subtitle .subtitle {
    font-family: monospace;
}
button.numbers-button {
    font-weight: normal;
    font-size: 0.9em;
    font-family: monospace;
}
entry.mono-entry {
    font-family: monospace;
}
button.before-swatch {
    padding: 0;
    min-width: 0;
    min-height: 0;
    border-radius: 8px 0 0 8px;
}
.oog-indicator {
    font-weight: bold;
    padding: 2px 8px;
    border-radius: 99px;
    background: alpha(@window_bg_color, 0.85);
}
label.channel-name {
    font-weight: bold;
}
scale.channel-scale > trough {
    min-height: 12px;
    border-radius: 6px;
    background: none;
    box-shadow: inset 0 0 0 1px alpha(currentColor, 0.15);
}
scale.channel-scale > trough > highlight,
scale.channel-scale > trough > fill {
    background: none;
    box-shadow: none;
}
.mono-text {
    font-family: monospace;
}
.harmony-card {
    border-radius: 10px;
}
.harmony-card.current {
    box-shadow: 0 0 0 2px @window_bg_color, 0 0 0 4px @accent_bg_color;
}
.palette-bar {
    background: alpha(currentColor, 0.06);
    border-radius: 12px;
    padding: 6px 8px;
}
button.palette-chip {
    padding: 0;
    min-width: 30px;
    min-height: 30px;
    border-radius: 7px;
}
button.palette-chip.current {
    box-shadow: 0 0 0 2px @window_bg_color, 0 0 0 4px @accent_bg_color;
}
button.swatch-chip {
    padding: 0;
    min-width: 28px;
    min-height: 28px;
    border-radius: 6px;
}
";

/// Loads the app's CSS for the default display, once.
pub fn load_css() {
    thread_local!(static LOADED: Cell<bool> = const { Cell::new(false) });
    if LOADED.replace(true) {
        return;
    }
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// Where the bundled icons live, in the hicolor layout
/// (`scalable/apps/<app id>.svg`).
const ICON_RESOURCE_PATH: &str = "/io/github/partty/Colorz/icons";

/// Registers the bundled app icon with the default icon theme, so the icon
/// shows when running from `target/`; an installed icon is found as well.
pub fn load_icons() {
    if let Err(e) = gio::resources_register_include!("colorz.gresource") {
        eprintln!("colorz: cannot load bundled resources: {e}");
        return;
    }
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let theme = gtk::IconTheme::for_display(&display);
    if !theme
        .resource_path()
        .iter()
        .any(|p| p == ICON_RESOURCE_PATH)
    {
        theme.add_resource_path(ICON_RESOURCE_PATH);
    }
}

/// Default name offered for a new palette.
pub const NEW_PALETTE_NAME: &str = "New palette";

/// Puts `text` on the clipboard; false when the app is not available.
pub fn copy_text(window: &ColorzWindow, text: &str) -> bool {
    let app = window
        .window()
        .application()
        .and_downcast::<adw::Application>();
    match app {
        Some(app) => {
            crate::platform::clipboard::copy_text(&app, window, text);
            true
        }
        None => false,
    }
}

/// Puts `text` on the clipboard and confirms with a "Copied …" toast.
pub fn copy_with_toast(window: &ColorzWindow, text: &str) {
    if copy_text(window, text) {
        window.show_toast(&format!("Copied {text}"));
    }
}

/// Asks for a palette name in a dialog; `on_confirm` gets the trimmed,
/// non-empty name.
pub fn prompt_name(
    window: &ColorzWindow,
    heading: &str,
    initial: &str,
    confirm_label: &str,
    on_confirm: impl Fn(String) + 'static,
) {
    let entry = gtk::Entry::builder()
        .text(initial)
        .activates_default(true)
        .build();
    entry.update_property(&[gtk::accessible::Property::Label("Palette name")]);
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .extra_child(&entry)
        .default_response("ok")
        .close_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", "_Cancel"), ("ok", confirm_label)]);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.set_response_enabled("ok", !initial.trim().is_empty());
    entry.connect_changed({
        let dialog = dialog.downgrade();
        move |e| {
            if let Some(d) = dialog.upgrade() {
                d.set_response_enabled("ok", !e.text().trim().is_empty());
            }
        }
    });
    dialog.connect_response(Some("ok"), {
        let entry = entry.clone();
        move |_, _| {
            let name = entry.text();
            let name = name.trim();
            if !name.is_empty() {
                on_confirm(name.to_owned());
            }
        }
    });
    dialog.set_focus(Some(&entry));
    entry.select_region(0, -1);
    dialog.present(Some(window.window()));
}

/// Waits for `n` frames of `widget`'s frame clock (at most a second each).
async fn wait_frames(widget: &impl IsA<gtk::Widget>, n: u32) {
    for _ in 0..n {
        let (tx, rx) = async_channel::bounded::<()>(1);
        widget.add_tick_callback(move |_, _| {
            let _ = tx.try_send(());
            glib::ControlFlow::Break
        });
        let frame = pin!(rx.recv());
        let timeout = pin!(glib::timeout_future(Duration::from_secs(1)));
        select(frame, timeout).await;
    }
}

/// Renders the window as it is now to `path`.
fn save_window_png(window: &adw::ApplicationWindow, path: &std::path::Path) -> Result<(), String> {
    let (w, h) = (window.width(), window.height());
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(w), f64::from(h));
    let node = snapshot.to_node().ok_or("nothing was drawn")?;
    let renderer = window
        .native()
        .and_then(|n| n.renderer())
        .ok_or("the window has no renderer")?;
    let texture = renderer.render_texture(&node, None);
    texture.save_to_png(path).map_err(|e| e.to_string())
}

/// Development aid: with `COLORZ_DEBUG_SNAPSHOT=<dir>` set, shows the
/// window, saves each page as `<dir>/<page>.png` and quits. Does nothing
/// when the variable is unset. With `COLORZ_DEBUG_SNAPSHOT_IMAGE=<file>`
/// also set, it then opens the image dialog for that file and saves
/// `<dir>/image.png`. With `COLORZ_DEBUG_SNAPSHOT_DIALOGS=1` also set, it
/// finally saves `settings.png`, `shortcuts.png` and `about.png` of those
/// dialogs.
pub fn debug_snapshot(app: &adw::Application, window: &ColorzWindow, state: &AppState) {
    let Some(dir) = std::env::var_os("COLORZ_DEBUG_SNAPSHOT").map(PathBuf::from) else {
        return;
    };
    let image = std::env::var_os("COLORZ_DEBUG_SNAPSHOT_IMAGE").map(PathBuf::from);
    let (app, window, state) = (app.clone(), window.clone(), state.clone());
    glib::spawn_future_local(async move {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("colorz: snapshot: cannot create {}: {e}", dir.display());
        }
        window.present();
        wait_frames(window.window(), 2).await;
        glib::timeout_future(Duration::from_millis(300)).await;
        for page in window.page_names() {
            window.set_page(page);
            wait_frames(window.window(), 2).await;
            glib::timeout_future(Duration::from_millis(300)).await;
            let path = dir.join(format!("{page}.png"));
            if let Err(e) = save_window_png(window.window(), &path) {
                eprintln!("colorz: snapshot: {}: {e}", path.display());
            }
        }
        if let Some(image) = image {
            image_dialog::debug_open(&window, &state, &image, Duration::from_secs(3)).await;
            wait_frames(window.window(), 2).await;
            glib::timeout_future(Duration::from_millis(300)).await;
            let path = dir.join("image.png");
            if let Err(e) = save_window_png(window.window(), &path) {
                eprintln!("colorz: snapshot: {}: {e}", path.display());
            }
        }
        if std::env::var_os("COLORZ_DEBUG_SNAPSHOT_DIALOGS").is_some_and(|v| v == "1") {
            snapshot_dialogs(&window, &state, &dir).await;
        }
        app.quit();
    });
}

/// Saves `settings.png`, `shortcuts.png` and `about.png` of the dialogs.
async fn snapshot_dialogs(window: &ColorzWindow, state: &AppState, dir: &std::path::Path) {
    use settings::{ShortcutCell, ShortcutStatus};
    let shot = |name: &'static str| async move {
        wait_frames(window.window(), 2).await;
        glib::timeout_future(Duration::from_millis(500)).await;
        let path = dir.join(name);
        if let Err(e) = save_window_png(window.window(), &path) {
            eprintln!("colorz: snapshot: {}: {e}", path.display());
        }
        if let Some(d) = window.window().visible_dialog() {
            d.force_close();
        }
        glib::timeout_future(Duration::from_millis(300)).await;
    };
    let status = Rc::new(ShortcutCell::default());
    status.set(ShortcutStatus::Bound("Super+Shift+C".into()));
    settings::show_settings(window, state, &status);
    shot("settings.png").await;
    settings::show_shortcuts(window, Some("Super+Shift+C"));
    shot("shortcuts.png").await;
    settings::show_about(window);
    shot("about.png").await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_icon_is_bundled_in_hicolor_layout() {
        gio::resources_register_include!("colorz.gresource").unwrap();
        let path = format!("{ICON_RESOURCE_PATH}/scalable/apps/{}.svg", crate::APP_ID);
        let svg = gio::resources_lookup_data(&path, gio::ResourceLookupFlags::NONE).unwrap();
        assert!(std::str::from_utf8(&svg).unwrap().contains("<svg"));
    }
}
