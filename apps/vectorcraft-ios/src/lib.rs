//! Native VectorCraft iPadOS runner: the Rust static library the Xcode harness in `packaging/ios`
//! links. UIKit startup and the Metal surface lifecycle belong to winit and eframe; this crate adds
//! the single UIKit scene, safe-area insets, document pickers and the browser.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_os = "ios")]
mod app;
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod apple_entry;
mod browser;
#[cfg(any(target_os = "ios", test))]
mod documents;
#[cfg(any(target_os = "ios", test))]
mod logging;
#[cfg(any(target_os = "ios", test))]
pub mod pencil;
#[cfg(any(target_os = "ios", test))]
mod prefs;
#[cfg(target_os = "ios")]
mod safe_area;
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod scenes;
#[cfg(target_os = "ios")]
mod services;
#[cfg(any(target_os = "ios", test))]
mod storage;

/// Configure Metal explicitly; UIKit/winit supplies the current window dimensions.
pub fn native_options() -> eframe::NativeOptions {
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = eframe::wgpu::Backends::METAL;
    setup.device_descriptor = std::sync::Arc::new(|adapter| {
        let available = adapter.limits();
        let requested = eframe::wgpu::Limits { max_texture_dimension_2d: 8192, ..Default::default() };
        eframe::wgpu::DeviceDescriptor {
            label: Some("vectorcraft iOS Metal device"),
            required_limits: compatible_limits(requested, &available),
            ..Default::default()
        }
    });
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_app_id("ai.storyteller.vectorcraft").with_title("VectorCraft"),
        renderer: eframe::Renderer::Wgpu,
        run_and_return: false,
        wgpu_options: eframe::WgpuConfiguration { wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(setup), ..Default::default() },
        ..Default::default()
    }
}

/// egui uses two inter-stage locations; Metal simulators may expose 15 instead of wgpu's default
/// 16. Keep every other limit and the default feature set.
fn compatible_limits(mut requested: eframe::wgpu::Limits, available: &eframe::wgpu::Limits) -> eframe::wgpu::Limits {
    // Below two, device creation must fail rather than pretend the egui shader is supported.
    requested.max_inter_stage_shader_variables = requested.max_inter_stage_shader_variables.min(available.max_inter_stage_shader_variables).max(2);
    requested
}

/// Launch on iOS's main thread; other platforms return an error without a window.
pub fn run() -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        use vectorcraft_ui_egui::VectorcraftApp;

        if let Err(error) = logging::install() {
            eprintln!("VectorCraft logging unavailable: {error}");
        }
        log::info!("Starting VectorCraft iPadOS with Metal");
        scenes::register();
        eframe::run_native(
            "VectorCraft",
            native_options(),
            Box::new(|cc| {
                scenes::attach(cc)?;
                // Hide the status bar through the safe platform API so it cannot overlay egui's menu.
                use winit::platform::ios::WindowExtIOS;
                if let Some(window) = cc.winit_window() {
                    window.set_prefers_status_bar_hidden(true);
                }
                let (services, root) = match services::create(cc.egui_ctx.clone()) {
                    Ok(platform) => (platform.services, Some(platform.root)),
                    Err(error) => {
                        log::error!("iOS platform services unavailable: {error}");
                        (Default::default(), None)
                    }
                };
                let mut app = VectorcraftApp::new(vectorcraft_engine::Session::new(), services);
                // Apple Pencil: which touches are the pen, and its hover (winit forwards only touch force).
                if let Err(error) = pencil::install(app.pen.clone(), &cc.egui_ctx) {
                    log::error!("Apple Pencil input unavailable: {error}");
                }
                match &root {
                    Some(root) => {
                        prefs::load(&mut app, prefs::read(root), root);
                        // The user's libraries and Data Recovery copies live next to the preferences.
                        let folder = |name: &str| Some(root.join(name).to_string_lossy().to_string());
                        app.session.swatch_libraries.set_user_dir(folder("Swatches"));
                        app.session.style_libraries.set_user_dir(folder("Graphic Styles"));
                        app.session.libraries.set_dir(folder("Libraries"));
                        app.session.recovery.set_default_folder(folder("Data Recovery"));
                    }
                    None => {
                        app.status("File access and persistence could not be initialized: documents stay in this session only. Check the runner log.")
                    }
                }
                Ok(Box::new(app::IosApp::new(app, root)))
            }),
        )
        .map_err(|error| format!("VectorCraft iOS startup failed: {error}"))
    }
    #[cfg(not(target_os = "ios"))]
    {
        Err("VectorCraft iOS runner requires an iOS device or simulator".into())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn simulator_varying_limit_does_not_reduce_other_limits() {
        let requested = eframe::wgpu::Limits { max_texture_dimension_2d: 8192, max_buffer_size: 512 * 1024 * 1024, ..Default::default() };
        let available = eframe::wgpu::Limits { max_inter_stage_shader_variables: 15, ..requested.clone() };
        let actual = super::compatible_limits(requested.clone(), &available);
        assert_eq!(actual, eframe::wgpu::Limits { max_inter_stage_shader_variables: 15, ..requested });
    }

    #[test]
    fn adapters_below_egui_minimum_cannot_silently_lower_shader_requirements() {
        let available = eframe::wgpu::Limits { max_inter_stage_shader_variables: 1, ..Default::default() };
        assert_eq!(super::compatible_limits(eframe::wgpu::Limits::default(), &available).max_inter_stage_shader_variables, 2);
    }

    #[test]
    fn capable_adapters_keep_the_egui_varying_requirement() {
        let requested = eframe::wgpu::Limits::default();
        let available = eframe::wgpu::Limits { max_inter_stage_shader_variables: 32, ..requested.clone() };
        assert_eq!(super::compatible_limits(requested.clone(), &available), requested);
    }

    #[test]
    fn runner_rejects_non_ios_hosts() {
        if !cfg!(target_os = "ios") {
            assert!(super::run().unwrap_err().contains("iOS"));
        }
    }

    #[test]
    fn viewport_uses_os_size_and_only_metal() {
        let options = super::native_options();
        assert_eq!(options.renderer, eframe::Renderer::Wgpu);
        assert_eq!(options.viewport.app_id.as_deref(), Some("ai.storyteller.vectorcraft"));
        assert!(options.viewport.inner_size.is_none());
        assert!(!options.run_and_return);
        match options.wgpu_options.wgpu_setup {
            eframe::egui_wgpu::WgpuSetup::CreateNew(setup) => {
                assert_eq!(setup.instance_descriptor.backends, eframe::wgpu::Backends::METAL);
            }
            _ => panic!("runner must create its own Metal device"),
        }
    }
}
