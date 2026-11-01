//! The notification-area icon on Windows: left click opens the window,
//! right click shows a menu with Paste color, Open Colorz and Quit.
//!
//! The icon lives on the GTK main thread, whose loop also pumps the Win32
//! messages of the icon's hidden window. Its handlers only forward events
//! into the channel that the GTK main loop reads.

use std::cell::RefCell;

use colorz_core::wheel::render_okhsl_disc;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::platform::tray_model::{ICON_LIGHTNESS, ICON_SIZE};
pub use crate::platform::tray_model::{TrayCommand, TrayEvent, TrayModel};

/// Keeps the tray icon alive until [`TrayHandle::close`].
pub struct TrayHandle {
    icon: RefCell<Option<TrayIcon>>,
}

impl TrayHandle {
    /// The Windows menu has no model-dependent items, so nothing changes.
    pub fn update(&self, _model: TrayModel) {}

    /// Removes the icon. Windows would otherwise keep showing a dead icon
    /// after exit until the mouse passes over it.
    pub fn close(&self) {
        drop(self.icon.take());
    }
}

/// (menu id, label, command) of each menu item, top to bottom.
pub fn menu_entries() -> [(&'static str, &'static str, TrayCommand); 3] {
    [
        ("paste", "Paste color", TrayCommand::Paste),
        ("open", "Open Colorz", TrayCommand::Open),
        ("quit", "Quit", TrayCommand::Quit),
    ]
}

/// The command of the menu item with `id`.
pub fn command_for_id(id: &str) -> Option<TrayCommand> {
    menu_entries()
        .into_iter()
        .find(|(entry, _, _)| *entry == id)
        .map(|(_, _, command)| command)
}

/// Shows the tray icon; its clicks and menu choices are sent on `sender`.
/// Must run on the GTK main thread. If the icon cannot be created the
/// failure is logged and the app runs without one.
pub fn spawn(sender: async_channel::Sender<TrayEvent>, _model: TrayModel) -> TrayHandle {
    let send = |sender: &async_channel::Sender<TrayEvent>, command| {
        // Fails only once the app is shutting down.
        let _ = sender.try_send(TrayEvent {
            command,
            token: None,
        });
    };
    let clicks = sender.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            send(&clicks, TrayCommand::Open);
        }
    }));
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Some(command) = command_for_id(event.id.as_ref()) {
            send(&sender, command);
        }
    }));
    let icon = match build() {
        Ok(icon) => Some(icon),
        Err(e) => {
            eprintln!("colorz: cannot show the tray icon: {e}");
            None
        }
    };
    TrayHandle {
        icon: RefCell::new(icon),
    }
}

fn build() -> Result<TrayIcon, Box<dyn std::error::Error>> {
    let menu = Menu::new();
    for (id, label, _) in menu_entries() {
        menu.append(&MenuItem::with_id(id, label, true, None))?;
    }
    Ok(TrayIconBuilder::new()
        .with_tooltip("Colorz")
        .with_icon(icon()?)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?)
}

/// The exe's icon resource (linked by build.rs), or a rendered color wheel
/// if it cannot be loaded.
fn icon() -> Result<Icon, tray_icon::BadIcon> {
    Icon::from_resource(1, Some((ICON_SIZE, ICON_SIZE))).or_else(|_| {
        Icon::from_rgba(
            render_okhsl_disc(ICON_SIZE, ICON_LIGHTNESS),
            ICON_SIZE,
            ICON_SIZE,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_has_only_paste_open_quit() {
        let labels: Vec<_> = menu_entries().iter().map(|(_, l, _)| *l).collect();
        assert_eq!(labels, ["Paste color", "Open Colorz", "Quit"]);
        assert_eq!(command_for_id("paste"), Some(TrayCommand::Paste));
        assert_eq!(command_for_id("open"), Some(TrayCommand::Open));
        assert_eq!(command_for_id("quit"), Some(TrayCommand::Quit));
        assert_eq!(command_for_id("pick"), None);
    }
}
