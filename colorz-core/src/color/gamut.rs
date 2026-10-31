//! CSS Color 4 gamut mapping (binary search in OKLCH with deltaEOK clip).

use super::{Color, RgbSpace};

const JND: f64 = 0.02;
const EPSILON: f64 = 0.0001;
const TOL: f64 = 1e-9;

fn clip(rgb: [f64; 3]) -> [f64; 3] {
    rgb.map(|v| v.clamp(0.0, 1.0))
}

fn in_range(rgb: &[f64; 3]) -> bool {
    rgb.iter().all(|v| *v >= -TOL && *v <= 1.0 + TOL)
}

fn delta_e_ok(x: &Color, y: &Color) -> f64 {
    ((x.l - y.l).powi(2) + (x.a - y.a).powi(2) + (x.b - y.b).powi(2)).sqrt()
}

/// CSS Color 4 gamut mapping: bisect chroma in OKLCH, accepting a clipped
/// result once it is within the JND of the chroma-reduced color.
fn map_impl(
    c: &Color,
    encode: &dyn Fn(&Color) -> [f64; 3],
    decode: &dyn Fn([f64; 3]) -> Color,
) -> [f64; 3] {
    if c.l >= 1.0 {
        return [1.0; 3];
    }
    if c.l <= 0.0 {
        return [0.0; 3];
    }
    let enc = encode(c);
    if in_range(&enc) {
        return clip(enc);
    }
    let [l, chroma, h] = c.to_oklch();
    let mut current = *c;
    let mut clipped = clip(enc);
    let e = delta_e_ok(&decode(clipped), &current);
    if e < JND {
        return clipped;
    }
    let (mut min, mut max) = (0.0, chroma);
    let mut min_in_gamut = true;
    while max - min > EPSILON {
        let mid = (min + max) / 2.0;
        current = Color::from_oklch([l, mid, h]);
        let enc = encode(&current);
        if min_in_gamut && in_range(&enc) {
            min = mid;
            continue;
        }
        clipped = clip(enc);
        let e = delta_e_ok(&decode(clipped), &current);
        if e < JND {
            if JND - e < EPSILON {
                return clipped;
            }
            min_in_gamut = false;
            min = mid;
        } else {
            max = mid;
        }
    }
    clipped
}

/// Gamut-maps `c` into `space`; returns encoded channels in `[0, 1]`.
pub fn gamut_map(c: &Color, space: RgbSpace) -> [f64; 3] {
    gamut_map_with(c, |x| x.to_rgb(space), |rgb| Color::from_rgb(space, rgb))
}

/// The algorithm against an arbitrary encoder/decoder pair (e.g. an ICC
/// transform). The encoder may return out-of-range channels; the result is
/// clamped to `[0, 1]`. `from_encoded` maps clipped channels back to a color.
pub fn gamut_map_with(
    c: &Color,
    to_encoded: impl Fn(&Color) -> [f64; 3],
    from_encoded: impl Fn([f64; 3]) -> Color,
) -> [f64; 3] {
    map_impl(c, &to_encoded, &from_encoded)
}

impl Color {
    /// Gamut-mapped sRGB, rounded to 8 bits.
    pub fn to_srgb8(&self) -> [u8; 3] {
        gamut_map(self, RgbSpace::Srgb).map(|v| (v * 255.0).round() as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamut_map_in_range() {
        let c = Color::from_oklch([0.7, 0.4, 150.0]);
        let rgb = gamut_map(&c, RgbSpace::Srgb);
        assert!(rgb.iter().all(|v| (0.0..=1.0).contains(v)), "{rgb:?}");
        let h = Color::from_rgb(RgbSpace::Srgb, rgb).to_oklch()[2];
        assert!((h - 150.0).abs() < 3.0, "hue {h}");

        let blue = Color::from_srgb8([0x34, 0x78, 0xF6]);
        assert_eq!(
            gamut_map(&blue, RgbSpace::Srgb),
            blue.to_rgb(RgbSpace::Srgb)
        );
    }

    #[test]
    fn to_srgb8_extremes() {
        assert_eq!(Color::from_oklch([1.2, 0.0, 0.0]).to_srgb8(), [255; 3]);
        assert_eq!(Color::from_oklch([-0.1, 0.0, 0.0]).to_srgb8(), [0; 3]);
    }

    #[test]
    fn gamut_map_with_matches() {
        let c = Color::from_oklch([0.7, 0.4, 150.0]);
        let a = gamut_map(&c, RgbSpace::Srgb);
        let b = gamut_map_with(
            &c,
            |x| x.to_rgb(RgbSpace::Srgb),
            |rgb| Color::from_rgb(RgbSpace::Srgb, rgb),
        );
        assert_eq!(a, b);
    }
}
