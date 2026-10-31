use super::*;

// HOME is always disposable before removing JCODE_HOME. Never probe the real
// production home, including when testing the missing-override regression.
async fn synthetic_reload_preserves_manifest(override_home: bool, mode: ToolExecutionMode) {
    let _storage_guard = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set("HOME", home.path());
    let isolated = home.path().join("isolated");
    let _jcode_home = if override_home {
        EnvVarGuard::set("JCODE_HOME", &isolated)
    } else {
        EnvVarGuard::remove("JCODE_HOME")
    };
    let _test = EnvVarGuard::set("JCODE_TEST_SESSION", "1");
    let expected = if override_home {
        isolated
    } else {
        home.path().join(".jcode")
    };
    assert_eq!(storage::jcode_dir().unwrap(), expected);
    let manifest = expected.join("builds/manifest.json");
    std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    let original = br#"{"stable":"real-build","canary":"real-canary","canary_status":"testing","pending_activation":{"session_id":"synthetic-manifest-regression","new_version":"real-pending","requested_at":"2026-10-29T00:00:00Z"}}"#;
    std::fs::write(&manifest, original).unwrap();
    let repo = create_repo_fixture();
    let mut rx = server::subscribe_reload_signal_for_tests();
    let acker = tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            if let Some(signal) = rx.borrow_and_update().clone() {
                server::acknowledge_reload_signal(&signal);
            }
        }
    });
    let result = SelfDevTool::new()
        .do_reload(
            None,
            "synthetic-manifest-regression",
            mode,
            Some(repo.path()),
        )
        .await;
    acker.abort();
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(std::fs::read(&manifest).unwrap(), original);
    let ctx = ReloadContext::peek_for_session("synthetic-manifest-regression")
        .unwrap()
        .unwrap();
    assert_eq!(ctx.version_after, "test-reload-hash");

    // An absent manifest must stay absent, not just retain existing state.
    std::fs::remove_file(&manifest).unwrap();
    let _timeout = EnvVarGuard::set("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS", "1");
    let error = SelfDevTool::new()
        .do_reload(
            None,
            "synthetic-manifest-regression",
            mode,
            Some(repo.path()),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Timed out waiting"), "{error}");
    assert!(!manifest.exists());

    // A context-save failure must not roll back an unrelated real activation,
    // even if its session id happens to match the synthetic reload.
    std::fs::write(&manifest, original).unwrap();
    let context_path = ReloadContext::path_for_session("synthetic-manifest-regression").unwrap();
    std::fs::remove_file(&context_path).unwrap();
    std::fs::create_dir(&context_path).unwrap();
    assert!(
        SelfDevTool::new()
            .do_reload(
                None,
                "synthetic-manifest-regression",
                mode,
                Some(repo.path())
            )
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&manifest).unwrap(), original);
}

// dirs::home_dir honors HOME on Unix. Windows may resolve its known-folder
// profile independently, so do not remove JCODE_HOME in that environment.
#[cfg(unix)]
#[tokio::test]
async fn synthetic_reload_without_jcode_home_preserves_manifest() {
    synthetic_reload_preserves_manifest(false, ToolExecutionMode::Direct).await;
}

#[tokio::test]
async fn synthetic_reload_with_jcode_home_preserves_manifest() {
    synthetic_reload_preserves_manifest(true, ToolExecutionMode::AgentTurn).await;
}
