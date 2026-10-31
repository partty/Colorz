//! Target color spaces for "copy as" output.

use crate::color::{Color, RgbSpace, gamut_map};

pub trait TargetSpace {
    fn name(&self) -> String;
    fn css_name(&self) -> Option<&'static str>;
    fn is_srgb(&self) -> bool;
    /// Gamut-mapped channels in `[0, 1]` and whether the color was out of gamut.
    fn encode(&self, c: &Color) -> ([f64; 3], bool);
    fn decode(&self, rgb: [f64; 3]) -> Color;
}

pub struct BuiltinTarget(pub RgbSpace);

impl TargetSpace for BuiltinTarget {
    fn name(&self) -> String {
        match self.0 {
            RgbSpace::Srgb => "sRGB",
            RgbSpace::DisplayP3 => "Display P3",
            RgbSpace::A98Rgb => "Adobe RGB",
        }
        .to_string()
    }

    fn css_name(&self) -> Option<&'static str> {
        Some(match self.0 {
            RgbSpace::Srgb => "srgb",
            RgbSpace::DisplayP3 => "display-p3",
            RgbSpace::A98Rgb => "a98-rgb",
        })
    }

    fn is_srgb(&self) -> bool {
        self.0 == RgbSpace::Srgb
    }

    fn encode(&self, c: &Color) -> ([f64; 3], bool) {
        (gamut_map(c, self.0), !c.in_gamut(self.0))
    }

    fn decode(&self, rgb: [f64; 3]) -> Color {
        Color::from_rgb(self.0, rgb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to8(rgb: [f64; 3]) -> [u8; 3] {
        rgb.map(|v| (v * 255.0).round() as u8)
    }

    #[test]
    fn builtin_target_a98() {
        let t = BuiltinTarget(RgbSpace::A98Rgb);
        let blue = Color::from_srgb8([0x34, 0x78, 0xF6]);
        let (rgb, out) = t.encode(&blue);
        assert_eq!(to8(rgb), [80, 119, 242]);
        assert!(!out);
        let (rgb, _) = t.encode(&Color::from_srgb8([255, 0, 0]));
        assert_eq!(to8(rgb), [219, 0, 0]);
        for x in [[0x34, 0x78, 0xF6], [0xF6, 0xB2, 0x34], [0x2E, 0xC2, 0x7E]] {
            let c = Color::from_srgb8(x);
            assert_eq!(t.decode(t.encode(&c).0).to_srgb8(), c.to_srgb8());
        }
    }

    #[test]
    fn names() {
        let t = BuiltinTarget(RgbSpace::DisplayP3);
        assert_eq!(t.name(), "Display P3");
        assert_eq!(t.css_name(), Some("display-p3"));
        assert!(!t.is_srgb());
        assert_eq!(BuiltinTarget(RgbSpace::Srgb).name(), "sRGB");
        assert_eq!(BuiltinTarget(RgbSpace::A98Rgb).css_name(), Some("a98-rgb"));
        assert_eq!(BuiltinTarget(RgbSpace::A98Rgb).name(), "Adobe RGB");
        assert_eq!(BuiltinTarget(RgbSpace::Srgb).css_name(), Some("srgb"));
    }

    #[test]
    fn p3_out_of_srgb() {
        let c = Color::from_rgb(RgbSpace::DisplayP3, [0.0, 1.0, 0.0]);
        let (rgb, out) = BuiltinTarget(RgbSpace::Srgb).encode(&c);
        assert!(out);
        assert!(rgb.iter().all(|v| (0.0..=1.0).contains(v)));
    }
}
