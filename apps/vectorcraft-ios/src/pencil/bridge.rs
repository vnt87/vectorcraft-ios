//! Platform-free Apple Pencil state. The UIKit callbacks in `native.rs` only read raw values and
//! call into a [`PencilBridge`].
//!
//! What winit (0.30) delivers on iPadOS: every `UITouch`, Pencil included, as `WindowEvent::Touch`
//! with its force, plus egui's emulated pointer that follows the first touch. It does not say which
//! touch is the pen, and it delivers no hover. The bridge adds only that: the pen's touch ids, a
//! hover flag, and the hover position as an egui pointer move ([`HoverInjector`]).

use std::sync::{Arc, Mutex, PoisonError};
use vectorcraft_ui_egui::pen::PenFeed;

/// Where the hovering pen is, for the egui pointer (UIKit window points).
#[derive(Debug, Default)]
struct HoverShared {
    /// Latest hover position not yet handed to egui.
    pending: Option<(f32, f32)>,
    /// The hover ended (or turned into a touch): egui's pointer should go.
    gone: bool,
}

/// Owns the pen state between UIKit callbacks.
pub struct PencilBridge {
    feed: PenFeed,
    /// A pencil touch is down (hover callbacks are ignored meanwhile).
    touching: bool,
    hover: Arc<Mutex<HoverShared>>,
    repaint: Option<egui::Context>,
}

impl PencilBridge {
    pub fn new(feed: PenFeed, repaint: Option<egui::Context>) -> Self {
        Self { feed, touching: false, hover: Arc::default(), repaint }
    }

    /// The egui plugin that turns hover positions into pointer moves.
    pub fn hover_injector(&self) -> HoverInjector {
        HoverInjector { shared: Arc::clone(&self.hover) }
    }

    fn wake(&self) {
        if let Some(ctx) = &self.repaint {
            ctx.request_repaint();
        }
    }

    fn hover_gone(&self) {
        let mut h = self.hover.lock().unwrap_or_else(PoisonError::into_inner);
        h.pending = None;
        h.gone = true;
    }

    /// A Pencil touch began (`id`: the id egui will see, i.e. winit's `UITouch` address).
    pub fn pen_began(&mut self, id: u64) {
        self.feed.mark_pen_touch(id);
        self.feed.set_hovering(false);
        self.touching = true;
        self.hover_gone();
    }

    /// The Pencil touch ended or was cancelled. Its id stays marked: egui handles the end a frame
    /// later and must still see a pen.
    pub fn pen_ended(&mut self) {
        self.touching = false;
    }

    /// A finger touch began: UIKit may have reused a pen touch's address for it.
    pub fn finger_began(&mut self, id: u64) {
        self.feed.mark_finger_touch(id);
    }

    /// The Pencil hovers at `pos` (window points).
    pub fn hover(&mut self, pos: (f64, f64)) {
        if self.touching {
            return;
        }
        self.feed.set_hovering(true);
        if pos.0.is_finite() && pos.1.is_finite() {
            let mut h = self.hover.lock().unwrap_or_else(PoisonError::into_inner);
            h.pending = Some((pos.0 as f32, pos.1 as f32));
            h.gone = false;
        }
        self.wake();
    }

    /// The Pencil stopped hovering (left the range, or touched down).
    pub fn hover_ended(&mut self) {
        if self.touching {
            return;
        }
        self.feed.set_hovering(false);
        self.hover_gone();
        self.wake();
    }
}

/// egui plugin: turns the hovering pen's position into `PointerMoved` / `PointerGone`, so the tool
/// cursor follows the floating tip (winit delivers no hover on iOS).
pub struct HoverInjector {
    shared: Arc<Mutex<HoverShared>>,
}

impl HoverInjector {
    /// Append the pending hover events to `events`. `zoom` is egui's zoom factor: UIKit window
    /// points are egui points at zoom 1. A frame where a touch is down or moving keeps the touch's
    /// pointer: the hover is dropped (it would fight the stroke).
    pub fn inject(&self, events: &mut Vec<egui::Event>, zoom: f32) {
        let mut h = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        let touching = events.iter().any(|e| matches!(e, egui::Event::Touch { phase: egui::TouchPhase::Start | egui::TouchPhase::Move, .. }));
        let pending = h.pending.take();
        if std::mem::take(&mut h.gone) && !touching {
            events.push(egui::Event::PointerGone);
        }
        if touching {
            return;
        }
        let zoom = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
        if let Some((x, y)) = pending {
            let p = egui::pos2(x / zoom, y / zoom);
            if p.x.is_finite() && p.y.is_finite() {
                events.push(egui::Event::PointerMoved(p));
            }
        }
    }
}

impl egui::Plugin for HoverInjector {
    fn debug_name(&self) -> &'static str {
        "vectorcraft-ios pencil hover"
    }

    fn input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        self.inject(&mut input.events, ctx.zoom_factor());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge() -> (PencilBridge, PenFeed) {
        let feed = PenFeed::default();
        (PencilBridge::new(feed.clone(), None), feed)
    }

    fn touch_start() -> egui::Event {
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(1),
            phase: egui::TouchPhase::Start,
            pos: egui::pos2(0.0, 0.0),
            force: None,
        }
    }

    #[test]
    fn a_touch_marks_the_pen_and_ends_the_hover() {
        let (mut b, feed) = bridge();
        b.hover((1.0, 2.0));
        assert!(feed.hovering());
        b.pen_began(42);
        assert!(feed.is_pen_touch(42) && !feed.hovering());
        b.hover((5.0, 5.0));
        assert!(!feed.hovering(), "a late hover during the touch changes nothing");
        b.pen_ended();
        assert!(feed.is_pen_touch(42), "egui still has to see the end as a pen");
        b.finger_began(42);
        assert!(!feed.is_pen_touch(42), "a finger reusing the address is a finger");
    }

    #[test]
    fn hover_moves_the_pointer_and_its_end_removes_it_once() {
        let (mut b, feed) = bridge();
        let inj = b.hover_injector();
        b.hover((100.0, 50.0));
        let mut ev = Vec::new();
        inj.inject(&mut ev, 2.0);
        assert_eq!(ev, vec![egui::Event::PointerMoved(egui::pos2(50.0, 25.0))]);
        b.hover_ended();
        assert!(!feed.hovering());
        let mut ev = Vec::new();
        inj.inject(&mut ev, 1.0);
        assert_eq!(ev, vec![egui::Event::PointerGone]);
        let mut ev = Vec::new();
        inj.inject(&mut ev, 1.0);
        assert!(ev.is_empty(), "reported once");
    }

    #[test]
    fn the_injector_skips_touch_frames_and_hostile_values() {
        let (mut b, _) = bridge();
        let inj = b.hover_injector();
        b.hover((f64::NAN, 3.0));
        let mut ev = Vec::new();
        inj.inject(&mut ev, 1.0);
        assert!(ev.is_empty(), "no position, no move");
        b.hover((10.0, 20.0));
        let mut ev = vec![touch_start()];
        inj.inject(&mut ev, 1.0);
        assert_eq!(ev, vec![touch_start()], "a touch keeps the pointer");
        b.hover((10.0, 20.0));
        let mut ev = Vec::new();
        inj.inject(&mut ev, f32::NAN);
        assert_eq!(ev, vec![egui::Event::PointerMoved(egui::pos2(10.0, 20.0))], "a bad zoom falls back to 1");
    }
}
