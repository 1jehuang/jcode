use super::*;

#[cfg(unix)]
#[test]
fn harden_secret_file_permissions_sets_owner_only_modes() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("create temp dir");
    let secret_dir = dir.path().join("jcode");
    std::fs::create_dir_all(&secret_dir).expect("create secret dir");

    let secret_file = secret_dir.join("openrouter.env");
    std::fs::write(&secret_file, "OPENROUTER_API_KEY=sk-or-v1-test\n").expect("write secret file");

    std::fs::set_permissions(&secret_dir, std::fs::Permissions::from_mode(0o755))
        .expect("set initial dir perms");
    std::fs::set_permissions(&secret_file, std::fs::Permissions::from_mode(0o644))
        .expect("set initial file perms");

    harden_secret_file_permissions(&secret_file);

    let dir_mode = std::fs::metadata(&secret_dir)
        .expect("stat dir")
        .permissions()
        .mode()
        & 0o777;
    let file_mode = std::fs::metadata(&secret_file)
        .expect("stat file")
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(dir_mode, 0o700);
    assert_eq!(file_mode, 0o600);
}

#[test]
fn user_home_path_uses_external_dir_under_jcode_home() {
    let _guard = lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().expect("create temp dir");
    crate::env::set_var("JCODE_HOME", temp.path());

    let resolved = user_home_path(".codex/auth.json").expect("resolve user home path");
    assert_eq!(
        resolved,
        temp.path()
            .join("external")
            .join(".codex")
            .join("auth.json")
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[cfg(unix)]
#[test]
fn validate_external_auth_file_rejects_symlink() {
    use std::os::unix::fs as unix_fs;

    let dir = tempfile::TempDir::new().expect("create temp dir");
    let target = dir.path().join("auth.json");
    let link = dir.path().join("auth-link.json");
    std::fs::write(&target, "{}\n").expect("write target");
    unix_fs::symlink(&target, &link).expect("create symlink");

    let err = validate_external_auth_file(&link).expect_err("symlink should be rejected");
    assert!(err.to_string().contains("symlink"));
}

#[test]
fn app_config_dir_uses_jcode_home_when_set() {
    let _guard = lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().expect("create temp dir");
    crate::env::set_var("JCODE_HOME", temp.path());

    let resolved = app_config_dir().expect("resolve app config dir");
    assert_eq!(resolved, temp.path().join("config").join("jcode"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn upsert_env_file_value_writes_replaces_and_removes_entries() {
    let dir = tempfile::TempDir::new().expect("create temp dir");
    let file = dir.path().join("test.env");

    upsert_env_file_value(&file, "API_KEY", Some("one")).expect("write initial env value");
    assert_eq!(
        std::fs::read_to_string(&file).expect("read env file"),
        "API_KEY=one\n"
    );

    upsert_env_file_value(&file, "OTHER", Some("two")).expect("append second value");
    upsert_env_file_value(&file, "API_KEY", Some("updated")).expect("replace existing value");
    assert_eq!(
        std::fs::read_to_string(&file).expect("read env file after replace"),
        "API_KEY=updated\nOTHER=two\n"
    );

    upsert_env_file_value(&file, "API_KEY", None).expect("remove env value");
    assert_eq!(
        std::fs::read_to_string(&file).expect("read env file after remove"),
        "OTHER=two\n"
    );
}

#[cfg(unix)]
#[test]
fn write_text_secret_sets_owner_only_modes() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("create temp dir");
    let file = dir.path().join("secret.env");

    write_text_secret(&file, "SECRET=value\n").expect("write secret text");

    let dir_mode = std::fs::metadata(dir.path())
        .expect("stat dir")
        .permissions()
        .mode()
        & 0o777;
    let file_mode = std::fs::metadata(&file)
        .expect("stat file")
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(dir_mode, 0o700);
    assert_eq!(file_mode, 0o600);
}

/// Restores `JCODE_TEST_ENV_LOCK_TIMEOUT_SECS` even when an assertion fails, so
/// a failing timeout test cannot leave a short bound set for the whole process.
struct TestEnvLockTimeoutRestore(Option<std::ffi::OsString>);

impl Drop for TestEnvLockTimeoutRestore {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => crate::env::set_var("JCODE_TEST_ENV_LOCK_TIMEOUT_SECS", value),
            None => crate::env::remove_var("JCODE_TEST_ENV_LOCK_TIMEOUT_SECS"),
        }
    }
}

#[test]
fn shared_test_env_lock_timeout_defaults_and_parses_override() {
    let _guard = lock_test_env();
    let _restore = TestEnvLockTimeoutRestore(std::env::var_os("JCODE_TEST_ENV_LOCK_TIMEOUT_SECS"));

    // The default keeps a legitimately slow holder from failing the suite.
    crate::env::remove_var("JCODE_TEST_ENV_LOCK_TIMEOUT_SECS");
    assert_eq!(test_env_lock_timeout(), DEFAULT_TEST_ENV_LOCK_TIMEOUT);

    crate::env::set_var("JCODE_TEST_ENV_LOCK_TIMEOUT_SECS", "7");
    assert_eq!(test_env_lock_timeout(), Duration::from_secs(7));

    crate::env::set_var("JCODE_TEST_ENV_LOCK_TIMEOUT_SECS", " 12 ");
    assert_eq!(test_env_lock_timeout(), Duration::from_secs(12));

    // Zero and garbage fall back to the default instead of failing instantly.
    for invalid in ["0", "-4", "soon"] {
        crate::env::set_var("JCODE_TEST_ENV_LOCK_TIMEOUT_SECS", invalid);
        assert_eq!(
            test_env_lock_timeout(),
            DEFAULT_TEST_ENV_LOCK_TIMEOUT,
            "{invalid:?} must not shorten the bound"
        );
    }
}

#[test]
fn a_contended_env_lock_is_acquired_once_the_holder_releases() {
    let guard = lock_test_env();
    let waiter = std::thread::Builder::new()
        .name("storage::tests::env-lock-waiter".to_string())
        .spawn(|| {
            let _inner = lock_test_env();
            7
        })
        .expect("spawn waiter");

    // Let the waiter reach the bounded wait before the holder releases.
    std::thread::sleep(Duration::from_millis(50));
    drop(guard);

    assert_eq!(waiter.join().expect("waiter thread"), 7);
}

#[test]
fn re_entering_the_env_lock_is_reported_instead_of_hanging() {
    // std::sync::Mutex is not reentrant, so this nested acquisition can only end
    // by being detected: without the guard this test hangs the whole suite.
    let handle = std::thread::Builder::new()
        .name("storage::tests::env-lock-reentrant".to_string())
        .spawn(|| {
            let _outer = lock_test_env();
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _inner = lock_test_env();
            }))
            .is_err()
        })
        .expect("spawn reentrant thread");

    assert!(
        handle.join().expect("reentrant thread"),
        "a nested lock_test_env() call must be reported, not waited out"
    );
}
