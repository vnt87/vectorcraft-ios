//! The saved UI preferences (`ui.json` in the app's support folder), read before the window opens
//! and written when the app quits or goes to the background. Mirrors the desktop app's file, so a
//! preferences file means the same thing on every platform.
use std::path::{Path, PathBuf};

use vectorcraft_engine::cmd::fileio;
use vectorcraft_ui_egui::{UiState, VectorcraftApp};

/// Preferences are small; anything bigger is not one of ours.
const MAX_PREFS: u64 = 4 * 1024 * 1024;

pub fn path(root: &Path) -> PathBuf {
    root.join("ui.json")
}

/// Recover only optional persisted docking metadata. Runtime state and command decoding remain
/// strict, and an unreadable workspace layout cannot discard unrelated preferences.
pub fn decode(bytes: &[u8]) -> Option<UiState> {
    use serde_json::{Map, Value, json};

    fn recover(object: &mut Map<String, Value>, hidden_key: &str) {
        if let Some(layout) = object.get("docking")
            && serde_json::from_value::<UiState>(json!({"docking": layout})).is_err()
        {
            object.remove("docking");
        }
        if let Some(hidden) = object.get_mut(hidden_key) {
            if let Value::Object(entries) = hidden {
                entries.retain(|panel, location| serde_json::from_value::<UiState>(json!({"docking_hidden": {panel: location}})).is_ok());
            } else {
                object.remove(hidden_key);
            }
        }
    }

    let mut value: Value = serde_json::from_slice(bytes).ok()?;
    let object = value.as_object_mut()?;
    recover(object, "docking_hidden");
    if let Some(Value::Array(workspaces)) = object.get_mut("custom_workspaces") {
        for workspace in workspaces {
            if let Some(object) = workspace.as_object_mut() {
                recover(object, "dockingHidden");
            }
        }
    }
    serde_json::from_value(value).ok()
}

/// The saved preferences, if there are readable ones.
pub fn read(root: &Path) -> Option<UiState> {
    decode(&crate::storage::bounded_read(&path(root), MAX_PREFS).ok()?)
}

/// Put the saved preferences into `app`; recent files that live in a moved container are rebased.
pub fn load(app: &mut VectorcraftApp, saved: Option<UiState>, root: &Path) {
    if let Some(ui) = saved {
        app.ui = ui.sanitized();
        crate::storage::remap_recent_files(&mut app.ui.recent_files, root);
    }
    vectorcraft_ui_egui::prefs_dialog::restore(app);
}

/// Write `app`'s preferences; a failed write keeps the previous file.
pub fn save(app: &VectorcraftApp, root: &Path) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let mut ui = app.ui.clone();
    ui.engine_prefs = app.session.prefs.to_json();
    let bytes = serde_json::to_vec_pretty(&ui).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_PREFS {
        return Err("preferences exceed their size limit".into());
    }
    fileio::write_atomic(&path(root), &bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectorcraft_engine::Session;

    fn temp(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("vectorcraft-ios-prefs-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn preferences_survive_a_relaunch() {
        let root = temp("relaunch");
        let mut app = VectorcraftApp::new(Session::new(), Default::default());
        app.ui.recent_files = vec!["/external/poster.svg".into()];
        save(&app, &root).unwrap();
        let mut again = VectorcraftApp::new(Session::new(), Default::default());
        load(&mut again, read(&root), &root);
        assert_eq!(again.ui.recent_files, ["/external/poster.svg"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unreadable_preferences_are_ignored_not_fatal() {
        assert!(decode(b"{broken JSON").is_none());
        assert!(decode(b"[]").is_none());
        let root = temp("unreadable");
        std::fs::write(path(&root), b"not json").unwrap();
        assert!(read(&root).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
