use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

fn temp() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!("vectorcraft-ios-storage-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn managed_snapshot(root: &Path) -> (String, String) {
    let current = root.join("documents/reservation/poster.vectorcraft");
    std::fs::create_dir_all(current.parent().unwrap()).unwrap();
    std::fs::write(&current, b"managed snapshot").unwrap();
    let old = "/var/mobile/Containers/Data/Application/11111111-2222-3333-4444-555555555555/Library/Application Support/ai.storyteller.vectorcraft/documents/reservation/poster.vectorcraft".to_string();
    (old, current.to_string_lossy().into_owned())
}

#[test]
fn sandbox_home_never_defaults_to_ambient_configuration() {
    assert!(app_support_dir(None).is_err());
    assert!(app_support_dir(Some("relative".into())).is_err());
    assert!(app_support_dir(Some("/sandbox/../outside".into())).is_err());
    assert!(app_support_dir(Some("/".into())).is_err());
    assert_eq!(app_support_dir(Some("/sandbox".into())).unwrap(), Path::new("/sandbox/Library/Application Support/ai.storyteller.vectorcraft"));
}

#[test]
fn moved_container_rebases_a_matching_recent_snapshot_and_leaves_external_files_alone() {
    let root = temp();
    let (old, current) = managed_snapshot(&root);
    let mut recent = vec![old, "/external/poster.svg".to_string()];
    assert!(remap_recent_files(&mut recent, &root));
    assert_eq!(recent, [current, "/external/poster.svg".to_string()]);
    assert!(!remap_recent_files(&mut recent, &root), "already current: nothing moves");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unrelated_missing_and_traversal_paths_are_never_rebased() {
    let root = temp();
    managed_snapshot(&root);
    let app = "/var/mobile/Containers/Data/Application/11111111-2222-3333-4444-555555555555/Library/Application Support/ai.storyteller.vectorcraft/documents";
    for path in [
        "/external/poster.svg".to_string(),
        format!("{app}/missing/poster.vectorcraft"),
        format!("{app}/reservation/../poster.vectorcraft"),
        format!("{app}/reservation//poster.vectorcraft"),
        "relative/Library/Application Support/ai.storyteller.vectorcraft/documents/reservation/poster.vectorcraft".to_string(),
        "/Volumes/backup/Library/Application Support/ai.storyteller.vectorcraft/documents/reservation/poster.vectorcraft".to_string(),
    ] {
        assert_eq!(remap_managed_path(&path, &root), None, "{path}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn a_symlink_out_of_the_documents_folder_does_not_rebase() {
    let root = temp();
    let outside = temp();
    std::fs::write(outside.join("poster.vectorcraft"), b"outside").unwrap();
    std::fs::create_dir_all(root.join("documents")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("documents/reservation")).unwrap();
    let old = "/var/mobile/Containers/Data/Application/11111111-2222-3333-4444-555555555555/Library/Application Support/ai.storyteller.vectorcraft/documents/reservation/poster.vectorcraft";
    assert_eq!(remap_managed_path(old, &root), None);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(outside).unwrap();
}

#[test]
fn physical_and_simulator_container_prefixes_require_valid_uuids() {
    let root = temp();
    let (_, current) = managed_snapshot(&root);
    let app = "11111111-2222-3333-4444-555555555555";
    let device = "AAAA1111-BBBB-2222-CCCC-333333333333";
    let tail = "Library/Application Support/ai.storyteller.vectorcraft/documents/reservation/poster.vectorcraft";
    for prefix in [
        format!("/var/mobile/Containers/Data/Application/{app}"),
        format!("/private/var/mobile/Containers/Data/Application/{app}"),
        format!("/Users/owner/Library/Developer/CoreSimulator/Devices/{device}/data/Containers/Data/Application/{app}"),
    ] {
        assert_eq!(remap_managed_path(&format!("{prefix}/{tail}"), &root), Some(current.clone()));
    }
    for prefix in [
        "/var/mobile/Containers/Data/Application/not-a-uuid",
        "/var/mobile/Containers/Data/Application/11111111-2222-3333-4444-55555555555Z",
        "/Users/owner/Library/Developer/CoreSimulator/Devices/not-a-uuid/data/Containers/Data/Application/11111111-2222-3333-4444-555555555555",
    ] {
        assert_eq!(remap_managed_path(&format!("{prefix}/{tail}"), &root), None);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_reads_refuse_oversized_files_and_non_files() {
    let root = temp();
    let file = root.join("a.bin");
    std::fs::write(&file, b"12345").unwrap();
    assert_eq!(bounded_read(&file, 5).unwrap(), b"12345");
    assert!(bounded_read(&file, 4).is_err());
    assert!(bounded_read(&root, 1024).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
