//! Safe-area handling for the iPad window.
//!
//! egui lays the UI out in `content_rect`, which is the full view minus the safe-area insets, and
//! winit sizes the Metal surface to the safe area. egui-winit reads the insets from a foreground
//! scene's key window only on resize, scale, focus and occlusion events, and reports zero before that
//! window exists (launch, scene activation). UIKit's window is the source of truth, so [`apply`] sets
//! the insets from it on every frame, and [`report`] logs egui's and UIKit's numbers when they change.

use eframe::egui;
use objc2_foundation::MainThreadMarker;

/// Set the safe-area insets egui lays out with from UIKit's window.
pub fn apply(raw_input: &mut egui::RawInput) {
    if let Some([_, _, top, left, bottom, right]) = MainThreadMarker::new().and_then(crate::scenes::metrics) {
        raw_input.safe_area_insets =
            Some(egui::SafeAreaInsets(egui::epaint::MarginF32 { top: top as f32, left: left as f32, bottom: bottom as f32, right: right as f32 }));
    }
}

/// Log what egui lays out with next to UIKit's window metrics, but only when they change.
pub fn report(ctx: &egui::Context, last: &mut Option<String>) {
    let (screen, insets, content) = ctx.input(|i| (i.viewport_rect(), i.safe_area_insets().0, i.content_rect()));
    let line = match MainThreadMarker::new().and_then(crate::scenes::metrics) {
        Some([w, h, top, left, bottom, right]) => format!(
            "egui screen {:.0}x{:.0} insets t{:.0} l{:.0} b{:.0} r{:.0} content {:.0}x{:.0}; UIWindow bounds {w:.0}x{h:.0} insets t{top:.0} l{left:.0} b{bottom:.0} r{right:.0}; surface {:.0}x{:.0}",
            screen.width(),
            screen.height(),
            insets.top,
            insets.left,
            insets.bottom,
            insets.right,
            content.width(),
            content.height(),
            w - left - right,
            h - top - bottom,
        ),
        None => format!("egui screen {:.0}x{:.0}; UIKit metrics unavailable", screen.width(), screen.height()),
    };
    if last.as_deref() != Some(line.as_str()) {
        log::info!("safe area: {line}");
        *last = Some(line);
    }
}
