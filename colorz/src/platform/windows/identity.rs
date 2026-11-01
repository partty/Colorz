//! The AppUserModelID that ties the taskbar button, notifications and the
//! Start menu shortcut to Colorz.

use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

use crate::APP_ID;
use crate::platform::debug;

/// Sets [`APP_ID`] as the process's AppUserModelID. Must run before any
/// window is shown; a failure is logged and otherwise ignored.
pub fn set_app_user_model_id() {
    let wide: Vec<u16> = APP_ID.encode_utf16().chain([0]).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the
    // call, which copies it.
    let hr = unsafe { SetCurrentProcessExplicitAppUserModelID(wide.as_ptr()) };
    if hr < 0 {
        debug(&format!(
            "cannot set the AppUserModelID: HRESULT {hr:#010x}"
        ));
    }
}
