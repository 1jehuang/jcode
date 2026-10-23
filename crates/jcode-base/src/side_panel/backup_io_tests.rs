use super::*;
use crate::side_panel::side_panel_tests::EnvVarGuard;

#[test]
fn side_panel_backup_io_failure_is_not_treated_as_repairable_json_corruption() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    write_markdown_page("backup-io", "doc", None, "saved", true).unwrap();
    focus_page("backup-io", "doc").unwrap();
    let dir = temp.path().join("side_panel/backup-io");
    let before = std::fs::read(dir.join("index.json")).unwrap();
    std::fs::remove_file(dir.join("index.bak")).unwrap();
    std::fs::create_dir(dir.join("index.bak")).unwrap();
    assert!(focus_page("backup-io", "doc").is_err());
    assert_eq!(std::fs::read(dir.join("index.json")).unwrap(), before);
    std::fs::remove_file(dir.join("index.json")).unwrap();
    assert!(snapshot_for_session("backup-io").is_err());
}
