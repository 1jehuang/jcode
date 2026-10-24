use super::*;
use crate::side_panel::side_panel_tests::EnvVarGuard;

#[test]
fn side_panel_retains_missing_linked_source_and_recovers_empty_markdown() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let source = temp.path().join("source.md");
    std::fs::write(&source, "last good").unwrap();
    let first = load_file("retain", "doc", None, &source, true).unwrap();
    std::fs::remove_file(&source).unwrap();
    let stale = snapshot_for_session("retain").unwrap();
    assert_eq!(stale.pages[0].content, "last good");
    assert!(stale.pages[0].status.stale_reason.is_some());
    assert_eq!(
        stale.pages[0].status.last_successful_update_ms,
        first.pages[0].status.last_successful_update_ms
    );
    assert!(stale.revision > first.revision);
    std::fs::write(&source, "").unwrap();
    let recovered = snapshot_for_session("retain").unwrap();
    assert_eq!(recovered.pages[0].content, "");
    assert!(recovered.pages[0].status.stale_reason.is_none());
}

#[test]
fn side_panel_archive_reopen_purge_is_session_scoped_and_late_refresh_cannot_resurrect() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let source = temp.path().join("external.md");
    std::fs::write(&source, "external").unwrap();
    let mut old = load_file("archive", "doc", None, &source, true).unwrap();
    let closed = archive_page("archive", "doc").unwrap();
    assert!(closed.pages.is_empty());
    assert_eq!(closed.archived.len(), 1);
    assert_eq!(history("archive").unwrap().len(), 1);
    assert!(history("different").unwrap().is_empty());
    std::fs::remove_file(&source).unwrap();
    let reopened = reopen_page("archive", "doc").unwrap();
    assert_eq!(reopened.pages[0].content, "external");
    assert!(reopened.archived.is_empty());
    std::fs::write(&source, "external remains").unwrap();
    let purged = purge_page("archive", "doc").unwrap();
    assert!(purged.pages.is_empty());
    assert!(refresh_linked_page_content(&mut old, None));
    assert!(old.pages.is_empty());
    assert_eq!(
        std::fs::read_to_string(&source).unwrap(),
        "external remains"
    );
}

#[test]
fn side_panel_history_count_is_bounded_and_recovery_owns_bodies() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    for i in 0..MAX_ARCHIVED_PAGES + 2 {
        let id = format!("p{i}");
        write_markdown_page("count", &id, None, "body", false).unwrap();
        archive_page("count", &id).unwrap();
    }
    assert_eq!(history("count").unwrap().len(), MAX_ARCHIVED_PAGES);
    assert!(reopen_page("count", "p0").is_err());
    assert!(!temp.path().join("side_panel/count/p0.md").exists());
    write_markdown_page("source", "doc", None, "recover me", true).unwrap();
    let recovered = inherit_for_recovery("source", "target").unwrap();
    assert_eq!(
        recovered.pages[0].status.restored_from_session.as_deref(),
        Some("source")
    );
    purge_page("source", "doc").unwrap();
    assert_eq!(
        snapshot_for_session("target").unwrap().pages[0].content,
        "recover me"
    );
}
