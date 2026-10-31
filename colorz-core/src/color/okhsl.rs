//! OKHSL, ported from Björn Ottosson's reference implementation
//! (<https://bottosson.github.io/posts/colorpicker/>).

use super::Color;
use std::f64::consts::PI;

#[derive(Clone, Copy)]
struct Lc {
    l: f64,
    c: f64,
}

fn toe(x: f64) -> f64 {
    const K1: f64 = 0.206;
    const K2: f64 = 0.03;
    const K3: f64 = (1.0 + K1) / (1.0 + K2);
    0.5 * (K3 * x - K1 + ((K3 * x - K1) * (K3 * x - K1) + 4.0 * K2 * K3 * x).sqrt())
}

fn toe_inv(x: f64) -> f64 {
    const K1: f64 = 0.206;
    const K2: f64 = 0.03;
    const K3: f64 = (1.0 + K1) / (1.0 + K2);
    (x * x + K1 * x) / (K3 * (x + K2))
}

/// Linear sRGB of an OKLab triple using Ottosson's reference matrices.
fn oklab_to_linear_srgb(l: f64, a: f64, b: f64) -> [f64; 3] {
    let l_ = l + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = l - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = l - 0.0894841775 * a - 1.2914855480 * b;
    let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    ]
}

/// Maximum saturation S = C/L reachable in sRGB for a unit direction (a, b).
fn compute_max_saturation(a: f64, b: f64) -> f64 {
    let (k, w) = if -1.88170328 * a - 0.80936493 * b > 1.0 {
        (
            [1.19086277, 1.76576728, 0.59662641, 0.75515197, 0.56771245],
            [4.0767416621, -3.3077115913, 0.2309699292],
        )
    } else if 1.81444104 * a - 1.19445276 * b > 1.0 {
        (
            [0.73956515, -0.45954404, 0.08285427, 0.12541070, 0.14503204],
            [-1.2684380046, 2.6097574011, -0.3413193965],
        )
    } else {
        (
            [
                1.35733652,
                -0.00915799,
                -1.15130210,
                -0.50559606,
                0.00692167,
            ],
            [-0.0041960863, -0.7034186147, 1.7076147010],
        )
    };
    let mut sat = k[0] + k[1] * a + k[2] * b + k[3] * a * a + k[4] * a * b;

    let k_l = 0.3963377774 * a + 0.2158037573 * b;
    let k_m = -0.1055613458 * a - 0.0638541728 * b;
    let k_s = -0.0894841775 * a - 1.2914855480 * b;
    let l_ = 1.0 + sat * k_l;
    let m_ = 1.0 + sat * k_m;
    let s_ = 1.0 + sat * k_s;
    let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let (l_d, m_d, s_d) = (
        3.0 * k_l * l_ * l_,
        3.0 * k_m * m_ * m_,
        3.0 * k_s * s_ * s_,
    );
    let (l_d2, m_d2, s_d2) = (
        6.0 * k_l * k_l * l_,
        6.0 * k_m * k_m * m_,
        6.0 * k_s * k_s * s_,
    );
    let f = w[0] * l + w[1] * m + w[2] * s;
    let f1 = w[0] * l_d + w[1] * m_d + w[2] * s_d;
    let f2 = w[0] * l_d2 + w[1] * m_d2 + w[2] * s_d2;
    sat -= f * f1 / (f1 * f1 - 0.5 * f * f2);
    sat
}

fn find_cusp(a: f64, b: f64) -> Lc {
    let s_cusp = compute_max_saturation(a, b);
    let rgb = oklab_to_linear_srgb(1.0, s_cusp * a, s_cusp * b);
    let l_cusp = (1.0 / rgb[0].max(rgb[1]).max(rgb[2])).cbrt();
    Lc {
        l: l_cusp,
        c: l_cusp * s_cusp,
    }
}

/// Intersection of the line from (L0, 0) towards (L1, C1) with the sRGB gamut.
fn find_gamut_intersection(a: f64, b: f64, l1: f64, c1: f64, l0: f64, cusp: Lc) -> f64 {
    if (l1 - l0) * cusp.c - (cusp.l - l0) * c1 <= 0.0 {
        return cusp.c * l0 / (c1 * cusp.l + cusp.c * (l0 - l1));
    }
    let mut t = cusp.c * (l0 - 1.0) / (c1 * (cusp.l - 1.0) + cusp.c * (l0 - l1));

    let d_l = l1 - l0;
    let d_c = c1;
    let k_l = 0.3963377774 * a + 0.2158037573 * b;
    let k_m = -0.1055613458 * a - 0.0638541728 * b;
    let k_s = -0.0894841775 * a - 1.2914855480 * b;
    let l_dt = d_l + d_c * k_l;
    let m_dt = d_l + d_c * k_m;
    let s_dt = d_l + d_c * k_s;

    let l = l0 * (1.0 - t) + t * l1;
    let c = t * c1;
    let l_ = l + c * k_l;
    let m_ = l + c * k_m;
    let s_ = l + c * k_s;
    let (lc, mc, sc) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let (ldt, mdt, sdt) = (
        3.0 * l_dt * l_ * l_,
        3.0 * m_dt * m_ * m_,
        3.0 * s_dt * s_ * s_,
    );
    let (ldt2, mdt2, sdt2) = (
        6.0 * l_dt * l_dt * l_,
        6.0 * m_dt * m_dt * m_,
        6.0 * s_dt * s_dt * s_,
    );

    let channel = |w: [f64; 3]| {
        let v = w[0] * lc + w[1] * mc + w[2] * sc - 1.0;
        let v1 = w[0] * ldt + w[1] * mdt + w[2] * sdt;
        let v2 = w[0] * ldt2 + w[1] * mdt2 + w[2] * sdt2;
        let u = v1 / (v1 * v1 - 0.5 * v * v2);
        if u >= 0.0 { -v * u } else { f64::MAX }
    };
    let t_r = channel([4.0767416621, -3.3077115913, 0.2309699292]);
    let t_g = channel([-1.2684380046, 2.6097574011, -0.3413193965]);
    let t_b = channel([-0.0041960863, -0.7034186147, 1.7076147010]);
    t += t_r.min(t_g.min(t_b));
    t
}

/// (S, T) at the cusp, used to approximate the gamut triangle.
fn get_st_max(cusp: Lc) -> (f64, f64) {
    (cusp.c / cusp.l, cusp.c / (1.0 - cusp.l))
}

/// (S, T) of the smooth mid-chroma curve.
fn get_st_mid(a: f64, b: f64) -> (f64, f64) {
    let s = 0.11516993
        + 1.0
            / (7.44778970
                + 4.15901240 * b
                + a * (-2.19557347
                    + 1.75198401 * b
                    + a * (-2.13704948 - 10.02301043 * b
                        + a * (-4.24894561 + 5.38770819 * b + 4.69891013 * a))));
    let t = 0.11239642
        + 1.0
            / (1.61320320 - 0.68124379 * b
                + a * (0.40370612
                    + 0.90148123 * b
                    + a * (-0.27087943
                        + 0.61223990 * b
                        + a * (0.00299215 - 0.45399568 * b - 0.14661872 * a))));
    (s, t)
}

/// (C_0, C_mid, C_max) for lightness `l` and hue direction (a_, b_).
fn get_cs(l: f64, a_: f64, b_: f64) -> (f64, f64, f64) {
    let cusp = find_cusp(a_, b_);
    let c_max = find_gamut_intersection(a_, b_, l, 1.0, l, cusp);
    let (s_max, t_max) = get_st_max(cusp);
    let k = c_max / (l * s_max).min((1.0 - l) * t_max);

    let (s_mid, t_mid) = get_st_mid(a_, b_);
    let c_a = l * s_mid;
    let c_b = (1.0 - l) * t_mid;
    let c_mid = 0.9
        * k
        * (1.0 / (1.0 / c_a.powi(4) + 1.0 / c_b.powi(4)))
            .sqrt()
            .sqrt();

    let c_a = l * 0.4;
    let c_b = (1.0 - l) * 0.8;
    let c_0 = (1.0 / (1.0 / (c_a * c_a) + 1.0 / (c_b * c_b))).sqrt();
    (c_0, c_mid, c_max)
}

const MID: f64 = 0.8;
const MID_INV: f64 = 1.25;

impl Color {
    /// From `[h°, s, l]` with s, l in 0..1.
    pub fn from_okhsl([h, s, l]: [f64; 3]) -> Self {
        if l >= 1.0 {
            return Color::from_oklab(1.0, 0.0, 0.0);
        }
        if l <= 0.0 {
            return Color::from_oklab(0.0, 0.0, 0.0);
        }
        let hr = h.rem_euclid(360.0) / 360.0 * 2.0 * PI;
        let (a_, b_) = (hr.cos(), hr.sin());
        let big_l = toe_inv(l);
        let (c_0, c_mid, c_max) = get_cs(big_l, a_, b_);

        let c = if s < MID {
            let t = MID_INV * s;
            let k_1 = MID * c_0;
            let k_2 = 1.0 - k_1 / c_mid;
            t * k_1 / (1.0 - k_2 * t)
        } else {
            let t = (s - MID) / (1.0 - MID);
            let k_1 = (1.0 - MID) * c_mid * c_mid * MID_INV * MID_INV / c_0;
            let k_2 = 1.0 - k_1 / (c_max - c_mid);
            c_mid + t * k_1 / (1.0 - k_2 * t)
        };
        Color::from_oklab(big_l, c * a_, c * b_)
    }

    /// `[h°, s, l]` with h in `[0, 360)` and s, l in 0..1 (sRGB gamut).
    pub fn to_okhsl(&self) -> [f64; 3] {
        let c = self.a.hypot(self.b);
        let l = toe(self.l);
        if c < 1e-9 || self.l <= 0.0 || self.l >= 1.0 {
            return [0.0, 0.0, l.clamp(0.0, 1.0)];
        }
        let (a_, b_) = (self.a / c, self.b / c);
        let h = 0.5 + 0.5 * (-self.b).atan2(-self.a) / PI;
        let (c_0, c_mid, c_max) = get_cs(self.l, a_, b_);

        let s = if c < c_mid {
            let k_1 = MID * c_0;
            let k_2 = 1.0 - k_1 / c_mid;
            let t = c / (k_1 + k_2 * c);
            t * MID
        } else {
            let k_1 = 0.2 * c_mid * c_mid * MID_INV * MID_INV / c_0;
            let k_2 = 1.0 - k_1 / (c_max - c_mid);
            let cc = c - c_mid;
            let t = cc / (k_1 + k_2 * cc);
            MID + 0.2 * t
        };
        let h = h * 360.0;
        [if h >= 360.0 { 0.0 } else { h }, s, l]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn lcg_colors(n: usize) -> Vec<[u8; 3]> {
        let mut s: u64 = 0x2545F4914F6CDD1D;
        (0..n)
            .map(|_| {
                let mut ch = [0u8; 3];
                for c in &mut ch {
                    s = s
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    *c = (s >> 56) as u8;
                }
                ch
            })
            .collect()
    }

    const REFS: [[u8; 3]; 4] = [
        [0x34, 0x78, 0xF6],
        [0xF6, 0xB2, 0x34],
        [0x2E, 0xC2, 0x7E],
        [0x80, 0x10, 0xC0],
    ];

    #[test]
    fn okhsl_round_trip() {
        for rgb in REFS.into_iter().chain(lcg_colors(200)) {
            let c = Color::from_srgb8(rgb);
            let back = Color::from_okhsl(c.to_okhsl());
            assert_eq!(back.to_srgb8(), c.to_srgb8(), "{rgb:?}");
        }
    }

    #[test]
    fn okhsl_endpoints() {
        let red = Color::from_srgb8([255, 0, 0]).to_okhsl();
        let blue = Color::from_srgb8([0, 0, 255]).to_okhsl();
        assert!((red[1] - 1.0).abs() < 1e-3, "{red:?}");
        assert!((blue[1] - 1.0).abs() < 1e-3, "{blue:?}");
        let gray = Color::from_srgb8([128, 128, 128]).to_okhsl();
        assert!(gray[1].abs() < 1e-6, "{gray:?}");
        assert_eq!(Color::from_okhsl([200.0, 0.7, 0.0]).to_srgb8(), [0, 0, 0]);
        assert_eq!(
            Color::from_okhsl([200.0, 0.7, 1.0]).to_srgb8(),
            [255, 255, 255]
        );
    }
}
