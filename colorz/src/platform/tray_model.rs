//! What the tray shows and the commands it sends, shared by the tray of
//! every platform.

/// Edge length of the fallback tray icon.
#[cfg_attr(windows, expect(dead_code, reason = "no Windows tray yet"))]
pub const ICON_SIZE: u32 = 32;
/// OKHSL lightness of the fallback icon's color wheel.
#[cfg_attr(windows, expect(dead_code, reason = "no Windows tray yet"))]
pub const ICON_LIGHTNESS: f64 = 0.65;

/// What the user chose in the tray, handled on the GTK main loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(windows, expect(dead_code, reason = "no Windows tray yet"))]
pub enum TrayCommand {
    Open,
    Pick,
    Paste,
    ToggleChrome,
    /// Copy history entry `i` in the default format.
    CopyRecent(usize),
    Quit,
}

#[cfg(not(windows))]
impl TrayCommand {
    /// Whether the command may show a window, which on Wayland needs the
    /// click's activation token to get focus.
    pub(super) fn presents(self) -> bool {
        !matches!(self, TrayCommand::ToggleChrome | TrayCommand::Quit)
    }
}

/// A [`TrayCommand`] with the XDG activation token of the click, if the
/// tray host provided one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayEvent {
    pub command: TrayCommand,
    pub token: Option<String>,
}

/// Everything the menu shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrayModel {
    /// (hex label, sRGB) of the recent picks, newest first.
    pub recent: Vec<(String, [u8; 3])>,
    /// Whether "Correct picks from Chrome" is on.
    pub chrome: bool,
    /// How the pick shortcut is triggered; empty when there is none.
    pub shortcut_label: String,
}
