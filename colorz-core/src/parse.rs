//! Parsing of clipboard text and CSS color values into a [`Color`].

use std::f64::consts::PI;

use crate::color::{Color, RgbSpace, hsl_to_rgb, hsv_to_rgb};
use crate::format::Format;
use crate::named::named_color;
use crate::target::{BuiltinTarget, TargetSpace};

pub struct ParseContext<'a> {
    pub default_format: Format,
    pub target: &'a dyn TargetSpace,
}

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum ParseError {
    #[error("empty input")]
    Empty,
    #[error("no color found")]
    NoColor,
}

/// Clipboard semantics: hex (with or without `#`) and bare numbers are read
/// in `ctx.target`, as is `hsv()`; CSS functions follow CSS rules.
pub fn parse_color(input: &str, ctx: &ParseContext) -> Result<Color, ParseError> {
    let s = clean(input);
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    parse_inner(s, Some(ctx)).ok_or(ParseError::NoColor)
}

/// Strict CSS value semantics: sRGB, hex needs `#`, no bare numbers, no `hsv()`.
pub fn parse_css_color(input: &str) -> Option<Color> {
    let s = clean(input);
    if s.is_empty() {
        return None;
    }
    parse_inner(s, None)
}

fn clean(input: &str) -> &str {
    let s = input.trim();
    s.strip_suffix(';').unwrap_or(s).trim()
}

fn parse_inner(s: &str, ctx: Option<&ParseContext>) -> Option<Color> {
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex, ctx);
    }
    if let Some(open) = s.find('(') {
        let inner = s[open + 1..].strip_suffix(')')?;
        if inner.contains(['(', ')']) {
            return None;
        }
        return parse_function(&s[..open].trim().to_ascii_lowercase(), inner, ctx);
    }
    if let Some(rgb) = named_color(s) {
        return Some(Color::from_srgb8(rgb));
    }
    let ctx = ctx?;
    if let Some(c) = parse_hex(s, Some(ctx)) {
        return Some(c);
    }
    parse_bare_numbers(s, ctx)
}

fn parse_hex(hex: &str, ctx: Option<&ParseContext>) -> Option<Color> {
    if !matches!(hex.len(), 3 | 4 | 6 | 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok();
    let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let rgb = if hex.len() <= 4 {
        [digit(0)? * 17, digit(1)? * 17, digit(2)? * 17]
    } else {
        [pair(0)?, pair(2)?, pair(4)?]
    };
    Some(match ctx {
        Some(ctx) => ctx.target.decode(rgb.map(|v| f64::from(v) / 255.0)),
        None => Color::from_srgb8(rgb),
    })
}

/// One channel token: its value and whether it carried a `%`.
#[derive(Clone, Copy)]
struct Ch {
    v: f64,
    pct: bool,
}

fn number(t: &str) -> Option<f64> {
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn channel(t: &str, allow_none: bool) -> Option<Ch> {
    if allow_none && t.eq_ignore_ascii_case("none") {
        return Some(Ch { v: 0.0, pct: false });
    }
    match t.strip_suffix('%') {
        Some(n) => Some(Ch {
            v: number(n)?,
            pct: true,
        }),
        None => Some(Ch {
            v: number(t)?,
            pct: false,
        }),
    }
}

/// Hue in degrees; unitless means degrees.
fn hue(t: &str) -> Option<f64> {
    let t = t.to_ascii_lowercase();
    if t == "none" {
        return Some(0.0);
    }
    for (unit, scale) in [
        ("deg", 1.0),
        ("grad", 0.9),
        ("rad", 180.0 / PI),
        ("turn", 360.0),
    ] {
        if let Some(n) = t.strip_suffix(unit) {
            return Some(number(n)? * scale);
        }
    }
    number(&t)
}

fn tokens(s: &str) -> Vec<&str> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .collect()
}

fn parse_function(name: &str, inner: &str, ctx: Option<&ParseContext>) -> Option<Color> {
    let (main, alpha) = match inner.split_once('/') {
        Some((a, b)) => (a, Some(b.trim())),
        None => (inner, None),
    };
    let mut args = tokens(main);
    let legacy = main.contains(',');
    let alpha_ok = |t: &str| channel(t, true).is_some();
    if let Some(a) = alpha
        && (legacy || !alpha_ok(a))
    {
        return None;
    }
    let css_legacy_ok = matches!(name, "rgb" | "rgba" | "hsl" | "hsla" | "hsv");
    if legacy && (!css_legacy_ok || main.split(',').any(|p| p.split_whitespace().count() != 1)) {
        return None;
    }
    if legacy && args.len() == 4 {
        if !alpha_ok(args[3]) {
            return None;
        }
        args.pop();
    }

    if name == "color" {
        return parse_color_fn(&args);
    }
    if args.len() != 3 {
        return None;
    }
    let c = |i: usize| channel(args[i], true);
    match name {
        "rgb" | "rgba" => {
            let f = |ch: Ch| (if ch.pct { ch.v / 100.0 } else { ch.v / 255.0 }).clamp(0.0, 1.0);
            Some(Color::from_rgb(
                RgbSpace::Srgb,
                [f(c(0)?), f(c(1)?), f(c(2)?)],
            ))
        }
        "hsl" | "hsla" => Some(Color::from_hsl([
            hue(args[0])?,
            (c(1)?.v / 100.0).max(0.0),
            c(2)?.v / 100.0,
        ])),
        "hsv" => {
            let ctx = ctx?;
            let rgb = hsv_to_rgb([hue(args[0])?, (c(1)?.v / 100.0).max(0.0), c(2)?.v / 100.0]);
            Some(ctx.target.decode(rgb))
        }
        "hwb" => Some(Color::from_hwb([
            hue(args[0])?,
            (c(1)?.v / 100.0).max(0.0),
            (c(2)?.v / 100.0).max(0.0),
        ])),
        "lab" => Some(Color::from_cielab_d50([
            c(0)?.v,
            scaled(c(1)?, 1.0, 1.25),
            scaled(c(2)?, 1.0, 1.25),
        ])),
        "lch" => Some(Color::from_cielch_d50([
            c(0)?.v,
            scaled(c(1)?, 1.0, 1.5).max(0.0),
            hue(args[2])?,
        ])),
        "oklab" => Some(Color::from_oklab(
            unit_l(c(0)?),
            scaled(c(1)?, 1.0, 0.004),
            scaled(c(2)?, 1.0, 0.004),
        )),
        "oklch" => Some(Color::from_oklch([
            unit_l(c(0)?),
            scaled(c(1)?, 1.0, 0.004).max(0.0),
            hue(args[2])?,
        ])),
        _ => None,
    }
}

/// `v` as-is when unitless, `v * pct_scale` when a percentage.
fn scaled(ch: Ch, plain: f64, pct_scale: f64) -> f64 {
    ch.v * if ch.pct { pct_scale } else { plain }
}

/// Lightness where `%` maps 100% to 1.
fn unit_l(ch: Ch) -> f64 {
    scaled(ch, 1.0, 0.01)
}

fn parse_color_fn(args: &[&str]) -> Option<Color> {
    let [space, a, b, c] = args else { return None };
    let ch = |t: &str| {
        let ch = channel(t, true)?;
        Some(if ch.pct { ch.v / 100.0 } else { ch.v })
    };
    let v = [ch(a)?, ch(b)?, ch(c)?];
    Some(match space.to_ascii_lowercase().as_str() {
        "srgb" => Color::from_rgb(RgbSpace::Srgb, v),
        "display-p3" => Color::from_rgb(RgbSpace::DisplayP3, v),
        "a98-rgb" => Color::from_rgb(RgbSpace::A98Rgb, v),
        "srgb-linear" => Color::from_linear_srgb(v),
        "xyz" | "xyz-d65" => Color::from_xyz_d65(v),
        "xyz-d50" => Color::from_xyz_d50(v),
        _ => return None,
    })
}

fn parse_bare_numbers(s: &str, ctx: &ParseContext) -> Option<Color> {
    let toks = tokens(s);
    let [a, b, c] = toks[..] else { return None };
    let chs = [channel(a, false)?, channel(b, false)?, channel(c, false)?];
    let first = match ctx.default_format {
        Format::Hex => Format::Rgb,
        f => f,
    };
    let order = [
        Format::Rgb,
        Format::Hsl,
        Format::Hsv,
        Format::CssColor,
        Format::Oklch,
        Format::Oklab,
    ];
    std::iter::once(first)
        .chain(order.into_iter().filter(|f| *f != first))
        .find_map(|f| bare_as(f, chs, ctx))
}

fn bare_as(format: Format, chs: [Ch; 3], ctx: &ParseContext) -> Option<Color> {
    let [x, y, z] = chs;
    let v = chs.map(|c| c.v);
    let within = |c: Ch, lo: f64, hi: f64| (lo..=hi).contains(&c.v);
    match format {
        Format::Hex => None,
        Format::Rgb => {
            if chs.iter().all(|c| !c.pct) {
                let ints = chs
                    .iter()
                    .all(|c| c.v.fract() == 0.0 && within(*c, 0.0, 255.0));
                ints.then(|| ctx.target.decode(v.map(|n| n / 255.0)))
            } else {
                let pcts = chs.iter().all(|c| c.pct && within(*c, 0.0, 100.0));
                pcts.then(|| ctx.target.decode(v.map(|n| n / 100.0)))
            }
        }
        Format::Hsl | Format::Hsv => {
            // Unitless 0..1 triples are color() numbers, not hue/sat/light.
            let css_like = chs.iter().all(|c| !c.pct && within(*c, 0.0, 1.0));
            let ok = !x.pct
                && within(x, 0.0, 360.0)
                && within(y, 0.0, 100.0)
                && within(z, 0.0, 100.0)
                && !css_like;
            ok.then(|| {
                let hsx = [v[0], v[1] / 100.0, v[2] / 100.0];
                ctx.target.decode(if format == Format::Hsl {
                    hsl_to_rgb(hsx)
                } else {
                    hsv_to_rgb(hsx)
                })
            })
        }
        Format::CssColor => {
            let ok = chs.iter().all(|c| !c.pct && within(*c, 0.0, 1.0));
            ok.then(|| {
                if ctx.target.is_srgb() {
                    BuiltinTarget(RgbSpace::DisplayP3).decode(v)
                } else {
                    ctx.target.decode(v)
                }
            })
        }
        Format::Oklch => {
            let ok =
                lightness_ok(x) && !y.pct && within(y, 0.0, 0.4) && !z.pct && within(z, 0.0, 360.0);
            ok.then(|| Color::from_oklch([unit_l(x), v[1], v[2]]))
        }
        Format::Oklab => {
            let ok =
                lightness_ok(x) && !y.pct && within(y, -0.4, 0.4) && !z.pct && within(z, -0.4, 0.4);
            ok.then(|| Color::from_oklab(unit_l(x), v[1], v[2]))
        }
    }
}

fn lightness_ok(ch: Ch) -> bool {
    if ch.pct {
        (0.0..=100.0).contains(&ch.v)
    } else {
        (0.0..=1.0).contains(&ch.v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{CssSyntax, Form, FormatOptions, format_color};

    const SRGB: BuiltinTarget = BuiltinTarget(RgbSpace::Srgb);

    fn ctx(f: Format) -> ParseContext<'static> {
        ParseContext {
            default_format: f,
            target: &SRGB,
        }
    }

    fn p8(s: &str) -> [u8; 3] {
        parse_color(s, &ctx(Format::Hex)).unwrap().to_srgb8()
    }

    fn near(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
        a.iter()
            .zip(b)
            .all(|(x, y)| (i32::from(*x) - i32::from(y)).abs() <= tol)
    }

    const BLUE: [u8; 3] = [0x34, 0x78, 0xF6];

    #[test]
    fn four_or_more_decimals_round_trip_within_one() {
        let target = &BuiltinTarget(RgbSpace::Srgb);
        let steps: Vec<u8> = (0..=255).step_by(17).chain([1, 12, 254]).collect();
        let mut colors = vec![[0x0C, 0xFE, 0xFC]];
        for &r in &steps {
            for &g in &steps {
                for &b in &steps {
                    colors.push([r, g, b]);
                }
            }
        }
        for decimals in [4, crate::format::MAX_DECIMALS] {
            let o = FormatOptions {
                decimals,
                ..Default::default()
            };
            for f in Format::ALL {
                let ctx = ParseContext {
                    default_format: f,
                    target,
                };
                for &px in &colors {
                    let c = Color::from_srgb8(px);
                    let text = format_color(&c, f, Form::Full, &o, target).unwrap().text;
                    let got = parse_color(&text, &ctx)
                        .unwrap_or_else(|e| panic!("{text}: {e}"))
                        .to_srgb8();
                    assert!(near(got, px, 1), "{text}: {got:?} vs {px:?}");
                }
            }
        }
    }

    #[test]
    fn round_trips_every_format() {
        let refs = [
            BLUE,
            [0xF6, 0xB2, 0x34],
            [0x2E, 0xC2, 0x7E],
            [0x80, 0x80, 0x80],
        ];
        let targets = [
            BuiltinTarget(RgbSpace::Srgb),
            BuiltinTarget(RgbSpace::A98Rgb),
        ];
        for syntax in [CssSyntax::Modern, CssSyntax::Legacy] {
            let o = FormatOptions {
                css_syntax: syntax,
                ..Default::default()
            };
            for target in &targets {
                let a98 = !target.is_srgb();
                for px in refs {
                    let c = Color::from_srgb8(px);
                    for f in Format::ALL {
                        for form in [Form::Full, Form::Numbers] {
                            let numeric =
                                matches!(f, Format::Hex | Format::Rgb | Format::Hsl | Format::Hsv);
                            if a98 && !(numeric && form == Form::Numbers) {
                                continue;
                            }
                            let text = format_color(&c, f, form, &o, target).unwrap().text;
                            let ctx = ParseContext {
                                default_format: f,
                                target,
                            };
                            let got = parse_color(&text, &ctx)
                                .unwrap_or_else(|e| panic!("{text}: {e}"))
                                .to_srgb8();
                            let tol = match f {
                                Format::Hex | Format::Rgb | Format::CssColor => 0,
                                Format::Oklch | Format::Oklab => 1,
                                Format::Hsl | Format::Hsv => 3,
                            };
                            // Target-space numbers are quantized in the target gamut.
                            let tol = if a98 { tol.max(1) } else { tol };
                            assert!(near(got, px, tol), "{text}: {got:?} vs {px:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn hex_variants() {
        assert_eq!(p8("#3478F6"), BLUE);
        assert_eq!(p8("3478f6"), BLUE);
        assert_eq!(p8("#38F"), [0x33, 0x88, 0xFF]);
        assert_eq!(p8("#3478F6CC"), BLUE);
        assert_eq!(p8("  3478F6;\n"), BLUE);
    }

    #[test]
    fn css_syntaxes() {
        for s in [
            "rgb(52, 120, 246)",
            "rgba(52,120,246,0.5)",
            "rgb(20% 47% 96% / 50%)",
            "hsl(219deg 92% 58%)",
            "hsl(0.6083turn 92% 58%)",
            "hwb(219 20% 4%)",
            "lab(51.5 6 -66)",
            "lch(51.5 66 275)",
            "oklch(60% 0.2 261)",
            "oklab(0.6 -0.03 -0.2)",
            "color(display-p3 0.2753 0.4646 0.9325)",
            "color(srgb-linear 1 0 0)",
            "rebeccapurple",
            "RED",
            "hsl(none 0% 50%)",
        ] {
            assert!(parse_color(s, &ctx(Format::Hex)).is_ok(), "{s}");
        }
        assert_eq!(p8("color(display-p3 0.2753 0.4646 0.9325)"), BLUE);
        assert_eq!(p8("color(srgb-linear 1 0 0)"), [255, 0, 0]);
        assert_eq!(p8("rebeccapurple"), [0x66, 0x33, 0x99]);
        assert_eq!(p8("RED"), [255, 0, 0]);
        assert!(near(p8("hsl(219deg 92% 58%)"), BLUE, 3));
        assert!(near(p8("hsl(0.6083turn 92% 58%)"), BLUE, 3));
        assert!(near(p8("hsl(3.8222rad 92% 58%)"), BLUE, 3));
        assert!(near(p8("hsl(243.33grad 92% 58%)"), BLUE, 3));
        assert!(near(p8("rgb(20% 47% 96% / 50%)"), BLUE, 2));
    }

    #[test]
    fn bare_numbers_rule() {
        let hex = ctx(Format::Hex);
        let get = |s: &str, c: &ParseContext| parse_color(s, c).unwrap();
        assert_eq!(get("52 120 246", &hex).to_srgb8(), BLUE);
        assert_eq!(get("52, 120, 246", &hex).to_srgb8(), BLUE);
        assert!(near(get("219 92% 58%", &hex).to_srgb8(), BLUE, 3));
        assert!(near(get("0.6 0.201 261.4", &hex).to_srgb8(), BLUE, 1));
        let oklch = Color::from_oklch([0.6, 0.201, 261.4]);
        assert!((get("0.6 0.201 261.4", &hex).l - oklch.l).abs() < 1e-9);
        assert_eq!(get("0.2753 0.4646 0.9325", &hex).to_srgb8(), BLUE);
        assert!(near(get("0.27 0.46 0.93", &hex).to_srgb8(), BLUE, 3));
        let hsv = ctx(Format::Hsv);
        assert!(near(get("219 79% 96%", &hsv).to_srgb8(), BLUE, 1));
    }

    #[test]
    fn target_applies_to_hex_not_css() {
        let a98 = BuiltinTarget(RgbSpace::A98Rgb);
        let ctx = ParseContext {
            default_format: Format::Hex,
            target: &a98,
        };
        let got = parse_color("5077F2", &ctx).unwrap().to_srgb8();
        assert!(near(got, BLUE, 1), "{got:?}");
        let got = parse_color("rgb(80 119 242)", &ctx).unwrap().to_srgb8();
        assert_eq!(got, [80, 119, 242]);
    }

    #[test]
    fn rejects_non_colors() {
        for s in [
            "",
            "   ",
            "hello world",
            "transparent",
            "var(--x)",
            "12px",
            "1 2",
            "1 2 3 4",
            "#12345",
            "rgb(1 2)",
            "Some text #3478F6 inside",
            "none none none",
            "rgb(1, 2 3)",
            "rgb(1 2 3) rgb(4 5 6)",
            "hsv(219 79% 96%",
        ] {
            assert!(parse_color(s, &ctx(Format::Hex)).is_err(), "{s:?}");
        }
        assert_eq!(parse_color("  ", &ctx(Format::Hex)), Err(ParseError::Empty));
        assert_eq!(
            parse_color("nope", &ctx(Format::Hex)),
            Err(ParseError::NoColor)
        );
    }

    #[test]
    fn percent_scaling_and_malformed() {
        let c = ctx(Format::Hex);
        let same = |a: &str, b: &str| {
            let (x, y) = (parse_color(a, &c).unwrap(), parse_color(b, &c).unwrap());
            assert!(
                (x.l - y.l).abs() < 1e-3 && (x.a - y.a).abs() < 1e-3 && (x.b - y.b).abs() < 1e-3,
                "{a} vs {b}"
            );
        };
        same("oklch(60% 0.2 261)", "oklch(0.6 0.2 261)");
        same("oklch(0.6 50% 261)", "oklch(0.6 0.2 261)");
        same("oklab(0.6 -7.5% -50%)", "oklab(0.6 -0.03 -0.2)");
        same("lch(51.5 44% 275)", "lch(51.5 66 275)");
        same("lab(51.5 4.8% -52.8%)", "lab(51.5 6 -66)");
        same("hsv(219, 79%, 96%)", "hsv(219 79% 96%)");
        for s in ["hsl(1 2 3 4)", "color(srgb 1 0)", "color(foo 1 0 0)"] {
            assert!(parse_color(s, &c).is_err(), "{s}");
        }
    }

    #[test]
    fn css_strict() {
        assert!(parse_css_color("3478F6").is_none());
        assert!(parse_css_color("52 120 246").is_none());
        assert!(parse_css_color("hsv(219 79% 96%)").is_none());
        assert!(parse_css_color("hsv(219, 79%, 96%)").is_none());
        assert_eq!(parse_css_color("#3478F6").unwrap().to_srgb8(), BLUE);
        assert!(parse_css_color("rebeccapurple").is_some());
    }
}
