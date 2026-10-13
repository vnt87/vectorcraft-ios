//! Audited UIKit boundary for Apple Pencil input. Main thread only; every callback only reads
//! values out of UIKit objects borrowed for the call and hands them to the safe
//! [`PencilBridge`](super::bridge::PencilBridge).
//!
//! Installed on the key `UIWindow` winit created (the window, not winit's view, so nothing here
//! depends on winit's private view class):
//!
//! - `TouchObserver`, a `UIGestureRecognizer` subclass that never recognizes and never delays or
//!   cancels touches. UIKit delivers each touch to a window's gesture recognizers before the
//!   hit-tested view, so a Pencil touch is marked as a pen before winit turns it into
//!   `WindowEvent::Touch`.
//! - A `UIHoverGestureRecognizer` limited to Pencil touches: the tip's position goes into egui's
//!   pointer (winit delivers no hover on iOS).
//!
//! UNVERIFIED until run with a Pencil: delivery order (recognizer before view), that the observer
//! never delays winit's touch ends, and that the hover recognizer's touch-type filter excludes
//! trackpad and mouse hover.

use super::bridge::PencilBridge;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{NSArray, NSNumber, NSObject, NSObjectProtocol, NSSet};
use objc2_ui_kit::{UIEvent, UIGestureRecognizer, UIGestureRecognizerState, UIHoverGestureRecognizer, UITouch, UITouchType, UIWindow};
use std::cell::RefCell;
use vectorcraft_ui_egui::pen::PenFeed;

/// Objects UIKit holds weakly (target) or that must outlive the window's use of them.
struct Installed {
    _observer: Retained<TouchObserver>,
    _hover: Retained<UIHoverGestureRecognizer>,
    _target: Retained<PencilTarget>,
}

thread_local! {
    static BRIDGE: RefCell<Option<PencilBridge>> = const { RefCell::new(None) };
    static INSTALLED: RefCell<Option<Installed>> = const { RefCell::new(None) };
}

/// Run `f` on the bridge; a re-entrant callback (UIKit calling back into us from inside one) is
/// skipped rather than panicking on the `RefCell`.
fn with_bridge<R>(f: impl FnOnce(&mut PencilBridge) -> R) -> Option<R> {
    BRIDGE.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().map(f),
        Err(_) => {
            log::warn!("Apple Pencil callback re-entered; event dropped");
            None
        }
    })
}

/// The id egui-winit gives this touch: winit 0.30.13 uses the `UITouch` address
/// (`touch as *const UITouch as u64`, `platform_impl/ios/view.rs`).
fn touch_id(touch: &UITouch) -> u64 {
    std::ptr::from_ref(touch) as usize as u64
}

#[derive(Clone, Copy)]
enum Phase {
    Began,
    Ended,
}

fn observe(touches: &NSSet<UITouch>, phase: Phase) {
    for touch in touches.iter() {
        let pencil = touch.r#type() == UITouchType::Pencil;
        let id = touch_id(&touch);
        match (phase, pencil) {
            (Phase::Began, true) => {
                with_bridge(|b| b.pen_began(id));
            }
            (Phase::Ended, true) => {
                with_bridge(PencilBridge::pen_ended);
            }
            (Phase::Began, false) => {
                with_bridge(|b| b.finger_began(id));
            }
            _ => {}
        }
    }
}

define_class!(
    // SAFETY: UIGestureRecognizer may be subclassed (UIGestureRecognizerSubclass.h); this
    // subclass adds no ivars, is main-thread only like UIKit, and every override calls `super`
    // with the arguments it received, as UIKit requires.
    #[unsafe(super(UIGestureRecognizer, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "VectorCraftPencilTouchObserver"]
    struct TouchObserver;

    impl TouchObserver {
        // SAFETY (all four): UIKit calls these on the main thread with a valid NSSet of UITouch
        // and an event that is borrowed for the duration of the call (taken as `Option` in case
        // UIKit passes nil). Nothing is retained past the call.
        #[unsafe(method(touchesBegan:withEvent:))]
        fn touches_began(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            // SAFETY: forwards the exact arguments to the superclass implementation.
            let _: () = unsafe { msg_send![super(self), touchesBegan: touches, withEvent: event] };
            observe(touches, Phase::Began);
        }

        #[unsafe(method(touchesMoved:withEvent:))]
        fn touches_moved(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            // SAFETY: as above.
            let _: () = unsafe { msg_send![super(self), touchesMoved: touches, withEvent: event] };
        }

        #[unsafe(method(touchesEnded:withEvent:))]
        fn touches_ended(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            // SAFETY: as above.
            let _: () = unsafe { msg_send![super(self), touchesEnded: touches, withEvent: event] };
            observe(touches, Phase::Ended);
            self.fail_when_idle(event);
        }

        #[unsafe(method(touchesCancelled:withEvent:))]
        fn touches_cancelled(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            // SAFETY: as above.
            let _: () = unsafe { msg_send![super(self), touchesCancelled: touches, withEvent: event] };
            observe(touches, Phase::Ended);
            self.fail_when_idle(event);
        }
    }
);

impl TouchObserver {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: `init` is UIGestureRecognizer's inherited initializer (target and action nil);
        // the class has no ivars to initialize. Runs on the main thread (`mtm`).
        let this: Retained<Self> = unsafe { msg_send![Self::alloc(mtm), init] };
        // A pure observer: never hold back or cancel the touches winit's view receives.
        this.setCancelsTouchesInView(false);
        this.setDelaysTouchesBegan(false);
        this.setDelaysTouchesEnded(false);
        this
    }

    /// Once every touch of the sequence is over, leave `Possible` so UIKit resets the recognizer
    /// for the next sequence instead of keeping it pending.
    fn fail_when_idle(&self, event: Option<&UIEvent>) {
        use objc2_ui_kit::UITouchPhase;
        let live = event
            .and_then(UIEvent::allTouches)
            .is_some_and(|all| all.iter().any(|t| !matches!(t.phase(), UITouchPhase::Ended | UITouchPhase::Cancelled)));
        if !live {
            self.setState(UIGestureRecognizerState::Failed);
        }
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no ivars; main-thread only.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "VectorCraftPencilTarget"]
    struct PencilTarget;

    impl PencilTarget {
        // SAFETY: the hover recognizer's action, `- (void)hover:(UIHoverGestureRecognizer *)`,
        // called by UIKit on the main thread with the recognizer it was registered on.
        #[unsafe(method(hover:))]
        fn hover(&self, recognizer: &UIHoverGestureRecognizer) {
            match recognizer.state() {
                UIGestureRecognizerState::Began | UIGestureRecognizerState::Changed => {
                    // `None`: window coordinates, like winit's touch positions.
                    let p = recognizer.locationInView(None);
                    with_bridge(|b| b.hover((p.x, p.y)));
                }
                _ => {
                    with_bridge(PencilBridge::hover_ended);
                }
            }
        }
    }

    // SAFETY: NSObjectProtocol has no additional invariants.
    unsafe impl NSObjectProtocol for PencilTarget {}
);

impl PencilTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: NSObject's `init` on a freshly allocated instance without ivars, main thread.
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

/// Attach Apple Pencil input to the app's window: pen touches are marked in `feed`, hover moves
/// egui's pointer through a plugin on `ctx`. Call once on the main thread after winit created the
/// window (eframe's app-creation callback). A second call is a no-op.
pub fn install(feed: PenFeed, ctx: &egui::Context) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("Apple Pencil input must be installed on the main thread")?;
    if INSTALLED.with(|i| i.borrow().is_some()) {
        return Ok(());
    }
    let window: Retained<UIWindow> = crate::scenes::window(mtm).ok_or("No UIKit window to attach Apple Pencil input to")?;
    let bridge = PencilBridge::new(feed, Some(ctx.clone()));
    ctx.add_plugin(bridge.hover_injector());
    BRIDGE.with(|cell| match cell.try_borrow_mut() {
        Ok(mut b) => {
            *b = Some(bridge);
            Ok(())
        }
        Err(_) => Err("Apple Pencil bridge is busy".to_string()),
    })?;

    let observer = TouchObserver::new(mtm);
    window.addGestureRecognizer(&observer);

    let target = PencilTarget::new(mtm);
    // SAFETY: `target` implements `hover:` with the action signature UIKit uses, and is kept
    // alive in `INSTALLED` for the app's lifetime (UIKit does not retain targets).
    let hover = unsafe { UIHoverGestureRecognizer::initWithTarget_action(UIHoverGestureRecognizer::alloc(mtm), Some(&target), Some(sel!(hover:))) };
    // Pencil hover only, not trackpad/mouse pointer hover (that is `indirectPointer`).
    hover.setAllowedTouchTypes(&NSArray::from_retained_slice(&[NSNumber::new_isize(UITouchType::Pencil.0)]));
    hover.setCancelsTouchesInView(false);
    window.addGestureRecognizer(&hover);

    INSTALLED.with(|i| {
        if let Ok(mut i) = i.try_borrow_mut() {
            *i = Some(Installed { _observer: observer, _hover: hover, _target: target });
        }
    });
    log::info!("Apple Pencil input installed (pen touches, hover)");
    Ok(())
}
