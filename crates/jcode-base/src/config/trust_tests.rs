use super::Config;

struct TrustEnv {
    home: tempfile::TempDir,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl TrustEnv {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let previous = ["HOME", "JCODE_HOME", "JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        crate::env::set_var("HOME", home.path());
        crate::env::set_var("JCODE_HOME", home.path());
        crate::env::remove_var("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES");
        Config::invalidate_cache();
        Self { home, previous }
    }

    fn warm(&self) -> (std::path::PathBuf, String) {
        let auth = self.home.path().join("external-auth.json");
        std::fs::write(&auth, "{}").unwrap();
        let entry = Config::trusted_external_auth_path_entry("sample", &auth).unwrap();
        let content = format!(
            "[auth]\ntrusted_external_sources = [\"sample\"]\ntrusted_external_source_paths = [{entry:?}]\n"
        );
        std::fs::write(Config::path().unwrap(), &content).unwrap();
        assert!(Config::external_auth_source_allowed("sample"));
        assert!(Config::external_auth_source_allowed_for_path(
            "sample", &auth
        ));
        assert!(Config::external_auth_source_allowed_for_path_cached(
            "sample", &auth
        ));
        (auth, content)
    }
}

impl Drop for TrustEnv {
    fn drop(&mut self) {
        for (key, previous) in self.previous.drain(..) {
            match previous {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
        Config::invalidate_cache();
    }
}

#[test]
fn trust_revocation_observes_same_length_same_mtime_edit() {
    let _lock = crate::storage::lock_test_env();
    let env = TrustEnv::new();
    let (auth, content) = env.warm();
    let path = Config::path().unwrap();
    let before = std::fs::metadata(&path).unwrap();
    let revoked = content.replace("sample", "denied");
    std::fs::write(&path, revoked).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(before.modified().unwrap()))
        .unwrap();
    let after = std::fs::metadata(&path).unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    assert_eq!(
        Config::load_strict().unwrap().auth.trusted_external_sources,
        ["denied"]
    );
    let decisions = (
        Config::external_auth_source_allowed("sample"),
        Config::external_auth_source_allowed_for_path("sample", &auth),
        Config::external_auth_source_allowed_for_path_cached("sample", &auth),
    );
    assert_eq!(decisions, (false, false, false));
}

#[test]
fn trust_revocation_fails_closed_on_malformed_config() {
    let _lock = crate::storage::lock_test_env();
    let env = TrustEnv::new();
    let (auth, content) = env.warm();
    let path = Config::path().unwrap();
    let before = std::fs::metadata(&path).unwrap();
    std::fs::write(&path, "[".repeat(content.len())).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(before.modified().unwrap()))
        .unwrap();
    assert_eq!(
        (
            Config::external_auth_source_allowed("sample"),
            Config::external_auth_source_allowed_for_path("sample", &auth),
            Config::external_auth_source_allowed_for_path_cached("sample", &auth)
        ),
        (false, false, false)
    );
}

#[test]
fn trust_revocation_keeps_unchanged_lists_memoized() {
    let _lock = crate::storage::lock_test_env();
    let env = TrustEnv::new();
    env.warm();
    let first = Config::trusted_external_auth_lists();
    let second = Config::trusted_external_auth_lists();
    assert!(std::sync::Arc::ptr_eq(&first, &second));
}

#[test]
fn trust_revocation_observes_environment_override_changes() {
    let _lock = crate::storage::lock_test_env();
    let env = TrustEnv::new();
    let (auth, _) = env.warm();
    crate::env::set_var("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES", "denied");
    assert_eq!(
        (
            Config::external_auth_source_allowed("sample"),
            Config::external_auth_source_allowed_for_path("sample", &auth)
        ),
        (false, false)
    );
    crate::env::remove_var("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES");
    assert!(Config::external_auth_source_allowed("sample"));
}

#[test]
fn trust_revocation_observes_missing_config() {
    let _lock = crate::storage::lock_test_env();
    let env = TrustEnv::new();
    let (auth, _) = env.warm();
    std::fs::remove_file(Config::path().unwrap()).unwrap();
    assert_eq!(
        (
            Config::external_auth_source_allowed("sample"),
            Config::external_auth_source_allowed_for_path("sample", &auth)
        ),
        (false, false)
    );
    let entry = Config::trusted_external_auth_path_entry("sample", &auth).unwrap();
    crate::env::set_var(
        "JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES",
        format!(" SAMPLE,\n{entry}, ,\n"),
    );
    assert_eq!(
        (
            Config::external_auth_source_allowed("sample"),
            Config::external_auth_source_allowed_for_path("sample", &auth)
        ),
        (true, true)
    );
    crate::env::set_var("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES", "");
    assert!(!Config::external_auth_source_allowed("sample"));
}

#[test]
fn trust_revocation_fails_closed_on_invalid_utf8() {
    let _lock = crate::storage::lock_test_env();
    let env = TrustEnv::new();
    let (auth, _) = env.warm();
    std::fs::write(Config::path().unwrap(), [0xff]).unwrap();
    assert_eq!(
        (
            Config::external_auth_source_allowed("sample"),
            Config::external_auth_source_allowed_for_path("sample", &auth)
        ),
        (false, false)
    );
}

#[test]
fn trust_revocation_fails_closed_on_read_error() {
    let _lock = crate::storage::lock_test_env();
    let env = TrustEnv::new();
    let (auth, _) = env.warm();
    let path = Config::path().unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(
        (
            Config::external_auth_source_allowed("sample"),
            Config::external_auth_source_allowed_for_path("sample", &auth),
            Config::external_auth_source_allowed_for_path_cached("sample", &auth)
        ),
        (false, false, false)
    );
}
