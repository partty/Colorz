//! Windows replacements for the parts of the desktop integration that
//! differ from Linux.

pub mod identity;
pub mod instance;
pub mod run_key;
pub mod toast;
pub mod tray;

/// Keeps GLib off the D-Bus session bus.
///
/// On Windows, GLib autolaunches one session bus per user session
/// (`gdbus.exe`, found through fixed names in `gio/gdbusprivate.c`) and
/// every GLib app shares it. Colorz starts at login, so it would usually
/// start that shared bus, and other GLib apps (GIMP, Inkscape) would exit
/// with it when Colorz is updated or removed. Colorz needs no bus, so it
/// points GLib at an address with a transport GLib does not know:
/// `g_bus_get_sync()` then fails at once, without autolaunching or warning,
/// and GApplication runs as a plain local instance (see [`instance`] for
/// keeping one Colorz). Child processes inherit the setting.
pub fn disable_session_bus() {
    // SAFETY: runs from `platform::init()` at the start of `main`, before
    // any other thread exists.
    unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", "disabled:") };
}
