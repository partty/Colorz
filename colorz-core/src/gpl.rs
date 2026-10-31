//! GIMP palette (`.gpl`) import and export.

use crate::color::Color;
use crate::palette::{Palette, PaletteColor};

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum GplError {
    #[error("not a GIMP palette (missing \"GIMP Palette\" header)")]
    MissingHeader,
    #[error("invalid color on line {0}")]
    BadColor(usize),
}

fn one_line(s: &str) -> String {
    s.replace(['\r', '\n'], " ")
}

fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

pub fn export_gpl(p: &Palette) -> String {
    let mut out = format!("GIMP Palette\nName: {}\nColumns: 0\n#\n", one_line(&p.name));
    for c in &p.colors {
        let rgb = c.color.to_srgb8();
        let name = if !c.label.is_empty() {
            one_line(&c.label)
        } else if !c.var_name.is_empty() {
            c.var_name.clone()
        } else {
            hex(rgb)
        };
        out.push_str(&format!(
            "{:3} {:3} {:3}\t{}\n",
            rgb[0], rgb[1], rgb[2], name
        ));
    }
    out
}

pub fn import_gpl(text: &str, fallback_name: &str) -> Result<Palette, GplError> {
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.lines().enumerate();
    match lines.by_ref().find(|(_, l)| !l.trim().is_empty()) {
        Some((_, l)) if l.trim() == "GIMP Palette" => {}
        _ => return Err(GplError::MissingHeader),
    }
    let mut name = None;
    let mut colors = Vec::new();
    for (i, line) in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("Columns:") {
            continue;
        }
        if let Some(n) = line.strip_prefix("Name:") {
            name = Some(n.trim().to_string());
            continue;
        }
        let mut rest = line;
        let mut rgb = [0u8; 3];
        for v in &mut rgb {
            rest = rest.trim_start();
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            *v = rest[..end].parse().map_err(|_| GplError::BadColor(i + 1))?;
            rest = &rest[end..];
        }
        let label = rest.trim();
        let label = if label == "Untitled" || label.eq_ignore_ascii_case(&hex(rgb)) {
            ""
        } else {
            label
        };
        colors.push(PaletteColor {
            color: Color::from_srgb8(rgb),
            label: label.to_string(),
            var_name: String::new(),
        });
    }
    let mut p = Palette::new(
        name.as_deref()
            .filter(|n| !n.is_empty())
            .unwrap_or(fallback_name),
    );
    p.colors = colors;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpl_round_trip() {
        let mut p = Palette::new("Brand");
        for (rgb, label, var) in [
            ([27, 42, 74], "Navy", "navy"),
            ([52, 120, 246], "", "primary"),
            ([246, 178, 52], "", ""),
            ([0, 0, 0], "Black", ""),
        ] {
            p.push(PaletteColor {
                color: Color::from_srgb8(rgb),
                label: label.into(),
                var_name: var.into(),
            });
        }
        let text = export_gpl(&p);
        assert!(text.starts_with("GIMP Palette\nName: Brand\nColumns: 0\n#\n"));
        assert!(text.contains(" 27  42  74\tNavy\n"));
        let q = import_gpl(&text, "x").unwrap();
        assert_eq!(q.name, "Brand");
        assert_eq!(q.colors.len(), 4);
        let labels: Vec<&str> = q.colors.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Navy", "primary", "", "Black"]);
        for (a, b) in p.colors.iter().zip(&q.colors) {
            assert_eq!(a.color.to_srgb8(), b.color.to_srgb8());
        }
    }

    #[test]
    fn gpl_import_details() {
        let q = import_gpl(
            "GIMP Palette\n# c\n255 0 0 Untitled\n  0 255 0\tGreen\n",
            "fb",
        )
        .unwrap();
        assert_eq!(q.name, "fb");
        assert_eq!(q.colors[0].label, "");
        assert_eq!(q.colors[1].label, "Green");
    }

    #[test]
    fn gpl_rejects_non_gpl() {
        assert!(import_gpl("hello", "x").is_err());
        assert_eq!(
            import_gpl("GIMP Palette\n1 2\n", "x"),
            Err(GplError::BadColor(2))
        );
    }
}
