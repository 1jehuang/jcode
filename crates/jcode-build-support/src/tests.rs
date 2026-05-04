use super::*;

fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    ENV_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn with_temp_jcode_home<T>(f: impl FnOnce() -> T) -> T {
    let _guard = test_env_lock();
    let temp_home = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    jcode_core::env::set_var("JCODE_HOME", temp_home.path());
    let result = f();
    if let Some(prev_home) = prev_home {
        jcode_core::env::set_var("JCODE_HOME", prev_home);
    } else {
        jcode_core::env::remove_var("JCODE_HOME");
    }
    result
}

fn create_git_repo_fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".git")).expect("create .git dir");
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"jcode\"\nversion = \"0.0.0\"\n",
    )
    .expect("write Cargo.toml");
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(temp.path())
        .output()
        .expect("git init");
    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(temp.path())
        .output()
        .expect("git config email");
    std::process::Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(temp.path())
        .output()
        .expect("git config name");
    std::process::Command::new("git")
        .args(["add", "Cargo.toml"])
        .current_dir(temp.path())
        .output()
        .expect("git add");
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(temp.path())
        .output()
        .expect("git commit");
    temp
}

fn source_state_fixture(short_hash: &str, fingerprint: &str) -> SourceState {
    SourceState {
        repo_scope: "repo-scope".to_string(),
        worktree_scope: "worktree-scope".to_string(),
        short_hash: short_hash.to_string(),
        full_hash: format!("{short_hash}-full"),
        dirty: true,
        fingerprint: fingerprint.to_string(),
        version_label: format!("{short_hash}-dirty-{}", &fingerprint[..12]),
        changed_paths: 1,
    }
}

#[test]
fn test_build_manifest_default() {
    let manifest = BuildManifest::default();
    assert!(manifest.stable.is_none());
    assert!(manifest.canary.is_none());
    assert!(manifest.history.is_empty());
}

#[test]
fn test_binary_version_hash_mismatch_rejects_publish_candidate() {
    let source = source_state_fixture("newhash", "123456789abcffff");
    let report = BinaryVersionReport {
        version: Some("v0.0.0-dev (oldhash, dirty)".to_string()),
        git_hash: Some("oldhash".to_string()),
    };

    let error = validate_binary_version_matches_source_report(&report, Path::new("jcode"), &source)
        .expect_err("mismatched git hash should be rejected");

    assert!(
        error
            .to_string()
            .contains("binary was built from git hash oldhash")
    );
}

#[test]
fn test_dev_binary_source_metadata_mismatch_rejects_publish_candidate() {
    let temp = tempfile::tempdir().expect("tempdir");
    let binary = temp.path().join(binary_name());
    std::fs::write(&binary, b"fake").expect("write fake binary");
    let source = source_state_fixture("abc1234", "1111111111112222");
    let stale_source = source_state_fixture("abc1234", "999999999999aaaa");
    write_dev_binary_source_metadata(&binary, &stale_source).expect("write metadata");

    let error = validate_dev_binary_source_metadata(&binary, &source)
        .expect_err("mismatched source metadata should be rejected");

    assert!(error.to_string().contains("source metadata"));
    assert!(error.to_string().contains("999999999999aaaa"));
}

#[cfg(unix)]
#[test]
fn test_smoke_test_server_protocol_uses_fresh_connection_after_ping() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    let temp = tempfile::tempdir().expect("tempdir");
    let socket_path = temp.path().join("smoke.sock");
    let listener = UnixListener::bind(&socket_path).expect("bind unix listener");

    let server = std::thread::spawn(move || {
        let (first, _) = listener.accept().expect("accept ping client");
        let mut first = BufReader::new(first);
        let mut line = String::new();
        first.read_line(&mut line).expect("read ping request");
        assert!(line.contains("\"type\":\"ping\""));
        first
            .get_mut()
            .write_all(b"{\"type\":\"pong\",\"id\":1}\n")
            .expect("write pong");

        let (second, _) = listener.accept().expect("accept subscribe client");
        let mut second = BufReader::new(second);
        line.clear();
        second.read_line(&mut line).expect("read subscribe request");
        assert!(line.contains("\"type\":\"subscribe\""));
        second
            .get_mut()
            .write_all(b"{\"type\":\"ack\",\"id\":2}\n")
            .expect("write subscribe ack");
    });

    smoke_test_server_protocol(&socket_path, "/tmp").expect("smoke test protocol succeeds");
    server.join().expect("server thread join");
}

#[test]
fn test_binary_choice_for_canary_session() {
    let manifest = BuildManifest {
        canary: Some("abc123".to_string()),
        canary_session: Some("session_test".to_string()),
        ..Default::default()
    };

    // Canary session should get canary binary
    match manifest.binary_for_session("session_test") {
        BinaryChoice::Canary(hash) => assert_eq!(hash, "abc123"),
        _ => panic!("Expected canary binary"),
    }

    // Other sessions should get stable (or current if no stable)
    match manifest.binary_for_session("other_session") {
        BinaryChoice::Current => {}
        _ => panic!("Expected current binary"),
    }
}

#[test]
fn test_find_repo_in_ancestors_walks_upward() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("jcode-repo");
    let nested = repo.join("a").join("b").join("c");

    std::fs::create_dir_all(repo.join(".git")).expect("create .git");
    std::fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"jcode\"\nversion = \"0.0.0\"\n",
    )
    .expect("write Cargo.toml");
    std::fs::create_dir_all(&nested).expect("create nested dirs");

    let found = find_repo_in_ancestors(&nested).expect("repo should be found");
    assert_eq!(found, repo);
}

#[test]
fn test_client_update_candidate_prefers_dev_binary_for_selfdev() {
    let _guard = test_env_lock();
    let temp_home = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    jcode_core::env::set_var("JCODE_HOME", temp_home.path());

    let version = "test-current";
    let version_binary =
        install_binary_at_version(std::env::current_exe().as_ref().unwrap(), version)
            .expect("install test version");
    update_current_symlink(version).expect("update current symlink");

    let candidate = client_update_candidate(true).expect("expected selfdev candidate");
    assert_eq!(candidate.1, "current");
    assert_eq!(
        std::fs::canonicalize(candidate.0).expect("canonical candidate"),
        std::fs::canonicalize(version_binary).expect("canonical version binary")
    );

    if let Some(prev_home) = prev_home {
        jcode_core::env::set_var("JCODE_HOME", prev_home);
    } else {
        jcode_core::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn launcher_dir_uses_sandbox_bin_when_jcode_home_is_set() {
    with_temp_jcode_home(|| {
        let launcher_dir = launcher_dir().expect("launcher dir");
        let expected = storage::jcode_dir().expect("jcode dir").join("bin");
        assert_eq!(launcher_dir, expected);
    });
}

#[test]
fn update_launcher_symlink_stays_inside_sandbox_home() {
    with_temp_jcode_home(|| {
        let version = "sandbox-current";
        let version_binary =
            install_binary_at_version(std::env::current_exe().as_ref().unwrap(), version)
                .expect("install test version");
        update_current_symlink(version).expect("update current symlink");

        let launcher = update_launcher_symlink_to_current().expect("update launcher");
        let expected_launcher = storage::jcode_dir()
            .expect("jcode dir")
            .join("bin")
            .join(binary_name());
        assert_eq!(launcher, expected_launcher);
        assert_eq!(
            std::fs::canonicalize(&launcher).expect("canonical launcher"),
            std::fs::canonicalize(version_binary).expect("canonical version binary")
        );
    });
}

#[test]
fn test_canary_status_serialization() {
    assert_eq!(
        serde_json::to_string(&CanaryStatus::Testing).unwrap(),
        "\"testing\""
    );
    assert_eq!(
        serde_json::to_string(&CanaryStatus::Passed).unwrap(),
        "\"passed\""
    );
}

#[test]
fn dirty_source_state_uses_fingerprint_in_version_label() {
    let repo = create_git_repo_fixture();
    std::fs::write(repo.path().join("notes.txt"), "dirty change\n").expect("write dirty file");

    let state = current_source_state(repo.path()).expect("source state");
    assert!(state.dirty);
    assert!(
        state
            .version_label
            .starts_with(&format!("{}-dirty-", state.short_hash))
    );
    assert!(state.version_label.len() > state.short_hash.len() + 7);
}

#[test]
fn pending_activation_can_complete_and_roll_back() {
    with_temp_jcode_home(|| {
        let current_version = "stable-prev";
        let shared_version = "shared-prev";
        install_binary_at_version(std::env::current_exe().as_ref().unwrap(), current_version)
            .expect("install previous version");
        install_binary_at_version(std::env::current_exe().as_ref().unwrap(), shared_version)
            .expect("install previous shared version");
        update_current_symlink(current_version).expect("publish previous current");
        update_shared_server_symlink(shared_version).expect("publish previous shared");

        let mut manifest = BuildManifest::default();
        manifest
            .set_pending_activation(PendingActivation {
                session_id: "session-a".to_string(),
                new_version: "canary-next".to_string(),
                previous_current_version: Some(current_version.to_string()),
                previous_shared_server_version: Some(shared_version.to_string()),
                source_fingerprint: Some("fingerprint-a".to_string()),
                requested_at: Utc::now(),
            })
            .expect("set pending activation");

        let completed = complete_pending_activation_for_session("session-a")
            .expect("complete activation")
            .expect("completed version");
        assert_eq!(completed, "canary-next");
        let manifest = BuildManifest::load().expect("load manifest");
        assert!(manifest.pending_activation.is_none());
        assert_eq!(manifest.canary.as_deref(), Some("canary-next"));
        assert_eq!(manifest.canary_status, Some(CanaryStatus::Passed));

        let mut manifest = BuildManifest::load().expect("reload manifest");
        manifest
            .set_pending_activation(PendingActivation {
                session_id: "session-b".to_string(),
                new_version: "canary-bad".to_string(),
                previous_current_version: Some(current_version.to_string()),
                previous_shared_server_version: Some(shared_version.to_string()),
                source_fingerprint: Some("fingerprint-b".to_string()),
                requested_at: Utc::now(),
            })
            .expect("set second pending activation");

        let rolled_back = rollback_pending_activation_for_session("session-b")
            .expect("rollback activation")
            .expect("rolled back version");
        assert_eq!(rolled_back, "canary-bad");
        let restored = read_current_version()
            .expect("read current version")
            .expect("restored current version");
        assert_eq!(restored, current_version);
        let restored_shared = read_shared_server_version()
            .expect("read shared server version")
            .expect("restored shared server version");
        assert_eq!(restored_shared, shared_version);
    });
}

#[test]
fn shared_server_candidate_prefers_approved_channel_over_current() {
    with_temp_jcode_home(|| {
        let approved_version = "shared-ok";
        let current_version = "current-dev";
        install_binary_at_version(std::env::current_exe().as_ref().unwrap(), approved_version)
            .expect("install approved version");
        install_binary_at_version(std::env::current_exe().as_ref().unwrap(), current_version)
            .expect("install current version");
        update_shared_server_symlink(approved_version).expect("update shared server");
        update_current_symlink(current_version).expect("update current");

        let candidate =
            shared_server_update_candidate(true).expect("expected shared-server candidate");
        assert_eq!(candidate.1, "shared-server");
        let selected = std::fs::canonicalize(candidate.0).expect("canonical selected");
        let approved = std::fs::canonicalize(version_binary_path(approved_version).unwrap())
            .expect("canonical approved");
        assert_eq!(selected, approved);
    });
}
