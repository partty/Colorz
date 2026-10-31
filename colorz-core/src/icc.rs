//! ICC profile support: custom "copy as" target spaces and image-pixel
//! ICC to sRGB conversion, via lcms2.

use std::path::Path;

use lcms2::{
    ColorSpaceSignature, Flags, InfoType, Intent, Locale, PixelFormat, Profile, Transform,
};

use crate::color::{Color, RgbSpace, gamut_map_with};
use crate::target::TargetSpace;

#[derive(thiserror::Error, Debug)]
pub enum IccError {
    #[error("lcms2: {0}")]
    Lcms(String),
    #[error("not an RGB profile")]
    NotRgb,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<lcms2::Error> for IccError {
    fn from(e: lcms2::Error) -> Self {
        IccError::Lcms(e.to_string())
    }
}

type RgbTransform = Transform<[f64; 3], [f64; 3]>;

/// A custom ICC RGB profile used as a "copy as" target. Relative
/// colorimetric, no black point compensation, unbounded float transforms.
pub struct IccTarget {
    name: String,
    to_profile: RgbTransform,
    to_srgb: RgbTransform,
}

fn new_transform(from: &Profile, to: &Profile) -> Result<RgbTransform, IccError> {
    Ok(Transform::new_flags(
        from,
        PixelFormat::RGB_DBL,
        to,
        PixelFormat::RGB_DBL,
        Intent::RelativeColorimetric,
        Flags::default(),
    )?)
}

impl IccTarget {
    pub fn load(path: &Path) -> Result<IccTarget, IccError> {
        let bytes = std::fs::read(path)?;
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self::from_bytes(&bytes, &stem)
    }

    pub fn from_bytes(bytes: &[u8], fallback_name: &str) -> Result<IccTarget, IccError> {
        let profile = Profile::new_icc(bytes)?;
        if profile.color_space() != ColorSpaceSignature::RgbData {
            return Err(IccError::NotRgb);
        }
        let name = profile
            .info(InfoType::Description, Locale::none())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| fallback_name.to_string());
        let srgb = Profile::new_srgb();
        Ok(IccTarget {
            name,
            to_profile: new_transform(&srgb, &profile)?,
            to_srgb: new_transform(&profile, &srgb)?,
        })
    }

    fn forward(&self, srgb: [f64; 3]) -> [f64; 3] {
        let mut out = [[0.0; 3]];
        self.to_profile.transform_pixels(&[srgb], &mut out);
        out[0]
    }

    fn backward(&self, rgb: [f64; 3]) -> [f64; 3] {
        let mut out = [[0.0; 3]];
        self.to_srgb.transform_pixels(&[rgb], &mut out);
        out[0]
    }
}

impl TargetSpace for IccTarget {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn css_name(&self) -> Option<&'static str> {
        None
    }

    fn is_srgb(&self) -> bool {
        false
    }

    fn encode(&self, c: &Color) -> ([f64; 3], bool) {
        let rgb = self.forward(c.to_rgb(RgbSpace::Srgb));
        const EPS: f64 = 1e-4;
        if rgb.iter().all(|v| (-EPS..=1.0 + EPS).contains(v)) {
            return (rgb.map(|v| v.clamp(0.0, 1.0)), false);
        }
        let mapped = gamut_map_with(
            c,
            |x| self.forward(x.to_rgb(RgbSpace::Srgb)),
            |rgb| Color::from_rgb(RgbSpace::Srgb, self.backward(rgb)),
        );
        (mapped, true)
    }

    fn decode(&self, rgb: [f64; 3]) -> Color {
        Color::from_rgb(RgbSpace::Srgb, self.backward(rgb))
    }
}

/// Converts 8-bit pixels from the embedded ICC profile `icc` to sRGB in place.
pub fn icc_rgb8_to_srgb(icc: &[u8], pixels: &mut [[u8; 3]]) -> Result<(), IccError> {
    let profile = Profile::new_icc(icc)?;
    if profile.color_space() != ColorSpaceSignature::RgbData {
        return Err(IccError::NotRgb);
    }
    let t: Transform<[u8; 3], [u8; 3]> = Transform::new(
        &profile,
        PixelFormat::RGB_8,
        &Profile::new_srgb(),
        PixelFormat::RGB_8,
        Intent::RelativeColorimetric,
    )?;
    t.transform_in_place(pixels);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::BuiltinTarget;
    use lcms2::{CIExyY, CIExyYTRIPLE, ToneCurve};

    fn rgb_profile(primaries: [(f64, f64); 3], gamma: f64) -> Profile {
        let wp = CIExyY {
            x: 0.3127,
            y: 0.3290,
            Y: 1.0,
        };
        let p = CIExyYTRIPLE {
            Red: CIExyY {
                x: primaries[0].0,
                y: primaries[0].1,
                Y: 1.0,
            },
            Green: CIExyY {
                x: primaries[1].0,
                y: primaries[1].1,
                Y: 1.0,
            },
            Blue: CIExyY {
                x: primaries[2].0,
                y: primaries[2].1,
                Y: 1.0,
            },
        };
        let curve = ToneCurve::new(gamma);
        Profile::new_rgb(&wp, &p, &[&curve, &curve, &curve]).unwrap()
    }

    fn adobe_bytes() -> Vec<u8> {
        rgb_profile([(0.64, 0.33), (0.21, 0.71), (0.15, 0.06)], 563.0 / 256.0)
            .icc()
            .unwrap()
    }

    fn to8(rgb: [f64; 3]) -> [u8; 3] {
        rgb.map(|v| (v * 255.0).round() as u8)
    }

    const REFS: [[u8; 3]; 4] = [
        [0x34, 0x78, 0xF6],
        [0xF6, 0xB2, 0x34],
        [0x2E, 0xC2, 0x7E],
        [255, 0, 0],
    ];

    #[test]
    fn adobe_icc_matches_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("adobe.icc");
        std::fs::write(&path, adobe_bytes()).unwrap();
        let t = IccTarget::load(&path).unwrap();
        let b = BuiltinTarget(RgbSpace::A98Rgb);
        let c = Color::from_srgb8([0x34, 0x78, 0xF6]);
        let (rgb, out) = t.encode(&c);
        assert!(!out);
        let (want, _) = b.encode(&c);
        for i in 0..3 {
            assert!((rgb[i] - want[i]).abs() < 1e-3, "{rgb:?} vs {want:?}");
        }
        assert_eq!(to8(rgb), [80, 119, 242]);
        let (red, _) = t.encode(&Color::from_srgb8([255, 0, 0]));
        // A profile loaded from bytes has s15Fixed16 matrix rounding, so the
        // near-zero channel can land one 8-bit step off ([219, 1, 0]).
        for (v, w) in to8(red).iter().zip([219i32, 0, 0]) {
            assert!((*v as i32 - w).abs() <= 1, "{red:?}");
        }
        assert!(!t.is_srgb());
        assert_eq!(t.css_name(), None);
        assert!(!t.name().is_empty());
    }

    #[test]
    fn icc_round_trip() {
        let t = IccTarget::from_bytes(&adobe_bytes(), "adobe").unwrap();
        for x in REFS {
            let c = Color::from_srgb8(x);
            assert_eq!(t.decode(t.encode(&c).0).to_srgb8(), c.to_srgb8());
        }
    }

    #[test]
    fn srgb_icc_is_identity() {
        let bytes = Profile::new_srgb().icc().unwrap();
        let t = IccTarget::from_bytes(&bytes, "srgb").unwrap();
        let (rgb, out) = t.encode(&Color::from_srgb8([0x34, 0x78, 0xF6]));
        assert!(!out);
        for (v, w) in rgb.iter().zip([52.0, 120.0, 246.0]) {
            assert!((v - w / 255.0).abs() < 1e-4, "{rgb:?}");
        }
    }

    #[test]
    fn oog_into_smaller_space() {
        let bytes = Profile::new_srgb().icc().unwrap();
        let t = IccTarget::from_bytes(&bytes, "srgb").unwrap();
        let c = Color::from_rgb(RgbSpace::DisplayP3, [0.0, 1.0, 0.0]);
        let (rgb, out) = t.encode(&c);
        assert!(out);
        assert!(rgb.iter().all(|v| (0.0..=1.0).contains(v)), "{rgb:?}");
    }

    #[test]
    fn rejects_gray_profile() {
        let wp = CIExyY {
            x: 0.3127,
            y: 0.3290,
            Y: 1.0,
        };
        let bytes = Profile::new_gray(&wp, &ToneCurve::new(2.2))
            .unwrap()
            .icc()
            .unwrap();
        assert!(matches!(
            IccTarget::from_bytes(&bytes, "g"),
            Err(IccError::NotRgb)
        ));
    }

    #[test]
    fn image_pixels_converted() {
        let mut px = [[80u8, 119, 242]];
        icc_rgb8_to_srgb(&adobe_bytes(), &mut px).unwrap();
        for (v, w) in px[0].iter().zip([52i32, 120, 246]) {
            assert!((*v as i32 - w).abs() <= 1, "{px:?}");
        }
    }
}
