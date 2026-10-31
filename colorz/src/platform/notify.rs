//! Desktop notifications. GNOME only shows them for an app whose `.desktop`
//! file (matching the app ID) is installed.

use adw::prelude::*;
use colorz_core::color::Color;
use gtk::{gdk, gio, glib};

/// Edge length of the swatch used as the notification icon, in pixels.
const SWATCH_SIZE: usize = 64;
/// Notification id of "Copied …"; each one replaces the previous.
const COPIED_ID: &str = "copied";

/// `Copied <text>`, plus ` (<target>)` and ` — out of gamut` when given.
pub fn copied_title(text: &str, target_name: Option<&str>, oog: bool) -> String {
    let mut title = format!("Copied {text}");
    if let Some(target) = target_name {
        title.push_str(&format!(" ({target})"));
    }
    if oog {
        title.push_str(" — out of gamut");
    }
    title
}

/// Announces a copy with a swatch of `color` as the icon. `target_name` is
/// the "copy as" space when it is not sRGB.
pub fn notify_copied(
    app: &adw::Application,
    text: &str,
    color: &Color,
    target_name: Option<&str>,
    oog: bool,
) {
    let n = gio::Notification::new(&copied_title(text, target_name, oog));
    n.set_icon(&swatch_icon(color));
    app.send_notification(Some(COPIED_ID), &n);
}

/// A notification with just a title; a later one with the same `id`
/// replaces it.
pub fn notify(app: &adw::Application, id: &str, title: &str) {
    app.send_notification(Some(id), &gio::Notification::new(title));
}

fn swatch_icon(color: &Color) -> gio::BytesIcon {
    let pixels = color.to_srgb8().repeat(SWATCH_SIZE * SWATCH_SIZE);
    let size = SWATCH_SIZE as i32;
    let texture = gdk::MemoryTexture::new(
        size,
        size,
        gdk::MemoryFormat::R8g8b8,
        &glib::Bytes::from_owned(pixels),
        SWATCH_SIZE * 3,
    );
    gio::BytesIcon::new(&texture.save_to_png_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_titles() {
        assert_eq!(copied_title("3478F6", None, false), "Copied 3478F6");
        assert_eq!(
            copied_title("3478F6", Some("Adobe RGB"), true),
            "Copied 3478F6 (Adobe RGB) — out of gamut"
        );
        assert_eq!(
            copied_title("#00FF00", None, true),
            "Copied #00FF00 — out of gamut"
        );
    }
}
