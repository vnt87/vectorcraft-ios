//! Apple Pencil input: which touches are the pen, and where it hovers.
//!
//! - [`bridge`]: platform-free state between the UIKit callbacks and VectorCraft's
//!   [`PenFeed`](vectorcraft_ui_egui::pen::PenFeed), plus the egui plugin that turns the hovering
//!   tip into pointer moves (host-tested).
//! - `native` (iOS only): the audited UIKit boundary that reads `UITouch` and
//!   `UIHoverGestureRecognizer` and calls into the bridge.
//!
//! Pressure needs none of this: winit already forwards each touch's force. Tilt, roll and the
//! double-tap are not read, because no VectorCraft tool takes them yet.

pub mod bridge;

#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod native;
#[cfg(target_os = "ios")]
pub use native::install;
