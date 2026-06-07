use super::{load_testers, save_testers};
use std::ffi::OsString;

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    crate::storage::lock_test_env()
}

struct TestHomeGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev_home: Option<OsString>,
    _temp_home: tempfile::TempDir,
}

impl TestHomeGuard {
    fn new() -> Self {
        let lock = lock_env();
        let temp_home = tempfile::Builder::new()
            .prefix("jcode-server-debug-testers-home-")
            .tempdir()
            .expect("create temp home");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp_home.path());
        Self {
            _lock: lock,
            prev_home,
            _temp_home: temp_home,
        }
    }
}

impl Drop for TestHomeGuard {
    fn drop(&mut self) {
        if let Some(prev_home) = &self.prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

#[test]
fn load_and_save_testers_roundtrip_manifest() {
    let _guard = TestHomeGuard::new();
    let testers = vec![serde_json::json!({
        "id": "tester_1",
        "pid": 1234,
        "cwd": ".",
    })];

    save_testers(&testers).expect("save testers");
    let loaded = load_testers().expect("load testers");
    assert_eq!(loaded.len(), 1);
    assert_eq!(
        loaded[0].get("id").and_then(|v| v.as_str()),
        Some("tester_1")
    );
}

#[test]
fn load_testers_returns_empty_for_missing_or_empty_manifest() {
    let _guard = TestHomeGuard::new();
    assert!(
        load_testers()
            .expect("missing manifest returns empty")
            .is_empty()
    );

    let manifest_path = crate::storage::jcode_dir()
        .expect("jcode dir")
        .join("testers.json");
    std::fs::write(&manifest_path, "").expect("write empty manifest");
    assert!(
        load_testers()
            .expect("empty manifest returns empty")
            .is_empty()
    );
}
