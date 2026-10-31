//! One channel of the Wheel tab: a name, a slider whose track shows the
//! channel's gradient, and a numeric field.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

mod imp {
    use super::*;

    /// A `gtk::Scale` that paints `stops` under its (transparent) trough.
    #[derive(Default)]
    pub struct GradientScale {
        pub(super) stops: RefCell<Vec<[u8; 3]>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GradientScale {
        const NAME: &'static str = "ColorzGradientScale";
        type Type = super::GradientScale;
        type ParentType = gtk::Scale;
    }

    impl ObjectImpl for GradientScale {}
    impl RangeImpl for GradientScale {}
    impl ScaleImpl for GradientScale {}

    impl WidgetImpl for GradientScale {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.draw_gradient(snapshot);
            self.parent_snapshot(snapshot);
        }
    }

    impl GradientScale {
        /// Fills the trough's area with the gradient, under the trough.
        fn draw_gradient(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let stops = self.stops.borrow();
            if stops.len() < 2 {
                return;
            }
            let bounds = std::iter::successors(obj.first_child(), |c| c.next_sibling())
                .find(|c| c.css_name() == "trough")
                .and_then(|t| t.compute_bounds(&*obj));
            let Some(b) = bounds else {
                return;
            };
            let last = (stops.len() - 1) as f32;
            let color_stops: Vec<gsk::ColorStop> = stops
                .iter()
                .enumerate()
                .map(|(i, &[r, g, b])| {
                    let f = |v: u8| f32::from(v) / 255.0;
                    gsk::ColorStop::new(i as f32 / last, gdk::RGBA::new(f(r), f(g), f(b), 1.0))
                })
                .collect();
            let (left, right) = (
                graphene::Point::new(b.x(), b.y()),
                graphene::Point::new(b.x() + b.width(), b.y()),
            );
            let (start, end) = if obj.direction() == gtk::TextDirection::Rtl {
                (right, left)
            } else {
                (left, right)
            };
            snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(b, b.height() / 2.0));
            snapshot.append_linear_gradient(&b, &start, &end, &color_stops);
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    pub struct GradientScale(ObjectSubclass<imp::GradientScale>)
        @extends gtk::Scale, gtk::Range, gtk::Widget,
        @implements gtk::Accessible, gtk::AccessibleRange, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Orientable;
}

impl GradientScale {
    fn new(adjustment: &gtk::Adjustment) -> GradientScale {
        glib::Object::builder()
            .property("orientation", gtk::Orientation::Horizontal)
            .property("adjustment", adjustment)
            .build()
    }
}

type ChangedHandler = Rc<dyn Fn(f64)>;
type GradientFn = Rc<dyn Fn(f64) -> [u8; 3]>;

/// Colors painted along the track.
const GRADIENT_STOPS: usize = 16;

/// Touchpad scrolling moves one step per this many pixels.
const PIXELS_PER_STEP: f64 = 10.0;

/// Adds a scroll `delta` (wheel notches, or touchpad steps) to `pending`
/// and takes out the whole steps it now holds, keeping the remainder so
/// high-resolution wheels and touchpads still add up to one step at a time.
/// Scrolling down or left gives negative steps.
fn take_scroll_steps(pending: &mut f64, delta: f64) -> i32 {
    *pending += delta;
    let whole = pending.trunc();
    *pending -= whole;
    -(whole as i32)
}

/// What the slider's event handlers share; holds the scale weakly.
struct Shared {
    scale: glib::WeakRef<GradientScale>,
    adjustment: gtk::Adjustment,
    /// The channel's range; the adjustment widens it to show a value
    /// outside (e.g. an out-of-sRGB chroma).
    range: (f64, f64),
    /// The value as last set or emitted; the widgets may show it rounded.
    exact: Cell<f64>,
    /// Set while the widgets are updated from code.
    syncing: Cell<bool>,
    /// Set while a pointer or touch holds the slider: a widened range is
    /// then kept, so the track does not rescale under the pointer.
    dragging: Cell<bool>,
    /// The color at a channel value, for the track.
    gradient: RefCell<Option<GradientFn>>,
    on_changed: RefCell<Option<ChangedHandler>>,
}

impl Shared {
    /// Shows `v` without emitting `changed`, widening the shown range to
    /// include it. Unless dragging, the range otherwise shrinks back to the
    /// channel's.
    fn show(&self, v: f64) {
        let adj = &self.adjustment;
        let (lo, hi) = if self.dragging.get() {
            (adj.lower(), adj.upper())
        } else {
            self.range
        };
        let before = (adj.lower(), adj.upper());
        self.exact.set(v);
        self.syncing.set(true);
        adj.set_lower(lo.min(v));
        adj.set_upper(hi.max(v));
        adj.set_value(v);
        self.syncing.set(false);
        if (adj.lower(), adj.upper()) != before {
            self.refresh_gradient();
        }
    }

    /// Paints the gradient over the range the track shows now.
    fn refresh_gradient(&self) {
        let (Some(scale), Some(color_at)) = (self.scale.upgrade(), self.gradient.borrow().clone())
        else {
            return;
        };
        let (lo, hi) = (self.adjustment.lower(), self.adjustment.upper());
        let stops = gradient_stops(lo, hi, |v| color_at(v));
        let imp = scale.imp();
        if *imp.stops.borrow() != stops {
            imp.stops.replace(stops);
            scale.queue_draw();
        }
    }
}

/// `GRADIENT_STOPS` colors evenly spaced from `lo` to `hi`.
fn gradient_stops(lo: f64, hi: f64, color_at: impl Fn(f64) -> [u8; 3]) -> Vec<[u8; 3]> {
    let last = (GRADIENT_STOPS - 1) as f64;
    (0..GRADIENT_STOPS)
        .map(|k| color_at(lo + (hi - lo) * k as f64 / last))
        .collect()
}

#[derive(Clone)]
pub struct ChannelSlider {
    root: gtk::Box,
    shared: Rc<Shared>,
}

impl ChannelSlider {
    pub fn new(name: &str, min: f64, max: f64, step: f64, digits: u32) -> ChannelSlider {
        let adjustment = gtk::Adjustment::new(min, min, max, step, step * 10.0, 0.0);
        let label = gtk::Label::builder()
            .label(name)
            .width_chars(2)
            .xalign(0.0)
            .css_classes(["channel-name"])
            .build();
        let scale = GradientScale::new(&adjustment);
        scale.set_digits(digits as i32);
        scale.set_draw_value(false);
        scale.set_hexpand(true);
        scale.add_css_class("channel-scale");
        let spin = gtk::SpinButton::new(Some(&adjustment), step, digits);
        spin.set_numeric(true);
        spin.set_width_chars(6);
        spin.set_valign(gtk::Align::Center);
        for w in [scale.upcast_ref::<gtk::Widget>(), spin.upcast_ref()] {
            w.update_property(&[gtk::accessible::Property::Label(name)]);
        }
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        root.append(&label);
        root.append(&scale);
        root.append(&spin);

        let shared = Rc::new(Shared {
            scale: scale.downgrade(),
            adjustment: adjustment.clone(),
            range: (min, max),
            exact: Cell::new(min),
            syncing: Cell::default(),
            dragging: Cell::default(),
            gradient: RefCell::default(),
            on_changed: RefCell::default(),
        });
        // Half of the last shown digit: the spin button re-reads its text
        // on focus-out, which would otherwise round the exact value. (The
        // exact value is always within the adjustment, see `Shared::show`,
        // so typing a bound is a real change.)
        let half_digit = 0.5 * 10f64.powi(-(digits as i32));
        // Weak: `shared` owns the adjustment, which owns this handler.
        adjustment.connect_value_changed({
            let shared = Rc::downgrade(&shared);
            move |adj| {
                let Some(shared) = shared.upgrade() else {
                    return;
                };
                if shared.syncing.get() {
                    return;
                }
                let v = adj.value();
                if (v - shared.exact.get()).abs() < half_digit - 1e-12 {
                    return;
                }
                shared.exact.set(v);
                let handler = shared.on_changed.borrow().clone();
                if let Some(f) = handler {
                    f(v);
                }
            }
        });
        // Watches presses without taking part in the scale's own gestures.
        let presses = gtk::EventControllerLegacy::new();
        presses.set_propagation_phase(gtk::PropagationPhase::Capture);
        presses.connect_event({
            let shared = shared.clone();
            move |_, event| {
                use gdk::EventType as E;
                match event.event_type() {
                    E::ButtonPress | E::TouchBegin => shared.dragging.set(true),
                    E::ButtonRelease | E::TouchEnd | E::TouchCancel
                        if shared.dragging.replace(false) =>
                    {
                        // Shrink back now that nothing is held.
                        shared.show(shared.exact.get());
                    }
                    _ => {}
                }
                glib::Propagation::Proceed
            }
        });
        scale.add_controller(presses);
        // GtkScale moves a page (ten steps) per wheel notch; move one step,
        // like the numeric field. Capture phase runs before the scale's own
        // handler, which is then skipped.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        let pending = Rc::new(Cell::new(0.0));
        scroll.connect_scroll_begin({
            let pending = pending.clone();
            move |_| pending.set(0.0)
        });
        scroll.connect_scroll({
            let adjustment = adjustment.clone();
            move |ctrl, dx, dy| {
                // Vertical wheels are the norm; a tilt wheel or sideways
                // swipe scrolls right to increase.
                let delta = if dy != 0.0 { dy } else { -dx };
                let delta = match ctrl.unit() {
                    gdk::ScrollUnit::Wheel => delta,
                    _ => delta / PIXELS_PER_STEP,
                };
                let mut p = pending.get();
                let steps = take_scroll_steps(&mut p, delta);
                pending.set(p);
                if steps != 0 {
                    let step = adjustment.step_increment();
                    adjustment.set_value(adjustment.value() + f64::from(steps) * step);
                }
                glib::Propagation::Stop
            }
        });
        scale.add_controller(scroll);
        ChannelSlider { root, shared }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Shows `v` without emitting `changed`. A value outside the range
    /// widens it, so the true value is shown and typing the bound applies
    /// it; the range shrinks back as the value returns, but not while the
    /// slider is being dragged.
    pub fn set_value(&self, v: f64) {
        self.shared.show(v);
    }

    /// Paints the track with `color_at` of the channel values it shows,
    /// following the range as it widens and shrinks.
    pub fn set_gradient(&self, color_at: impl Fn(f64) -> [u8; 3] + 'static) {
        *self.shared.gradient.borrow_mut() = Some(Rc::new(color_at));
        self.shared.refresh_gradient();
    }

    /// Called with the new value when the user moves the slider or types.
    pub fn connect_changed(&self, f: impl Fn(f64) + 'static) {
        *self.shared.on_changed.borrow_mut() = Some(Rc::new(f));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_spans_the_shown_range() {
        let stops = gradient_stops(0.0, 0.5, |v| [(v * 510.0).round() as u8, 0, 0]);
        assert_eq!(stops.len(), GRADIENT_STOPS);
        assert_eq!(stops[0], [0, 0, 0]);
        assert_eq!(stops[GRADIENT_STOPS - 1], [255, 0, 0]);
        assert_eq!(stops[5], [85, 0, 0]);
    }

    #[test]
    fn scroll_moves_one_step_per_notch() {
        let mut p = 0.0;
        assert_eq!(take_scroll_steps(&mut p, 1.0), -1);
        assert_eq!(take_scroll_steps(&mut p, -1.0), 1);
        assert_eq!(take_scroll_steps(&mut p, -3.0), 3);
        assert_eq!(p, 0.0);
    }

    #[test]
    fn scroll_fractions_add_up_to_whole_steps() {
        let mut p = 0.0;
        // A high-resolution wheel sends eighths of a notch.
        let steps: i32 = (0..8).map(|_| take_scroll_steps(&mut p, -0.125)).sum();
        assert_eq!(steps, 1);
        assert_eq!(take_scroll_steps(&mut p, 0.4), 0);
        assert_eq!(take_scroll_steps(&mut p, 0.4), 0);
        assert_eq!(take_scroll_steps(&mut p, 0.4), -1);
        assert!((p - 0.2).abs() < 1e-9);
    }
}
