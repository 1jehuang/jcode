use std::sync::{LazyLock, RwLock};

use jcode_provider_metadata::{is_safe_env_file_name, is_safe_env_key_name};

/// Fallback resolvers consulted by [`load_api_key_from_env_or_config`] after the
/// environment and config-file lookups fail. Higher-level crates register
/// resolvers at startup so this leaf crate does not need to depend on auth.
type ApiKeyFallbackResolver = fn(&str) -> Option<String>;

static API_KEY_FALLBACK_RESOLVERS: LazyLock<RwLock<Vec<ApiKeyFallbackResolver>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

/// Register a fallback API-key resolver consulted when env/config lookups miss.
pub fn register_api_key_fallback_resolver(resolver: ApiKeyFallbackResolver) {
    API_KEY_FALLBACK_RESOLVERS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(resolver);
}

fn resolve_api_key_fallback(env_key: &str) -> Option<String> {
    let resolvers = API_KEY_FALLBACK_RESOLVERS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for resolver in resolvers.iter() {
        if let Some(key) = resolver(env_key) {
            return Some(key);
        }
    }
    None
}

pub fn load_api_key_from_env_or_config(env_key: &str, file_name: &str) -> Option<String> {
    if !is_safe_env_key_name(env_key) {
        jcode_logging::warn(&format!(
            "Ignoring invalid API key variable name '{}' while loading credentials",
            env_key
        ));
        return None;
    }
    if !is_safe_env_file_name(file_name) {
        jcode_logging::warn(&format!(
            "Ignoring invalid env file name '{}' while loading credentials",
            file_name
        ));
        return None;
    }

    if let Ok(key) = std::env::var(env_key) {
        let key = key.trim();
        if !key.is_empty() {
            return Some(key.to_string());
        }
    }

    let config_path = jcode_storage::app_config_dir().ok()?.join(file_name);
    jcode_storage::harden_secret_file_permissions(&config_path);
    let content = std::fs::read_to_string(config_path).ok()?;
    let prefix = format!("{}=", env_key);

    for line in content.lines() {
        if let Some(key) = line.strip_prefix(&prefix) {
            let key = key.trim().trim_matches('"').trim_matches('\'');
            if !key.is_empty() {
                return Some(key.to_string());
            }
        }
    }

    if env_key == "ZHIPU_API_KEY" {
        if let Ok(key) = std::env::var("ZAI_API_KEY") {
            let key = key.trim();
            if !key.is_empty() {
                return Some(key.to_string());
            }
        }

        let legacy_prefix = "ZAI_API_KEY=";
        for line in content.lines() {
            if let Some(key) = line.strip_prefix(legacy_prefix) {
                let key = key.trim().trim_matches('"').trim_matches('\'');
                if !key.is_empty() {
                    return Some(key.to_string());
                }
            }
        }
    }

    if let Some(key) = resolve_api_key_fallback(env_key) {
        return Some(key);
    }

    None
}

pub fn load_env_value_from_env_or_config(env_key: &str, file_name: &str) -> Option<String> {
    if !is_safe_env_key_name(env_key) {
        jcode_logging::warn(&format!(
            "Ignoring invalid variable name '{}' while loading config value",
            env_key
        ));
        return None;
    }
    if !is_safe_env_file_name(file_name) {
        jcode_logging::warn(&format!(
            "Ignoring invalid env file name '{}' while loading config value",
            file_name
        ));
        return None;
    }

    if let Ok(value) = std::env::var(env_key) {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }

    let config_path = jcode_storage::app_config_dir().ok()?.join(file_name);
    jcode_storage::harden_secret_file_permissions(&config_path);
    let content = std::fs::read_to_string(config_path).ok()?;
    let prefix = format!("{}=", env_key);

    for line in content.lines() {
        if let Some(value) = line.strip_prefix(&prefix) {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }

    None
}

pub fn save_env_value_to_env_file(
    env_key: &str,
    file_name: &str,
    value: Option<&str>,
) -> anyhow::Result<()> {
    if !is_safe_env_key_name(env_key) {
        anyhow::bail!("Invalid variable name: {}", env_key);
    }
    if !is_safe_env_file_name(file_name) {
        anyhow::bail!("Invalid env file name: {}", file_name);
    }

    let config_dir = jcode_storage::app_config_dir()?;
    let file_path = config_dir.join(file_name);
    jcode_storage::upsert_env_file_value(&file_path, env_key, value)?;

    if let Some(value) = value {
        jcode_core::env::set_var(env_key, value);
    } else {
        jcode_core::env::remove_var(env_key);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        _lock: MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<OsString>)>,
    }

    impl EnvGuard {
        fn new(keys: &[&'static str]) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let saved = keys
                .iter()
                .map(|key| (*key, std::env::var_os(key)))
                .collect::<Vec<_>>();
            for key in keys {
                jcode_core::env::remove_var(key);
            }
            Self { _lock: lock, saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..) {
                match value {
                    Some(value) => jcode_core::env::set_var(key, value),
                    None => jcode_core::env::remove_var(key),
                }
            }
        }
    }

    #[test]
    fn loads_api_key_from_env_before_config_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = EnvGuard::new(&["JCODE_HOME", "JCODE_PROVIDER_ENV_TEST_KEY"]);
        jcode_core::env::set_var("JCODE_HOME", temp.path());

        save_env_value_to_env_file(
            "JCODE_PROVIDER_ENV_TEST_KEY",
            "provider-env-test.env",
            Some("file-key"),
        )
        .expect("save file key");
        jcode_core::env::set_var("JCODE_PROVIDER_ENV_TEST_KEY", "env-key");

        assert_eq!(
            load_api_key_from_env_or_config("JCODE_PROVIDER_ENV_TEST_KEY", "provider-env-test.env")
                .as_deref(),
            Some("env-key")
        );
    }

    #[test]
    fn loads_and_removes_values_from_sandboxed_config_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = EnvGuard::new(&["JCODE_HOME", "JCODE_PROVIDER_ENV_TEST_VALUE"]);
        jcode_core::env::set_var("JCODE_HOME", temp.path());

        save_env_value_to_env_file(
            "JCODE_PROVIDER_ENV_TEST_VALUE",
            "provider-env-test.env",
            Some("file-value"),
        )
        .expect("save file value");

        jcode_core::env::remove_var("JCODE_PROVIDER_ENV_TEST_VALUE");
        assert_eq!(
            load_env_value_from_env_or_config(
                "JCODE_PROVIDER_ENV_TEST_VALUE",
                "provider-env-test.env"
            )
            .as_deref(),
            Some("file-value")
        );

        save_env_value_to_env_file(
            "JCODE_PROVIDER_ENV_TEST_VALUE",
            "provider-env-test.env",
            None,
        )
        .expect("remove file value");
        assert_eq!(
            load_env_value_from_env_or_config(
                "JCODE_PROVIDER_ENV_TEST_VALUE",
                "provider-env-test.env"
            ),
            None
        );
    }

    #[test]
    fn accepts_legacy_zai_key_for_zhipu() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = EnvGuard::new(&["JCODE_HOME", "ZHIPU_API_KEY", "ZAI_API_KEY"]);
        jcode_core::env::set_var("JCODE_HOME", temp.path());

        save_env_value_to_env_file("ZAI_API_KEY", "zai.env", Some("legacy-zai-key"))
            .expect("save legacy key");
        jcode_core::env::remove_var("ZAI_API_KEY");

        assert_eq!(
            load_api_key_from_env_or_config("ZHIPU_API_KEY", "zai.env").as_deref(),
            Some("legacy-zai-key")
        );
    }
}
