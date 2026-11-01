//! The StatusNotifierItem tray icon (shown by GNOME's AppIndicator
//! extension).
//!
//! ksni runs the item on its own executor thread, so the tray only holds
//! plain data ([`TrayModel`]) and sends [`TrayEvent`]s to the GTK main
//! loop; it never touches GTK objects.

use std::io::Cursor;
use std::path::PathBuf;

use colorz_core::wheel::render_okhsl_disc;
use gtk::glib;
use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};
use ksni::{Icon, MenuItem, OfflineReason, TrayMethods};

use super::tray_model::{ICON_LIGHTNESS, ICON_SIZE};
pub use super::tray_model::{TrayCommand, TrayEvent, TrayModel};
use crate::APP_ID;

/// Most recent picks listed in the menu.
const MAX_RECENT: usize = 5;
/// Edge length of the menu swatches.
const SWATCH_SIZE: u32 = 16;

/// Updates the tray from the GTK main loop.
pub struct TrayHandle {
    models: async_channel::Sender<TrayModel>,
}

impl TrayHandle {
    /// Replaces what the tray shows.
    pub fn update(&self, model: TrayModel) {
        // Fails only when the tray could not be started.
        let _ = self.models.try_send(model);
    }

    /// Nothing to do: the tray host drops the icon when the process exits.
    pub fn close(&self) {}
}

/// Starts the tray icon. Its events arrive on `sender`. When there
/// is no tray host the failure is logged and the app runs without one.
pub fn spawn(sender: async_channel::Sender<TrayEvent>, model: TrayModel) -> TrayHandle {
    let (models, updates) = async_channel::unbounded::<TrayModel>();
    glib::spawn_future_local(async move {
        let tray = ColorzTray::new(sender, model);
        // The AppIndicator extension may still be starting at login; the
        // tray then shows up once it is ready.
        let handle = match tray.assume_sni_available(true).spawn().await {
            Ok(handle) => handle,
            Err(e) => {
                eprintln!("colorz: cannot show the tray icon: {e}");
                updates.close();
                return;
            }
        };
        while let Ok(mut model) = updates.recv().await {
            // Only the newest model matters.
            while let Ok(newer) = updates.try_recv() {
                model = newer;
            }
            let swatches = swatches(&model);
            handle
                .update(move |t| {
                    t.model = model;
                    t.swatches = swatches;
                })
                .await;
        }
    });
    TrayHandle { models }
}

struct ColorzTray {
    events: async_channel::Sender<TrayEvent>,
    model: TrayModel,
    /// PNG swatch for each of the first [`MAX_RECENT`] recent picks.
    swatches: Vec<Vec<u8>>,
    /// The themed icon's name, or empty when the icon is not installed.
    icon_name: String,
    icon: Icon,
}

impl ColorzTray {
    fn new(events: async_channel::Sender<TrayEvent>, model: TrayModel) -> ColorzTray {
        ColorzTray {
            events,
            swatches: swatches(&model),
            model,
            icon_name: themed_icon_name(),
            icon: wheel_icon(ICON_SIZE),
        }
    }

    /// Runs inside the click callback, where ksni knows the click's token.
    /// Read once: a token arriving late is attached by a D-Bus handler on
    /// the executor this callback is blocking, so waiting here cannot see it.
    fn send(&self, command: TrayCommand) {
        let token = command
            .presents()
            .then(ksni::current_xdg_activation_token)
            .flatten();
        let event = TrayEvent { command, token };
        // Unbounded: fails only once the app is shutting down.
        let _ = self.events.try_send(event);
    }

    fn item(label: &str, cmd: TrayCommand) -> MenuItem<Self> {
        StandardItem {
            label: label.to_owned(),
            activate: Box::new(move |t: &mut Self| t.send(cmd)),
            ..Default::default()
        }
        .into()
    }
}

impl ksni::Tray for ColorzTray {
    fn id(&self) -> String {
        "colorz".into()
    }

    fn title(&self) -> String {
        "Colorz".into()
    }

    fn icon_name(&self) -> String {
        self.icon_name.clone()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![self.icon.clone()]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(TrayCommand::Open);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let recent: Vec<MenuItem<Self>> = recent_labels(&self.model)
            .into_iter()
            .zip(&self.swatches)
            .enumerate()
            .map(|(i, (label, swatch))| {
                StandardItem {
                    label,
                    icon_data: swatch.clone(),
                    activate: Box::new(move |t: &mut Self| t.send(TrayCommand::CopyRecent(i))),
                    ..Default::default()
                }
                .into()
            })
            .collect();
        vec![
            Self::item(&pick_label(&self.model), TrayCommand::Pick),
            Self::item("Paste color", TrayCommand::Paste),
            CheckmarkItem {
                label: "Correct picks from Chrome".into(),
                checked: self.model.chrome,
                activate: Box::new(|t: &mut Self| t.send(TrayCommand::ToggleChrome)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            SubMenu {
                label: "Recent picks".into(),
                enabled: !recent.is_empty(),
                submenu: recent,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            Self::item("Open Colorz", TrayCommand::Open),
            Self::item("Quit", TrayCommand::Quit),
        ]
    }

    fn watcher_offline(&self, reason: OfflineReason) -> bool {
        eprintln!("colorz: the tray icon is not shown: {reason:?}");
        // Keep waiting; the icon comes back when a tray host appears.
        true
    }
}

/// `Pick color`, followed by the shortcut when there is one.
fn pick_label(model: &TrayModel) -> String {
    if model.shortcut_label.is_empty() {
        "Pick color".into()
    } else {
        format!("Pick color  {}", model.shortcut_label)
    }
}

/// Labels of the recent picks the menu lists.
fn recent_labels(model: &TrayModel) -> Vec<String> {
    let recent = model.recent.iter().take(MAX_RECENT);
    recent.map(|(hex, _)| hex.clone()).collect()
}

fn swatches(model: &TrayModel) -> Vec<Vec<u8>> {
    let recent = model.recent.iter().take(MAX_RECENT);
    recent.map(|&(_, rgb)| swatch_png(rgb)).collect()
}

/// A `SWATCH_SIZE` square of `rgb`, as PNG.
fn swatch_png(rgb: [u8; 3]) -> Vec<u8> {
    let pixels = rgb.repeat((SWATCH_SIZE * SWATCH_SIZE) as usize);
    let mut png = Vec::new();
    PngEncoder::new(Cursor::new(&mut png))
        .write_image(&pixels, SWATCH_SIZE, SWATCH_SIZE, ExtendedColorType::Rgb8)
        .expect("encoding a PNG in memory cannot fail");
    png
}

/// The app's icon name when an icon theme directory has it, else empty.
/// GNOME's AppIndicator extension shows a placeholder for a name it cannot
/// find instead of using the pixmap, so the name is only sent when the
/// icon is installed. In a snap or Flatpak the app's own icon directories
/// are not the shell's, so the pixmap is always used there.
fn themed_icon_name() -> String {
    let mut dirs = vec![glib::user_data_dir()];
    dirs.extend(glib::system_data_dirs());
    if !sandboxed() && icon_installed(&dirs, APP_ID) {
        APP_ID.into()
    } else {
        String::new()
    }
}

/// Whether this runs as a snap or a Flatpak.
fn sandboxed() -> bool {
    std::env::var_os("SNAP").is_some() || std::env::var_os("FLATPAK_ID").is_some()
}

/// Whether `name` is in the hicolor theme under one of `data_dirs`, where
/// `make install` puts it.
fn icon_installed(data_dirs: &[PathBuf], name: &str) -> bool {
    let file = format!("{name}.svg");
    data_dirs
        .iter()
        .any(|d| d.join("icons/hicolor/scalable/apps").join(&file).is_file())
}

/// A color wheel as an ARGB32 (network byte order) tray icon, used when
/// the icon theme has no Colorz icon (e.g. when not installed).
fn wheel_icon(size: u32) -> Icon {
    let rgba = render_okhsl_disc(size, ICON_LIGHTNESS);
    let data = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, a]| [a, r, g, b])
        .collect();
    Icon {
        width: size as i32,
        height: size as i32,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(n: usize) -> TrayModel {
        TrayModel {
            recent: (0..n)
                .map(|i| (format!("#0000{i:02X}"), [0, 0, i as u8]))
                .collect(),
            chrome: false,
            shortcut_label: String::new(),
        }
    }

    #[test]
    fn tray_menu_model() {
        assert!(recent_labels(&model(0)).is_empty());
        assert_eq!(recent_labels(&model(2)), ["#000000", "#000001"]);
        assert_eq!(
            recent_labels(&model(8)),
            ["#000000", "#000001", "#000002", "#000003", "#000004"]
        );
        assert_eq!(swatches(&model(8)).len(), 5);
        assert_eq!(pick_label(&model(0)), "Pick color");
        let bound = TrayModel {
            shortcut_label: "Shift+Super+C".into(),
            ..model(0)
        };
        assert_eq!(pick_label(&bound), "Pick color  Shift+Super+C");
    }

    #[test]
    fn only_window_commands_want_a_token() {
        assert!(TrayCommand::Open.presents());
        assert!(TrayCommand::Pick.presents());
        assert!(TrayCommand::Paste.presents());
        assert!(TrayCommand::CopyRecent(0).presents());
        assert!(!TrayCommand::ToggleChrome.presents());
        assert!(!TrayCommand::Quit.presents());
    }

    #[test]
    fn swatch_is_a_png_of_the_color() {
        let png = swatch_png([0x34, 0x78, 0xF6]);
        let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .unwrap()
            .to_rgb8();
        assert_eq!(img.dimensions(), (SWATCH_SIZE, SWATCH_SIZE));
        assert!(img.pixels().all(|p| p.0 == [0x34, 0x78, 0xF6]));
    }

    #[test]
    fn icon_installed_looks_in_hicolor() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let dirs = [other.path().to_owned(), dir.path().to_owned()];
        assert!(!icon_installed(&dirs, "a.b.C"));
        let apps = dir.path().join("icons/hicolor/scalable/apps");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(apps.join("a.b.C.svg"), "<svg/>").unwrap();
        assert!(icon_installed(&dirs, "a.b.C"));
        assert!(!icon_installed(&dirs, "a.b.D"));
    }

    #[test]
    fn wheel_icon_is_argb() {
        let icon = wheel_icon(ICON_SIZE);
        assert_eq!((icon.width, icon.height), (32, 32));
        assert_eq!(icon.data.len(), 32 * 32 * 4);
        // Transparent corner, opaque center (alpha comes first).
        assert_eq!(icon.data[0], 0);
        let center = ((16 * 32 + 16) * 4) as usize;
        assert_eq!(icon.data[center], 255);
    }
}
