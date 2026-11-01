//! The notification-area icon on Windows. Not shown yet: [`spawn`] returns
//! a handle without an icon.

pub use crate::platform::tray_model::{TrayCommand, TrayEvent, TrayModel};

/// Updates the tray from the GTK main loop.
pub struct TrayHandle {}

impl TrayHandle {
    /// Replaces what the tray shows; there is no icon yet, so nothing does.
    pub fn update(&self, _model: TrayModel) {}
}

/// Starts the tray icon. There is none on Windows yet, so no events are
/// ever sent on `_sender`.
pub fn spawn(_sender: async_channel::Sender<TrayEvent>, _model: TrayModel) -> TrayHandle {
    TrayHandle {}
}
