//! Clipboard writes that also work while no Colorz window has focus.
//!
//! On Wayland the compositor only takes a new selection from the client
//! that has keyboard focus. When the main window is not focused, a tiny,
//! nearly invisible window is mapped to get focus, the clipboard is set and
//! the window goes away again, so focus returns to the previous app (the
//! wl-clipboard technique). If that window is refused focus, the main
//! window is shown briefly instead. The app is held, so GTK keeps serving
//! the selection afterwards.
//!
//! Run with `COLORZ_DEBUG=1` to see which path each copy took.
//!
//! On Windows any window may set the clipboard, so it is set directly.

#[cfg(not(windows))]
use std::cell::{Cell, RefCell};
#[cfg(not(windows))]
use std::time::Duration;

use adw::prelude::*;
#[cfg(not(windows))]
use gtk::glib;

use super::debug;
#[cfg(not(windows))]
use super::focus::when_active;
use crate::ui::window::ColorzWindow;

/// How long the tiny window stays mapped after setting the clipboard.
#[cfg(not(windows))]
const TINY_LINGER: Duration = Duration::from_millis(300);
/// How long the tiny window may take to get focus before falling back.
#[cfg(not(windows))]
const TINY_ACTIVATE_TIMEOUT: Duration = Duration::from_millis(500);
/// How long a main window shown only for the copy stays up afterwards.
#[cfg(not(windows))]
const MAIN_LINGER: Duration = Duration::from_millis(600);
/// How long the main window may take to get focus before giving up.
#[cfg(not(windows))]
const MAIN_ACTIVATE_TIMEOUT: Duration = Duration::from_millis(2000);
/// Size and opacity of the tiny focus window.
#[cfg(not(windows))]
const TINY_SIZE: i32 = 1;
#[cfg(not(windows))]
const TINY_OPACITY: f64 = 0.01;

#[cfg(not(windows))]
thread_local! {
    /// Bumped on every copy, so a slow attempt never overwrites a newer one.
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    /// XDG activation token of the user action that led to the next copy.
    static ACTIVATION_TOKEN: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Sets (or clears) the XDG activation token for the next copy, e.g. the
/// one the global shortcut came with. Wayland only lets a window take
/// focus with such a token; it is handed to the window the copy presents.
#[cfg(not(windows))]
pub fn set_activation_token(token: Option<String>) {
    ACTIVATION_TOKEN.replace(token);
}

/// Presents `win`, using the pending activation token if there is one.
#[cfg(not(windows))]
fn present_with_token(win: &gtk::Window, present: impl FnOnce()) {
    if let Some(token) = ACTIVATION_TOKEN.take() {
        debug("clipboard: presenting with an activation token");
        win.set_startup_id(&token);
    }
    present();
}

#[cfg(not(windows))]
fn is_current(generation: u64) -> bool {
    GENERATION.get() == generation
}

/// Activation tokens are a Wayland thing; Windows has nothing to pass on.
#[cfg(windows)]
pub fn set_activation_token(_token: Option<String>) {}

/// Puts `text` on the clipboard. Windows takes it from any window, focused
/// or not.
#[cfg(windows)]
pub fn copy_text(_app: &adw::Application, window: &ColorzWindow, text: &str) {
    window.window().clipboard().set_text(text);
    debug(&format!("clipboard: direct: {text}"));
}

/// Puts `text` on the clipboard, whether or not the main window is shown.
#[cfg(not(windows))]
pub fn copy_text(app: &adw::Application, window: &ColorzWindow, text: &str) {
    let generation = GENERATION.get() + 1;
    GENERATION.set(generation);
    let main = window.window();
    if main.is_visible() && main.is_active() {
        set_activation_token(None);
        main.clipboard().set_text(text);
        debug(&format!("clipboard: direct: {text}"));
        return;
    }
    copy_via_tiny_window(app, window, text.to_owned(), generation);
}

#[cfg(not(windows))]
fn copy_via_tiny_window(
    app: &adw::Application,
    window: &ColorzWindow,
    text: String,
    generation: u64,
) {
    // Deliberately not transient for the main window: when a transient
    // window closes, Mutter focuses its parent instead of the previous app.
    let tiny = gtk::Window::builder()
        .application(app)
        .title("Colorz")
        .decorated(false)
        .resizable(false)
        .default_width(TINY_SIZE)
        .default_height(TINY_SIZE)
        .opacity(TINY_OPACITY)
        .build();
    let on_active = {
        let (tiny, text) = (tiny.clone(), text.clone());
        move || {
            if is_current(generation) {
                tiny.clipboard().set_text(&text);
                debug(&format!("clipboard: tiny window: {text}"));
            }
            glib::timeout_add_local_once(TINY_LINGER, move || tiny.destroy());
        }
    };
    let on_timeout = {
        let (tiny, window) = (tiny.clone(), window.clone());
        move || {
            tiny.destroy();
            if is_current(generation) {
                debug("clipboard: tiny window got no focus; using the main window");
                copy_via_main_window(&window, text, generation);
            }
        }
    };
    when_active(&tiny, TINY_ACTIVATE_TIMEOUT, on_active, on_timeout);
    present_with_token(&tiny, || tiny.present());
}

#[cfg(not(windows))]
fn copy_via_main_window(window: &ColorzWindow, text: String, generation: u64) {
    let main: gtk::Window = window.window().clone().upcast();
    let was_visible = main.is_visible();
    let on_active = {
        let (main, window, text) = (main.clone(), window.clone(), text.clone());
        move || {
            if is_current(generation) {
                main.clipboard().set_text(&text);
                debug(&format!("clipboard: main window fallback: {text}"));
            }
            if !was_visible {
                glib::timeout_add_local_once(MAIN_LINGER, move || {
                    // Keep it if it was opened for real in the meantime.
                    if !window.shown_on_purpose() {
                        main.set_visible(false);
                    }
                });
            }
        }
    };
    let on_timeout = {
        let (main, window) = (main.clone(), window.clone());
        move || {
            if !is_current(generation) {
                return;
            }
            // Best effort; the window stays up so the color can still be
            // copied from it by hand.
            main.clipboard().set_text(&text);
            debug("clipboard: main window got no focus; copy may not have landed");
            window.show_toast("Could not copy to the clipboard");
        }
    };
    when_active(&main, MAIN_ACTIVATE_TIMEOUT, on_active, on_timeout);
    if was_visible {
        // Shown but unfocused (buried, minimized or on another workspace):
        // it has to be raised to get focus, and Wayland offers no way to
        // lower it again afterwards, so it stays in front.
        debug("clipboard: raising the unfocused main window; it stays raised");
        present_with_token(&main, || main.present());
    } else {
        present_with_token(&main, || window.present_briefly());
    }
}
