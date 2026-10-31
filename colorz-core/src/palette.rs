//! Palette model and CSS variable name rules.

use serde::{Deserialize, Serialize};

use crate::color::Color;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaletteColor {
    pub color: Color,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub var_name: String,
}

fn default_selector() -> String {
    ":root".to_string()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    pub id: String,
    pub name: String,
    #[serde(default = "default_selector")]
    pub selector: String,
    pub colors: Vec<PaletteColor>,
}

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum VarNameError {
    #[error("not a valid CSS variable name")]
    Invalid,
    #[error("another color in this palette already uses that name")]
    Duplicate,
}

/// Trims whitespace and strips a leading `--`.
pub fn normalize_var_name(s: &str) -> String {
    let s = s.trim();
    s.strip_prefix("--").unwrap_or(s).to_string()
}

/// Empty is allowed; otherwise only ASCII letters, digits, `-` and `_`.
pub fn is_valid_var_name(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

impl Palette {
    pub fn new(name: &str) -> Palette {
        Palette {
            id: new_id(),
            name: name.to_string(),
            selector: default_selector(),
            colors: Vec::new(),
        }
    }

    pub fn duplicate(&self, name: &str) -> Palette {
        Palette {
            id: new_id(),
            name: name.to_string(),
            ..self.clone()
        }
    }

    pub fn push(&mut self, c: PaletteColor) {
        self.colors.push(c);
    }

    pub fn remove(&mut self, idx: usize) -> PaletteColor {
        self.colors.remove(idx)
    }

    pub fn insert(&mut self, idx: usize, c: PaletteColor) {
        self.colors.insert(idx, c);
    }

    /// Moves the color at `from` so that it ends up at index `to`.
    pub fn move_color(&mut self, from: usize, to: usize) {
        let c = self.colors.remove(from);
        self.colors.insert(to.min(self.colors.len()), c);
    }

    pub fn set_var_name(&mut self, idx: usize, name: &str) -> Result<(), VarNameError> {
        let name = normalize_var_name(name);
        if !is_valid_var_name(&name) {
            return Err(VarNameError::Invalid);
        }
        if !name.is_empty() && self.index_of_var(&name).is_some_and(|i| i != idx) {
            return Err(VarNameError::Duplicate);
        }
        self.colors[idx].var_name = name;
        Ok(())
    }

    pub fn index_of_var(&self, name: &str) -> Option<usize> {
        if name.is_empty() {
            return None;
        }
        self.colors.iter().position(|c| c.var_name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pc(rgb: [u8; 3]) -> PaletteColor {
        PaletteColor {
            color: Color::from_srgb8(rgb),
            label: String::new(),
            var_name: String::new(),
        }
    }

    #[test]
    fn var_name_rules() {
        assert_eq!(normalize_var_name("--primary"), "primary");
        assert_eq!(normalize_var_name("  --x "), "x");
        assert!(is_valid_var_name("") && is_valid_var_name("a-b_9"));
        for bad in ["a b", "é", "x:y"] {
            assert!(!is_valid_var_name(bad), "{bad}");
        }
        let mut p = Palette::new("p");
        for _ in 0..3 {
            p.push(pc([1, 2, 3]));
        }
        assert_eq!(p.set_var_name(0, "--a"), Ok(()));
        assert_eq!(p.colors[0].var_name, "a");
        assert_eq!(p.set_var_name(1, "a"), Err(VarNameError::Duplicate));
        assert_eq!(p.set_var_name(1, "a b"), Err(VarNameError::Invalid));
        assert_eq!(p.set_var_name(0, "a"), Ok(()));
        assert_eq!(p.set_var_name(0, ""), Ok(()));
        assert_eq!(p.set_var_name(1, ""), Ok(()));
        assert_eq!(p.set_var_name(2, ""), Ok(()));
        assert_eq!(p.index_of_var(""), None);
    }

    #[test]
    fn list_operations_and_duplicate() {
        let mut p = Palette::new("p");
        assert_eq!(p.selector, ":root");
        for v in 1..=3 {
            p.push(pc([v, 0, 0]));
        }
        p.move_color(0, 2);
        let reds: Vec<u8> = p.colors.iter().map(|c| c.color.to_srgb8()[0]).collect();
        assert_eq!(reds, [2, 3, 1]);
        let c = p.remove(1);
        p.insert(0, c);
        assert_eq!(p.colors[0].color.to_srgb8()[0], 3);
        let d = p.duplicate("copy");
        assert_ne!(d.id, p.id);
        assert_eq!(d.colors, p.colors);
        assert_eq!(d.name, "copy");
    }
}
