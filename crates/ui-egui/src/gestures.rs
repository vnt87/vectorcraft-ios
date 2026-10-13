//! Two-finger canvas gestures for touch screens (iPad): pan, pinch-zoom and rotate, plus palm
//! rejection. It consumes `egui::Event::Touch` and produces per-event [`GestureDelta`]s the canvas
//! applies to the view ([`apply_to_view`]).
//!
//! - The first two contacts are tracked; further ones are ignored until one lifts.
//! - Each move yields the change since the previous one in centroid (pan), finger distance (zoom
//!   factor) and finger angle (rotation, wrapped to (-pi, pi] so crossing +-180 degrees is
//!   seamless). [`GestureDelta::pivot`] is the *previous* centroid: zooming and rotating about it,
//!   then panning, keeps the document point that was under the fingers under them.
//! - Palm rejection: a lone contact never produces a delta, and
//!   [`GestureRecognizer::blocks_painting`] tells the canvas to ignore the emulated pointer of a
//!   lone finger while a pen is near, and of any two-finger gesture.
//! - Pen contacts ([`crate::pen::PenFeed`]): a pen touch is never a finger: not a gesture contact
//!   nor a palm. egui-winit drives its emulated pointer from the first touch that lands while no
//!   touch owns it; when that touch is the pen, a palm resting on the canvas doesn't block the
//!   stroke. A palm that landed first owns the pointer and keeps painting blocked.
//!
//! Never panics: non-finite positions are dropped, zero finger distance neither zooms nor rotates.

use egui::{Pos2, Rect, Vec2};

use crate::canvas::Xf;
use crate::state::View;

/// Finger distances (points) at or below this give no reliable scale or angle.
const MIN_DISTANCE: f32 = 1.0e-3;
/// One event never scales the view by more than this factor (either way): a glitchy jump.
const MAX_STEP: f32 = 4.0;
/// A change in scale or angle below this (f32 rounding) is none.
const SETTLE: f32 = 1.0e-5;
/// The zoom range of the canvas (3.13% to 640%).
const ZOOM_RANGE: (f64, f64) = (0.0313, 640.0);

/// The phase of one touch contact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Start,
    Move,
    End,
    Cancel,
}

impl From<egui::TouchPhase> for TouchPhase {
    fn from(p: egui::TouchPhase) -> Self {
        match p {
            egui::TouchPhase::Start => Self::Start,
            egui::TouchPhase::Move => Self::Move,
            egui::TouchPhase::End => Self::End,
            egui::TouchPhase::Cancel => Self::Cancel,
        }
    }
}

/// One touch contact event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchPoint {
    pub id: u64,
    pub pos: Pos2,
    pub phase: TouchPhase,
}

/// What a two-finger move changed since the previous one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GestureDelta {
    /// How far the fingers' centroid moved (screen points).
    pub pan_delta: Vec2,
    /// Finger distance now over before (>1 zooms in).
    pub zoom_factor: f32,
    /// Change of the angle between the fingers, radians in (-pi, pi]; clockwise on screen is positive.
    pub rotation_delta_rad: f32,
    /// The centroid before this move: zoom and rotate about it, then pan.
    pub pivot: Pos2,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Contact {
    id: u64,
    pos: Pos2,
}

/// The two-finger state machine.
#[derive(Clone, Debug, Default)]
pub struct GestureRecognizer {
    /// At most two tracked contacts, in the order they landed.
    active_touches: Vec<Contact>,
    last_distance: Option<f32>,
    last_angle: Option<f32>,
    last_centroid: Option<Pos2>,
    /// The egui frame [`Self::feed_frame_with`] last ran in.
    last_frame: Option<u64>,
    /// Finger contacts currently down (tracked or not), for [`Self::blocks_painting`].
    down: Vec<u64>,
    /// The touch driving egui-winit's emulated pointer (the first one to land while none did),
    /// and whether it is a pen.
    pointer: Option<(u64, bool)>,
}

fn finite(p: Pos2) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

/// `a` wrapped to (-pi, pi].
fn wrap_rad(a: f32) -> f32 {
    if !a.is_finite() {
        return 0.0;
    }
    let tau = std::f32::consts::TAU;
    let mut x = a % tau;
    if x > std::f32::consts::PI {
        x -= tau;
    } else if x <= -std::f32::consts::PI {
        x += tau;
    }
    x
}

impl GestureRecognizer {
    /// Two contacts are tracked: a pan/zoom/rotate gesture is in progress.
    pub fn is_gesturing(&self) -> bool {
        self.active_touches.len() >= 2
    }

    /// Should the canvas ignore the emulated pointer (no painting, no tool drag)? True during a
    /// two-finger gesture, and for a lone finger while a pen is near (a palm). A pen that owns the
    /// emulated pointer is never blocked by a resting palm.
    pub fn blocks_painting(&self, pen_near: bool) -> bool {
        let pen_drives_pointer = matches!(self.pointer, Some((_, true)));
        self.is_gesturing() || (pen_near && !self.down.is_empty() && !pen_drives_pointer)
    }

    /// Mirror egui-winit's choice of the touch that drives the emulated pointer.
    fn track_pointer(&mut self, id: u64, phase: TouchPhase, pen: bool) {
        match phase {
            TouchPhase::Start if self.pointer.is_none() => self.pointer = Some((id, pen)),
            TouchPhase::End | TouchPhase::Cancel if self.pointer.is_some_and(|(p, _)| p == id) => self.pointer = None,
            _ => {}
        }
    }

    fn baseline(&mut self) {
        match (self.active_touches.first(), self.active_touches.get(1)) {
            (Some(a), Some(b)) => {
                self.last_centroid = Some(centroid(a.pos, b.pos));
                let (d, ang) = dist_angle(a.pos, b.pos);
                self.last_distance = (d > MIN_DISTANCE).then_some(d);
                self.last_angle = (d > MIN_DISTANCE).then_some(ang);
            }
            _ => {
                self.last_centroid = None;
                self.last_distance = None;
                self.last_angle = None;
            }
        }
    }

    /// Feed one touch event; returns the gesture change it caused, if any. A lone contact never
    /// yields a delta; two fingers pan, zoom and rotate even with a pen near, so the hand holding
    /// the pencil can pinch with the other one.
    pub fn update(&mut self, touch: TouchPoint) -> Option<GestureDelta> {
        if !finite(touch.pos) {
            return None;
        }
        match touch.phase {
            TouchPhase::Start => {
                if !self.down.contains(&touch.id) {
                    self.down.push(touch.id);
                }
                if let Some(c) = self.active_touches.iter_mut().find(|c| c.id == touch.id) {
                    c.pos = touch.pos;
                } else if self.active_touches.len() < 2 {
                    self.active_touches.push(Contact { id: touch.id, pos: touch.pos });
                }
                self.baseline();
                None
            }
            TouchPhase::End | TouchPhase::Cancel => {
                self.down.retain(|&id| id != touch.id);
                let before = self.active_touches.len();
                self.active_touches.retain(|c| c.id != touch.id);
                if self.active_touches.len() != before {
                    // A freed slot goes to a contact that was down but untracked.
                    self.baseline();
                }
                None
            }
            TouchPhase::Move => {
                let c = self.active_touches.iter_mut().find(|c| c.id == touch.id)?;
                c.pos = touch.pos;
                self.step()
            }
        }
    }

    fn step(&mut self) -> Option<GestureDelta> {
        let (a, b) = (self.active_touches.first()?.pos, self.active_touches.get(1)?.pos);
        let now_c = centroid(a, b);
        let (dist, angle) = dist_angle(a, b);
        let pivot = self.last_centroid.unwrap_or(now_c);
        let pan_delta = now_c - pivot;
        let usable = dist > MIN_DISTANCE && dist.is_finite();
        let zoom_factor = match (self.last_distance, usable) {
            (Some(d0), true) => (dist / d0).clamp(1.0 / MAX_STEP, MAX_STEP),
            _ => 1.0,
        };
        let rotation_delta_rad = match (self.last_angle, usable) {
            (Some(a0), true) => wrap_rad(angle - a0),
            _ => 0.0,
        };
        // Rounding in the distance and angle of fingers that only slide must not zoom or turn.
        let zoom_factor = if (zoom_factor - 1.0).abs() < SETTLE { 1.0 } else { zoom_factor };
        let rotation_delta_rad = if rotation_delta_rad.abs() < SETTLE { 0.0 } else { rotation_delta_rad };
        self.last_centroid = Some(now_c);
        self.last_distance = usable.then_some(dist);
        self.last_angle = usable.then_some(angle);
        let delta = GestureDelta { pan_delta, zoom_factor, rotation_delta_rad, pivot };
        let moved = pan_delta != Vec2::ZERO || zoom_factor != 1.0 || rotation_delta_rad != 0.0;
        (moved && pan_delta.x.is_finite() && pan_delta.y.is_finite()).then_some(delta)
    }

    /// Feed a frame's raw events once per egui frame (the canvas can run in several passes; later
    /// passes of the same frame return nothing). Contacts that start outside `area` are ignored.
    /// `is_pen(touch id)` tells pen contacts from fingers: pen touches only decide who drives the
    /// emulated pointer; they never gesture and are never a palm.
    pub fn feed_frame_with(&mut self, frame: u64, events: &[egui::Event], area: Rect, is_pen: impl Fn(u64) -> bool) -> Vec<GestureDelta> {
        if self.last_frame.replace(frame) == Some(frame) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for e in events {
            let egui::Event::Touch { id, phase, pos, .. } = e else { continue };
            let phase = TouchPhase::from(*phase);
            // egui-winit hands the pointer to a touch wherever it lands, so track it first.
            let pen = is_pen(id.0);
            self.track_pointer(id.0, phase, pen);
            if pen {
                continue;
            }
            if phase == TouchPhase::Start && !area.contains(*pos) {
                continue;
            }
            out.extend(self.update(TouchPoint { id: id.0, pos: *pos, phase }));
        }
        out
    }
}

fn centroid(a: Pos2, b: Pos2) -> Pos2 {
    Pos2::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5)
}

fn dist_angle(a: Pos2, b: Pos2) -> (f32, f32) {
    let d = b - a;
    (d.length(), d.y.atan2(d.x))
}

/// Apply `d` to `view`: zoom (clamped to the zoom range) and rotate about the gesture pivot, then
/// pan, so the document point that was under the fingers stays under them. `rect` is the canvas.
/// Returns false (and leaves the view alone) for unusable input.
pub fn apply_to_view(view: &mut View, rect: Rect, d: &GestureDelta) -> bool {
    let ok = d.zoom_factor.is_finite()
        && d.zoom_factor > 0.0
        && d.rotation_delta_rad.is_finite()
        && d.pan_delta.x.is_finite()
        && d.pan_delta.y.is_finite()
        && finite(d.pivot)
        && view.zoom.is_finite()
        && view.zoom > 0.0;
    if !ok {
        return false;
    }
    let anchor = Xf::new(rect, view).to_doc(d.pivot);
    let zoom = (view.zoom * f64::from(d.zoom_factor)).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
    let rotation = vectorcraft_geom::normalize_deg(view.rotation + f64::from(d.rotation_delta_rad).to_degrees());
    let mut next = *view;
    next.zoom = zoom;
    next.rotation = rotation;
    let next_xf = Xf::new(rect, &next);
    let target = d.pivot + d.pan_delta;
    let back = next_xf.delta_to_doc(target - rect.center());
    let center = vectorcraft_geom::Point::new(anchor.x - back.x, anchor.y - back.y);
    if !(center.x.is_finite() && center.y.is_finite() && rotation.is_finite()) {
        return false;
    }
    view.zoom = zoom;
    view.rotation = rotation;
    view.center = center;
    true
}

#[cfg(test)]
mod tests {
    use egui::{pos2, vec2};

    use super::*;

    fn tp(id: u64, x: f32, y: f32, phase: TouchPhase) -> TouchPoint {
        TouchPoint { id, pos: pos2(x, y), phase }
    }

    fn down(r: &mut GestureRecognizer, a: (f32, f32), b: (f32, f32)) {
        r.update(tp(1, a.0, a.1, TouchPhase::Start));
        r.update(tp(2, b.0, b.1, TouchPhase::Start));
    }

    fn canvas() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))
    }

    fn ev(id: u64, phase: egui::TouchPhase, x: f32, y: f32) -> egui::Event {
        egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(id), phase, pos: pos2(x, y), force: None }
    }

    #[test]
    fn a_lone_contact_gives_no_delta() {
        let mut r = GestureRecognizer::default();
        r.update(tp(1, 10.0, 10.0, TouchPhase::Start));
        assert_eq!(r.update(tp(1, 50.0, 50.0, TouchPhase::Move)), None);
        assert!(!r.is_gesturing());
    }

    #[test]
    fn two_finger_drag_pans_by_the_centroid() {
        let mut r = GestureRecognizer::default();
        down(&mut r, (100.0, 100.0), (200.0, 100.0));
        let d = r.update(tp(1, 110.0, 120.0, TouchPhase::Move));
        let d = d.expect("a move of one finger changes the centroid");
        assert_eq!(d.pivot, pos2(150.0, 100.0));
        assert_eq!(d.pan_delta, vec2(5.0, 10.0));
    }

    #[test]
    fn pinch_out_zooms_in_and_rotation_is_clockwise_positive() {
        let mut r = GestureRecognizer::default();
        down(&mut r, (100.0, 100.0), (200.0, 100.0));
        let d = r.update(tp(2, 300.0, 100.0, TouchPhase::Move)).expect("pinch");
        assert!((d.zoom_factor - 2.0).abs() < 1e-5);
        assert!(d.rotation_delta_rad.abs() < 1e-6);
        let mut r = GestureRecognizer::default();
        down(&mut r, (100.0, 100.0), (200.0, 100.0));
        // The second finger swings from the right to below: clockwise on a y-down screen.
        let d = r.update(tp(2, 100.0, 200.0, TouchPhase::Move)).expect("turn");
        assert!((d.rotation_delta_rad - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    }

    #[test]
    fn rotation_wraps_across_the_branch_cut_and_hostile_input_is_dropped() {
        let mut r = GestureRecognizer::default();
        down(&mut r, (100.0, 100.0), (0.0, 100.001));
        let d = r.update(tp(2, 0.0, 99.999, TouchPhase::Move)).expect("across 180 degrees");
        assert!(d.rotation_delta_rad.abs() < 0.01, "a tiny swing, not a full turn: {}", d.rotation_delta_rad);
        assert_eq!(r.update(tp(2, f32::NAN, 1.0, TouchPhase::Move)), None);
        let mut r = GestureRecognizer::default();
        down(&mut r, (5.0, 5.0), (5.0, 5.0));
        assert_eq!(r.update(tp(2, 5.0, 5.0, TouchPhase::Move)), None, "zero distance neither zooms nor rotates");
    }

    #[test]
    fn a_third_finger_is_ignored_and_a_lifted_one_rebases() {
        let mut r = GestureRecognizer::default();
        down(&mut r, (100.0, 100.0), (200.0, 100.0));
        r.update(tp(3, 300.0, 300.0, TouchPhase::Start));
        assert_eq!(r.update(tp(3, 400.0, 400.0, TouchPhase::Move)), None);
        r.update(tp(2, 200.0, 100.0, TouchPhase::End));
        assert!(!r.is_gesturing());
        r.update(tp(2, 500.0, 100.0, TouchPhase::Start));
        assert_eq!(r.update(tp(2, 500.0, 100.0, TouchPhase::Move)), None, "a new finger starts from where it landed");
    }

    #[test]
    fn a_palm_is_blocked_only_while_a_pen_is_near() {
        let mut r = GestureRecognizer::default();
        assert!(!r.blocks_painting(true));
        r.update(tp(1, 10.0, 10.0, TouchPhase::Start));
        assert!(!r.blocks_painting(false), "a lone finger paints without a pen");
        assert!(r.blocks_painting(true), "a lone finger beside a pen is a palm");
        let mut r = GestureRecognizer::default();
        down(&mut r, (0.0, 0.0), (90.0, 0.0));
        assert!(r.blocks_painting(false), "two fingers never paint");
    }

    #[test]
    fn feed_frame_runs_once_per_frame_and_ignores_contacts_outside_the_canvas() {
        let mut r = GestureRecognizer::default();
        let start = [ev(1, egui::TouchPhase::Start, 100.0, 100.0), ev(2, egui::TouchPhase::Start, 900.0, 100.0)];
        assert!(r.feed_frame_with(1, &start, canvas(), |_| false).is_empty());
        assert!(!r.is_gesturing(), "the second contact started outside the canvas");
        assert!(r.feed_frame_with(1, &start, canvas(), |_| false).is_empty(), "the same frame again does nothing");
    }

    #[test]
    fn a_pen_touch_is_neither_a_palm_nor_a_gesture_finger() {
        let mut r = GestureRecognizer::default();
        let events = [ev(9, egui::TouchPhase::Start, 100.0, 100.0), ev(1, egui::TouchPhase::Start, 300.0, 300.0)];
        r.feed_frame_with(1, &events, canvas(), |id| id == 9);
        assert!(!r.is_gesturing());
        assert!(!r.blocks_painting(true), "the pen owns the pointer: a resting palm doesn't block it");
        let mut r = GestureRecognizer::default();
        let palm_first = [ev(1, egui::TouchPhase::Start, 300.0, 300.0), ev(9, egui::TouchPhase::Start, 100.0, 100.0)];
        r.feed_frame_with(1, &palm_first, canvas(), |id| id == 9);
        assert!(r.blocks_painting(true), "a palm that landed first owns the pointer");
    }

    fn view() -> View {
        View { zoom: 1.0, center: vectorcraft_geom::Point::new(400.0, 300.0), ..View::default() }
    }

    fn screen_of(v: &View, doc: vectorcraft_geom::Point) -> Pos2 {
        Xf::new(canvas(), v).to_screen(doc)
    }

    #[test]
    fn applying_a_gesture_keeps_the_document_point_under_the_fingers() {
        let mut v = view();
        let pivot = pos2(250.0, 180.0);
        let anchor = Xf::new(canvas(), &v).to_doc(pivot);
        let d = GestureDelta { pan_delta: vec2(30.0, -12.0), zoom_factor: 1.8, rotation_delta_rad: 0.4, pivot };
        assert!(apply_to_view(&mut v, canvas(), &d));
        let at = screen_of(&v, anchor);
        assert!((at.x - 280.0).abs() < 1e-2 && (at.y - 168.0).abs() < 1e-2, "{at:?}");
        assert!((v.zoom - 1.8).abs() < 1e-5);
        assert!((v.rotation - 0.4_f64.to_degrees()).abs() < 1e-3);
    }

    #[test]
    fn applying_clamps_the_zoom_wraps_the_rotation_and_rejects_garbage() {
        let mut v = view();
        let d = GestureDelta { pan_delta: Vec2::ZERO, zoom_factor: 4.0, rotation_delta_rad: 3.0, pivot: pos2(400.0, 300.0) };
        for _ in 0..20 {
            assert!(apply_to_view(&mut v, canvas(), &d));
        }
        assert!(v.zoom <= ZOOM_RANGE.1 && v.rotation > -180.0 && v.rotation <= 180.0);
        let before = v;
        for bad in [
            GestureDelta { zoom_factor: f32::NAN, ..d },
            GestureDelta { pan_delta: vec2(f32::INFINITY, 0.0), ..d },
            GestureDelta { zoom_factor: 0.0, ..d },
        ] {
            assert!(!apply_to_view(&mut v, canvas(), &bad));
        }
        assert_eq!((v.zoom, v.rotation, v.center), (before.zoom, before.rotation, before.center));
    }
}
