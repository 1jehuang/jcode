use super::*;
use crate::side_panel::side_panel_tests::EnvVarGuard;

#[test]
fn side_panel_recovered_managed_append_preserves_owned_prefix() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    write_markdown_page("origin", "doc", None, "original prefix", true).unwrap();
    inherit_for_recovery("origin", "recovered").unwrap();
    purge_page("origin", "doc").unwrap();
    let appended = append_markdown_page("recovered", "doc", None, "new suffix", false).unwrap();
    assert_eq!(appended.pages[0].content, "original prefix\nnew suffix");
}

#[test]
fn side_panel_backup_index_keeps_matching_last_good_cache() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let source = temp.path().join("source.md");
    std::fs::write(&source, "first saved body").unwrap();
    load_file("backup", "doc", None, &source, true).unwrap();
    std::fs::write(&source, "second saved body is longer").unwrap();
    snapshot_for_session("backup").unwrap();
    std::fs::remove_file(&source).unwrap();
    std::fs::write(temp.path().join("side_panel/backup/index.json"), "corrupt").unwrap();
    let recovered = snapshot_for_session("backup").unwrap();
    assert_eq!(recovered.pages[0].content, "first saved body");
    assert!(recovered.pages[0].status.stale_reason.is_some());
}

#[test]
fn side_panel_successful_purge_cannot_return_through_backup_index() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    write_markdown_page("purged", "doc", None, "permanently removed", true).unwrap();
    purge_page("purged", "doc").unwrap();
    std::fs::write(temp.path().join("side_panel/purged/index.json"), "corrupt").unwrap();
    let recovered = snapshot_for_session("purged").unwrap();
    assert!(recovered.pages.is_empty());
    assert!(recovered.archived.is_empty());
}

#[test]
fn side_panel_retry_clears_stale_when_original_source_metadata_returns() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let source = temp.path().join("source.md");
    let displaced = temp.path().join("displaced.md");
    std::fs::write(&source, "saved").unwrap();
    let original = load_file("retry", "doc", None, &source, true).unwrap();
    std::fs::rename(&source, &displaced).unwrap();
    assert!(
        snapshot_for_session("retry").unwrap().pages[0]
            .status
            .stale_reason
            .is_some()
    );
    std::fs::rename(&displaced, &source).unwrap();
    let retried = snapshot_for_session("retry").unwrap();
    assert_eq!(
        retried.pages[0].status.source_revision,
        original.pages[0].status.source_revision
    );
    assert!(retried.pages[0].status.stale_reason.is_none());
}

#[test]
fn side_panel_explicit_refresh_reads_same_metadata_revision() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let source = temp.path().join("source.md");
    std::fs::write(&source, "old body").unwrap();
    let modified = std::fs::metadata(&source).unwrap().modified().unwrap();
    let mut snapshot = load_file("force", "doc", None, &source, true).unwrap();
    std::fs::write(&source, "new body").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(
        snapshot_for_session("force").unwrap().pages[0].content,
        "old body"
    );
    assert!(refresh_linked_page_content(&mut snapshot, Some("doc")));
    assert_eq!(snapshot.pages[0].content, "new body");
}

#[test]
fn side_panel_default_metadata_preserves_legacy_wire_shape() {
    let snapshot = serde_json::to_value(SidePanelSnapshot::default()).unwrap();
    assert_eq!(
        snapshot,
        serde_json::json!({"focus_revision":0,"focused_page_id":null})
    );
    let page = serde_json::to_value(SidePanelPage::default()).unwrap();
    assert!(page.get("status").is_none());
    let snapshot = SidePanelSnapshot {
        session_id: Some("real-session".into()),
        revision: 7,
        unavailable: Some("unavailable".into()),
        ..Default::default()
    };
    let encoded = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(encoded["session_id"], "real-session");
    assert_eq!(encoded["revision"], 7);
    assert_eq!(encoded["unavailable"], "unavailable");
    assert_eq!(
        serde_json::from_value::<SidePanelSnapshot>(encoded).unwrap(),
        snapshot
    );
}
