//! Sandbox-local paths: the app's support folder, bounded reads, and rebasing managed document
//! snapshots when the app container moves.
use std::{
    ffi::OsString,
    io::Read,
    path::{Component, Path, PathBuf},
};

/// Where the app keeps everything it owns, inside the app container.
const SUPPORT_NAME: &str = "ai.storyteller.vectorcraft";

fn container_prefix(prefix: &str) -> bool {
    let Some(prefix) = prefix.strip_prefix('/') else { return false };
    let parts: Vec<_> = prefix.split('/').collect();
    if parts.iter().any(|part| part.is_empty() || *part == "." || *part == "..") {
        return false;
    }
    let uuid = |value: &str| {
        value.len() == 36
            && value.bytes().enumerate().all(|(i, byte)| if matches!(i, 8 | 13 | 18 | 23) { byte == b'-' } else { byte.is_ascii_hexdigit() })
    };
    match parts.as_slice() {
        ["var", "mobile", "Containers", "Data", "Application", app] | ["private", "var", "mobile", "Containers", "Data", "Application", app] => {
            uuid(app)
        }
        ["Users", _, "Library", "Developer", "CoreSimulator", "Devices", device, "data", "Containers", "Data", "Application", app] => {
            uuid(device) && uuid(app)
        }
        _ => false,
    }
}

/// App updates may move the container. Only existing managed snapshots are relocated; external
/// paths and malformed or escaping paths keep their original meaning.
pub fn remap_managed_path(path: &str, root: &Path) -> Option<String> {
    const MARKER: &str = "/Library/Application Support/ai.storyteller.vectorcraft/documents/";
    if path.len() > 4096 || !Path::new(path).is_absolute() || path.contains(['\\', '\0']) {
        return None;
    }
    let (prefix, relative) = path.split_once(MARKER)?;
    if !container_prefix(prefix) {
        return None;
    }
    if Path::new(path).components().any(|c| !matches!(c, Component::RootDir | Component::Normal(_))) {
        return None;
    }
    // Check the raw segments too: Path::components normalizes embedded `.` and empty segments.
    let mut parts = relative.split('/');
    let (reservation, name) = (parts.next()?, parts.next()?);
    if parts.next().is_some() || [reservation, name].iter().any(|part| part.is_empty() || *part == "." || *part == "..") {
        return None;
    }
    let documents = root.join("documents");
    let destination = documents.join(reservation).join(name);
    if !std::fs::symlink_metadata(&destination).ok()?.is_file() {
        return None;
    }
    let canonical_root = root.canonicalize().ok()?;
    let canonical_documents = documents.canonicalize().ok()?;
    let canonical_destination = destination.canonicalize().ok()?;
    if !canonical_documents.starts_with(&canonical_root) || !canonical_destination.starts_with(&canonical_documents) {
        return None;
    }
    let destination = destination.to_str()?;
    (destination != path).then(|| destination.to_owned())
}

/// Rebase every recent file that is a managed snapshot in a previous container; → whether any moved.
pub fn remap_recent_files(recent: &mut [String], root: &Path) -> bool {
    let mut changed = false;
    for entry in recent {
        if let Some(path) = remap_managed_path(entry, root) {
            *entry = path;
            changed = true;
        }
    }
    changed
}

/// Resolve only the supplied sandbox HOME; never fall back to desktop configuration paths.
pub fn app_support_dir(home: Option<OsString>) -> Result<PathBuf, String> {
    let home = PathBuf::from(home.ok_or("iOS sandbox HOME is unavailable")?);
    if !home.is_absolute() || home.components().any(|c| matches!(c, Component::ParentDir)) || home == Path::new("/") {
        return Err("iOS sandbox HOME must be an absolute directory".into());
    }
    Ok(home.join("Library/Application Support").join(SUPPORT_NAME))
}

/// A regular file's bytes, refusing symlinks, non-files and anything over `limit`.
pub fn bounded_read(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > limit {
        return Err("file is not regular or exceeds its size limit".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path).map_err(|e| e.to_string())?.take(limit.saturating_add(1)).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("file exceeds its size limit".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
