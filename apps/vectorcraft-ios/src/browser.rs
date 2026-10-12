//! Opens links in the system browser (Help → website, GitHub; About and Home links).

#[cfg(target_os = "ios")]
pub fn install(services: &mut vectorcraft_ui_egui::Services) {
    services.open_url = Some(Box::new(|url| {
        if let Err(error) = native::open_url(url) {
            log::warn!("VectorCraft could not open {url}: {error}");
        }
    }));
}

#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod native {
    use objc2::MainThreadMarker;
    use objc2_foundation::{NSDictionary, NSString, NSURL};
    use objc2_ui_kit::UIApplication;

    pub(super) fn open_url(value: &str) -> Result<(), String> {
        let mtm = MainThreadMarker::new().ok_or_else(|| "UIKit URL services require the main thread".to_string())?;
        if value.is_empty() || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err("invalid URL".into());
        }
        let url = NSURL::URLWithString(&NSString::from_str(value)).ok_or_else(|| "invalid URL".to_string())?;
        if url.scheme().is_none() {
            return Err("URL requires a scheme".into());
        }
        let app = UIApplication::sharedApplication(mtm);
        if !app.canOpenURL(&url) {
            return Err("no application can open this URL".into());
        }
        let completion = block2::RcBlock::new(|success: objc2::runtime::Bool| {
            if !success.as_bool() {
                log::warn!("VectorCraft system URL request was rejected");
            }
        });
        // SAFETY: Empty NSDictionary has the declared UIKit key/value generic types. App and URL
        // remain retained for the call on the proven main thread; UIKit copies the completion
        // block for asynchronous use. It captures no objects.
        unsafe {
            app.openURL_options_completionHandler(&url, &NSDictionary::new(), Some(&completion));
        }
        // This reports accepted dispatch; UIKit reports the final outcome through the handler above.
        Ok(())
    }
}
