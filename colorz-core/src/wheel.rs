//! Wheel geometry, hit-testing and RGBA rasters for the three large color
//! wheels (HSV ring + triangle, OKLCH polar plane, OKHSL disc).
//!
//! All coordinates are device pixels with the origin top-left and the wheel
//! centered in a `size x size` square. Hue 0 is at 3 o'clock and increases
//! counter-clockwise on screen (the y axis points down). Rasters are straight
//! (non-premultiplied) RGBA8, row-major, top-left origin; pixel `(x, y)` is
//! sampled at its center.

use crate::color::{Color, RgbSpace, hsv_to_rgb};
use std::f64::consts::TAU;

/// Ring thickness as a fraction of the outer radius.
pub const RING_WIDTH_FRAC: f64 = 0.12;
/// Chroma at the rim of the OKLCH plane.
pub const OKLCH_MAX_CHROMA: f64 = 0.37;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WheelGeom {
    pub size: f64,
}

impl WheelGeom {
    pub fn center(&self) -> (f64, f64) {
        (self.size / 2.0, self.size / 2.0)
    }

    pub fn outer_radius(&self) -> f64 {
        self.size / 2.0 - 1.0
    }

    pub fn inner_radius(&self) -> f64 {
        self.outer_radius() * (1.0 - RING_WIDTH_FRAC)
    }
}

fn dist_from_center(g: &WheelGeom, x: f64, y: f64) -> f64 {
    let (cx, cy) = g.center();
    (x - cx).hypot(y - cy)
}

/// Hue in degrees, `[0, 360)`, of the direction from the center to `(x, y)`.
pub fn hue_at(g: &WheelGeom, x: f64, y: f64) -> f64 {
    let (cx, cy) = g.center();
    let h = (-(y - cy)).atan2(x - cx).to_degrees();
    let h = if h < 0.0 { h + 360.0 } else { h };
    if h >= 360.0 { 0.0 } else { h }
}

pub fn in_ring(g: &WheelGeom, x: f64, y: f64) -> bool {
    let d = dist_from_center(g, x, y);
    d >= g.inner_radius() && d <= g.outer_radius()
}

fn polar_xy(g: &WheelGeom, hue: f64, radius: f64) -> (f64, f64) {
    let (cx, cy) = g.center();
    let a = hue.to_radians();
    (cx + radius * a.cos(), cy - radius * a.sin())
}

/// Triangle corners: pure hue (at angle `hue`), white, black.
pub fn triangle_vertices(g: &WheelGeom, hue: f64) -> [(f64, f64); 3] {
    let r = g.inner_radius();
    [
        polar_xy(g, hue, r),
        polar_xy(g, hue + 120.0, r),
        polar_xy(g, hue + 240.0, r),
    ]
}

/// Barycentric weights of `(x, y)` for the triangle `v` (pure, white, black).
fn barycentric(v: &[(f64, f64); 3], x: f64, y: f64) -> [f64; 3] {
    let det = (v[1].1 - v[2].1) * (v[0].0 - v[2].0) + (v[2].0 - v[1].0) * (v[0].1 - v[2].1);
    let w0 = ((v[1].1 - v[2].1) * (x - v[2].0) + (v[2].0 - v[1].0) * (y - v[2].1)) / det;
    let w1 = ((v[2].1 - v[0].1) * (x - v[2].0) + (v[0].0 - v[2].0) * (y - v[2].1)) / det;
    [w0, w1, 1.0 - w0 - w1]
}

fn sv_from_bary(w: [f64; 3]) -> (f64, f64) {
    let v = (1.0 - w[2]).clamp(0.0, 1.0);
    let s = if v > 1e-12 {
        (w[0] / v).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (s, v)
}

/// Saturation and value at a point; points outside the triangle are
/// projected onto its nearest boundary point. GTK2/GIMP
/// mapping: pure-hue weight = s*v, white = (1-s)*v, black = 1-v.
pub fn sv_at(g: &WheelGeom, hue: f64, x: f64, y: f64) -> (f64, f64) {
    let t = triangle_vertices(g, hue);
    let w = barycentric(&t, x, y);
    if w.iter().all(|&c| c >= 0.0) {
        return sv_from_bary(w);
    }
    // Outside: project onto the nearest point of the triangle boundary.
    let mut best = (f64::INFINITY, (x, y));
    for i in 0..3 {
        let (a, b) = (t[i], t[(i + 1) % 3]);
        let (ex, ey) = (b.0 - a.0, b.1 - a.1);
        let u = (((x - a.0) * ex + (y - a.1) * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
        let q = (a.0 + u * ex, a.1 + u * ey);
        let d = (x - q.0).hypot(y - q.1);
        if d < best.0 {
            best = (d, q);
        }
    }
    sv_from_bary(barycentric(&t, best.1.0, best.1.1))
}

pub fn sv_point(g: &WheelGeom, hue: f64, s: f64, v: f64) -> (f64, f64) {
    let t = triangle_vertices(g, hue);
    let w = [s * v, (1.0 - s) * v, 1.0 - v];
    (
        w[0] * t[0].0 + w[1] * t[1].0 + w[2] * t[2].0,
        w[0] * t[0].1 + w[1] * t[1].1 + w[2] * t[2].1,
    )
}

/// Hue (degrees) and radius (0 at the center, 1 at the rim, clamped).
pub fn polar_at(g: &WheelGeom, x: f64, y: f64) -> (f64, f64) {
    let r = (dist_from_center(g, x, y) / g.outer_radius()).clamp(0.0, 1.0);
    (hue_at(g, x, y), r)
}

pub fn polar_point(g: &WheelGeom, hue: f64, r: f64) -> (f64, f64) {
    polar_xy(g, hue, r * g.outer_radius())
}

/// Largest OKLCH chroma at lightness `l` and hue `h` inside `space`.
pub fn max_chroma(l: f64, h: f64, space: RgbSpace) -> f64 {
    let (mut lo, mut hi) = (0.0_f64, 0.6_f64);
    while hi - lo > 1e-5 {
        let mid = 0.5 * (lo + hi);
        if Color::from_oklch([l, mid, h]).in_gamut(space) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// `steps` points `(hue, radius 0..1)` at evenly spaced hues starting at 0.
/// The path is open (the last point is not repeated); the UI closes it.
pub fn p3_boundary(l: f64, steps: usize) -> Vec<(f64, f64)> {
    (0..steps)
        .map(|i| {
            let h = 360.0 * i as f64 / steps as f64;
            let r = max_chroma(l, h, RgbSpace::DisplayP3) / OKLCH_MAX_CHROMA;
            (h, r.min(1.0))
        })
        .collect()
}

/// Coverage of a pixel whose center is `d` inside an edge (negative = outside).
fn coverage(signed_inside: f64) -> f64 {
    (signed_inside + 0.5).clamp(0.0, 1.0)
}

fn srgb_encode(v: f64) -> f64 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn to_u8(v: f64) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Shared scaffold: calls `f(col, row, px, py, dx, dy)` for every pixel,
/// writing `f`'s color with disc-edge anti-aliasing (`f` returns the color and
/// coverage, or `None` to leave the pixel transparent). Rows are split over
/// `threads` workers; the output does not depend on the thread count.
fn render_disc<F>(size: u32, threads: usize, f: F) -> Vec<u8>
where
    F: Fn(usize, usize, f64, f64, f64, f64) -> Option<([u8; 3], f64)> + Sync,
{
    let n = size as usize;
    let mut out = vec![0u8; n * n * 4];
    if n == 0 {
        return out;
    }
    let g = WheelGeom { size: size as f64 };
    if g.outer_radius() <= 0.0 {
        return out;
    }
    let (cx, cy) = g.center();
    let rows_per_chunk = n.div_ceil(threads.max(1));
    let fill = |first_row: usize, chunk: &mut [u8]| {
        for (i, line) in chunk.chunks_exact_mut(n * 4).enumerate() {
            let row = first_row + i;
            let py = row as f64 + 0.5;
            for (col, px4) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let px = col as f64 + 0.5;
                if let Some((rgb, cov)) = f(col, row, px, py, px - cx, py - cy)
                    && cov > 0.0
                {
                    px4[..3].copy_from_slice(&rgb);
                    px4[3] = to_u8(cov);
                }
            }
        }
    };
    std::thread::scope(|sc| {
        for (i, chunk) in out.chunks_mut(n * 4 * rows_per_chunk).enumerate() {
            let fill = &fill;
            sc.spawn(move || fill(i * rows_per_chunk, chunk));
        }
    });
    out
}

fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

pub fn render_ring_triangle(size: u32, hue: f64) -> Vec<u8> {
    render_ring_triangle_with_threads(size, hue, default_threads())
}

fn render_ring_triangle_with_threads(size: u32, hue: f64, threads: usize) -> Vec<u8> {
    let g = WheelGeom { size: size as f64 };
    let (outer, inner) = (g.outer_radius(), g.inner_radius());
    let tri = triangle_vertices(&g, hue);
    // Triangle edges (for signed distance): inward-pointing unit normals.
    let mut edges = [(0.0, 0.0, 0.0); 3];
    for i in 0..3 {
        let (a, b) = (tri[i], tri[(i + 1) % 3]);
        let (ex, ey) = (b.0 - a.0, b.1 - a.1);
        let len = ex.hypot(ey);
        let opp = tri[(i + 2) % 3];
        let (mut nx, mut ny) = (-ey / len, ex / len);
        if nx * (opp.0 - a.0) + ny * (opp.1 - a.1) < 0.0 {
            nx = -nx;
            ny = -ny;
        }
        edges[i] = (nx, ny, -(nx * a.0 + ny * a.1));
    }
    // Ring colors depend only on the angle: 1 entry per 0.1 degree.
    let ring_lut: Vec<[u8; 3]> = (0..3600)
        .map(|i| hsv_to_rgb([i as f64 / 10.0, 1.0, 1.0]).map(to_u8))
        .collect();
    let (cx, cy) = g.center();
    render_disc(size, threads, |_, _, px, py, dx, dy| {
        let d = dx.hypot(dy);
        let ring_cov = coverage(outer - d).min(coverage(d - inner));
        // Triangle coverage (signed distance to the nearest edge).
        let tri_d = edges
            .iter()
            .map(|e| e.0 * px + e.1 * py + e.2)
            .fold(f64::INFINITY, f64::min);
        let tri_cov = coverage(tri_d);
        let ring = (ring_cov > 0.0).then(|| {
            let h = (-(py - cy)).atan2(px - cx).rem_euclid(TAU).to_degrees();
            ring_lut[((h * 10.0) as usize).min(3599)]
        });
        let tri_rgb = (tri_cov > 0.0).then(|| {
            let (s, v) = sv_from_bary(barycentric(&tri, px, py));
            hsv_to_rgb([hue, s, v]).map(to_u8)
        });
        match (tri_rgb, ring) {
            (Some(t), Some(r)) => {
                // Straight-alpha "triangle over ring".
                let a = tri_cov + ring_cov * (1.0 - tri_cov);
                let mix = |t: u8, r: u8| {
                    let c = (t as f64 * tri_cov + r as f64 * ring_cov * (1.0 - tri_cov)) / a;
                    c.round().clamp(0.0, 255.0) as u8
                };
                Some(([mix(t[0], r[0]), mix(t[1], r[1]), mix(t[2], r[2])], a))
            }
            (Some(t), None) => Some((t, tri_cov)),
            (None, Some(r)) => Some((r, ring_cov)),
            (None, None) => None,
        }
    })
}

pub fn render_oklch_plane(size: u32, l: f64) -> Vec<u8> {
    render_oklch_plane_with_threads(size, l, default_threads())
}

fn render_oklch_plane_with_threads(size: u32, l: f64, threads: usize) -> Vec<u8> {
    let outer = WheelGeom { size: size as f64 }.outer_radius();
    let gray_a = Color::from_oklab(l + 0.04, 0.0, 0.0).to_srgb8();
    let gray_b = Color::from_oklab(l - 0.04, 0.0, 0.0).to_srgb8();
    render_disc(size, threads, |col, row, _, _, dx, dy| {
        let d = dx.hypot(dy);
        let cov = coverage(outer - d);
        if cov <= 0.0 {
            return None;
        }
        // Chroma/hue as Cartesian a/b directly: no trig per pixel.
        let k = OKLCH_MAX_CHROMA / d.max(outer);
        let color = Color::from_oklab(l, dx * k, -dy * k);
        let lin = color.to_linear_srgb();
        let rgb = if lin.iter().all(|v| (-1e-6..=1.0 + 1e-6).contains(v)) {
            lin.map(|v| to_u8(srgb_encode(v.clamp(0.0, 1.0))))
        } else if (col + row) % 4 < 2 {
            gray_a
        } else {
            gray_b
        };
        Some((rgb, cov))
    })
}

pub fn render_okhsl_disc(size: u32, l: f64) -> Vec<u8> {
    render_okhsl_disc_with_threads(size, l, default_threads())
}

fn render_okhsl_disc_with_threads(size: u32, l: f64, threads: usize) -> Vec<u8> {
    let outer = WheelGeom { size: size as f64 }.outer_radius();
    render_disc(size, threads, |_, _, _, _, dx, dy| {
        let d = dx.hypot(dy);
        let cov = coverage(outer - d);
        if cov <= 0.0 {
            return None;
        }
        let hue = (-dy).atan2(dx).to_degrees();
        let s = (d / outer).min(1.0);
        Some((Color::from_okhsl([hue, s, l]).to_srgb8(), cov))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(buf: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn hue_and_ring() {
        let g = WheelGeom { size: 200.0 };
        assert!(hue_at(&g, 195.0, 100.0).abs() < 1e-9 && in_ring(&g, 195.0, 100.0));
        assert!((hue_at(&g, 100.0, 1.0) - 90.0).abs() < 1e-9);
        assert!(!in_ring(&g, 100.0, 100.0));
    }

    #[test]
    fn triangle_round_trip() {
        let g = WheelGeom { size: 300.0 };
        for hue in [0.0, 77.0, 219.0] {
            for i in 0..5 {
                for j in 0..5 {
                    let (s, v) = (i as f64 / 4.0, j as f64 / 4.0);
                    let (x, y) = sv_point(&g, hue, s, v);
                    let (s2, v2) = sv_at(&g, hue, x, y);
                    assert!((v - v2).abs() < 1e-6, "v {v} {v2}");
                    if v > 0.0 {
                        assert!((s - s2).abs() < 1e-6, "s {s} {s2}");
                    }
                }
            }
        }
        for (x, y) in [(-500.0, 900.0), (1e4, 1e4), (150.0, -80.0)] {
            let (s, v) = sv_at(&g, 77.0, x, y);
            assert!((0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&v));
        }
    }

    #[test]
    fn polar_round_trip() {
        let g = WheelGeom { size: 400.0 };
        for h in [0.0, 33.0, 90.0, 181.5, 270.0, 359.0] {
            for r in [0.02, 0.3, 0.77, 1.0] {
                let (x, y) = polar_point(&g, h, r);
                let (h2, r2) = polar_at(&g, x, y);
                assert!((h - h2).abs() < 1e-6 && (r - r2).abs() < 1e-9, "{h} {r}");
            }
        }
    }

    #[test]
    fn max_chroma_sane() {
        let c = max_chroma(0.6, 261.4, RgbSpace::Srgb);
        assert!((0.20..=0.30).contains(&c), "{c}");
        for i in 0..12 {
            let h = i as f64 * 30.0;
            assert!(max_chroma(0.6, h, RgbSpace::DisplayP3) >= max_chroma(0.6, h, RgbSpace::Srgb));
        }
        let b = p3_boundary(0.6, 12);
        assert_eq!(b.len(), 12);
        assert!(b.iter().all(|(_, r)| (0.0..=1.0).contains(r)));
    }

    #[test]
    fn parallel_matches_single_thread() {
        assert_eq!(
            render_ring_triangle(97, 33.0),
            render_ring_triangle_with_threads(97, 33.0, 1)
        );
        assert_eq!(
            render_oklch_plane(97, 0.7),
            render_oklch_plane_with_threads(97, 0.7, 1)
        );
        assert_eq!(
            render_okhsl_disc(97, 0.7),
            render_okhsl_disc_with_threads(97, 0.7, 1)
        );
        for t in [5, 200] {
            assert_eq!(
                render_ring_triangle_with_threads(97, 33.0, t),
                render_ring_triangle_with_threads(97, 33.0, 1)
            );
            assert_eq!(
                render_oklch_plane_with_threads(97, 0.7, t),
                render_oklch_plane_with_threads(97, 0.7, 1)
            );
            assert_eq!(
                render_okhsl_disc_with_threads(97, 0.7, t),
                render_okhsl_disc_with_threads(97, 0.7, 1)
            );
        }
    }

    #[test]
    fn degenerate_sizes() {
        for size in [0u32, 1, 2, 3] {
            for threads in [1, 8] {
                let n = (size * size * 4) as usize;
                let a = render_ring_triangle_with_threads(size, 10.0, threads);
                let b = render_oklch_plane_with_threads(size, 0.5, threads);
                let c = render_okhsl_disc_with_threads(size, 0.5, threads);
                for buf in [&a, &b, &c] {
                    assert_eq!(buf.len(), n);
                    if size <= 2 {
                        assert!(buf.iter().all(|&v| v == 0));
                    }
                }
            }
        }
    }

    #[test]
    fn sv_projection_outside() {
        let g = WheelGeom { size: 300.0 };
        let h = 40.0;
        let t = triangle_vertices(&g, h);
        let far = |p: (f64, f64), k: f64| {
            let (cx, cy) = g.center();
            (cx + (p.0 - cx) * k, cy + (p.1 - cy) * k)
        };
        let (s, v) = sv_at(&g, h, far(t[0], 3.0).0, far(t[0], 3.0).1);
        assert!((s - 1.0).abs() < 1e-6 && (v - 1.0).abs() < 1e-6);
        let (s, v) = sv_at(&g, h, far(t[1], 3.0).0, far(t[1], 3.0).1);
        assert!(s.abs() < 1e-6 && (v - 1.0).abs() < 1e-6);
        let (_, v) = sv_at(&g, h, far(t[2], 3.0).0, far(t[2], 3.0).1);
        assert!(v.abs() < 1e-6);
        // Just outside the middle of the pure-white edge.
        let m = ((t[0].0 + t[1].0) / 2.0, (t[0].1 + t[1].1) / 2.0);
        let (cx, cy) = g.center();
        let o = (m.0 + (m.0 - cx) * 0.05, m.1 + (m.1 - cy) * 0.05);
        let (s, v) = sv_at(&g, h, o.0, o.1);
        assert!((v - 1.0).abs() < 1e-6 && (s - 0.5).abs() < 1e-6, "{s} {v}");
    }

    #[test]
    fn ring_triangle_no_notch_at_vertices() {
        let size = 301u32;
        let g = WheelGeom { size: size as f64 };
        let buf = render_ring_triangle(size, 25.0);
        let inner = g.inner_radius();
        for v in triangle_vertices(&g, 25.0) {
            let (cx, cy) = g.center();
            let (dx, dy) = (v.0 - cx, v.1 - cy);
            let d = dx.hypot(dy);
            for off in [0.0, 0.5, 1.0, 2.0] {
                let r = inner + off;
                let (x, y) = (cx + dx / d * r, cy + dy / d * r);
                let p = px(&buf, size, x as u32, y as u32);
                // The ring alone is fully opaque a pixel outside the
                // inner edge and half-covered at the edge itself.
                let min_alpha = if off >= 1.0 { 255 } else { 100 };
                assert!(p[3] >= min_alpha, "off {off} at {x},{y}: {p:?}");
            }
        }
    }

    #[test]
    fn raster_matches_hit_testing() {
        let size = 400u32;
        let g = WheelGeom { size: size as f64 };
        let rt = render_ring_triangle(size, 200.0);
        for (s, v) in [(0.3, 0.8), (0.9, 0.5), (0.5, 0.5)] {
            let (x, y) = sv_point(&g, 200.0, s, v);
            let want = Color::from_hsv([200.0, s, v]).to_srgb8();
            let got = px(&rt, size, x as u32, y as u32);
            // sample cell is up to 1px away, so allow a small neighborhood slack
            assert!(
                (0..3).all(|i| got[i].abs_diff(want[i]) <= 6) && got[3] == 255,
                "{got:?} {want:?}"
            );
        }
        let pl = render_oklch_plane(size, 0.7);
        let (h, r) = (145.0, 0.3);
        let col = Color::from_oklch([0.7, r * OKLCH_MAX_CHROMA, h]);
        assert!(col.in_gamut(RgbSpace::Srgb));
        let (x, y) = polar_point(&g, h, r);
        let got = px(&pl, size, x as u32, y as u32);
        assert!(
            (0..3).all(|i| got[i].abs_diff(col.to_srgb8()[i]) <= 6) && got[3] == 255,
            "{got:?}"
        );
    }

    #[test]
    fn rasters_sized_and_colored() {
        let rt = render_ring_triangle(101, 0.0);
        assert_eq!(rt.len(), 101 * 101 * 4);
        assert_eq!(px(&rt, 101, 0, 0)[3], 0);
        let p = px(&rt, 101, 96, 50);
        assert!(p[0] > 240 && p[1] < 20 && p[2] < 20 && p[3] == 255, "{p:?}");
        // Triangle's pure-hue corner region is red, white and black corners exist.
        let g = WheelGeom { size: 101.0 };
        let (x, y) = sv_point(&g, 0.0, 0.9, 0.9);
        let p = px(&rt, 101, x as u32, y as u32);
        assert!(p[0] > 225 && p[1] < 40 && p[3] == 255, "{p:?}");

        let pl = render_oklch_plane(101, 0.6);
        assert_eq!(pl.len(), 101 * 101 * 4);
        let c = px(&pl, 101, 50, 50);
        assert!(c[0].abs_diff(c[1]) <= 2 && c[1].abs_diff(c[2]) <= 2 && c[3] == 255);
        // Rim at L=0.6 is out of sRGB for most hues: hatched grays only.
        let o = px(&pl, 101, 95, 50);
        assert!(o[0] == o[1] && o[1] == o[2], "{o:?}");

        let d = render_okhsl_disc(101, 0.6);
        let e = px(&d, 101, 98, 50);
        assert!(e[0] > e[1] + 60 && e[0] > e[2] + 60 && e[3] == 255, "{e:?}");
    }
}
