//! Toast notifications. GLib's Windows notification backend only prints a
//! warning, so toasts go through WinRT directly. Windows shows them only
//! for an app whose Start menu shortcut carries the [`APP_ID`]
//! AppUserModelID, which the installer creates.

use tauri_winrt_notification::Toast;

use crate::APP_ID;
use crate::platform::debug;

/// Shows a toast with just `title`; a failure is logged and otherwise
/// ignored.
pub fn show(title: &str) {
    if let Err(e) = Toast::new(APP_ID).title(title).show() {
        debug(&format!("cannot show a toast: {e}"));
    }
}
