// Test-only provider isolation. Callers must hold storage::lock_test_env().
pub(super) struct ProviderEnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl ProviderEnvGuard {
    pub(super) fn clear() -> Self {
        let mut keys = vec![
            "JCODE_NAMED_PROVIDER_PROFILE",
            "JCODE_OPENAI_COMPAT_API_BASE",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME",
            "JCODE_OPENAI_COMPAT_ENV_FILE",
            "JCODE_OPENAI_COMPAT_SETUP_URL",
            "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
            "JCODE_OPENAI_COMPAT_LOCAL_ENABLED",
            "OPENAI_COMPAT_API_KEY",
        ];
        for profile in crate::provider_catalog::openai_compatible_profiles() {
            if !keys.contains(&profile.api_key_env) {
                keys.push(profile.api_key_env);
            }
        }
        let guard = Self(
            keys.into_iter()
                .map(|key| (key, std::env::var_os(key)))
                .collect(),
        );
        for (key, _) in &guard.0 {
            crate::env::remove_var(key);
        }
        guard
    }
}

impl Drop for ProviderEnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..) {
            match value {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
    }
}

#[test]
fn provider_env_guard_restores_present_and_absent_values_even_on_unwind() {
    let _lock = crate::storage::lock_test_env();
    let _restore = EnvRestoreGuard::capture([
        "JCODE_NAMED_PROVIDER_PROFILE",
        "COMTEGRA_API_KEY",
        "FPT_API_KEY",
    ]);
    crate::env::set_var("JCODE_NAMED_PROVIDER_PROFILE", "inherited-profile");
    crate::env::set_var("COMTEGRA_API_KEY", "inherited-key");
    crate::env::remove_var("FPT_API_KEY");
    for panic in [false, true] {
        let result = std::panic::catch_unwind(|| {
            let _guard = ProviderEnvGuard::clear();
            for key in [
                "JCODE_NAMED_PROVIDER_PROFILE",
                "COMTEGRA_API_KEY",
                "FPT_API_KEY",
            ] {
                assert!(std::env::var_os(key).is_none(), "{key} was not isolated");
            }
            crate::env::set_var("FPT_API_KEY", "temporary-key");
            assert!(!panic, "exercise unwinding");
        });
        assert_eq!(result.is_err(), panic);
        assert_eq!(
            std::env::var("JCODE_NAMED_PROVIDER_PROFILE").unwrap(),
            "inherited-profile"
        );
        assert_eq!(std::env::var("COMTEGRA_API_KEY").unwrap(), "inherited-key");
        assert!(std::env::var_os("FPT_API_KEY").is_none());
    }
}
