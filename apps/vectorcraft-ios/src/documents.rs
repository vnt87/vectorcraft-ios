//! Managed document identities and asynchronous UIKit document services.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DOCUMENT: AtomicU64 = AtomicU64::new(0);
const MAX_DOCUMENT_BYTES: u64 = 512 * 1024 * 1024;

/// Keep the original extension (it selects the encoder), but never accept directories.
fn file_name(suggested: &str) -> Result<String, String> {
    let name = Path::new(suggested).file_name().and_then(|s| s.to_str()).filter(|s| !s.is_empty()).ok_or("The document needs a file name")?;
    if matches!(name, "." | "..") || name.contains('\0') {
        return Err("Invalid document name".into());
    }
    Ok(name.to_owned())
}

fn reserve_path(root: &Path, suggested: &str) -> Result<PathBuf, String> {
    let name = file_name(suggested)?;
    let documents = root.join("documents");
    std::fs::create_dir_all(&documents).map_err(|e| e.to_string())?;
    for _ in 0..128 {
        let sequence = NEXT_DOCUMENT.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
        let dir = documents.join(format!("{nanos}-{sequence}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir.join(name)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("Could not reserve a document identity".into())
}

/// Each import batch owns its newly created directories until all files are ready.
/// Dropping an unsuccessful batch removes snapshots and bookmark metadata together.
#[derive(Default)]
struct ReservationBatch {
    directories: Vec<PathBuf>,
    committed: bool,
}
impl ReservationBatch {
    fn reserve(&mut self, root: &Path, suggested: &str) -> Result<PathBuf, String> {
        let path = reserve_path(root, suggested)?;
        // reserve_path always creates a fresh parent before returning the file identity.
        if let Some(parent) = path.parent() {
            self.directories.push(parent.to_owned());
        }
        Ok(path)
    }
    fn commit(mut self) {
        self.committed = true;
    }
}
impl Drop for ReservationBatch {
    fn drop(&mut self) {
        if !self.committed {
            for directory in &self.directories {
                // These exact fresh directories belong to this batch, never preexisting files.
                let _ = std::fs::remove_dir_all(directory);
            }
        }
    }
}

fn bounded_read(path: &Path) -> Result<Vec<u8>, String> {
    bounded_read_limit(path, MAX_DOCUMENT_BYTES)
}

fn bounded_read_limit(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > limit {
        return Err("File exceeds the allowed import size".into());
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("File exceeds the allowed import size".into());
    }
    Ok(bytes)
}

fn validate_managed_path(root: &Path, path: &Path) -> Result<(), String> {
    let documents = std::fs::canonicalize(root.join("documents")).map_err(|e| e.to_string())?;
    let parent = path.parent().ok_or("Missing managed document directory")?;
    let parent = std::fs::canonicalize(parent).map_err(|e| e.to_string())?;
    if parent.parent() != Some(documents.as_path()) || path.file_name().is_none() || path.is_symlink() {
        return Err("Use Save As to select an iPadOS document destination".into());
    }
    Ok(())
}

#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod native;
#[cfg(target_os = "ios")]
pub use native::install;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_keep_encoder_extension_and_strip_directories() {
        assert_eq!(file_name("/external/保存.PSD").expect("name"), "保存.PSD");
        assert!(file_name("/").is_err());
        assert!(file_name("bad\0.svg").is_err());
    }
    #[test]
    fn reservations_never_create_empty_export_files() {
        let root = std::env::temp_dir().join(format!("vectorcraft-documents-test-{}", std::process::id()));
        let first = reserve_path(&root, "picture.svg").expect("first reservation");
        let second = reserve_path(&root, "picture.svg").expect("second reservation");
        assert_ne!(first, second);
        assert!(!first.exists());
        std::fs::write(&first, b"real file bytes").expect("write fixture");
        assert_eq!(bounded_read(&first).expect("bounded read"), b"real file bytes");
        std::fs::remove_dir_all(root).expect("cleanup");
    }
    #[test]
    fn oversized_documents_are_rejected_without_allocating_the_payload() {
        let root = std::env::temp_dir().join(format!("vectorcraft-document-limit-test-{}", std::process::id()));
        let path = reserve_path(&root, "large.svg").expect("reservation");
        std::fs::File::create(&path).expect("sparse fixture").set_len(MAX_DOCUMENT_BYTES + 1).expect("sparse length");
        assert!(bounded_read(&path).is_err());
        assert!(validate_managed_path(&root, &path).is_ok());
        assert!(validate_managed_path(&root, &root.join("documents/../prefs.json")).is_err());
        std::fs::remove_dir_all(root).expect("cleanup");
    }
    #[test]
    fn failed_second_import_rolls_back_completed_and_current_reservations() {
        let root = std::env::temp_dir().join(format!("vectorcraft-import-rollback-test-{}", std::process::id()));
        let mut paths = Vec::new();
        let result = (|| -> Result<(), String> {
            let mut batch = ReservationBatch::default();
            let first = batch.reserve(&root, "first.svg")?;
            paths.push(first.clone());
            std::fs::write(&first, b"completed snapshot").map_err(|e| e.to_string())?;
            let second = batch.reserve(&root, "second.svg")?;
            paths.push(second.clone());
            std::fs::create_dir(second.parent().ok_or("parent")?.join("metadata")).map_err(|e| e.to_string())?;
            Err("synthetic bookmark or snapshot failure".into())
        })();
        assert!(result.is_err());
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().all(|path| path.parent().is_some_and(|parent| !parent.exists())));
        std::fs::remove_dir_all(root).expect("cleanup");
    }
    #[test]
    fn successful_import_batch_preserves_every_reservation() {
        let root = std::env::temp_dir().join(format!("vectorcraft-import-commit-test-{}", std::process::id()));
        let mut batch = ReservationBatch::default();
        let first = batch.reserve(&root, "first.svg").expect("first");
        let second = batch.reserve(&root, "second.svg").expect("second");
        std::fs::write(&first, b"first snapshot").expect("first snapshot");
        std::fs::write(&second, b"second snapshot").expect("second snapshot");
        batch.commit();
        assert!(first.is_file() && second.is_file());
        std::fs::remove_dir_all(root).expect("cleanup");
    }
}
