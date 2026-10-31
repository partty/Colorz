//! Settings (TOML), palette files (JSON, one per palette) and history (JSON),
//! all written atomically.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::color::Color;
use crate::format::{CssSyntax, DEFAULT_DECIMALS, Form, Format, HexCase};
use crate::palette::Palette;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TargetChoice {
    Srgb,
    DisplayP3,
    AdobeRgb,
    Icc { path: PathBuf },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub default_format: Format,
    pub default_form: Form,
    pub hex_case: HexCase,
    pub css_syntax: CssSyntax,
    /// Decimal places for OKLCH/OKLab; see `FormatOptions::decimals`.
    pub decimals: usize,
    pub target: TargetChoice,
    pub custom_icc: Vec<PathBuf>,
    pub visible_formats: Vec<Format>,
    pub start_at_login: bool,
    pub history_size: usize,
    pub chrome_correction: bool,
    pub last_palette: Option<String>,
    pub shortcut_fallback_installed: bool,
    /// Set once an installed build has set up the autostart entry from
    /// `start_at_login`; after that only the Settings toggle changes it.
    pub autostart_initialized: bool,
    /// Add every pick to the target palette without asking.
    pub auto_add_picks: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            default_format: Format::Hex,
            default_form: Form::Numbers,
            hex_case: HexCase::Upper,
            css_syntax: CssSyntax::Modern,
            decimals: DEFAULT_DECIMALS,
            target: TargetChoice::Srgb,
            custom_icc: Vec::new(),
            visible_formats: Format::ALL.to_vec(),
            start_at_login: true,
            history_size: 20,
            chrome_correction: false,
            last_palette: None,
            shortcut_fallback_installed: false,
            autostart_initialized: false,
            auto_add_picks: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Paths {
    pub config_file: PathBuf,
    pub palettes_dir: PathBuf,
    pub history_file: PathBuf,
}

impl Paths {
    /// The real per-user locations (`~/.config/colorz`, `~/.local/share/colorz`).
    pub fn user() -> Paths {
        let config = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        let data = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
        Paths {
            config_file: config.join("colorz").join("config.toml"),
            palettes_dir: data.join("colorz").join("palettes"),
            history_file: data.join("colorz").join("history.json"),
        }
    }

    /// Everything under `root` (used by tests).
    pub fn in_dir(root: &Path) -> Paths {
        Paths {
            config_file: root.join("config.toml"),
            palettes_dir: root.join("palettes"),
            history_file: root.join("history.json"),
        }
    }
}

/// Writes `bytes` to `path` via a temp file in the same directory and a rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.to_string())
}

// ---------------------------------------------------------------- config

/// Result of [`load_config`].
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedConfig {
    pub config: Config,
    /// False when an existing config file could neither be read nor backed up.
    /// Callers must then skip `save_config`, or the user's file would be
    /// overwritten with defaults.
    pub writable: bool,
}

pub fn load_config(p: &Paths) -> LoadedConfig {
    let ok = |config| LoadedConfig {
        config,
        writable: true,
    };
    let bytes = match std::fs::read(&p.config_file) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return ok(Config::default()),
        Err(e) => {
            eprintln!("colorz: cannot read {}: {e}", p.config_file.display());
            return LoadedConfig {
                config: Config::default(),
                writable: false,
            };
        }
    };
    let parsed = String::from_utf8(bytes)
        .map_err(|e| e.to_string())
        .and_then(|t| toml::from_str(&t).map_err(|e| e.to_string()));
    match parsed {
        Ok(c) => ok(c),
        Err(e) => {
            eprintln!(
                "colorz: {} is not valid ({e}); backing it up",
                p.config_file.display()
            );
            // The .bak only ever holds a previously corrupt config, so it is
            // safe to overwrite an older one.
            let mut bak = p.config_file.clone().into_os_string();
            bak.push(".bak");
            match std::fs::rename(&p.config_file, PathBuf::from(bak)) {
                Ok(()) => ok(Config::default()),
                Err(e) => {
                    eprintln!("colorz: cannot back up config: {e}");
                    LoadedConfig {
                        config: Config::default(),
                        writable: false,
                    }
                }
            }
        }
    }
}

pub fn save_config(p: &Paths, c: &Config) -> io::Result<()> {
    let text = toml::to_string_pretty(c).map_err(io::Error::other)?;
    write_atomic(&p.config_file, text.as_bytes())
}

// -------------------------------------------------------------- palettes

const PALETTE_VERSION: u32 = 1;

fn default_version() -> u32 {
    PALETTE_VERSION
}

#[derive(Serialize, Deserialize)]
struct PaletteFile {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(flatten)]
    palette: Palette,
}

fn palette_path(p: &Paths, id: &str) -> io::Result<PathBuf> {
    if id.is_empty() || id == "." || id.contains(['/', '\\', '\0']) || id.contains("..") {
        return Err(invalid("invalid palette id"));
    }
    Ok(p.palettes_dir.join(format!("{id}.json")))
}

pub fn load_palettes(p: &Paths) -> Vec<Palette> {
    let Ok(dir) = std::fs::read_dir(&p.palettes_dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let parsed = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice::<PaletteFile>(&b).map_err(|e| e.to_string()));
        match parsed {
            Ok(f) if f.version <= PALETTE_VERSION => out.push(f.palette),
            Ok(f) => eprintln!(
                "colorz: skipping {}: unsupported palette version {}",
                path.display(),
                f.version
            ),
            Err(e) => eprintln!("colorz: skipping corrupt palette {}: {e}", path.display()),
        }
    }
    out.sort_by_cached_key(|pal| (pal.name.to_lowercase(), pal.id.clone()));
    out
}

pub fn save_palette(p: &Paths, pal: &Palette) -> io::Result<()> {
    let path = palette_path(p, &pal.id)?;
    let file = PaletteFile {
        version: PALETTE_VERSION,
        palette: pal.clone(),
    };
    let json = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
    write_atomic(&path, &json)
}

pub fn delete_palette(p: &Paths, id: &str) -> io::Result<()> {
    let path = palette_path(p, id)?;
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

// --------------------------------------------------------------- history

/// Recent picks, newest first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct History {
    pub entries: Vec<Color>,
}

impl History {
    pub fn push(&mut self, c: Color, cap: usize) {
        let key = c.to_srgb8();
        self.entries.retain(|e| e.to_srgb8() != key);
        self.entries.insert(0, c);
        self.entries.truncate(cap);
    }
}

pub fn load_history(p: &Paths) -> History {
    let bytes = match std::fs::read(&p.history_file) {
        Ok(b) => b,
        Err(e) => {
            if e.kind() != io::ErrorKind::NotFound {
                eprintln!("colorz: cannot read {}: {e}", p.history_file.display());
            }
            return History::default();
        }
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        eprintln!("colorz: ignoring corrupt {}: {e}", p.history_file.display());
        History::default()
    })
}

pub fn save_history(p: &Paths, h: &History) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(h).map_err(io::Error::other)?;
    write_atomic(&p.history_file, &json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::PaletteColor;

    fn col(r: u8, g: u8, b: u8) -> Color {
        Color::from_srgb8([r, g, b])
    }

    fn pal(name: &str) -> Palette {
        let mut p = Palette::new(name);
        p.push(PaletteColor {
            color: col(52, 120, 246),
            label: "blue".into(),
            var_name: "blue".into(),
        });
        p
    }

    #[test]
    fn autostart_initialized_defaults_to_false() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(dir.path());
        std::fs::write(&p.config_file, "start_at_login = false\n").unwrap();
        let c = load_config(&p).config;
        assert!(!c.autostart_initialized);
        assert!(!Config::default().autostart_initialized);
        assert!(!c.auto_add_picks);
        assert!(!Config::default().auto_add_picks);
        let on = Config {
            autostart_initialized: true,
            ..c
        };
        save_config(&p, &on).unwrap();
        assert!(load_config(&p).config.autostart_initialized);
    }

    #[test]
    fn decimals_default_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(dir.path());
        std::fs::write(&p.config_file, "hex_case = \"lower\"\n").unwrap();
        assert_eq!(load_config(&p).config.decimals, 3);
    }

    #[test]
    fn config_defaults_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(dir.path());
        let d = load_config(&p).config;
        assert_eq!(d, Config::default());
        assert_eq!(d.default_format, Format::Hex);
        assert_eq!(d.default_form, Form::Numbers);
        assert_eq!(d.visible_formats.len(), 7);
        assert!(d.start_at_login);
        assert_eq!(d.history_size, 20);
        assert!(!d.chrome_correction);
        assert_eq!(d.decimals, 3);

        let c = Config {
            default_format: Format::Hsl,
            default_form: Form::Full,
            hex_case: HexCase::Lower,
            css_syntax: CssSyntax::Legacy,
            decimals: 6,
            target: TargetChoice::Icc {
                path: PathBuf::from("/tmp/x.icc"),
            },
            custom_icc: vec![PathBuf::from("/tmp/x.icc")],
            visible_formats: vec![Format::Hex, Format::Hsl],
            start_at_login: false,
            history_size: 5,
            chrome_correction: true,
            last_palette: Some("abc".into()),
            shortcut_fallback_installed: true,
            autostart_initialized: true,
            auto_add_picks: true,
        };
        save_config(&p, &c).unwrap();
        assert_eq!(load_config(&p).config, c);

        std::fs::write(&p.config_file, "hex_case = \"lower\"\n").unwrap();
        let l = load_config(&p).config;
        assert_eq!(l.hex_case, HexCase::Lower);
        assert_eq!(
            Config {
                hex_case: HexCase::Upper,
                ..l
            },
            Config::default()
        );
    }

    #[test]
    fn corrupt_config_is_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(dir.path());
        std::fs::write(&p.config_file, "{{{").unwrap();
        let l = load_config(&p);
        assert_eq!(l.config, Config::default());
        assert!(l.writable);
        let bak = dir.path().join("config.toml.bak");
        assert_eq!(std::fs::read_to_string(bak).unwrap(), "{{{");
    }

    #[test]
    fn invalid_utf8_config_is_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(dir.path());
        std::fs::write(&p.config_file, [0xff, 0xfe, 0x41]).unwrap();
        let l = load_config(&p);
        assert_eq!(l.config, Config::default());
        assert!(l.writable);
        assert_eq!(
            std::fs::read(dir.path().join("config.toml.bak")).unwrap(),
            [0xff, 0xfe, 0x41]
        );
    }

    #[test]
    fn unreadable_config_is_not_writable() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(dir.path());
        std::fs::create_dir(&p.config_file).unwrap();
        let l = load_config(&p);
        assert_eq!(l.config, Config::default());
        assert!(!l.writable);
        assert!(p.config_file.is_dir());
    }

    #[test]
    fn default_config_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(dir.path());
        save_config(&p, &Config::default()).unwrap();
        assert_eq!(load_config(&p).config, Config::default());
    }

    #[test]
    fn palette_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(dir.path());
        let b = pal("banana");
        let a = pal("Apple");
        save_palette(&p, &b).unwrap();
        save_palette(&p, &a).unwrap();
        let loaded = load_palettes(&p);
        assert_eq!(loaded, vec![a.clone(), b.clone()]);
        let json = std::fs::read_to_string(p.palettes_dir.join(format!("{}.json", a.id))).unwrap();
        assert!(json.contains("\"version\": 1"));
        assert!(json.contains("\"l\":"));
        delete_palette(&p, &a.id).unwrap();
        delete_palette(&p, &a.id).unwrap();
        assert_eq!(load_palettes(&p), vec![b]);
    }

    #[test]
    fn palette_version_handling() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(dir.path());
        let a = pal("a");
        save_palette(&p, &a).unwrap();
        let path = p.palettes_dir.join(format!("{}.json", a.id));
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        v.as_object_mut().unwrap().remove("version");
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(load_palettes(&p), vec![a.clone()]);
        v["version"] = 2.into();
        let future = serde_json::to_vec(&v).unwrap();
        std::fs::write(&path, &future).unwrap();
        assert!(load_palettes(&p).is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), future);
    }

    #[test]
    fn bad_palette_ids_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(dir.path());
        for id in ["../x", "a/b", "a\\b", "..", "", "."] {
            let mut pal = pal("x");
            pal.id = id.into();
            assert_eq!(
                save_palette(&p, &pal).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
            assert_eq!(
                delete_palette(&p, id).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    fn corrupt_palette_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(dir.path());
        std::fs::create_dir_all(&p.palettes_dir).unwrap();
        let bad = p.palettes_dir.join("bad.json");
        std::fs::write(&bad, "garbage").unwrap();
        let ok = pal("ok");
        save_palette(&p, &ok).unwrap();
        assert_eq!(load_palettes(&p), vec![ok]);
        assert_eq!(std::fs::read_to_string(bad).unwrap(), "garbage");
    }

    #[test]
    fn history_push_dedup_cap() {
        let (a, b, c) = (col(1, 2, 3), col(4, 5, 6), col(7, 8, 9));
        let mut h = History::default();
        h.push(a, 2);
        h.push(b, 2);
        h.push(a, 2);
        assert_eq!(h.entries, vec![a, b]);
        h.push(c, 2);
        assert_eq!(h.entries, vec![c, a]);

        let mut z = History::default();
        z.push(a, 0);
        assert!(z.entries.is_empty());

        let dir = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(dir.path());
        assert_eq!(load_history(&p), History::default());
        save_history(&p, &h).unwrap();
        assert_eq!(load_history(&p), h);
        std::fs::write(&p.history_file, "nope").unwrap();
        assert_eq!(load_history(&p), History::default());
    }

    #[test]
    fn atomic_write_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("sub").join("f.txt");
        write_atomic(&f, b"one").unwrap();
        write_atomic(&f, b"two").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"two");
        let n = std::fs::read_dir(f.parent().unwrap()).unwrap().count();
        assert_eq!(n, 1);
    }
}
