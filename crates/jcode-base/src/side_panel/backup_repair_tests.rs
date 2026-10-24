use super::*;
use crate::side_panel::side_panel_tests::EnvVarGuard;

#[test]
fn side_panel_healthy_primary_repairs_corrupt_redundant_backup_on_write() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    write_markdown_page("repair", "doc", None, "one", true).unwrap();
    write_markdown_page("repair", "doc", None, "two", false).unwrap();
    let backup = temp.path().join("side_panel/repair/index.bak");
    std::fs::write(&backup, "corrupt redundant backup").unwrap();
    let updated = write_markdown_page("repair", "doc", None, "three", false).unwrap();
    assert_eq!(updated.pages[0].content, "three");
    assert!(
        serde_json::from_slice::<PersistedSidePanelState>(&std::fs::read(&backup).unwrap()).is_ok()
    );
    std::fs::write(&backup, "corrupt before focus").unwrap();
    focus_page("repair", "doc").unwrap();
    std::fs::write(&backup, "corrupt before archive").unwrap();
    archive_page("repair", "doc").unwrap();
}

#[test]
fn side_panel_missing_primary_recovers_valid_backup_instead_of_empty_inventory() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    write_markdown_page("missing-primary", "doc", None, "saved body", true).unwrap();
    focus_page("missing-primary", "doc").unwrap();
    let primary = temp.path().join("side_panel/missing-primary/index.json");
    std::fs::remove_file(&primary).unwrap();
    let recovered = snapshot_for_session("missing-primary").unwrap();
    assert_eq!(recovered.pages.len(), 1);
    assert_eq!(recovered.pages[0].id, "doc");
    assert_eq!(recovered.pages[0].content, "saved body");
    assert!(
        serde_json::from_slice::<PersistedSidePanelState>(&std::fs::read(primary).unwrap()).is_ok()
    );
    let fresh = snapshot_for_session("truly-new").unwrap();
    assert!(fresh.pages.is_empty());
    assert!(fresh.archived.is_empty());
    assert!(fresh.unavailable.is_none());
    assert_eq!(fresh.revision, 0);
}

#[test]
fn side_panel_invalid_primary_and_backup_fail_closed_without_body_update() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    write_markdown_page("invalid-both", "doc", None, "saved body", true).unwrap();
    focus_page("invalid-both", "doc").unwrap();
    let dir = temp.path().join("side_panel/invalid-both");
    std::fs::write(dir.join("index.json"), "corrupt primary").unwrap();
    std::fs::write(dir.join("index.bak"), "corrupt backup").unwrap();
    assert!(write_markdown_page("invalid-both", "doc", None, "must not publish", false).is_err());
    assert!(
        snapshot_for_session_or_unavailable("invalid-both")
            .unavailable
            .is_some()
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("doc.md")).unwrap(),
        "saved body"
    );
    std::fs::remove_file(dir.join("index.json")).unwrap();
    assert!(snapshot_for_session("invalid-both").is_err());
}
