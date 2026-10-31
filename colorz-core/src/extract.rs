//! Image loading (ICC-aware) and deterministic k-means palette extraction in OKLab.

use std::collections::BTreeMap;
use std::io::Cursor;

use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader, Limits};

use crate::color::Color;
use crate::icc::icc_rgb8_to_srgb;

const MAX_SIDE: u32 = 256;
const MAX_ITERATIONS: usize = 50;
const SEED: u64 = 0x9E37_79B9_7F4A_7C15;
const GRAY_CHROMA: f64 = 0.02;

#[derive(thiserror::Error, Debug)]
pub enum ExtractError {
    #[error("could not decode image: {0}")]
    Decode(String),
    #[error("image has no usable pixels")]
    Empty,
}

/// A downscaled sRGB sample of an image (long side <= 256), composited over white.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[u8; 3]>,
    /// Same length as `pixels`; true when the source alpha was >= 128.
    pub opaque: Vec<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortBy {
    Amount,
    Lightness,
    Hue,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExtractedColor {
    pub color: Color,
    /// Fraction of the sample, 0..1.
    pub weight: f64,
    /// Normalized (x, y) of a sample pixel in the color nearest the cluster
    /// center: the one nearest the centroid of the cluster's pixels.
    pub position: (f64, f64),
}

/// Largest accepted image side; the allocation limit is the `image` default.
const MAX_DECODE_SIDE: u32 = 20_000;

/// Decodes `bytes` with the EXIF orientation applied; also returns the
/// embedded ICC profile.
fn decode_oriented(bytes: &[u8]) -> Result<(DynamicImage, Option<Vec<u8>>), ExtractError> {
    let decode_err = |e: &dyn std::fmt::Display| ExtractError::Decode(e.to_string());
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| decode_err(&e))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|e| decode_err(&e))?;
    let icc = decoder.icc_profile().ok().flatten();
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| decode_err(&e))?;
    img.apply_orientation(orientation);
    if img.width() == 0 || img.height() == 0 {
        return Err(ExtractError::Empty);
    }
    Ok((img, icc))
}

/// An oriented RGBA8 rendering of an image for display, converted to sRGB
/// from the embedded ICC profile and downscaled to at most `max_side`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, straight (non-premultiplied) alpha.
    pub rgba: Vec<u8>,
}

fn fit(img: &DynamicImage, max_side: u32) -> std::borrow::Cow<'_, DynamicImage> {
    if img.width().max(img.height()) > max_side {
        std::borrow::Cow::Owned(img.resize(max_side, max_side, FilterType::Triangle))
    } else {
        std::borrow::Cow::Borrowed(img)
    }
}

fn preview_of(img: &DynamicImage, icc: Option<&[u8]>, max_side: u32) -> Preview {
    let mut rgba = fit(img, max_side).to_rgba8();
    let (width, height) = rgba.dimensions();
    if let Some(icc) = icc {
        let mut px: Vec<[u8; 3]> = rgba.pixels().map(|p| [p[0], p[1], p[2]]).collect();
        if icc_rgb8_to_srgb(icc, &mut px).is_ok() {
            for (dst, src) in rgba.pixels_mut().zip(px) {
                dst.0[..3].copy_from_slice(&src);
            }
        }
    }
    Preview {
        width,
        height,
        rgba: rgba.into_raw(),
    }
}

fn sample_of(img: &DynamicImage, icc: Option<&[u8]>) -> SampleImage {
    let rgba = fit(img, MAX_SIDE).to_rgba8();
    let (width, height) = rgba.dimensions();

    let mut pixels: Vec<[u8; 3]> = rgba.pixels().map(|p| [p[0], p[1], p[2]]).collect();
    if let Some(icc) = icc {
        let mut converted = pixels.clone();
        if icc_rgb8_to_srgb(icc, &mut converted).is_ok() {
            pixels = converted;
        }
    }
    let opaque: Vec<bool> = rgba.pixels().map(|p| p[3] >= 128).collect();
    // Composite over white.
    for (px, src) in pixels.iter_mut().zip(rgba.pixels()) {
        let a = u32::from(src[3]);
        if a < 255 {
            for c in px.iter_mut() {
                *c = ((u32::from(*c) * a + 255 * (255 - a) + 127) / 255) as u8;
            }
        }
    }
    SampleImage {
        width,
        height,
        pixels,
        opaque,
    }
}

/// Decodes `bytes` for display; see [`Preview`].
pub fn load_preview(bytes: &[u8], max_side: u32) -> Result<Preview, ExtractError> {
    let (img, icc) = decode_oriented(bytes)?;
    Ok(preview_of(&img, icc.as_deref(), max_side))
}

/// Decodes `bytes`, converts from the embedded ICC profile to sRGB (if any),
/// and downscales so the long side is at most 256.
pub fn load_image(bytes: &[u8]) -> Result<SampleImage, ExtractError> {
    let (img, icc) = decode_oriented(bytes)?;
    Ok(sample_of(&img, icc.as_deref()))
}

/// [`load_image`] and [`load_preview`] from a single decode.
pub fn load_image_and_preview(
    bytes: &[u8],
    preview_max: u32,
) -> Result<(SampleImage, Preview), ExtractError> {
    let (img, icc) = decode_oriented(bytes)?;
    Ok((
        sample_of(&img, icc.as_deref()),
        preview_of(&img, icc.as_deref(), preview_max),
    ))
}

/// Builds the sample and preview from raw sRGB RGBA8 pixels (straight
/// alpha), e.g. a downloaded texture.
pub fn from_rgba(
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    preview_max: u32,
) -> Result<(SampleImage, Preview), ExtractError> {
    if width == 0 || height == 0 {
        return Err(ExtractError::Empty);
    }
    let buf = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| ExtractError::Decode("pixel buffer has the wrong size".into()))?;
    let img = DynamicImage::ImageRgba8(buf);
    Ok((sample_of(&img, None), preview_of(&img, None, preview_max)))
}

struct XorShift64(u64);

impl XorShift64 {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in [0, 1).
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn dist2(p: &[f64; 3], q: &[f64; 3]) -> f64 {
    let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// Picks an index with probability proportional to `w`.
fn pick_weighted(w: &[f64], rng: &mut XorShift64) -> usize {
    let total: f64 = w.iter().sum();
    if total <= 0.0 {
        return 0;
    }
    let target = rng.next_f64() * total;
    let mut acc = 0.0;
    for (i, x) in w.iter().enumerate() {
        acc += x;
        if target < acc && *x > 0.0 {
            return i;
        }
    }
    w.iter().rposition(|x| *x > 0.0).unwrap_or(0)
}

/// Extracts up to `k` (clamped to 3..=16 and to the distinct color count) dominant colors.
pub fn extract(img: &SampleImage, k: usize, sort: SortBy) -> Vec<ExtractedColor> {
    // Distinct colors with counts (and, below, their index), in sorted order.
    let mut distinct: BTreeMap<[u8; 3], (usize, usize)> = BTreeMap::new();
    let use_all = !img.opaque.iter().any(|o| *o) || img.opaque.len() != img.pixels.len();
    let used = |i: usize| use_all || img.opaque[i];
    let mut total = 0.0;
    for (i, px) in img.pixels.iter().enumerate() {
        if used(i) {
            total += 1.0;
            distinct.entry(*px).or_insert((0, 0)).0 += 1;
        }
    }
    if distinct.is_empty() {
        return Vec::new();
    }
    let n = distinct.len();
    let mut points = Vec::with_capacity(n);
    let mut counts = Vec::with_capacity(n);
    for (i, (rgb, (count, index))) in distinct.iter_mut().enumerate() {
        let c = Color::from_srgb8(*rgb);
        points.push([c.l, c.a, c.b]);
        counts.push(*count as f64);
        *index = i;
    }
    // The distinct color of each used pixel.
    let pixel_color: Vec<Option<usize>> = img
        .pixels
        .iter()
        .enumerate()
        .map(|(i, px)| used(i).then(|| distinct[px].1))
        .collect();

    let k = k.clamp(3, 16).min(n);
    let mut rng = XorShift64(SEED);

    // k-means++ seeding.
    let mut centers: Vec<[f64; 3]> = Vec::with_capacity(k);
    centers.push(points[pick_weighted(&counts, &mut rng)]);
    let mut min_d: Vec<f64> = points.iter().map(|p| dist2(p, &centers[0])).collect();
    while centers.len() < k {
        let w: Vec<f64> = min_d.iter().zip(&counts).map(|(d, c)| d * c).collect();
        let c = points[pick_weighted(&w, &mut rng)];
        for (d, p) in min_d.iter_mut().zip(&points) {
            *d = d.min(dist2(p, &c));
        }
        centers.push(c);
    }

    // Lloyd iterations.
    let mut assign = vec![usize::MAX; n];
    for _ in 0..MAX_ITERATIONS {
        let mut changed = false;
        for (a, p) in assign.iter_mut().zip(&points) {
            let mut best = 0;
            let mut best_d = f64::INFINITY;
            for (ci, c) in centers.iter().enumerate() {
                let d = dist2(p, c);
                if d < best_d {
                    best_d = d;
                    best = ci;
                }
            }
            if *a != best {
                *a = best;
                changed = true;
            }
        }
        if !changed {
            break;
        }
        let means = cluster_means(&points, &counts, &assign, centers.len());
        for (c, m) in centers.iter_mut().zip(means) {
            if let Some((mean, _)) = m {
                *c = mean;
            }
        }
    }

    let means = cluster_means(&points, &counts, &assign, centers.len());
    let width = img.width.max(1) as usize;
    let xy = |i: usize| ((i % width) as f64, (i / width) as f64);
    // Spatial centroid of each cluster's pixels.
    let mut sums = vec![(0.0f64, 0.0f64, 0.0f64); centers.len()];
    for (i, d) in pixel_color.iter().enumerate() {
        if let Some(d) = d {
            let (x, y) = xy(i);
            let s = &mut sums[assign[*d]];
            *s = (s.0 + x, s.1 + y, s.2 + 1.0);
        }
    }
    let (w, h) = (f64::from(img.width.max(1)), f64::from(img.height.max(1)));
    let mut out: Vec<ExtractedColor> = Vec::new();
    for (ci, m) in means.into_iter().enumerate() {
        let Some((mean, weight)) = m else { continue };
        // The distinct color nearest the center (ties: the lowest index)...
        let mut best = usize::MAX;
        let mut best_d = f64::INFINITY;
        for i in 0..n {
            if assign[i] == ci {
                let d = dist2(&points[i], &mean);
                if d < best_d {
                    best_d = d;
                    best = i;
                }
            }
        }
        // ...and its pixel nearest the cluster's centroid (ties: the first),
        // so the dot sits inside a region rather than on its top edge.
        let (sx, sy, sn) = sums[ci];
        let (cx, cy) = (sx / sn, sy / sn);
        let mut idx = 0;
        let mut idx_d = f64::INFINITY;
        for (i, d) in pixel_color.iter().enumerate() {
            if *d == Some(best) {
                let (x, y) = xy(i);
                let d = (x - cx).powi(2) + (y - cy).powi(2);
                if d < idx_d {
                    idx_d = d;
                    idx = i;
                }
            }
        }
        let (x, y) = xy(idx);
        out.push(ExtractedColor {
            color: Color::from_oklab(mean[0], mean[1], mean[2]),
            weight: weight / total,
            position: ((x + 0.5) / w, (y + 0.5) / h),
        });
    }

    match sort {
        SortBy::Amount => out.sort_by(|a, b| b.weight.total_cmp(&a.weight)),
        SortBy::Lightness => out.sort_by(|a, b| b.color.l.total_cmp(&a.color.l)),
        SortBy::Hue => out.sort_by(|a, b| {
            let [_, ca, ha] = a.color.to_oklch();
            let [_, cb, hb] = b.color.to_oklch();
            (ca < GRAY_CHROMA)
                .cmp(&(cb < GRAY_CHROMA))
                .then_with(|| ha.total_cmp(&hb))
        }),
    }
    out
}

/// Weighted mean and total count per non-empty cluster.
#[allow(clippy::type_complexity)]
fn cluster_means(
    points: &[[f64; 3]],
    counts: &[f64],
    assign: &[usize],
    k: usize,
) -> Vec<Option<([f64; 3], f64)>> {
    let mut sums = vec![([0.0f64; 3], 0.0f64); k];
    for ((p, c), a) in points.iter().zip(counts).zip(assign) {
        let s = &mut sums[*a];
        for (acc, v) in s.0.iter_mut().zip(p) {
            *acc += v * c;
        }
        s.1 += c;
    }
    sums.into_iter()
        .map(|(s, c)| (c > 0.0).then(|| ([s[0] / c, s[1] / c, s[2] / c], c)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, RgbImage};

    fn png(img: &RgbImage) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    #[test]
    fn preview_downscales_and_keeps_pixels() {
        let img = RgbImage::from_fn(400, 100, |x, _| {
            image::Rgb(if x < 200 { [255, 0, 0] } else { [0, 0, 255] })
        });
        let p = load_preview(&png(&img), 200).unwrap();
        assert_eq!((p.width, p.height), (200, 50));
        assert_eq!(p.rgba.len(), 200 * 50 * 4);
        assert_eq!(&p.rgba[..4], &[255, 0, 0, 255]);
        let last = p.rgba.len() - 4;
        assert_eq!(&p.rgba[last..], &[0, 0, 255, 255]);
        let small = load_preview(&png(&img), 1024).unwrap();
        assert_eq!((small.width, small.height), (400, 100));
    }

    #[test]
    fn preview_applies_exif_orientation() {
        // Orientation 6 = rotate 90 degrees clockwise: a 4x2 image becomes 2x4.
        let img = RgbImage::from_fn(4, 2, |x, _| image::Rgb([x as u8 * 60, 0, 0]));
        let mut jpg = Cursor::new(Vec::new());
        img.write_to(&mut jpg, ImageFormat::Jpeg).unwrap();
        let mut bytes = jpg.into_inner();
        // APP1 EXIF segment with orientation 6, right after SOI.
        let exif: &[u8] = &[
            0xFF, 0xE1, 0x00, 0x22, b'E', b'x', b'i', b'f', 0, 0, b'M', b'M', 0, 0x2A, 0, 0, 0, 8,
            0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0,
        ];
        bytes.splice(2..2, exif.iter().copied());
        let p = load_preview(&bytes, 1024).unwrap();
        assert_eq!((p.width, p.height), (2, 4));
        let s = load_image(&bytes).unwrap();
        assert_eq!((s.width, s.height), (2, 4));
    }

    #[test]
    fn combined_matches_separate() {
        let img = RgbImage::from_fn(600, 300, |x, y| {
            image::Rgb([(x / 3) as u8, (y / 2) as u8, 90])
        });
        let bytes = png(&img);
        let (s, p) = load_image_and_preview(&bytes, 400).unwrap();
        assert_eq!(s, load_image(&bytes).unwrap());
        assert_eq!(p, load_preview(&bytes, 400).unwrap());
    }

    #[test]
    fn from_rgba_matches_decoded() {
        let img = image::RgbaImage::from_fn(500, 300, |x, y| {
            image::Rgba([(x / 2) as u8, (y / 2) as u8, 40, 255])
        });
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        let (s, p) = from_rgba(500, 300, img.into_raw(), 400).unwrap();
        let (s2, p2) = load_image_and_preview(&buf.into_inner(), 400).unwrap();
        assert_eq!(s, s2);
        assert_eq!(p, p2);
        assert!(from_rgba(2, 2, vec![0; 3], 10).is_err());
        assert!(from_rgba(0, 2, vec![], 10).is_err());
    }

    #[test]
    fn over_limit_header_rejected() {
        let img = RgbImage::new(MAX_DECODE_SIDE + 1, 1);
        assert!(matches!(
            load_image(&png(&img)),
            Err(ExtractError::Decode(_))
        ));
        assert!(load_preview(&png(&img), 100).is_err());
    }

    #[test]
    fn three_color_image() {
        let img = RgbImage::from_fn(60, 30, |x, _| {
            image::Rgb(if x < 30 {
                [255, 0, 0]
            } else if x < 50 {
                [0, 255, 0]
            } else {
                [0, 0, 255]
            })
        });
        let s = load_image(&png(&img)).unwrap();
        let out = extract(&s, 3, SortBy::Amount);
        let rgb: Vec<_> = out.iter().map(|c| c.color.to_srgb8()).collect();
        assert_eq!(rgb, vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]]);
        let w: Vec<_> = out.iter().map(|c| c.weight).collect();
        assert!((w[0] - 0.5).abs() < 1e-9);
        assert!((w[1] - 1.0 / 3.0).abs() < 1e-9);
        assert!((w[2] - 1.0 / 6.0).abs() < 1e-9);
        assert!(out[0].position.0 < 0.5);
        assert!((0.5..50.0 / 60.0).contains(&out[1].position.0));
        assert!(out[2].position.0 > 50.0 / 60.0);
        assert!(out.iter().all(|c| (0.0..=1.0).contains(&c.position.1)));
        // Dots sit inside their stripes, not on the top edge.
        for c in &out {
            assert!((0.3..=0.7).contains(&c.position.1), "{:?}", c.position);
        }
        let x: Vec<_> = out.iter().map(|c| c.position.0 * 60.0).collect();
        assert!((x[0] - 15.0).abs() <= 1.0, "{x:?}");
        assert!((x[1] - 40.0).abs() <= 1.0, "{x:?}");
        assert!((x[2] - 55.0).abs() <= 1.0, "{x:?}");
    }

    #[test]
    fn dot_is_on_the_region_nearest_its_centroid() {
        // A red ring around a white square: the ring's centroid is in the
        // white area, so the dot is the nearest red pixel, the first one
        // found in raster order among equally near pixels.
        let img = RgbImage::from_fn(30, 30, |x, y| {
            let inside = (5..25).contains(&x) && (5..25).contains(&y);
            image::Rgb(if inside { [255; 3] } else { [255, 0, 0] })
        });
        let s = load_image(&png(&img)).unwrap();
        let out = extract(&s, 3, SortBy::Amount);
        let red = out
            .iter()
            .find(|c| c.color.to_srgb8() == [255, 0, 0])
            .unwrap();
        let (x, y) = (red.position.0 * 30.0 - 0.5, red.position.1 * 30.0 - 0.5);
        assert_eq!((x.round(), y.round()), (14.0, 4.0), "{:?}", red.position);
        let white = out.iter().find(|c| c.color.to_srgb8() == [255; 3]).unwrap();
        // Four pixels tie around the square's center; the first one wins.
        assert_eq!(white.position, (14.5 / 30.0, 14.5 / 30.0));
    }

    #[test]
    fn deterministic() {
        let img = RgbImage::from_fn(200, 200, |x, y| {
            image::Rgb([x as u8, y as u8, ((x + y) / 2) as u8])
        });
        let s = load_image(&png(&img)).unwrap();
        let a = extract(&s, 6, SortBy::Amount);
        let b = extract(&s, 6, SortBy::Amount);
        assert_eq!(a, b);
        assert_eq!(a.len(), 6);
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.weight.to_bits(), y.weight.to_bits());
            assert_eq!(x.color.l.to_bits(), y.color.l.to_bits());
        }
    }

    #[test]
    fn solid_image_k6() {
        let img = RgbImage::from_pixel(50, 50, image::Rgb([0x34, 0x78, 0xF6]));
        let s = load_image(&png(&img)).unwrap();
        let out = extract(&s, 6, SortBy::Amount);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].weight, 1.0);
        assert_eq!(out[0].color.to_srgb8(), [0x34, 0x78, 0xF6]);
        assert!(!out[0].color.l.is_nan() && !out[0].position.0.is_nan());
    }

    #[test]
    fn downscaled() {
        let img = RgbImage::from_pixel(2000, 100, image::Rgb([10, 20, 30]));
        let s = load_image(&png(&img)).unwrap();
        assert_eq!(s.width, 256);
        assert_eq!(s.pixels.len(), (s.width * s.height) as usize);
    }

    #[test]
    fn bad_bytes_error() {
        assert!(matches!(
            load_image(b"not an image"),
            Err(ExtractError::Decode(_))
        ));
        let img = RgbImage::from_pixel(1, 1, image::Rgb([1, 2, 3]));
        let s = load_image(&png(&img)).unwrap();
        assert_eq!((s.width, s.height, s.pixels.len()), (1, 1, 1));
    }

    #[test]
    fn sort_modes() {
        let mut pixels = Vec::new();
        for c in [[0, 0, 0], [0, 0, 255], [255, 255, 255], [255, 0, 0]] {
            pixels.extend(std::iter::repeat_n(c, 10));
        }
        let s = SampleImage {
            width: 40,
            height: 1,
            opaque: vec![true; pixels.len()],
            pixels,
        };
        let l = extract(&s, 4, SortBy::Lightness);
        assert_eq!(l[0].color.to_srgb8(), [255, 255, 255]);
        assert_eq!(l[3].color.to_srgb8(), [0, 0, 0]);
        let h = extract(&s, 4, SortBy::Hue);
        let rgb: Vec<_> = h.iter().map(|c| c.color.to_srgb8()).collect();
        let red = rgb.iter().position(|c| *c == [255, 0, 0]).unwrap();
        let blue = rgb.iter().position(|c| *c == [0, 0, 255]).unwrap();
        assert!(red < blue);
        assert!(h[2].color.to_oklch()[1] < GRAY_CHROMA && h[3].color.to_oklch()[1] < GRAY_CHROMA);
    }

    #[test]
    fn transparent_pixels_ignored() {
        let img = image::RgbaImage::from_fn(20, 10, |x, _| {
            if x < 10 {
                image::Rgba([255, 255, 255, 0])
            } else {
                image::Rgba([0x34, 0x78, 0xF6, 255])
            }
        });
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        let s = load_image(&buf.into_inner()).unwrap();
        let out = extract(&s, 3, SortBy::Amount);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].color.to_srgb8(), [0x34, 0x78, 0xF6]);
        assert_eq!(out[0].weight, 1.0);
        assert!(out[0].position.0 > 0.5);
    }

    fn icc_png(profile: Vec<u8>, px: [u8; 3]) -> Vec<u8> {
        use image::codecs::png::PngEncoder;
        use image::{ExtendedColorType, ImageEncoder};
        let mut buf = Vec::new();
        let mut enc = PngEncoder::new(&mut buf);
        enc.set_icc_profile(profile).unwrap();
        let data: Vec<u8> = px.iter().copied().cycle().take(4 * 4 * 3).collect();
        enc.write_image(&data, 4, 4, ExtendedColorType::Rgb8)
            .unwrap();
        buf
    }

    #[test]
    fn embedded_icc_profile_is_applied() {
        use lcms2::{CIExyY, CIExyYTRIPLE, Profile, ToneCurve};
        let xy = |x, y| CIExyY { x, y, Y: 1.0 };
        let wp = xy(0.3127, 0.3290);
        let prim = CIExyYTRIPLE {
            Red: xy(0.64, 0.33),
            Green: xy(0.21, 0.71),
            Blue: xy(0.15, 0.06),
        };
        let curve = ToneCurve::new(563.0 / 256.0);
        let adobe = Profile::new_rgb(&wp, &prim, &[&curve, &curve, &curve])
            .unwrap()
            .icc()
            .unwrap();
        let s = load_image(&icc_png(adobe, [80, 119, 242])).unwrap();
        for px in &s.pixels {
            for (got, want) in px.iter().zip([52u8, 120, 246]) {
                assert!(got.abs_diff(want) <= 1, "{px:?}");
            }
        }

        // A gray profile cannot convert RGB: raw pixels are kept.
        let gray = Profile::new_gray(&wp, &ToneCurve::new(2.2))
            .unwrap()
            .icc()
            .unwrap();
        let s = load_image(&icc_png(gray, [80, 119, 242])).unwrap();
        assert!(s.pixels.iter().all(|px| *px == [80, 119, 242]));
        // Garbage profile bytes likewise.
        let s = load_image(&icc_png(vec![1, 2, 3, 4], [80, 119, 242])).unwrap();
        assert!(s.pixels.iter().all(|px| *px == [80, 119, 242]));
    }

    #[test]
    fn max_k_all_distinct() {
        let img = RgbImage::from_fn(256, 256, |x, y| {
            image::Rgb([x as u8, y as u8, (x ^ y) as u8])
        });
        let s = load_image(&png(&img)).unwrap();
        let out = extract(&s, 16, SortBy::Amount);
        assert_eq!(out.len(), 16);
    }
}
