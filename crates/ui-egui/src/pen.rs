//! Which touches are an Apple Pencil, and whether one hovers. The platform runner (iPadOS) writes
//! this; the canvas reads it so a pen is never taken for a finger: it neither pans the view nor
//! counts as a palm, and a resting palm doesn't paint while the pen is near.
//!
//! Pressure needs nothing here: winit hands every touch's force to egui as `Event::Touch { force }`.
//! Tilt and barrel roll are not read: no tool of the engine takes them yet.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

/// How many pen touch ids are remembered: UIKit reuses addresses, so old ones are dropped.
const MAX_IDS: usize = 64;

#[derive(Debug, Default)]
struct Inner {
    pen_touches: HashSet<u64>,
    hovering: bool,
}

/// A shared handle on the pen state; clones see the same state.
#[derive(Clone, Debug, Default)]
pub struct PenFeed(Arc<Mutex<Inner>>);

impl PenFeed {
    fn with<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Touch `id` is the pen. It stays marked after it lifts: egui reads the end a frame later.
    pub fn mark_pen_touch(&self, id: u64) {
        self.with(|s| {
            if s.pen_touches.len() >= MAX_IDS {
                s.pen_touches.clear();
            }
            s.pen_touches.insert(id);
        });
    }

    /// A finger landed on `id`: UIKit may have reused a pen touch's address for it.
    pub fn mark_finger_touch(&self, id: u64) {
        self.with(|s| {
            s.pen_touches.remove(&id);
        });
    }

    pub fn is_pen_touch(&self, id: u64) -> bool {
        self.with(|s| s.pen_touches.contains(&id))
    }

    /// The pen floats above the glass.
    pub fn set_hovering(&self, hovering: bool) {
        self.with(|s| s.hovering = hovering);
    }

    pub fn hovering(&self) -> bool {
        self.with(|s| s.hovering)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pen_touch_is_marked_until_a_finger_reuses_its_id() {
        let feed = PenFeed::default();
        let other = feed.clone();
        feed.mark_pen_touch(7);
        assert!(other.is_pen_touch(7), "clones share state");
        assert!(!other.is_pen_touch(8));
        other.mark_finger_touch(7);
        assert!(!feed.is_pen_touch(7));
    }

    #[test]
    fn hover_is_a_flag_and_the_id_set_stays_bounded() {
        let feed = PenFeed::default();
        assert!(!feed.hovering());
        feed.set_hovering(true);
        assert!(feed.hovering());
        for id in 0..(MAX_IDS as u64 * 3) {
            feed.mark_pen_touch(id);
        }
        assert!(feed.is_pen_touch(MAX_IDS as u64 * 3 - 1));
        assert!(feed.with(|s| s.pen_touches.len()) <= MAX_IDS);
    }
}
