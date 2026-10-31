//! The channel model of the Wheel tab, GTK-free. Channels are in slider
//! units: Ring `[H°, S %, V %]`, OKLCH `[L, C, H°]`, OKHSL `[H°, S %, L %]`.

use colorz_core::color::Color;
use colorz_core::wheel::OKLCH_MAX_CHROMA;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WheelMode {
    #[default]
    Ring,
    Oklch,
    Okhsl,
}

impl WheelMode {
    pub const ALL: [WheelMode; 3] = [WheelMode::Ring, WheelMode::Oklch, WheelMode::Okhsl];

    /// Index of the hue channel.
    pub fn hue_index(self) -> usize {
        match self {
            WheelMode::Oklch => 2,
            WheelMode::Ring | WheelMode::Okhsl => 0,
        }
    }
}

/// Range and precision of one channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelSpec {
    pub name: &'static str,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub digits: u32,
}

const fn spec(name: &'static str, max: f64, step: f64, digits: u32) -> ChannelSpec {
    ChannelSpec {
        name,
        min: 0.0,
        max,
        step,
        digits,
    }
}

pub fn channel_specs(mode: WheelMode) -> [ChannelSpec; 3] {
    match mode {
        WheelMode::Ring => [
            spec("H", 360.0, 1.0, 0),
            spec("S", 100.0, 1.0, 0),
            spec("V", 100.0, 1.0, 0),
        ],
        WheelMode::Oklch => [
            spec("L", 1.0, 0.001, 3),
            spec("C", OKLCH_MAX_CHROMA, 0.001, 3),
            spec("H", 360.0, 0.1, 1),
        ],
        WheelMode::Okhsl => [
            spec("H", 360.0, 1.0, 0),
            spec("S", 100.0, 1.0, 1),
            spec("L", 100.0, 1.0, 1),
        ],
    }
}

/// Below this, saturation or chroma has no meaningful hue.
const NO_HUE: f64 = 1e-6;

/// The channels of `c` in `mode`. Where `c` has no hue (grays) or no
/// saturation (black), those channels are taken from `prev`. OKLCH keeps
/// the true, possibly out-of-sRGB values; the bounded models are clamped.
pub fn channels_from_color(mode: WheelMode, c: &Color, prev: [f64; 3]) -> [f64; 3] {
    match mode {
        WheelMode::Ring => {
            let [h, s, v] = c.to_hsv();
            if v < NO_HUE {
                [prev[0], prev[1], 0.0]
            } else if s < NO_HUE {
                [prev[0], 0.0, v * 100.0]
            } else {
                [h, s * 100.0, v * 100.0]
            }
        }
        WheelMode::Oklch => {
            let [l, ch, h] = c.to_oklch();
            [l, ch, if ch < NO_HUE { prev[2] } else { h }]
        }
        WheelMode::Okhsl => {
            let [h, s, l] = c.to_okhsl();
            let (s, l) = (s.clamp(0.0, 1.0) * 100.0, l.clamp(0.0, 1.0) * 100.0);
            if l < NO_HUE || l > 100.0 - NO_HUE {
                [prev[0], prev[1], l]
            } else if s < NO_HUE {
                [prev[0], 0.0, l]
            } else {
                [h, s, l]
            }
        }
    }
}

pub fn color_from_channels(mode: WheelMode, ch: [f64; 3]) -> Color {
    match mode {
        WheelMode::Ring => Color::from_hsv([ch[0], ch[1] / 100.0, ch[2] / 100.0]),
        WheelMode::Oklch => Color::from_oklch(ch),
        WheelMode::Okhsl => Color::from_okhsl([ch[0], ch[1] / 100.0, ch[2] / 100.0]),
    }
}

pub fn wrap_hue(h: f64) -> f64 {
    let h = h.rem_euclid(360.0);
    if h >= 360.0 { 0.0 } else { h }
}

/// `v + d`, kept within `min..=max` unless `v` already lies outside in the
/// direction of travel (out-of-sRGB chroma is not cut by a nudge).
pub fn step_clamped(v: f64, d: f64, min: f64, max: f64) -> f64 {
    if d > 0.0 {
        (v + d).min(max.max(v))
    } else {
        (v + d).max(min.min(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_round_trip() {
        let blue = Color::from_srgb8([0x34, 0x78, 0xF6]);
        for mode in WheelMode::ALL {
            let ch = channels_from_color(mode, &blue, [0.0; 3]);
            assert_eq!(
                color_from_channels(mode, ch).to_srgb8(),
                [0x34, 0x78, 0xF6],
                "{mode:?} {ch:?}"
            );
        }
        // Gray has no hue: the previous one is kept, the color is unchanged.
        for gray in [[128u8; 3], [0; 3], [255; 3]] {
            let c = Color::from_srgb8(gray);
            for mode in WheelMode::ALL {
                let hue = mode.hue_index();
                let mut prev = [0.0; 3];
                prev[hue] = 123.0;
                let ch = channels_from_color(mode, &c, prev);
                assert_eq!(ch[hue], 123.0, "{mode:?} {gray:?}");
                assert_eq!(color_from_channels(mode, ch).to_srgb8(), gray, "{mode:?}");
            }
        }
        // Black has no saturation either: Ring keeps the previous S.
        let ch = channels_from_color(
            WheelMode::Ring,
            &Color::from_srgb8([0; 3]),
            [10.0, 45.0, 80.0],
        );
        assert_eq!(ch, [10.0, 45.0, 0.0]);
    }

    #[test]
    fn oklch_keeps_wide_gamut_and_okhsl_clamps() {
        let wide = Color::from_oklch([0.7, 0.3, 30.0]);
        let ch = channels_from_color(WheelMode::Oklch, &wide, [0.0; 3]);
        assert!(
            (ch[1] - 0.3).abs() < 1e-9 && (ch[2] - 30.0).abs() < 1e-9,
            "{ch:?}"
        );
        let ch = channels_from_color(WheelMode::Okhsl, &wide, [0.0; 3]);
        assert_eq!(ch[1], 100.0);
    }

    #[test]
    fn nudges_wrap_and_clamp() {
        assert_eq!(wrap_hue(-1.0), 359.0);
        assert_eq!(wrap_hue(360.0), 0.0);
        assert_eq!(step_clamped(99.5, 1.0, 0.0, 100.0), 100.0);
        assert_eq!(step_clamped(0.5, -1.0, 0.0, 100.0), 0.0);
        // Already past the end: moving outwards does not cut it back.
        assert_eq!(step_clamped(0.45, 0.001, 0.0, 0.37), 0.45);
        assert!((step_clamped(0.45, -0.01, 0.0, 0.37) - 0.44).abs() < 1e-12);
    }
}
