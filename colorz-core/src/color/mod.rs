//! Core color model: unbounded OKLab `Color` plus conversions to and from
//! the common RGB spaces, CIE Lab (D50), HSL/HSV/HWB and XYZ.

use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

pub mod gamut;
pub mod okhsl;

pub use gamut::{gamut_map, gamut_map_with};

type Mat3 = [[f64; 3]; 3];

const SRGB_TO_XYZ: Mat3 = [
    [0.41239079926595934, 0.357584339383878, 0.1804807884018343],
    [0.21263900587151027, 0.715168678767756, 0.07219231536073371],
    [0.01933081871559182, 0.11919477979462598, 0.9505321522496607],
];
const P3_TO_XYZ: Mat3 = [
    [0.4865709486482162, 0.26566769316909306, 0.1982172852343625],
    [0.2289745640697488, 0.6917385218365064, 0.079286914093745],
    [0.0, 0.04511338185890264, 1.043944368900976],
];
const A98_TO_XYZ: Mat3 = [
    [0.5766690429101305, 0.1855582379065463, 0.1882286462349947],
    [0.29734497525053605, 0.6273635662554661, 0.0752914584939978],
    [0.02703136138641234, 0.07068885253582723, 0.9913375368376388],
];
const D65_TO_D50: Mat3 = [
    [
        1.0479297925449969,
        0.022946870601609652,
        -0.05019226628920524,
    ],
    [
        0.02962780877005599,
        0.9904344267538799,
        -0.017073799063418826,
    ],
    [
        -0.009243040646204504,
        0.015055191490298152,
        0.7518742814281371,
    ],
];
#[allow(clippy::excessive_precision)] // reference values kept verbatim
const XYZ_TO_LMS: Mat3 = [
    [0.8190224379967030, 0.3619062600528904, -0.1288737815209879],
    [0.0329836539323885, 0.9292868615863434, 0.0361446663506424],
    [0.0481771893596242, 0.2642395317527308, 0.6335478284694309],
];
#[allow(clippy::excessive_precision)] // reference values kept verbatim
const LMS_TO_OKLAB: Mat3 = [
    [0.2104542683093140, 0.7936177747023054, -0.0040720430116193],
    [1.9779985324311684, -2.4285922420485799, 0.4505937096174110],
    [0.0259040424655478, 0.7827717124575296, -0.8086757549230774],
];

/// D50 reference white as used by CSS Color 4 (xy = 0.3457, 0.3585).
const D50_WHITE: [f64; 3] = [0.3457 / 0.3585, 1.0, (1.0 - 0.3457 - 0.3585) / 0.3585];

fn mul(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

static INV_LMS_TO_OKLAB: LazyLock<Mat3> = LazyLock::new(|| invert(&LMS_TO_OKLAB));
static INV_XYZ_TO_LMS: LazyLock<Mat3> = LazyLock::new(|| invert(&XYZ_TO_LMS));
static INV_D65_TO_D50: LazyLock<Mat3> = LazyLock::new(|| invert(&D65_TO_D50));
static INV_SRGB_TO_XYZ: LazyLock<Mat3> = LazyLock::new(|| invert(&SRGB_TO_XYZ));
static INV_P3_TO_XYZ: LazyLock<Mat3> = LazyLock::new(|| invert(&P3_TO_XYZ));
static INV_A98_TO_XYZ: LazyLock<Mat3> = LazyLock::new(|| invert(&A98_TO_XYZ));

fn invert(m: &Mat3) -> Mat3 {
    let c =
        |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let cof = [
        [c(1, 1, 2, 2), -c(1, 0, 2, 2), c(1, 0, 2, 1)],
        [-c(0, 1, 2, 2), c(0, 0, 2, 2), -c(0, 0, 2, 1)],
        [c(0, 1, 1, 2), -c(0, 0, 1, 2), c(0, 0, 1, 1)],
    ];
    let det = m[0][0] * cof[0][0] + m[0][1] * cof[0][1] + m[0][2] * cof[0][2];
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = cof[j][i] / det;
        }
    }
    out
}

/// A color as unbounded OKLab. Values outside any RGB gamut are kept as-is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub l: f64,
    pub a: f64,
    pub b: f64,
}

/// RGB working spaces with a gamma-encoded representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RgbSpace {
    Srgb,
    DisplayP3,
    A98Rgb,
}

impl RgbSpace {
    fn to_xyz_matrix(self) -> &'static Mat3 {
        match self {
            RgbSpace::Srgb => &SRGB_TO_XYZ,
            RgbSpace::DisplayP3 => &P3_TO_XYZ,
            RgbSpace::A98Rgb => &A98_TO_XYZ,
        }
    }

    fn xyz_to_rgb_matrix(self) -> &'static Mat3 {
        match self {
            RgbSpace::Srgb => &INV_SRGB_TO_XYZ,
            RgbSpace::DisplayP3 => &INV_P3_TO_XYZ,
            RgbSpace::A98Rgb => &INV_A98_TO_XYZ,
        }
    }

    fn decode(self, v: f64) -> f64 {
        match self {
            RgbSpace::Srgb | RgbSpace::DisplayP3 => srgb_decode(v),
            RgbSpace::A98Rgb => v.signum() * v.abs().powf(563.0 / 256.0),
        }
    }

    fn encode(self, v: f64) -> f64 {
        match self {
            RgbSpace::Srgb | RgbSpace::DisplayP3 => srgb_encode(v),
            RgbSpace::A98Rgb => v.signum() * v.abs().powf(256.0 / 563.0),
        }
    }
}

/// sRGB electro-optical transfer (encoded -> linear), sign-preserving.
fn srgb_decode(v: f64) -> f64 {
    let a = v.abs();
    if a <= 0.04045 {
        v / 12.92
    } else {
        v.signum() * ((a + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB inverse transfer (linear -> encoded), sign-preserving.
fn srgb_encode(v: f64) -> f64 {
    let a = v.abs();
    if a <= 0.0031308 {
        v * 12.92
    } else {
        v.signum() * (1.055 * a.powf(1.0 / 2.4) - 0.055)
    }
}

/// Emulates Chrome's pure-gamma-2.2 display of sRGB values: each
/// gamma-encoded sRGB channel is decoded with gamma 2.2 and re-encoded with
/// the sRGB curve.
pub fn chrome_correct(rgb: [f64; 3]) -> [f64; 3] {
    rgb.map(|v| srgb_encode(v.signum() * v.abs().powf(2.2)))
}

impl Color {
    pub fn from_oklab(l: f64, a: f64, b: f64) -> Self {
        Color { l, a, b }
    }

    /// Returns `[L, C, H]` with H in degrees in `[0, 360)`; H is 0 when the
    /// color is achromatic (C < 1e-7).
    pub fn to_oklch(&self) -> [f64; 3] {
        let c = self.a.hypot(self.b);
        let h = if c < 1e-7 {
            0.0
        } else {
            let h = self.b.atan2(self.a).to_degrees();
            let h = if h < 0.0 { h + 360.0 } else { h };
            if h >= 360.0 { 0.0 } else { h }
        };
        [self.l, c, h]
    }

    pub fn from_oklch([l, c, h]: [f64; 3]) -> Self {
        let r = h.to_radians();
        Color {
            l,
            a: c * r.cos(),
            b: c * r.sin(),
        }
    }

    pub fn from_xyz_d65(xyz: [f64; 3]) -> Self {
        let lms = mul(&XYZ_TO_LMS, xyz).map(f64::cbrt);
        let [l, a, b] = mul(&LMS_TO_OKLAB, lms);
        Color { l, a, b }
    }

    pub fn to_xyz_d65(&self) -> [f64; 3] {
        let lms = mul(&INV_LMS_TO_OKLAB, [self.l, self.a, self.b]).map(|v| v * v * v);
        mul(&INV_XYZ_TO_LMS, lms)
    }

    pub fn from_xyz_d50(xyz: [f64; 3]) -> Self {
        Self::from_xyz_d65(mul(&INV_D65_TO_D50, xyz))
    }

    pub fn to_xyz_d50(&self) -> [f64; 3] {
        mul(&D65_TO_D50, self.to_xyz_d65())
    }

    pub fn from_linear_srgb(rgb: [f64; 3]) -> Self {
        Self::from_xyz_d65(mul(&SRGB_TO_XYZ, rgb))
    }

    pub fn to_linear_srgb(&self) -> [f64; 3] {
        mul(&INV_SRGB_TO_XYZ, self.to_xyz_d65())
    }

    /// From gamma-encoded, possibly out-of-range channels of `space`.
    pub fn from_rgb(space: RgbSpace, rgb: [f64; 3]) -> Self {
        let linear = rgb.map(|v| space.decode(v));
        Self::from_xyz_d65(mul(space.to_xyz_matrix(), linear))
    }

    /// Gamma-encoded channels of `space`, **not** clamped.
    pub fn to_rgb(&self, space: RgbSpace) -> [f64; 3] {
        mul(space.xyz_to_rgb_matrix(), self.to_xyz_d65()).map(|v| space.encode(v))
    }

    pub fn from_srgb8(rgb: [u8; 3]) -> Self {
        Self::from_rgb(RgbSpace::Srgb, rgb.map(|v| f64::from(v) / 255.0))
    }

    pub fn from_cielab_d50([l, a, b]: [f64; 3]) -> Self {
        const EPS: f64 = 216.0 / 24389.0;
        const KAPPA: f64 = 24389.0 / 27.0;
        let fy = (l + 16.0) / 116.0;
        let fx = a / 500.0 + fy;
        let fz = fy - b / 200.0;
        let f = |t: f64| {
            if t * t * t > EPS {
                t * t * t
            } else {
                (116.0 * t - 16.0) / KAPPA
            }
        };
        let y = if l > KAPPA * EPS {
            fy * fy * fy
        } else {
            l / KAPPA
        };
        let xyz = [f(fx) * D50_WHITE[0], y * D50_WHITE[1], f(fz) * D50_WHITE[2]];
        Self::from_xyz_d50(xyz)
    }

    pub fn from_cielch_d50([l, c, h]: [f64; 3]) -> Self {
        let r = h.to_radians();
        Self::from_cielab_d50([l, c * r.cos(), c * r.sin()])
    }

    /// `[h°, s, v]` with s, v in 0..1.
    pub fn from_hsv(hsv: [f64; 3]) -> Self {
        Self::from_rgb(RgbSpace::Srgb, hsv_to_rgb(hsv))
    }

    /// `[h°, s, l]` with s, l in 0..1.
    pub fn from_hsl(hsl: [f64; 3]) -> Self {
        Self::from_rgb(RgbSpace::Srgb, hsl_to_rgb(hsl))
    }

    /// `[h°, w, b]` with w, b in 0..1.
    pub fn from_hwb([h, w, b]: [f64; 3]) -> Self {
        if w + b >= 1.0 {
            let g = w / (w + b);
            return Self::from_rgb(RgbSpace::Srgb, [g; 3]);
        }
        let base = Self::from_hsl([h, 1.0, 0.5]).to_rgb(RgbSpace::Srgb);
        Self::from_rgb(RgbSpace::Srgb, base.map(|v| v * (1.0 - w - b) + w))
    }

    /// `[h°, s, v]` with s, v in 0..1.
    pub fn to_hsv(&self) -> [f64; 3] {
        rgb_to_hsv(gamut_map(self, RgbSpace::Srgb))
    }

    /// `[h°, s, l]` with s, l in 0..1.
    pub fn to_hsl(&self) -> [f64; 3] {
        rgb_to_hsl(gamut_map(self, RgbSpace::Srgb))
    }

    /// True when every encoded channel of `space` lies in `[0, 1]` within 1e-6.
    pub fn in_gamut(&self, space: RgbSpace) -> bool {
        const TOL: f64 = 1e-6;
        self.to_rgb(space)
            .iter()
            .all(|v| *v >= -TOL && *v <= 1.0 + TOL)
    }
}

/// Encoded RGB channels in 0..1 from `[h°, s, l]` with s, l in 0..1.
pub(crate) fn hsl_to_rgb([h, s, l]: [f64; 3]) -> [f64; 3] {
    let a = s * l.min(1.0 - l);
    let k = |n: f64| (n + h / 30.0).rem_euclid(12.0);
    let f = |n: f64| l - a * (k(n) - 3.0).min(9.0 - k(n)).clamp(-1.0, 1.0);
    [f(0.0), f(8.0), f(4.0)]
}

/// Encoded RGB channels in 0..1 from `[h°, s, v]` with s, v in 0..1.
pub(crate) fn hsv_to_rgb([h, s, v]: [f64; 3]) -> [f64; 3] {
    let l = v * (1.0 - s / 2.0);
    let sl = if l == 0.0 || l == 1.0 {
        0.0
    } else {
        (v - l) / l.min(1.0 - l)
    };
    hsl_to_rgb([h, sl, l])
}

/// `[h°, s, v]` of encoded RGB channels in 0..1.
pub(crate) fn rgb_to_hsv(rgb: [f64; 3]) -> [f64; 3] {
    let [h, s, l] = rgb_to_hsl(rgb);
    let v = l + s * l.min(1.0 - l);
    let sv = if v == 0.0 { 0.0 } else { 2.0 * (1.0 - l / v) };
    [h, sv, v]
}

/// `[h°, s, l]` of encoded RGB channels in 0..1.
pub(crate) fn rgb_to_hsl([r, g, b]: [f64; 3]) -> [f64; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let l = (max + min) / 2.0;
    if d < 1e-9 {
        return [0.0, 0.0, l];
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [h * 60.0, s, l]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    fn assert_close(actual: &[f64], expected: &[f64], tol: f64) {
        for (a, e) in actual.iter().zip(expected) {
            assert!(close(*a, *e, tol), "{actual:?} vs {expected:?} (tol {tol})");
        }
    }

    fn to8(rgb: [f64; 3]) -> [u8; 3] {
        rgb.map(|v| (v * 255.0).round() as u8)
    }

    const BLUE: [u8; 3] = [0x34, 0x78, 0xF6];
    const ORANGE: [u8; 3] = [0xF6, 0xB2, 0x34];
    const GREEN: [u8; 3] = [0x2E, 0xC2, 0x7E];

    #[test]
    fn srgb8_round_trip_all_channels() {
        for v in 0..=255u8 {
            let triples = [[v, v, v], [v, 255 - v, ((v as u32 * 7) % 256) as u8]];
            for x in triples {
                let rgb = Color::from_srgb8(x).to_rgb(RgbSpace::Srgb);
                assert_eq!(to8(rgb), x);
            }
        }
    }

    #[test]
    fn oklab_reference() {
        let c = Color::from_srgb8(BLUE);
        assert_close(&[c.l, c.a, c.b], &[0.60041, -0.03022, -0.19907], 1e-4);
        let lch = c.to_oklch();
        assert_close(&lch[..2], &[0.60041, 0.20135], 1e-4);
        assert_close(&lch[2..], &[261.368], 1e-2);

        let lch = Color::from_srgb8(ORANGE).to_oklch();
        assert_close(&lch[..2], &[0.80775, 0.15332], 1e-4);
        assert_close(&lch[2..], &[77.832], 1e-2);

        let lch = Color::from_srgb8(GREEN).to_oklch();
        assert_close(&lch[..2], &[0.72246, 0.15681], 1e-4);
        assert_close(&lch[2..], &[157.738], 1e-2);
    }

    #[test]
    fn p3_and_a98_reference() {
        let c = Color::from_srgb8(BLUE);
        assert_close(
            &c.to_rgb(RgbSpace::DisplayP3),
            &[0.27528, 0.46460, 0.93252],
            1e-4,
        );
        assert_close(
            &c.to_rgb(RgbSpace::A98Rgb),
            &[0.31361, 0.46748, 0.94906],
            1e-4,
        );
        let x = [0.2, 0.7, 0.4];
        let back = Color::from_rgb(RgbSpace::A98Rgb, x).to_rgb(RgbSpace::A98Rgb);
        assert_close(&back, &x, 1e-9);
        let back = Color::from_rgb(RgbSpace::DisplayP3, x).to_rgb(RgbSpace::DisplayP3);
        assert_close(&back, &x, 1e-9);
    }

    #[test]
    fn hsl_hsv_reference() {
        let c = Color::from_srgb8(BLUE);
        assert_close(&c.to_hsl(), &[218.97, 0.9151, 0.5843], 1e-2);
        assert_close(&c.to_hsv(), &[218.97, 0.7886, 0.9647], 1e-2);
        for x in [BLUE, ORANGE, GREEN, [0x80, 0x10, 0xC0]] {
            let c = Color::from_srgb8(x);
            let back = Color::from_hsv(c.to_hsv()).to_rgb(RgbSpace::Srgb);
            assert_eq!(to8(back), x);
            let back = Color::from_hsl(c.to_hsl()).to_rgb(RgbSpace::Srgb);
            assert_eq!(to8(back), x);
        }
    }

    #[test]
    fn hwb_basics() {
        let red = Color::from_hwb([0.0, 0.0, 0.0]).to_rgb(RgbSpace::Srgb);
        assert_close(&red, &[1.0, 0.0, 0.0], 1e-9);
        let gray = Color::from_hwb([120.0, 0.5, 0.5]).to_rgb(RgbSpace::Srgb);
        assert_close(&gray, &[0.5, 0.5, 0.5], 1e-9);
        let pale = Color::from_hwb([0.0, 0.2, 0.2]).to_rgb(RgbSpace::Srgb);
        assert_close(&pale, &[0.8, 0.2, 0.2], 1e-9);
    }

    #[test]
    fn cielab_reference() {
        let c = Color::from_cielab_d50([50.0, 0.0, 0.0]);
        assert!(c.a.abs() < 1e-4 && c.b.abs() < 1e-4, "{c:?}");
        let white = Color::from_cielab_d50([100.0, 0.0, 0.0]).to_rgb(RgbSpace::Srgb);
        assert_close(&white, &[1.0, 1.0, 1.0], 1e-3);
        let lch = Color::from_cielch_d50([50.0, 30.0, 40.0]);
        let lab = Color::from_cielab_d50([
            50.0,
            30.0 * 40f64.to_radians().cos(),
            30.0 * 40f64.to_radians().sin(),
        ]);
        assert_close(&[lch.l, lch.a, lch.b], &[lab.l, lab.a, lab.b], 1e-12);
    }

    #[test]
    fn xyz_round_trips() {
        let c = Color::from_srgb8(BLUE);
        let d65 = Color::from_xyz_d65(c.to_xyz_d65());
        let d50 = Color::from_xyz_d50(c.to_xyz_d50());
        assert_close(&[d65.l, d65.a, d65.b], &[c.l, c.a, c.b], 1e-12);
        assert_close(&[d50.l, d50.a, d50.b], &[c.l, c.a, c.b], 1e-12);
    }

    #[test]
    fn chrome_correction_reference() {
        let out = chrome_correct([55.0 / 255.0, 119.0 / 255.0, 246.0 / 255.0]);
        assert_eq!(to8(out), [52, 120, 246]);
        assert_eq!(chrome_correct([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
        assert_close(&chrome_correct([1.0, 1.0, 1.0]), &[1.0, 1.0, 1.0], 1e-12);
    }

    #[test]
    fn unbounded_kept() {
        let c = Color::from_oklch([0.7, 0.4, 150.0]);
        let lch = c.to_oklch();
        assert_close(&lch, &[0.7, 0.4, 150.0], 1e-9);
        assert!(!c.in_gamut(RgbSpace::Srgb));
        assert!(Color::from_srgb8(BLUE).in_gamut(RgbSpace::Srgb));
    }

    #[test]
    fn gray_has_no_spurious_hue() {
        let c = Color::from_srgb8([0x80, 0x80, 0x80]);
        let hsl = c.to_hsl();
        assert_eq!((hsl[0], hsl[1]), (0.0, 0.0), "{hsl:?}");
        let hsv = c.to_hsv();
        assert_eq!((hsv[0], hsv[1]), (0.0, 0.0), "{hsv:?}");
    }

    #[test]
    fn achromatic_hue_is_zero() {
        let lch = Color::from_oklab(0.5, 0.0, 0.0).to_oklch();
        assert_eq!(lch[2], 0.0);
    }
}
