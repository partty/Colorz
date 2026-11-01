//! The login autostart entry, `~/.config/autostart/<app-id>.desktop`.
//! Not managed on Windows yet.
//!
//! Only an installed binary manages it, so development runs from `target/`
//! never write into the user's autostart directory.

use std::io;
use std::path::{Component, Path};

#[cfg(not(windows))]
use colorz_core::storage::write_atomic;

#[cfg(not(windows))]
use super::debug;
#[cfg(not(windows))]
use crate::APP_ID;

/// Contents of the autostart `.desktop` file.
#[cfg(not(windows))]
pub fn desktop_entry() -> String {
    [
        "[Desktop Entry]",
        "Type=Application",
        "Name=Colorz",
        "Exec=colorz --background",
        &format!("Icon={APP_ID}"),
        "X-GNOME-Autostart-enabled=true",
        "NoDisplay=true",
        "",
    ]
    .join("\n")
}

/// Writes (`on`) or removes `<dir>/<app-id>.desktop`.
#[cfg(not(windows))]
pub fn set_enabled(dir: &Path, on: bool) -> io::Result<()> {
    let file = dir.join(format!("{APP_ID}.desktop"));
    if on {
        write_atomic(&file, desktop_entry().as_bytes())
    } else {
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }
}

/// Whether `exe` is an installed binary rather than a cargo build.
fn is_installed(exe: &Path) -> bool {
    !exe.components()
        .any(|c| c == Component::Normal("target".as_ref()))
}

/// Whether this process runs an installed binary, which alone manages the
/// autostart entry.
pub fn running_installed() -> bool {
    match std::env::current_exe() {
        Ok(exe) => is_installed(&exe),
        Err(e) => {
            eprintln!("colorz: not updating autostart; cannot find the executable: {e}");
            false
        }
    }
}

/// Makes the user's autostart entry match `on`, when running installed;
/// otherwise leaves it alone.
#[cfg(not(windows))]
pub fn sync(on: bool) -> io::Result<()> {
    if !running_installed() {
        debug("autostart: not installed; leaving the autostart entry alone");
        return Ok(());
    }
    let Some(config) = dirs::config_dir() else {
        return Err(io::Error::other("no config directory"));
    };
    set_enabled(&config.join("autostart"), on)
}

/// Leaves start at login alone; Windows does not manage it yet.
#[cfg(windows)]
pub fn sync(_on: bool) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(windows))]
    fn autostart_writes_and_removes() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().join("autostart");
        let file = dir.join("io.github.partty.Colorz.desktop");
        set_enabled(&dir, true).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), desktop_entry());
        set_enabled(&dir, true).unwrap();
        set_enabled(&dir, false).unwrap();
        assert!(!file.exists());
        // Turning it off again is not an error.
        set_enabled(&dir, false).unwrap();
    }

    #[test]
    #[cfg(not(windows))]
    fn entry_launches_in_background() {
        let entry = desktop_entry();
        let lines: Vec<&str> = entry.lines().collect();
        assert_eq!(lines[0], "[Desktop Entry]");
        for line in [
            "Type=Application",
            "Name=Colorz",
            "Exec=colorz --background",
            "Icon=io.github.partty.Colorz",
            "X-GNOME-Autostart-enabled=true",
            "NoDisplay=true",
        ] {
            assert!(lines.contains(&line), "missing {line}");
        }
    }

    #[test]
    fn development_builds_are_not_installed() {
        assert!(!is_installed(Path::new(
            "/home/u/colorz/target/debug/colorz"
        )));
        assert!(!is_installed(Path::new(
            "/home/u/colorz/target/release/deps/colorz-1234"
        )));
        assert!(is_installed(Path::new("/home/u/.local/bin/colorz")));
        assert!(is_installed(Path::new("/usr/bin/colorz")));
    }
}
