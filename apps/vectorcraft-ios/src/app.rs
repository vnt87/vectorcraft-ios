//! The app eframe runs on iPadOS: VectorCraft's UI plus what the host owns (safe area, saving the
//! preferences when the app leaves the foreground).
use std::path::PathBuf;

use eframe::egui;
use vectorcraft_ui_egui::VectorcraftApp;

pub struct IosApp {
    app: VectorcraftApp,
    /// The app's support folder, where the preferences live (none: nowhere to keep them).
    root: Option<PathBuf>,
    was_focused: bool,
    last_safe_area: Option<String>,
    /// Hardware-keyboard state: the keys winit never delivers (Enter, arrows, ⌘ shortcuts).
    keys: crate::hw_keys::HwKeys,
    /// Events produced outside the input hook (releasing keys on focus loss), sent with the next frame.
    pending: Vec<egui::Event>,
}

impl IosApp {
    pub fn new(app: VectorcraftApp, root: Option<PathBuf>) -> Self {
        Self { app, root, was_focused: true, last_safe_area: None, keys: Default::default(), pending: Vec::new() }
    }

    fn save_prefs(&self) {
        if let Some(root) = &self.root
            && let Err(error) = crate::prefs::save(&self.app, root)
        {
            log::warn!("Could not save the preferences: {error}");
        }
    }
}

impl eframe::App for IosApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        crate::safe_area::report(ctx, &mut self.last_safe_area);
        // iPadOS may end a backgrounded app without a word: keep the preferences current.
        let focused = ctx.input(|i| i.focused);
        if self.was_focused && !focused {
            self.save_prefs();
        }
        if !focused {
            // Key-ups may never arrive once the app is in the background.
            self.pending.extend(self.keys.release_all());
        }
        self.was_focused = focused;
        self.app.logic(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // The keys winit's UIKit view never forwards: read the HID transitions and inject them.
        let now = raw_input.time.unwrap_or_default();
        for (code, pressed) in crate::gc_keyboard::drain() {
            raw_input.events.extend(self.keys.handle(code, pressed, now));
        }
        raw_input.events.append(&mut self.pending);
        raw_input.events.extend(self.keys.tick(now));
        if let Some(wait) = self.keys.next_wake(now) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait.max(0.001)));
        }
        self.app.raw_input_hook(ctx, raw_input);
        crate::safe_area::apply(raw_input);
    }

    fn on_exit(&mut self) {
        self.save_prefs();
    }
}
