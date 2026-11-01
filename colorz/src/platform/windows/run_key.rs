//! Start at login on Windows: a `Colorz` value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//!
//! `StartupApproved` is deliberately left alone, so the user's choice in
//! Settings > Apps > Startup stays in force.

use std::io;
use std::path::Path;

use windows_registry::CURRENT_USER;

/// The per-user key whose values Windows runs at login.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// The name of our value.
pub const VALUE_NAME: &str = "Colorz";

/// The command that starts Colorz hidden in the tray.
pub fn command_line(exe: &Path) -> String {
    format!("\"{}\" --background", exe.display())
}

/// Converts a registry error, unwrapping Win32 codes so that
/// [`io::Error::kind`] can tell "not found" apart.
fn io_res<T>(r: windows_registry::Result<T>) -> io::Result<T> {
    r.map_err(|e| {
        let code = e.code().0;
        if (code as u32) >> 16 == 0x8007 {
            io::Error::from_raw_os_error(code & 0xFFFF)
        } else {
            io::Error::from_raw_os_error(code)
        }
    })
}

fn not_found(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::NotFound
}

/// Writes (`Some`) the string value, creating the key if needed, or deletes
/// it (`None`); deleting a value that is not there is fine.
pub fn set(key_path: &str, value_name: &str, command: Option<&str>) -> io::Result<()> {
    match command {
        Some(command) => {
            let key = io_res(CURRENT_USER.create(key_path))?;
            io_res(key.set_string(value_name, command))
        }
        None => {
            let key = match io_res(CURRENT_USER.options().read().write().open(key_path)) {
                Ok(key) => key,
                Err(e) if not_found(&e) => return Ok(()),
                Err(e) => return Err(e),
            };
            match io_res(key.remove_value(value_name)) {
                Err(e) if not_found(&e) => Ok(()),
                r => r,
            }
        }
    }
}

/// The string value, or `None` when the key or the value is missing.
#[cfg(test)]
pub fn get(key_path: &str, value_name: &str) -> io::Result<Option<String>> {
    let key = match io_res(CURRENT_USER.open(key_path)) {
        Ok(key) => key,
        Err(e) if not_found(&e) => return Ok(None),
        Err(e) => return Err(e),
    };
    match io_res(key.get_string(value_name)) {
        Ok(v) => Ok(Some(v)),
        Err(e) if not_found(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deletes the test key even when an assertion fails.
    struct Cleanup(String);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = CURRENT_USER.remove_tree(&self.0);
        }
    }

    #[test]
    fn command_line_quotes_the_path() {
        let exe = Path::new(r"C:\Users\Jöhn Doe\AppData\Local\Programs\Colorz\bin\colorz.exe");
        assert_eq!(
            command_line(exe),
            r#""C:\Users\Jöhn Doe\AppData\Local\Programs\Colorz\bin\colorz.exe" --background"#
        );
    }

    #[test]
    fn set_get_delete_round_trip() {
        let key = format!(r"Software\ColorzTest-{}", std::process::id());
        let _cleanup = Cleanup(key.clone());
        let cmd = command_line(Path::new(r"C:\Users\Jöhn Doe\colorz.exe"));
        assert_eq!(get(&key, VALUE_NAME).unwrap(), None); // no key yet
        set(&key, VALUE_NAME, None).unwrap(); // nor anything to delete
        set(&key, VALUE_NAME, Some(&cmd)).unwrap();
        assert_eq!(
            get(&key, VALUE_NAME).unwrap().as_deref(),
            Some(cmd.as_str())
        );
        set(&key, VALUE_NAME, None).unwrap();
        assert_eq!(get(&key, VALUE_NAME).unwrap(), None);
        set(&key, VALUE_NAME, None).unwrap(); // deleting again is fine
    }
}
