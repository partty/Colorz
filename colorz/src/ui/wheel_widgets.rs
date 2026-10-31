//! The large color wheel widget (`WheelView`). Its channels are those of
//! `wheel_channels`.
//!
//! The raster comes from `colorz_core::wheel` at device-pixel size and is
//! placed on the device pixel grid, so it stays crisp at fractional scales.
//! Markers and the Display P3 outline are vector-drawn on top.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use adw::subclass::prelude::*;
use colorz_core::wheel::{
    OKLCH_MAX_CHROMA, WheelGeom, hue_at, p3_boundary, polar_at, polar_point, render_okhsl_disc,
    render_oklch_plane, render_ring_triangle, sv_at, sv_point,
};
use gtk::{gdk, glib, graphene, gsk};

use super::wheel_channels::{
    WheelMode, channel_specs, color_from_channels, step_clamped, wrap_hue,
};

/// The channel the raster depends on (hue for Ring, lightness for the
/// planes), normalized for `colorz_core::wheel`.
fn raster_value(mode: WheelMode, ch: [f64; 3]) -> f64 {
    match mode {
        WheelMode::Ring => ch[0],
        WheelMode::Oklch => ch[0].clamp(0.0, 1.0),
        WheelMode::Okhsl => (ch[2] / 100.0).clamp(0.0, 1.0),
    }
}

/// Shift+drag moves the point by this fraction of the pointer movement.
const FINE_DRAG: f64 = 0.1;
/// Radius of the selection marker, logical px.
const MARKER_RADIUS: f32 = 7.0;
const HARMONY_RADIUS: f32 = 5.5;
/// How close a press must be to a harmony marker to hit it, logical px.
const HARMONY_HIT: f64 = 10.0;
/// A press this close to a selection marker grabs it, logical px. It wins
/// over harmony markers, which are drawn below it.
const SELECTION_HIT: f64 = MARKER_RADIUS as f64 + 3.0;
/// Harmony markers closer than this to a selection marker are hidden.
const HARMONY_OVERLAP: f64 = (MARKER_RADIUS + HARMONY_RADIUS) as f64;
/// Hue change per arrow or Page Up/Down press (Shift: ×10), degrees.
const HUE_NUDGE: f64 = 1.0;
const P3_STEPS: usize = 180;
/// A full render slower than this switches rapid changes to half
/// resolution...
const SLOW_RENDER: Duration = Duration::from_millis(12);
/// ...until the raster value has been still for this long.
const REFINE_AFTER: Duration = Duration::from_millis(150);
const MIN_SIZE: i32 = 160;
const NATURAL_SIZE: i32 = 320;

type ChangedHandler = Rc<dyn Fn([f64; 3])>;
/// A lightness and the Display P3 boundary at it (see `p3_boundary`).
type P3Outline = (f64, Vec<(f64, f64)>);

/// A point, device px.
type Point = (f64, f64);

fn distance(a: Point, b: Point) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// The harmony markers `(hue, point)` that do not overlap a selection
/// marker. `scale` converts device to logical px.
fn visible_harmony(
    harmony: Vec<(f64, Point)>,
    selection: &[Point],
    scale: f64,
) -> Vec<(f64, Point)> {
    harmony
        .into_iter()
        .filter(|(_, m)| {
            selection
                .iter()
                .all(|&s| distance(*m, s) / scale > HARMONY_OVERLAP)
        })
        .collect()
}

/// The hue of the harmony marker a press at `p` hits: none when the press
/// is on a selection marker, else the nearest marker within reach.
fn harmony_hit(p: Point, harmony: &[(f64, Point)], selection: &[Point], scale: f64) -> Option<f64> {
    if selection
        .iter()
        .any(|&s| distance(s, p) / scale <= SELECTION_HIT)
    {
        return None;
    }
    harmony
        .iter()
        .map(|&(h, m)| (h, distance(m, p) / scale))
        .filter(|&(_, d)| d <= HARMONY_HIT)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(h, _)| h)
}

/// Which part of the wheel a drag acts on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Region {
    /// Ring mode's hue ring.
    Ring,
    /// Ring mode's triangle, or the whole plane in the other modes.
    #[default]
    Inside,
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    region: Region,
    /// The point being moved, device px.
    point: (f64, f64),
    /// The gesture offset at the last update, logical px.
    last: (f64, f64),
}

struct Raster {
    mode: WheelMode,
    dev: u32,
    value: f64,
    /// Rendered at half resolution while the value was changing.
    preview: bool,
    texture: gdk::Texture,
}

/// Where the wheel square is drawn: `dev` device px wide, at logical
/// `(x, y)` with logical side `side` (= `dev / scale`).
struct Layout {
    scale: f64,
    dev: u32,
    x: f64,
    y: f64,
    side: f64,
}

impl Layout {
    fn geom(&self) -> WheelGeom {
        WheelGeom {
            size: f64::from(self.dev),
        }
    }

    fn to_dev(&self, x: f64, y: f64) -> (f64, f64) {
        ((x - self.x) * self.scale, (y - self.y) * self.scale)
    }

    fn to_logical(&self, (px, py): (f64, f64)) -> graphene::Point {
        graphene::Point::new(
            (self.x + px / self.scale) as f32,
            (self.y + py / self.scale) as f32,
        )
    }
}

fn render(mode: WheelMode, size: u32, value: f64) -> Vec<u8> {
    match mode {
        WheelMode::Ring => render_ring_triangle(size, value),
        WheelMode::Oklch => render_oklch_plane(size, value),
        WheelMode::Okhsl => render_okhsl_disc(size, value),
    }
}

fn rgba([r, g, b]: [u8; 3]) -> gdk::RGBA {
    let f = |v: u8| f32::from(v) / 255.0;
    gdk::RGBA::new(f(r), f(g), f(b), 1.0)
}

fn circle(center: &graphene::Point, radius: f32) -> gsk::Path {
    let b = gsk::PathBuilder::new();
    b.add_circle(center, radius);
    b.to_path()
}

/// A circle outlined white over a dark halo, optionally filled.
fn draw_marker(
    snapshot: &gtk::Snapshot,
    center: &graphene::Point,
    radius: f32,
    fill: Option<[u8; 3]>,
) {
    let path = circle(center, radius);
    if let Some(rgb) = fill {
        snapshot.append_fill(&path, gsk::FillRule::Winding, &rgba(rgb));
    }
    snapshot.append_stroke(
        &path,
        &gsk::Stroke::new(3.5),
        &gdk::RGBA::new(0.0, 0.0, 0.0, 0.5),
    );
    snapshot.append_stroke(&path, &gsk::Stroke::new(1.75), &gdk::RGBA::WHITE);
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct WheelView {
        pub(super) mode: Cell<WheelMode>,
        pub(super) channels: Cell<[f64; 3]>,
        pub(super) harmony: RefCell<Vec<f64>>,
        pub(super) on_changed: RefCell<Option<ChangedHandler>>,
        pub(super) drag: Cell<Option<Drag>>,
        pub(super) raster: RefCell<Option<Raster>>,
        /// Display P3 boundary for a lightness.
        pub(super) p3: RefCell<Option<P3Outline>>,
        /// When the raster value last changed.
        pub(super) last_change: Cell<Option<Instant>>,
        /// The last change came soon after the one before: the value is
        /// being dragged, not set once.
        pub(super) streak: Cell<bool>,
        /// The last full render took longer than [`SLOW_RENDER`].
        pub(super) slow: Cell<bool>,
        pub(super) refine_pending: Cell<bool>,
        pub(super) scale_handler: RefCell<Option<(gdk::Surface, glib::SignalHandlerId)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WheelView {
        const NAME: &'static str = "ColorzWheelView";
        type Type = super::WheelView;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("wheelview");
        }
    }

    impl ObjectImpl for WheelView {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_focusable(true);
            obj.set_can_focus(true);
            obj.set_hexpand(true);
            obj.set_vexpand(true);
            obj.update_property(&[gtk::accessible::Property::Label("Color wheel")]);

            let drag = gtk::GestureDrag::new();
            drag.connect_drag_begin(|g, x, y| {
                if let Some(w) = view_of(g) {
                    w.imp().drag_begin(g, x, y);
                }
            });
            drag.connect_drag_update(|g, x, y| {
                if let Some(w) = view_of(g) {
                    w.imp().drag_update(g, x, y);
                }
            });
            drag.connect_drag_end(|g, _, _| {
                if let Some(w) = view_of(g) {
                    w.imp().drag.set(None);
                }
            });
            obj.add_controller(drag);

            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(|k, key, _, state| match view_of(k) {
                Some(w) => w.imp().key_pressed(key, state),
                None => glib::Propagation::Proceed,
            });
            obj.add_controller(keys);

            obj.connect_scale_factor_notify(|w| w.queue_draw());
            obj.connect_state_flags_changed(|w, old| {
                let focus = gtk::StateFlags::FOCUSED | gtk::StateFlags::FOCUS_VISIBLE;
                if (w.state_flags() ^ old).intersects(focus) {
                    w.queue_draw();
                }
            });
        }

        fn dispose(&self) {
            self.on_changed.replace(None);
        }
    }

    impl WidgetImpl for WheelView {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (MIN_SIZE, NATURAL_SIZE, -1, -1)
        }

        fn realize(&self) {
            self.parent_realize();
            // Fractional scale changes do not change the integer
            // scale-factor, so follow the surface's own scale too.
            let surface = self.obj().native().and_then(|n| n.surface());
            if let Some(surface) = surface {
                let weak = self.obj().downgrade();
                let id = surface.connect_scale_notify(move |_| {
                    if let Some(w) = weak.upgrade() {
                        w.queue_draw();
                    }
                });
                self.scale_handler.replace(Some((surface, id)));
            }
        }

        fn unrealize(&self) {
            if let Some((surface, id)) = self.scale_handler.take() {
                surface.disconnect(id);
            }
            self.parent_unrealize();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(lay) = self.layout() else {
                return;
            };
            let (mode, ch) = (self.mode.get(), self.channels.get());
            let texture = self.texture(mode, lay.dev, raster_value(mode, ch));
            let bounds =
                graphene::Rect::new(lay.x as f32, lay.y as f32, lay.side as f32, lay.side as f32);
            snapshot.append_texture(&texture, &bounds);
            if mode == WheelMode::Oklch {
                self.draw_p3_outline(snapshot, &lay, ch[0]);
            }
            self.draw_markers(snapshot, &lay);
            if self.obj().has_visible_focus() {
                let g = lay.geom();
                let center = lay.to_logical(g.center());
                let r = (g.outer_radius() / lay.scale) as f32 + 3.0;
                let accent = adw::StyleManager::default().accent_color_rgba();
                snapshot.append_stroke(&circle(&center, r), &gsk::Stroke::new(2.0), &accent);
            }
        }
    }

    fn view_of(c: &impl IsA<gtk::EventController>) -> Option<super::WheelView> {
        c.widget().and_downcast()
    }

    impl WheelView {
        fn layout(&self) -> Option<Layout> {
            let obj = self.obj();
            let (w, h) = (f64::from(obj.width()), f64::from(obj.height()));
            let size = w.min(h);
            if size < 1.0 {
                return None;
            }
            let native = obj.native();
            let scale = native
                .as_ref()
                .and_then(|n| n.surface())
                .map(|s| s.scale())
                .filter(|s| *s > 0.0)
                .unwrap_or_else(|| f64::from(obj.scale_factor()));
            // The epsilon keeps 400 × 1.25 from becoming 501 px.
            let dev = (size * scale - 1e-6).ceil().max(1.0) as u32;
            let side = f64::from(dev) / scale;
            let (mut x, mut y) = ((w - side) / 2.0, (h - side) / 2.0);
            // Move the square onto the device pixel grid so that texture
            // pixels map 1:1 to screen pixels.
            if let Some(native) = native
                && let Some(p) =
                    obj.compute_point(&native, &graphene::Point::new(x as f32, y as f32))
            {
                let (tx, ty) = native.surface_transform();
                let snap = |v: f64| ((v * scale).round() - v * scale) / scale;
                x += snap(f64::from(p.x()) + tx);
                y += snap(f64::from(p.y()) + ty);
            }
            Some(Layout {
                scale,
                dev,
                x,
                y,
                side,
            })
        }

        /// The raster for `mode` at `dev` px, from the cache when possible.
        fn texture(&self, mode: WheelMode, dev: u32, value: f64) -> gdk::Texture {
            let changing = self.streak.get()
                && self
                    .last_change
                    .get()
                    .is_some_and(|t| t.elapsed() < REFINE_AFTER);
            let mut cache = self.raster.borrow_mut();
            if let Some(r) = cache.as_ref()
                && r.mode == mode
                && r.dev == dev
                && r.value == value
                && (!r.preview || changing)
            {
                return r.texture.clone();
            }
            let preview = changing && self.slow.get();
            let size = if preview { dev.div_ceil(2) } else { dev };
            let started = Instant::now();
            let pixels = render(mode, size, value);
            if preview {
                self.schedule_refine();
            } else {
                self.slow.set(started.elapsed() > SLOW_RENDER);
            }
            let n = size as usize;
            let texture = gdk::MemoryTexture::new(
                size as i32,
                size as i32,
                gdk::MemoryFormat::R8g8b8a8,
                &glib::Bytes::from_owned(pixels),
                n * 4,
            )
            .upcast::<gdk::Texture>();
            *cache = Some(Raster {
                mode,
                dev,
                value,
                preview,
                texture: texture.clone(),
            });
            texture
        }

        /// Redraws at full resolution once the value stops changing.
        fn schedule_refine(&self) {
            if self.refine_pending.replace(true) {
                return;
            }
            let weak = self.obj().downgrade();
            glib::timeout_add_local(Duration::from_millis(50), move || {
                let Some(w) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let imp = w.imp();
                if imp
                    .last_change
                    .get()
                    .is_some_and(|t| t.elapsed() < REFINE_AFTER)
                {
                    return glib::ControlFlow::Continue;
                }
                imp.refine_pending.set(false);
                w.queue_draw();
                glib::ControlFlow::Break
            });
        }

        fn draw_p3_outline(&self, snapshot: &gtk::Snapshot, lay: &Layout, l: f64) {
            if !(0.0..1.0).contains(&l) || l == 0.0 {
                return;
            }
            let mut p3 = self.p3.borrow_mut();
            if p3.as_ref().is_none_or(|(pl, _)| *pl != l) {
                *p3 = Some((l, p3_boundary(l, P3_STEPS)));
            }
            let Some((_, points)) = p3.as_ref() else {
                return;
            };
            let g = lay.geom();
            let b = gsk::PathBuilder::new();
            for (i, &(h, r)) in points.iter().enumerate() {
                let p = lay.to_logical(polar_point(&g, h, r));
                if i == 0 {
                    b.move_to(p.x(), p.y());
                } else {
                    b.line_to(p.x(), p.y());
                }
            }
            b.close();
            let path = b.to_path();
            for (width, color) in [
                (2.5, gdk::RGBA::new(0.0, 0.0, 0.0, 0.45)),
                (1.25, gdk::RGBA::new(1.0, 1.0, 1.0, 0.9)),
            ] {
                let stroke = gsk::Stroke::new(width);
                stroke.set_dash(&[5.0, 4.0]);
                snapshot.append_stroke(&path, &stroke, &color);
            }
        }

        /// Ring mode: the middle of the hue ring, device px from the center.
        fn ring_mid(g: &WheelGeom) -> f64 {
            (g.inner_radius() + g.outer_radius()) / 2.0
        }

        /// Where the marker for `region` is, device px.
        fn marker_point(&self, lay: &Layout, region: Region) -> (f64, f64) {
            let g = lay.geom();
            let ch = self.channels.get();
            match (self.mode.get(), region) {
                (WheelMode::Ring, Region::Ring) => {
                    polar_point(&g, ch[0], Self::ring_mid(&g) / g.outer_radius())
                }
                (WheelMode::Ring, Region::Inside) => {
                    sv_point(&g, ch[0], ch[1] / 100.0, ch[2] / 100.0)
                }
                (WheelMode::Oklch, _) => {
                    polar_point(&g, ch[2], (ch[1] / OKLCH_MAX_CHROMA).clamp(0.0, 1.0))
                }
                (WheelMode::Okhsl, _) => polar_point(&g, ch[0], (ch[1] / 100.0).clamp(0.0, 1.0)),
            }
        }

        /// The selection markers, device px.
        fn selection_points(&self, lay: &Layout) -> Vec<Point> {
            let mut v = vec![self.marker_point(lay, Region::Inside)];
            if self.mode.get() == WheelMode::Ring {
                v.push(self.marker_point(lay, Region::Ring));
            }
            v
        }

        /// The shown harmony hues and where their markers are, device px.
        fn harmony_points(&self, lay: &Layout) -> Vec<(f64, Point)> {
            let g = lay.geom();
            let (mode, ch) = (self.mode.get(), self.channels.get());
            let r = match mode {
                WheelMode::Ring => Self::ring_mid(&g) / g.outer_radius(),
                WheelMode::Oklch => (ch[1] / OKLCH_MAX_CHROMA).clamp(0.0, 1.0),
                WheelMode::Okhsl => (ch[1] / 100.0).clamp(0.0, 1.0),
            };
            let all: Vec<_> = self
                .harmony
                .borrow()
                .iter()
                .map(|&h| (h, polar_point(&g, h, r)))
                .collect();
            visible_harmony(all, &self.selection_points(lay), lay.scale)
        }

        fn with_hue(&self, h: f64) -> [f64; 3] {
            let mode = self.mode.get();
            let mut ch = self.channels.get();
            ch[mode.hue_index()] = h;
            ch
        }

        fn draw_markers(&self, snapshot: &gtk::Snapshot, lay: &Layout) {
            let mode = self.mode.get();
            for (h, p) in self.harmony_points(lay) {
                let rgb = color_from_channels(mode, self.with_hue(h)).to_srgb8();
                draw_marker(snapshot, &lay.to_logical(p), HARMONY_RADIUS, Some(rgb));
            }
            if mode == WheelMode::Ring {
                let g = lay.geom();
                let half_ring = (g.outer_radius() - g.inner_radius()) / 2.0 / lay.scale;
                let r = (half_ring as f32 - 1.5).max(3.0);
                let p = lay.to_logical(self.marker_point(lay, Region::Ring));
                draw_marker(snapshot, &p, r, None);
            }
            let rgb = color_from_channels(mode, self.channels.get()).to_srgb8();
            let p = lay.to_logical(self.marker_point(lay, Region::Inside));
            draw_marker(snapshot, &p, MARKER_RADIUS, Some(rgb));
        }

        fn region_at(&self, g: &WheelGeom, (x, y): (f64, f64)) -> Region {
            let (cx, cy) = g.center();
            if self.mode.get() == WheelMode::Ring && (x - cx).hypot(y - cy) > g.inner_radius() {
                Region::Ring
            } else {
                Region::Inside
            }
        }

        fn drag_begin(&self, gesture: &gtk::GestureDrag, x: f64, y: f64) {
            self.obj().grab_focus();
            let Some(lay) = self.layout() else {
                return;
            };
            let p = lay.to_dev(x, y);
            let hit = harmony_hit(
                p,
                &self.harmony_points(&lay),
                &self.selection_points(&lay),
                lay.scale,
            );
            if let Some(h) = hit {
                self.drag.set(None);
                self.emit(self.with_hue(h));
                return;
            }
            let region = self.region_at(&lay.geom(), p);
            let fine = gesture
                .current_event_state()
                .contains(gdk::ModifierType::SHIFT_MASK);
            // A fine drag moves the current value instead of jumping to
            // the pointer.
            let point = if fine {
                self.marker_point(&lay, region)
            } else {
                p
            };
            self.drag.set(Some(Drag {
                region,
                point,
                last: (0.0, 0.0),
            }));
            if !fine {
                self.apply_point(&lay, region, point);
            }
        }

        fn drag_update(&self, gesture: &gtk::GestureDrag, ox: f64, oy: f64) {
            let (Some(mut d), Some(lay)) = (self.drag.get(), self.layout()) else {
                return;
            };
            let fine = gesture
                .current_event_state()
                .contains(gdk::ModifierType::SHIFT_MASK);
            let k = lay.scale * if fine { FINE_DRAG } else { 1.0 };
            d.point.0 += (ox - d.last.0) * k;
            d.point.1 += (oy - d.last.1) * k;
            d.last = (ox, oy);
            self.drag.set(Some(d));
            self.apply_point(&lay, d.region, d.point);
        }

        fn apply_point(&self, lay: &Layout, region: Region, (x, y): (f64, f64)) {
            let g = lay.geom();
            let mut ch = self.channels.get();
            match (self.mode.get(), region) {
                (WheelMode::Ring, Region::Ring) => ch[0] = hue_at(&g, x, y),
                (WheelMode::Ring, Region::Inside) => {
                    let (s, v) = sv_at(&g, ch[0], x, y);
                    ch[1] = s * 100.0;
                    ch[2] = v * 100.0;
                }
                (WheelMode::Oklch, _) => {
                    let (h, r) = polar_at(&g, x, y);
                    if r > 0.0 {
                        ch[2] = h;
                    }
                    ch[1] = r * OKLCH_MAX_CHROMA;
                }
                (WheelMode::Okhsl, _) => {
                    let (h, r) = polar_at(&g, x, y);
                    if r > 0.0 {
                        ch[0] = h;
                    }
                    ch[1] = r * 100.0;
                }
            }
            self.emit(ch);
        }

        /// Keys (Shift: ×10):
        /// - Ring: Left/Right change S, Up/Down change V by one step.
        /// - OKLCH, OKHSL: Left/Right turn the hue by 1°, Up/Down change
        ///   C or S by one step.
        /// - Page Up/Page Down turn the hue by 1° in every mode.
        fn key_pressed(&self, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
            use gdk::Key;
            let (dx, dy, dh) = match key {
                Key::Left | Key::KP_Left => (-1.0, 0.0, 0.0),
                Key::Right | Key::KP_Right => (1.0, 0.0, 0.0),
                Key::Up | Key::KP_Up => (0.0, 1.0, 0.0),
                Key::Down | Key::KP_Down => (0.0, -1.0, 0.0),
                Key::Page_Up | Key::KP_Page_Up => (0.0, 0.0, 1.0),
                Key::Page_Down | Key::KP_Page_Down => (0.0, 0.0, -1.0),
                _ => return glib::Propagation::Proceed,
            };
            if state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            let k = if state.contains(gdk::ModifierType::SHIFT_MASK) {
                10.0
            } else {
                1.0
            };
            let mode = self.mode.get();
            let specs = channel_specs(mode);
            let mut ch = self.channels.get();
            let nudge = |ch: &mut [f64; 3], i: usize, d: f64| {
                let s = specs[i];
                ch[i] = step_clamped(ch[i], d * s.step * k, s.min, s.max);
            };
            let hue = mode.hue_index();
            let (dx_hue, radius) = match mode {
                WheelMode::Ring => {
                    nudge(&mut ch, 1, dx);
                    (0.0, 2)
                }
                WheelMode::Oklch | WheelMode::Okhsl => (dx, 1),
            };
            nudge(&mut ch, radius, dy);
            ch[hue] = wrap_hue(ch[hue] + (dx_hue + dh) * HUE_NUDGE * k);
            self.emit(ch);
            glib::Propagation::Stop
        }

        /// Takes `ch` and tells the handler.
        fn emit(&self, ch: [f64; 3]) {
            if ch == self.channels.get() {
                return;
            }
            self.obj().set_channels(ch);
            let handler = self.on_changed.borrow().clone();
            if let Some(f) = handler {
                f(ch);
            }
        }
    }
}

glib::wrapper! {
    /// The big wheel of the Wheel tab. The page owns the channel values and
    /// pushes them in with [`WheelView::set_channels`]; user edits come
    /// back through [`WheelView::connect_changed`].
    pub struct WheelView(ObjectSubclass<imp::WheelView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for WheelView {
    fn default() -> Self {
        Self::new()
    }
}

impl WheelView {
    pub fn new() -> WheelView {
        glib::Object::new()
    }

    pub fn set_mode(&self, mode: WheelMode) {
        let imp = self.imp();
        if imp.mode.replace(mode) != mode {
            // A mode switch is a one-off change: render it at full
            // resolution.
            imp.last_change.set(None);
            imp.streak.set(false);
            self.queue_draw();
        }
    }

    pub fn set_channels(&self, ch: [f64; 3]) {
        let imp = self.imp();
        let mode = imp.mode.get();
        let old = imp.channels.replace(ch);
        if raster_value(mode, old) != raster_value(mode, ch) {
            let now = Instant::now();
            let prev = imp.last_change.replace(Some(now));
            imp.streak
                .set(prev.is_some_and(|t| now.duration_since(t) < REFINE_AFTER));
        }
        if old != ch {
            self.queue_draw();
        }
    }

    /// Hues of the harmony markers; empty for none.
    pub fn set_harmony_hues(&self, hues: Vec<f64>) {
        if *self.imp().harmony.borrow() != hues {
            self.imp().harmony.replace(hues);
            self.queue_draw();
        }
    }

    /// Called with the new channels after a drag, click or key press.
    pub fn connect_changed(&self, f: impl Fn([f64; 3]) + 'static) {
        self.imp().on_changed.replace(Some(Rc::new(f)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_marker_wins_over_harmony() {
        let sel = [(100.0, 100.0)];
        // At C = 0 the harmony markers sit on the selection marker: hidden.
        let on_top = vec![(120.0, (100.0, 100.0)), (240.0, (104.0, 100.0))];
        assert!(visible_harmony(on_top, &sel, 1.0).is_empty());
        // Near the selection marker, a press grabs the selection.
        let near = [(120.0, (115.0, 100.0))];
        assert_eq!(harmony_hit((106.0, 100.0), &near, &sel, 1.0), None);
        // Elsewhere the nearest harmony marker within reach wins.
        let two = [(120.0, (130.0, 100.0)), (240.0, (138.0, 100.0))];
        assert_eq!(harmony_hit((136.0, 100.0), &two, &sel, 1.0), Some(240.0));
        assert_eq!(harmony_hit((131.0, 100.0), &two, &sel, 1.0), Some(120.0));
        assert_eq!(harmony_hit((160.0, 100.0), &two, &sel, 1.0), None);
        // Distances are in logical px: at scale 2 device px count half.
        assert_eq!(harmony_hit((150.0, 100.0), &two, &sel, 2.0), Some(240.0));
    }
}
