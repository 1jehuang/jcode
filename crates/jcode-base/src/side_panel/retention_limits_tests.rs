use super::*;
use crate::side_panel::side_panel_tests::EnvVarGuard;

#[test]
fn side_panel_history_disk_cap_prunes_owned_bodies_without_loading_archives() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let mut first_path = String::new();
    for id in ["first", "second"] {
        let page = write_markdown_page("disk", id, None, "small fixture", false)
            .unwrap()
            .pages
            .remove(0);
        // Sparse synthetic bodies exercise the logical-byte cap without allocating 130 MiB.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&page.file_path)
            .unwrap()
            .set_len(MAX_ARCHIVED_BYTES / 2 + 1)
            .unwrap();
        if id == "first" {
            first_path = page.file_path;
        }
        archive_page("disk", id).unwrap();
    }
    let entries = history("disk").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, "second");
    assert!(!Path::new(&first_path).exists());
    assert!(
        !serde_json::to_string(&entries)
            .unwrap()
            .contains("small fixture")
    );
}

#[test]
fn side_panel_refresh_failure_preserves_snapshot_and_revision() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let mut snapshot = write_markdown_page("unavailable", "doc", None, "saved", true).unwrap();
    let old = snapshot.clone();
    std::fs::write(
        temp.path().join("side_panel/unavailable/index.json"),
        "invalid",
    )
    .unwrap();
    assert!(refresh_linked_page_content(&mut snapshot, None));
    assert_eq!(snapshot.pages, old.pages);
    assert_eq!(snapshot.revision, old.revision);
    assert!(snapshot.unavailable.is_some());
    assert!(!snapshot_is_empty(&snapshot));
    let unavailable = snapshot_for_session_or_unavailable("unavailable");
    assert_eq!(unavailable.session_id.as_deref(), Some("unavailable"));
    assert_eq!(unavailable.revision, 0);
    assert!(unavailable.unavailable.is_some());
    assert!(!snapshot_is_empty(&unavailable));
}

#[test]
fn side_panel_concurrent_mutations_keep_inventory_and_revisions() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    std::thread::scope(|scope| {
        for i in 0..12 {
            scope.spawn(move || {
                write_markdown_page("concurrent", &format!("p{i}"), None, "body", false).unwrap();
            });
        }
    });
    let snapshot = snapshot_for_session("concurrent").unwrap();
    assert_eq!(snapshot.pages.len(), 12);
    assert_eq!(snapshot.revision, 12);
    assert_eq!(snapshot.session_id.as_deref(), Some("concurrent"));
    assert!(snapshot_for_session("../escape").is_err());
}
