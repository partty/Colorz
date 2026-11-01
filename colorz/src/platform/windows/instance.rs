//! Keeps one Colorz running per Windows session.
//!
//! GApplication's uniqueness needs a D-Bus session bus, which Colorz does
//! not use on Windows (see [`super::disable_session_bus`]). Instead, a
//! named mutex marks the running instance, and its hidden message-only
//! window lets a later launch ask it to show its window. The later launch
//! then exits without starting GTK.
//!
//! The message window belongs to the GTK main thread, whose loop pumps the
//! Win32 messages of every window on the thread.

use std::cell::{Cell, RefCell};
use std::ffi::OsStr;
use std::ptr::{null, null_mut};
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, SetLastError, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, FindWindowExW,
    GetWindowThreadProcessId, HWND_MESSAGE, PostMessageW, RegisterClassW, RegisterWindowMessageW,
    WNDCLASSW,
};

use crate::APP_ID;
use crate::platform::debug;

/// Class of the running instance's message-only window.
const WINDOW_CLASS: &str = "ColorzInstance";
/// Registered message that asks the running instance to show its window.
const SHOW_MESSAGE: &str = "ColorzShow";

/// What a launch asks of the Colorz that already runs.
#[derive(Debug, PartialEq, Eq)]
pub enum Forward {
    /// Show its window, as a launch from the Start menu expects.
    Show,
    /// Nothing: a `--background` start at login only needs one Colorz.
    Nothing,
}

/// The request for a launch with `args` (without the program name).
pub fn forward_for<I, S>(args: I) -> Forward
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    if args.into_iter().any(|a| a.as_ref() == "--background") {
        Forward::Nothing
    } else {
        Forward::Show
    }
}

thread_local! {
    /// The id of [`SHOW_MESSAGE`] in this process; 0 until registered.
    static SHOW_ID: Cell<u32> = const { Cell::new(0) };
    static ON_SHOW: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
    /// A show request arrived before [`set_on_show`].
    static SHOW_PENDING: Cell<bool> = const { Cell::new(false) };
}

/// Makes this process the running Colorz, or passes the launch on to the
/// Colorz that already runs and exits with code 0. Runs on the thread that
/// later runs GTK, before GTK starts. If the mutex cannot be created,
/// Colorz runs without the guard.
pub fn claim_or_forward() {
    let show_id = register_message(SHOW_MESSAGE);
    let name = wide(&format!("Local\\{APP_ID}"));
    // SAFETY: `name` is NUL-terminated and outlives the call; null
    // attributes are allowed. The handle is never closed: Windows closes it
    // when the process ends, which frees the name for the next launch. The
    // last error is cleared first, so it only says ERROR_ALREADY_EXISTS if
    // CreateMutexW opened an existing mutex.
    let (mutex, error) = unsafe {
        SetLastError(0);
        let mutex = CreateMutexW(null(), 0, name.as_ptr());
        (mutex, GetLastError())
    };
    if mutex.is_null() {
        debug(&format!("cannot create the instance mutex: error {error}"));
        return;
    }
    if error == ERROR_ALREADY_EXISTS {
        if forward_for(std::env::args_os().skip(1)) == Forward::Show {
            ask_to_show(show_id);
        }
        std::process::exit(0);
    }
    create_message_window(show_id);
}

/// Runs `f` (on the GTK main thread) whenever another launch asks this
/// instance to show its window, including a request that came before.
pub fn set_on_show(f: impl Fn() + 'static) {
    let f: Rc<dyn Fn()> = Rc::new(f);
    ON_SHOW.set(Some(f.clone()));
    if SHOW_PENDING.replace(false) {
        f();
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// The id of a registered window message, or 0 if registering failed.
fn register_message(name: &str) -> u32 {
    let name = wide(name);
    // SAFETY: `name` is NUL-terminated and outlives the call.
    let id = unsafe { RegisterWindowMessageW(name.as_ptr()) };
    if id == 0 {
        debug("cannot register the show message");
    }
    id
}

/// Posts the show message to the running instance's window. A launch that
/// races the first one's start waits up to 2 s for its window.
fn ask_to_show(show_id: u32) {
    if show_id == 0 {
        return;
    }
    let class = wide(WINDOW_CLASS);
    for _ in 0..20 {
        // SAFETY: `class` is NUL-terminated and outlives the call;
        // HWND_MESSAGE searches message-only windows.
        let hwnd = unsafe { FindWindowExW(HWND_MESSAGE, null_mut(), class.as_ptr(), null()) };
        if !hwnd.is_null() {
            let mut pid = 0;
            // SAFETY: `hwnd` came from FindWindowExW; `pid` is a valid out
            // pointer. A stale handle only makes these calls fail.
            unsafe {
                GetWindowThreadProcessId(hwnd, &mut pid);
                // This launch is the foreground app the user just started,
                // so it can let the running instance bring its window up.
                if pid != 0 {
                    AllowSetForegroundWindow(pid);
                }
                if PostMessageW(hwnd, show_id, 0, 0) == 0 {
                    debug(&format!(
                        "cannot post the show message: error {}",
                        GetLastError()
                    ));
                }
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    debug("found no running Colorz window to show");
}

fn create_message_window(show_id: u32) {
    SHOW_ID.set(show_id);
    let class = wide(WINDOW_CLASS);
    // SAFETY: a null name returns this exe's module handle.
    let instance = unsafe { GetModuleHandleW(null()) };
    let wc = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: class.as_ptr(),
        ..Default::default()
    };
    // SAFETY: `wc` and the class name it points to outlive the call, which
    // copies them.
    if unsafe { RegisterClassW(&wc) } == 0 {
        // SAFETY: plain FFI call.
        debug(&format!(
            "cannot register {WINDOW_CLASS}: error {}",
            unsafe { GetLastError() }
        ));
        return;
    }
    // SAFETY: the class is registered above; HWND_MESSAGE makes a
    // message-only window, which is never shown.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            instance,
            null(),
        )
    };
    if hwnd.is_null() {
        // SAFETY: plain FFI call.
        debug(&format!(
            "cannot create the instance window: error {}",
            unsafe { GetLastError() }
        ));
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let show_id = SHOW_ID.get();
    if show_id != 0 && msg == show_id {
        // Runs the callback after GDK's message dispatch has returned.
        glib::idle_add_local_once(|| match ON_SHOW.with_borrow(Clone::clone) {
            Some(f) => f(),
            None => SHOW_PENDING.set(true),
        });
        return 0;
    }
    // SAFETY: passes the arguments of this call on unchanged.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_launch_shows_the_running_window() {
        assert_eq!(forward_for(Vec::<&str>::new()), Forward::Show);
        assert_eq!(forward_for(["--other"]), Forward::Show);
    }

    #[test]
    fn a_background_launch_asks_for_nothing() {
        assert_eq!(forward_for(["--background"]), Forward::Nothing);
        assert_eq!(forward_for(["--x", "--background"]), Forward::Nothing);
    }

    #[test]
    fn only_the_exact_option_counts() {
        assert_eq!(forward_for(["--backgrounds"]), Forward::Show);
        assert_eq!(forward_for(["background"]), Forward::Show);
    }
}
