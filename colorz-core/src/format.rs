//! Text output of a [`Color`] in the supported formats and copy forms.

use serde::{Deserialize, Serialize};

use crate::color::{Color, RgbSpace};
use crate::color::{rgb_to_hsl, rgb_to_hsv};
use crate::target::{BuiltinTarget, TargetSpace};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Hex,
    Rgb,
    Hsl,
    Hsv,
    Oklch,
    Oklab,
    CssColor,
}

impl Format {
    pub const ALL: [Format; 7] = [
        Format::Hex,
        Format::Rgb,
        Format::Hsl,
        Format::Hsv,
        Format::Oklch,
        Format::Oklab,
        Format::CssColor,
    ];

    /// Whether the full form is valid CSS; `hsv()` is not, so it is never
    /// offered for CSS export.
    pub fn is_css(self) -> bool {
        self != Format::Hsv
    }

    /// Whether [`format_color`] can write this format for `target`:
    /// `color()` needs a target with a CSS name (an sRGB target writes
    /// Display P3), which a custom ICC profile lacks.
    pub fn shows_in(self, target: &dyn TargetSpace) -> bool {
        self != Format::CssColor || target.is_srgb() || target.css_name().is_some()
    }

    /// `self`, or HEX when `target` cannot show it. Default-format copies
    /// use this so they never fail.
    pub fn or_hex_for(self, target: &dyn TargetSpace) -> Format {
        if self.shows_in(target) {
            self
        } else {
            Format::Hex
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Hex => "HEX",
            Format::Rgb => "RGB",
            Format::Hsl => "HSL",
            Format::Hsv => "HSV",
            Format::Oklch => "OKLCH",
            Format::Oklab => "OKLab",
            Format::CssColor => "color()",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Form {
    Full,
    Numbers,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HexCase {
    #[default]
    Upper,
    Lower,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CssSyntax {
    #[default]
    Modern,
    Legacy,
}

/// The decimal places setting: its default and allowed range.
pub const DEFAULT_DECIMALS: usize = 3;
pub const MIN_DECIMALS: usize = 2;
pub const MAX_DECIMALS: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FormatOptions {
    pub hex_case: HexCase,
    pub css_syntax: CssSyntax,
    /// Decimal places for OKLCH and OKLab, clamped to
    /// `MIN_DECIMALS..=MAX_DECIMALS`. The other fractional numbers keep
    /// their offset from it: OKLCH hue two fewer, HSL/HSV three fewer,
    /// `color()` one more. RGB and HEX are always whole bytes.
    pub decimals: usize,
}

impl Default for FormatOptions {
    fn default() -> Self {
        FormatOptions {
            hex_case: HexCase::default(),
            css_syntax: CssSyntax::default(),
            decimals: DEFAULT_DECIMALS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Formatted {
    pub text: String,
    pub out_of_gamut: bool,
}

/// Rounds to `decimals`, trims trailing zeros and never yields `-0`.
pub fn fmt_num(x: f64, decimals: usize) -> String {
    let s = format!("{x:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        &s
    };
    if s == "-0" || s == "NaN" || s == "-NaN" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

fn byte(v: f64) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

fn percent(v: f64, decimals: usize) -> String {
    format!("{}%", fmt_num(v * 100.0, decimals))
}

/// A hue in `[0, 360)`: a value that rounds up to 360 prints as 0.
fn hue(h: f64, decimals: usize) -> String {
    match fmt_num(h.rem_euclid(360.0), decimals).as_str() {
        "360" => "0".to_string(),
        other => other.to_string(),
    }
}

fn wrap(full: bool, name: &str, body: String) -> String {
    if full {
        format!("{name}({body})")
    } else {
        body
    }
}

pub fn format_color(
    c: &Color,
    f: Format,
    form: Form,
    opts: &FormatOptions,
    target: &dyn TargetSpace,
) -> Option<Formatted> {
    let full = form == Form::Full;
    let dp = opts.decimals.clamp(MIN_DECIMALS, MAX_DECIMALS);
    let legacy = opts.css_syntax == CssSyntax::Legacy;
    let sep = if legacy { ", " } else { " " };
    let join = |parts: &[String]| parts.join(sep);
    let plain = |text: String| Formatted {
        text,
        out_of_gamut: false,
    };

    Some(match f {
        Format::Hex => {
            let (rgb, oog) = target.encode(c);
            let [r, g, b] = rgb.map(byte);
            let digits = match opts.hex_case {
                HexCase::Upper => format!("{r:02X}{g:02X}{b:02X}"),
                HexCase::Lower => format!("{r:02x}{g:02x}{b:02x}"),
            };
            Formatted {
                text: if full { format!("#{digits}") } else { digits },
                out_of_gamut: oog,
            }
        }
        Format::Rgb => {
            let (rgb, oog) = target.encode(c);
            let parts = rgb.map(|v| byte(v).to_string());
            Formatted {
                text: wrap(full, "rgb", join(&parts)),
                out_of_gamut: oog,
            }
        }
        Format::Hsl | Format::Hsv => {
            let (rgb, oog) = target.encode(c);
            let (name, [h, s, x]) = if f == Format::Hsl {
                ("hsl", rgb_to_hsl(rgb))
            } else {
                ("hsv", rgb_to_hsv(rgb))
            };
            let d = dp.saturating_sub(3);
            let parts = [hue(h, d), percent(s, d), percent(x, d)];
            Formatted {
                text: wrap(full, name, join(&parts)),
                out_of_gamut: oog,
            }
        }
        Format::Oklch => {
            let [l, ch, h] = c.to_oklch();
            let chroma = fmt_num(ch, dp);
            let h = if chroma == "0" {
                "0".to_string()
            } else {
                hue(h, dp.saturating_sub(2))
            };
            let body = format!("{} {chroma} {h}", fmt_num(l, dp));
            plain(wrap(full, "oklch", body))
        }
        Format::Oklab => {
            let body = format!(
                "{} {} {}",
                fmt_num(c.l, dp),
                fmt_num(c.a, dp),
                fmt_num(c.b, dp)
            );
            plain(wrap(full, "oklab", body))
        }
        Format::CssColor => {
            let (css, (rgb, oog)) = if target.is_srgb() {
                ("display-p3", BuiltinTarget(RgbSpace::DisplayP3).encode(c))
            } else {
                (target.css_name()?, target.encode(c))
            };
            let nums = rgb.map(|v| fmt_num(v, dp + 1)).join(" ");
            Formatted {
                text: if full {
                    format!("color({css} {nums})")
                } else {
                    nums
                },
                out_of_gamut: oog,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blue() -> Color {
        Color::from_srgb8([0x34, 0x78, 0xF6])
    }

    fn fmt(c: &Color, f: Format, form: Form, opts: &FormatOptions) -> String {
        format_color(c, f, form, opts, &BuiltinTarget(RgbSpace::Srgb))
            .unwrap()
            .text
    }

    fn both(f: Format) -> (String, String) {
        let o = FormatOptions::default();
        (
            fmt(&blue(), f, Form::Full, &o),
            fmt(&blue(), f, Form::Numbers, &o),
        )
    }

    #[test]
    fn hex() {
        assert_eq!(both(Format::Hex), ("#3478F6".into(), "3478F6".into()));
        let lower = FormatOptions {
            hex_case: HexCase::Lower,
            ..Default::default()
        };
        assert_eq!(fmt(&blue(), Format::Hex, Form::Full, &lower), "#3478f6");
    }

    #[test]
    fn rgb_modern_legacy() {
        assert_eq!(
            both(Format::Rgb),
            ("rgb(52 120 246)".into(), "52 120 246".into())
        );
        let legacy = FormatOptions {
            css_syntax: CssSyntax::Legacy,
            ..Default::default()
        };
        assert_eq!(
            fmt(&blue(), Format::Rgb, Form::Full, &legacy),
            "rgb(52, 120, 246)"
        );
        assert_eq!(
            fmt(&blue(), Format::Rgb, Form::Numbers, &legacy),
            "52, 120, 246"
        );
    }

    #[test]
    fn hsl_hsv() {
        assert_eq!(
            both(Format::Hsl),
            ("hsl(219 92% 58%)".into(), "219 92% 58%".into())
        );
        assert_eq!(
            both(Format::Hsv),
            ("hsv(219 79% 96%)".into(), "219 79% 96%".into())
        );
        let legacy = FormatOptions {
            css_syntax: CssSyntax::Legacy,
            ..Default::default()
        };
        assert_eq!(
            fmt(&blue(), Format::Hsl, Form::Full, &legacy),
            "hsl(219, 92%, 58%)"
        );
    }

    #[test]
    fn oklch_oklab() {
        assert_eq!(
            both(Format::Oklch),
            ("oklch(0.6 0.201 261.4)".into(), "0.6 0.201 261.4".into())
        );
        assert_eq!(
            both(Format::Oklab),
            ("oklab(0.6 -0.03 -0.199)".into(), "0.6 -0.03 -0.199".into())
        );
    }

    #[test]
    fn css_color() {
        assert_eq!(
            both(Format::CssColor),
            (
                "color(display-p3 0.2753 0.4646 0.9325)".into(),
                "0.2753 0.4646 0.9325".into()
            )
        );
    }

    #[test]
    fn adobe_target() {
        let t = BuiltinTarget(RgbSpace::A98Rgb);
        let o = FormatOptions::default();
        let run = |f| format_color(&blue(), f, Form::Full, &o, &t).unwrap().text;
        assert_eq!(run(Format::Hex), "#5077F2");
        assert_eq!(run(Format::CssColor), "color(a98-rgb 0.3136 0.4675 0.9491)");
        assert_eq!(run(Format::Oklch), "oklch(0.6 0.201 261.4)");
    }

    #[test]
    fn achromatic_outputs_are_clean() {
        let o = FormatOptions::default();
        for px in [[0, 0, 0], [255, 255, 255], [128, 128, 128]] {
            let c = Color::from_srgb8(px);
            for f in Format::ALL {
                for form in [Form::Full, Form::Numbers] {
                    let t = fmt(&c, f, form, &o);
                    assert!(!t.contains("NaN") && !t.contains("-0"), "{t}");
                    let t = fmt(
                        &c,
                        f,
                        form,
                        &FormatOptions {
                            css_syntax: CssSyntax::Legacy,
                            ..o
                        },
                    );
                    assert!(!t.contains("NaN") && !t.contains("-0"), "{t}");
                }
            }
        }
        let black = Color::from_srgb8([0, 0, 0]);
        assert_eq!(fmt(&black, Format::Oklch, Form::Full, &o), "oklch(0 0 0)");
        let white = Color::from_srgb8([255, 255, 255]);
        assert_eq!(fmt(&white, Format::Hsl, Form::Full, &o), "hsl(0 0% 100%)");
    }

    #[test]
    fn oog_flagged() {
        let c = Color::from_oklch([0.7, 0.4, 150.0]);
        let t = BuiltinTarget(RgbSpace::Srgb);
        let o = FormatOptions::default();
        let hex = format_color(&c, Format::Hex, Form::Full, &o, &t).unwrap();
        assert!(hex.out_of_gamut);
        assert_eq!(hex.text.len(), 7);
        assert!(hex.text[1..].chars().all(|ch| ch.is_ascii_hexdigit()));
        let lch = format_color(&c, Format::Oklch, Form::Full, &o, &t).unwrap();
        assert_eq!(lch.text, "oklch(0.7 0.4 150)");
        assert!(!lch.out_of_gamut);
    }

    #[test]
    fn oklch_hue_zero_when_chroma_prints_zero() {
        let o = FormatOptions::default();
        let white = Color::from_srgb8([255, 255, 255]);
        assert_eq!(fmt(&white, Format::Oklch, Form::Full, &o), "oklch(1 0 0)");
        let gray = Color::from_srgb8([128, 128, 128]);
        assert!(fmt(&gray, Format::Oklch, Form::Full, &o).ends_with(" 0 0)"));
        let wrap = Color::from_oklch([0.5, 0.1, 359.97]);
        assert_eq!(
            fmt(&wrap, Format::Oklch, Form::Full, &o),
            "oklch(0.5 0.1 0)"
        );
    }

    /// Like a custom ICC profile: no CSS name.
    struct Unnamed;

    impl TargetSpace for Unnamed {
        fn name(&self) -> String {
            "Custom".into()
        }
        fn css_name(&self) -> Option<&'static str> {
            None
        }
        fn is_srgb(&self) -> bool {
            false
        }
        fn encode(&self, c: &Color) -> ([f64; 3], bool) {
            BuiltinTarget(RgbSpace::A98Rgb).encode(c)
        }
        fn decode(&self, rgb: [f64; 3]) -> Color {
            BuiltinTarget(RgbSpace::A98Rgb).decode(rgb)
        }
    }

    #[test]
    fn unavailable_format_falls_back_to_hex() {
        let o = FormatOptions::default();
        let targets: [&dyn TargetSpace; 3] = [
            &BuiltinTarget(RgbSpace::Srgb),
            &BuiltinTarget(RgbSpace::DisplayP3),
            &Unnamed,
        ];
        for t in targets {
            for f in Format::ALL {
                let shown = format_color(&blue(), f, Form::Full, &o, t).is_some();
                assert_eq!(f.shows_in(t), shown, "{f:?} in {}", t.name());
                let fallback = f.or_hex_for(t);
                assert!(format_color(&blue(), fallback, Form::Numbers, &o, t).is_some());
                assert_eq!(fallback == f, shown);
            }
        }
        assert_eq!(Format::CssColor.or_hex_for(&Unnamed), Format::Hex);
        assert_eq!(
            Format::CssColor.or_hex_for(&BuiltinTarget(RgbSpace::Srgb)),
            Format::CssColor
        );
        assert_eq!(Format::Oklch.or_hex_for(&Unnamed), Format::Oklch);
    }

    fn with_decimals(decimals: usize) -> FormatOptions {
        FormatOptions {
            decimals,
            ..Default::default()
        }
    }

    #[test]
    fn decimals_scale_every_fractional_format() {
        let c = blue();
        let at = |d: usize, f: Format| fmt(&c, f, Form::Full, &with_decimals(d));
        // The default matches the original fixed precision.
        assert_eq!(
            at(3, Format::Oklch),
            fmt(&c, Format::Oklch, Form::Full, &FormatOptions::default())
        );
        assert_eq!(at(2, Format::Oklch), "oklch(0.6 0.2 261)");
        assert_eq!(at(3, Format::Oklch), "oklch(0.6 0.201 261.4)");
        assert_eq!(at(4, Format::Oklch), "oklch(0.6004 0.2013 261.37)");
        assert_eq!(at(5, Format::Oklch), "oklch(0.60041 0.20135 261.368)");
        assert_eq!(at(5, Format::Oklab), "oklab(0.60041 -0.03022 -0.19907)");
        assert_eq!(at(3, Format::Hsl), "hsl(219 92% 58%)");
        assert_eq!(at(4, Format::Hsl), "hsl(219 91.5% 58.4%)");
        assert_eq!(at(5, Format::Hsl), "hsl(218.97 91.51% 58.43%)");
        assert_eq!(
            at(3, Format::CssColor),
            "color(display-p3 0.2753 0.4646 0.9325)"
        );
        assert_eq!(
            at(4, Format::CssColor),
            "color(display-p3 0.27528 0.4646 0.93252)"
        );
        // RGB and HEX stay whole bytes.
        for d in MIN_DECIMALS..=MAX_DECIMALS {
            assert_eq!(at(d, Format::Hex), "#3478F6");
            assert_eq!(at(d, Format::Rgb), "rgb(52 120 246)");
        }
    }

    #[test]
    fn decimals_out_of_range_are_clamped() {
        let c = blue();
        for f in Format::ALL {
            let low = fmt(&c, f, Form::Full, &with_decimals(MIN_DECIMALS));
            assert_eq!(fmt(&c, f, Form::Full, &with_decimals(0)), low);
            let high = fmt(&c, f, Form::Full, &with_decimals(MAX_DECIMALS));
            assert_eq!(fmt(&c, f, Form::Full, &with_decimals(usize::MAX)), high);
        }
    }

    #[test]
    fn hue_rounding_up_to_360_prints_zero() {
        assert_eq!(hue(359.96, 1), "0");
        assert_eq!(hue(359.96, 2), "359.96");
        assert_eq!(hue(-0.01, 1), "0");
    }

    #[test]
    fn fmt_num_trims() {
        assert_eq!(fmt_num(0.5000, 3), "0.5");
        assert_eq!(fmt_num(-0.0001, 3), "0");
        assert_eq!(fmt_num(2.0, 1), "2");
    }
}
