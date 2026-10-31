//! Pure palette operations used by the app: add picks, replace a color,
//! undo both safely, and edit-session validity.

use crate::color::Color;
use crate::palette::{Palette, PaletteColor};

/// Appends each color with an empty label and variable name.
pub fn append_colors(p: &mut Palette, colors: &[Color]) {
    for &color in colors {
        p.push(PaletteColor {
            color,
            label: String::new(),
            var_name: String::new(),
        });
    }
}

/// Removes the last `added.len()` colors, but only if they are exactly
/// `added` in order. Returns whether anything was removed.
pub fn undo_append(p: &mut Palette, added: &[Color]) -> bool {
    let Some(start) = p.colors.len().checked_sub(added.len()) else {
        return false;
    };
    let matches = p.colors[start..]
        .iter()
        .zip(added)
        .all(|(pc, c)| pc.color == *c);
    if matches {
        p.colors.truncate(start);
    }
    matches
}

/// Sets the color at `index`, keeping label and variable name. Returns the
/// old color, or `None` (no change) if `index` is out of range.
pub fn replace_color(p: &mut Palette, index: usize, c: Color) -> Option<Color> {
    let slot = p.colors.get_mut(index)?;
    Some(std::mem::replace(&mut slot.color, c))
}

/// Restores `old` at `index` only if the color there is still `new`.
pub fn undo_replace(p: &mut Palette, index: usize, old: Color, new: Color) -> bool {
    match p.colors.get_mut(index) {
        Some(slot) if slot.color == new => {
            slot.color = old;
            true
        }
        _ => false,
    }
}

/// An in-progress edit of one palette color.
#[derive(Clone, Debug, PartialEq)]
pub struct EditSession {
    pub palette_id: String,
    pub index: usize,
    pub original: Color,
}

impl EditSession {
    /// True while the edited color still sits at the same place unchanged.
    pub fn is_valid_in(&self, palettes: &[Palette]) -> bool {
        palettes
            .iter()
            .find(|p| p.id == self.palette_id)
            .and_then(|p| p.colors.get(self.index))
            .is_some_and(|pc| pc.color == self.original)
    }
}

/// The label if non-blank, else the uppercase `#RRGGBB` hex.
pub fn color_name(pc: &PaletteColor) -> String {
    if !pc.label.trim().is_empty() {
        return pc.label.clone();
    }
    let [r, g, b] = pc.color.to_srgb8();
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// The palette name, or "Untitled palette" if blank.
pub fn palette_display_name(p: &Palette) -> String {
    if p.name.trim().is_empty() {
        "Untitled palette".to_string()
    } else {
        p.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::palette::{Palette, PaletteColor};

    fn c(r: u8, g: u8, b: u8) -> Color {
        Color::from_srgb8([r, g, b])
    }

    fn pal(colors: &[Color]) -> Palette {
        let mut p = Palette::new("P");
        append_colors(&mut p, colors);
        p
    }

    #[test]
    fn append_then_undo_restores() {
        let (a, b) = (c(1, 2, 3), c(4, 5, 6));
        let mut p = pal(&[c(9, 9, 9)]);
        let before = p.clone();
        append_colors(&mut p, &[a, b]);
        assert_eq!(p.colors.len(), 3);
        assert_eq!(p.colors[1].label, "");
        assert_eq!(p.colors[1].var_name, "");
        assert!(undo_append(&mut p, &[a, b]));
        assert_eq!(p, before);
    }

    #[test]
    fn undo_append_refuses_after_changes() {
        let (a, b, x) = (c(1, 2, 3), c(4, 5, 6), c(7, 8, 9));
        let mut p = Palette::new("P");
        append_colors(&mut p, &[a, b]);
        append_colors(&mut p, &[x]);
        let snap = p.clone();
        assert!(!undo_append(&mut p, &[a, b]));
        assert_eq!(p, snap);

        let mut p = Palette::new("P");
        append_colors(&mut p, &[a, b]);
        replace_color(&mut p, 1, x);
        let snap = p.clone();
        assert!(!undo_append(&mut p, &[a, b]));
        assert_eq!(p, snap);

        let mut p = pal(&[a]);
        assert!(!undo_append(&mut p, &[a, b]));
        assert_eq!(p.colors.len(), 1);
    }

    #[test]
    fn replace_keeps_label_and_var() {
        let (a, n) = (c(1, 2, 3), c(7, 8, 9));
        let mut p = Palette::new("P");
        p.push(PaletteColor {
            color: a,
            label: "L".into(),
            var_name: "v".into(),
        });
        assert_eq!(replace_color(&mut p, 0, n), Some(a));
        assert_eq!(p.colors[0].color, n);
        assert_eq!(p.colors[0].label, "L");
        assert_eq!(p.colors[0].var_name, "v");
    }

    #[test]
    fn replace_out_of_range_is_none() {
        let mut p = pal(&[c(1, 2, 3)]);
        let snap = p.clone();
        assert_eq!(replace_color(&mut p, 1, c(7, 8, 9)), None);
        assert_eq!(p, snap);
    }

    #[test]
    fn undo_replace_refuses_after_changes() {
        let (a, n, z) = (c(1, 2, 3), c(7, 8, 9), c(5, 5, 5));
        let mut p = pal(&[a]);
        replace_color(&mut p, 0, n);
        assert!(undo_replace(&mut p, 0, a, n));
        assert_eq!(p.colors[0].color, a);

        replace_color(&mut p, 0, n);
        replace_color(&mut p, 0, z);
        assert!(!undo_replace(&mut p, 0, a, n));
        assert_eq!(p.colors[0].color, z);
        assert!(!undo_replace(&mut p, 3, a, z));
    }

    #[test]
    fn session_validity() {
        let (a, b, x) = (c(1, 2, 3), c(4, 5, 6), c(7, 8, 9));
        let p = pal(&[a, b]);
        let s = EditSession {
            palette_id: p.id.clone(),
            index: 1,
            original: b,
        };
        assert!(s.is_valid_in(std::slice::from_ref(&p)));

        let mut r = p.clone();
        r.move_color(0, 1);
        assert!(!s.is_valid_in(&[r]));

        let mut r = p.clone();
        r.remove(0);
        assert!(!s.is_valid_in(&[r]));

        let mut r = p.clone();
        replace_color(&mut r, 1, x);
        assert!(!s.is_valid_in(&[r]));

        let other = pal(&[a, b]);
        assert!(!s.is_valid_in(&[other]));

        let s2 = EditSession { index: 5, ..s };
        assert!(!s2.is_valid_in(&[p]));
    }

    #[test]
    fn color_name_prefers_label() {
        let col = c(0x34, 0x78, 0xF6);
        let mut pc = PaletteColor {
            color: col,
            label: "Primary".into(),
            var_name: String::new(),
        };
        assert_eq!(color_name(&pc), "Primary");
        pc.label = "  ".into();
        assert_eq!(color_name(&pc), "#3478F6");
        pc.label = String::new();
        assert_eq!(color_name(&pc), "#3478F6");
        assert_eq!(palette_display_name(&Palette::new(" ")), "Untitled palette");
        assert_eq!(palette_display_name(&Palette::new("X")), "X");
    }
}
