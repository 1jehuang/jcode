pub use jcode_provider_metadata::*;
use std::collections::HashSet;

pub const OPENAI_COMPAT_LOCAL_ENABLED_ENV: &str = "JCODE_OPENAI_COMPAT_LOCAL_ENABLED";

pub fn resolve_openai_compatible_profile(
    profile: OpenAiCompatibleProfile,
) -> ResolvedOpenAiCompatibleProfile {
    let mut resolved = ResolvedOpenAiCompatibleProfile {
        id: profile.id.to_string(),
        display_name: profile.display_name.to_string(),
        api_base: profile.api_base.to_string(),
        api_key_env: profile.api_key_env.to_string(),
        env_file: profile.env_file.to_string(),
        setup_url: profile.setup_url.to_string(),
        default_model: profile.default_model.map(ToString::to_string),
        requires_api_key: profile.requires_api_key,
    };

    if profile.id != OPENAI_COMPAT_PROFILE.id {
        return resolved;
    }

    if let Some(base) = env_override("JCODE_OPENAI_COMPAT_API_BASE") {
        if let Some(normalized) = normalize_api_base(&base) {
            resolved.api_base = normalized;
        } else {
            eprintln!(
                "Warning: ignoring invalid JCODE_OPENAI_COMPAT_API_BASE '{}'. Use https://... (or http://localhost).",
                base
            );
        }
    }

    if let Some(key_name) = env_override("JCODE_OPENAI_COMPAT_API_KEY_NAME") {
        if is_safe_env_key_name(&key_name) {
            resolved.api_key_env = key_name;
        } else {
            eprintln!(
                "Warning: ignoring invalid JCODE_OPENAI_COMPAT_API_KEY_NAME '{}'.",
                key_name
            );
        }
    }

    if let Some(env_file) = env_override("JCODE_OPENAI_COMPAT_ENV_FILE") {
        if is_safe_env_file_name(&env_file) {
            resolved.env_file = env_file;
        } else {
            eprintln!(
                "Warning: ignoring invalid JCODE_OPENAI_COMPAT_ENV_FILE '{}'.",
                env_file
            );
        }
    }

    if let Some(setup_url) = env_override("JCODE_OPENAI_COMPAT_SETUP_URL") {
        resolved.setup_url = setup_url;
    }

    if let Some(model) = env_override("JCODE_OPENAI_COMPAT_DEFAULT_MODEL") {
        resolved.default_model = Some(model);
    }

    resolved
}

pub fn apply_openai_compatible_profile_env(profile: Option<OpenAiCompatibleProfile>) {
    let vars = [
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_AUTH_HEADER",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
        "JCODE_OPENROUTER_PROVIDER",
        "JCODE_OPENROUTER_NO_FALLBACK",
    ];

    for var in vars {
        crate::env::remove_var(var);
    }

    if let Some(profile) = profile {
        let resolved = resolve_openai_compatible_profile(profile);
        crate::env::set_var("JCODE_OPENROUTER_API_BASE", &resolved.api_base);
        crate::env::set_var("JCODE_OPENROUTER_API_KEY_NAME", &resolved.api_key_env);
        crate::env::set_var("JCODE_OPENROUTER_ENV_FILE", &resolved.env_file);
        crate::env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", &resolved.id);
        crate::env::set_var("JCODE_OPENROUTER_PROVIDER_FEATURES", "0");
        if resolved.requires_api_key {
            crate::env::remove_var("JCODE_OPENROUTER_ALLOW_NO_AUTH");
        } else {
            crate::env::set_var("JCODE_OPENROUTER_ALLOW_NO_AUTH", "1");
        }
    }
}

pub fn openrouter_like_api_key_sources() -> Vec<(String, String)> {
    let mut sources = Vec::with_capacity(10);
    sources.push((
        "OPENROUTER_API_KEY".to_string(),
        "openrouter.env".to_string(),
    ));

    for profile in openai_compatible_profiles() {
        if profile.requires_api_key {
            sources.push((
                profile.api_key_env.to_string(),
                profile.env_file.to_string(),
            ));
        }
    }

    if let Some(source) = configured_api_key_source(
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "OPENROUTER_API_KEY",
        "openrouter.env",
    ) {
        sources.push(source);
    }

    if let Some(source) = configured_api_key_source(
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
        OPENAI_COMPAT_PROFILE.api_key_env,
        OPENAI_COMPAT_PROFILE.env_file,
    ) {
        sources.push(source);
    }

    dedup_sources(sources)
}

fn parse_bool_like(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub fn openai_compatible_profile_is_configured(profile: OpenAiCompatibleProfile) -> bool {
    let resolved = resolve_openai_compatible_profile(profile);
    if load_api_key_from_env_or_config(&resolved.api_key_env, &resolved.env_file).is_some() {
        return true;
    }

    if resolved.requires_api_key {
        return false;
    }

    load_env_value_from_env_or_config(OPENAI_COMPAT_LOCAL_ENABLED_ENV, &resolved.env_file)
        .map(|value| parse_bool_like(&value))
        .unwrap_or(false)
}

pub fn configured_api_key_source(
    key_var: &str,
    file_var: &str,
    default_key: &str,
    default_file: &str,
) -> Option<(String, String)> {
    if std::env::var_os(key_var).is_none() && std::env::var_os(file_var).is_none() {
        return None;
    }

    let env_key = std::env::var(key_var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default_key.to_string());
    let file_name = std::env::var(file_var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default_file.to_string());

    if !is_safe_env_key_name(&env_key) {
        crate::logging::warn(&format!(
            "Ignoring invalid {}='{}' while probing auth status",
            key_var, env_key
        ));
        return None;
    }
    if !is_safe_env_file_name(&file_name) {
        crate::logging::warn(&format!(
            "Ignoring invalid {}='{}' while probing auth status",
            file_var, file_name
        ));
        return None;
    }

    Some((env_key, file_name))
}

pub fn load_api_key_from_env_or_config(env_key: &str, file_name: &str) -> Option<String> {
    if !is_safe_env_key_name(env_key) {
        crate::logging::warn(&format!(
            "Ignoring invalid API key variable name '{}' while loading credentials",
            env_key
        ));
        return None;
    }
    if !is_safe_env_file_name(file_name) {
        crate::logging::warn(&format!(
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

    let config_path = crate::storage::app_config_dir().ok()?.join(file_name);
    crate::storage::harden_secret_file_permissions(&config_path);
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

    None
}

pub fn load_env_value_from_env_or_config(env_key: &str, file_name: &str) -> Option<String> {
    if !is_safe_env_key_name(env_key) {
        crate::logging::warn(&format!(
            "Ignoring invalid variable name '{}' while loading config value",
            env_key
        ));
        return None;
    }
    if !is_safe_env_file_name(file_name) {
        crate::logging::warn(&format!(
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

    let config_path = crate::storage::app_config_dir().ok()?.join(file_name);
    crate::storage::harden_secret_file_permissions(&config_path);
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

    let config_dir = crate::storage::app_config_dir()?;
    let file_path = config_dir.join(file_name);
    crate::storage::upsert_env_file_value(&file_path, env_key, value)?;

    if let Some(value) = value {
        crate::env::set_var(env_key, value);
    } else {
        crate::env::remove_var(env_key);
    }

    Ok(())
}

fn env_override(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| load_env_value_from_env_or_config(name, OPENAI_COMPAT_PROFILE.env_file))
}

fn dedup_sources(sources: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(sources.len());
    for (env_key, env_file) in sources {
        if seen.insert((env_key.clone(), env_file.clone())) {
            deduped.push((env_key, env_file));
        }
    }
    deduped
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvGuard {
        vars: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn save(keys: &[&str]) -> Self {
            let vars = keys
                .iter()
                .map(|key| (key.to_string(), std::env::var(key).ok()))
                .collect();
            Self { vars }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.vars {
                if let Some(value) = value {
                    crate::env::set_var(key, value);
                } else {
                    crate::env::remove_var(key);
                }
            }
        }
    }

    #[test]
    fn matrix_profiles_have_unique_ids_and_safe_metadata() {
        let mut ids = HashSet::new();
        for profile in openai_compatible_profiles() {
            assert!(
                ids.insert(profile.id),
                "duplicate provider profile id: {}",
                profile.id
            );
            assert!(is_safe_env_key_name(profile.api_key_env));
            assert!(is_safe_env_file_name(profile.env_file));
            assert_eq!(
                normalize_api_base(profile.api_base).as_deref(),
                Some(profile.api_base)
            );
        }
    }

    #[test]
    fn matrix_login_provider_aliases_resolve_to_canonical_ids() {
        assert_eq!(
            resolve_login_provider("subscription").map(|provider| provider.id),
            Some("jcode")
        );
        assert_eq!(
            resolve_login_provider("anthropic").map(|provider| provider.id),
            Some("claude")
        );
        assert_eq!(
            resolve_login_provider("opencodego").map(|provider| provider.id),
            Some("opencode-go")
        );
        assert_eq!(
            resolve_login_provider("z.ai").map(|provider| provider.id),
            Some("zai")
        );
        assert_eq!(
            resolve_login_provider("compat").map(|provider| provider.id),
            Some("openai-compatible")
        );
        assert_eq!(
            resolve_login_provider("aoai").map(|provider| provider.id),
            Some("azure")
        );
        assert_eq!(
            resolve_login_provider("cerberascode").map(|provider| provider.id),
            Some("cerebras")
        );
        assert_eq!(
            resolve_login_provider("bailian").map(|provider| provider.id),
            Some("alibaba-coding-plan")
        );
        assert_eq!(
            resolve_login_provider("gmail").map(|provider| provider.id),
            Some("google")
        );
    }

    #[test]
    fn matrix_login_provider_ids_and_aliases_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for provider in login_providers() {
            assert!(
                seen.insert(provider.id),
                "duplicate login provider identifier: {}",
                provider.id
            );
            for alias in provider.aliases {
                assert!(
                    seen.insert(*alias),
                    "duplicate login provider alias: {}",
                    alias
                );
            }
        }
    }

    #[test]
    fn matrix_tui_login_selection_supports_numbers_and_names() {
        let providers = tui_login_providers();
        assert_eq!(
            resolve_login_selection("1", &providers).map(|provider| provider.id),
            Some("claude")
        );
        assert_eq!(
            resolve_login_selection("14", &providers).map(|provider| provider.id),
            Some("cursor")
        );
        assert_eq!(
            resolve_login_selection("compat", &providers).map(|provider| provider.id),
            Some("openai-compatible")
        );
        assert!(resolve_login_selection("google", &providers).is_none());
    }

    #[test]
    fn matrix_cli_login_selection_preserves_existing_order() {
        let providers = cli_login_providers();
        assert_eq!(
            resolve_login_selection("3", &providers).map(|provider| provider.id),
            Some("jcode")
        );
        assert_eq!(
            resolve_login_selection("4", &providers).map(|provider| provider.id),
            Some("copilot")
        );
        assert_eq!(
            resolve_login_selection("5", &providers).map(|provider| provider.id),
            Some("openrouter")
        );
        assert_eq!(
            resolve_login_selection("6", &providers).map(|provider| provider.id),
            Some("azure")
        );
        assert_eq!(
            resolve_login_selection("16", &providers).map(|provider| provider.id),
            Some("gemini")
        );
        assert_eq!(
            resolve_login_selection("17", &providers).map(|provider| provider.id),
            Some("google")
        );
    }

    #[test]
    fn matrix_openrouter_like_sources_include_all_static_profiles() {
        let _lock = crate::storage::lock_test_env();
        let guard = EnvGuard::save(&[
            "JCODE_OPENROUTER_API_KEY_NAME",
            "JCODE_OPENROUTER_ENV_FILE",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME",
            "JCODE_OPENAI_COMPAT_ENV_FILE",
        ]);
        crate::env::remove_var("JCODE_OPENROUTER_API_KEY_NAME");
        crate::env::remove_var("JCODE_OPENROUTER_ENV_FILE");
        crate::env::remove_var("JCODE_OPENAI_COMPAT_API_KEY_NAME");
        crate::env::remove_var("JCODE_OPENAI_COMPAT_ENV_FILE");

        let sources = openrouter_like_api_key_sources();
        drop(guard);

        assert!(sources.contains(&(
            "OPENROUTER_API_KEY".to_string(),
            "openrouter.env".to_string()
        )));
        for profile in openai_compatible_profiles() {
            if profile.requires_api_key {
                assert!(sources.contains(&(
                    profile.api_key_env.to_string(),
                    profile.env_file.to_string()
                )));
            }
        }
    }

    #[test]
    fn matrix_openrouter_like_sources_accept_valid_overrides() {
        let _lock = crate::storage::lock_test_env();
        let _guard = EnvGuard::save(&[
            "JCODE_OPENROUTER_API_KEY_NAME",
            "JCODE_OPENROUTER_ENV_FILE",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME",
            "JCODE_OPENAI_COMPAT_ENV_FILE",
        ]);

        crate::env::set_var("JCODE_OPENROUTER_API_KEY_NAME", "ALT_OPENROUTER_KEY");
        crate::env::set_var("JCODE_OPENROUTER_ENV_FILE", "alt-openrouter.env");
        crate::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "ALT_COMPAT_KEY");
        crate::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "alt-compat.env");

        let sources = openrouter_like_api_key_sources();
        assert!(sources.contains(&(
            "ALT_OPENROUTER_KEY".to_string(),
            "alt-openrouter.env".to_string()
        )));
        assert!(sources.contains(&("ALT_COMPAT_KEY".to_string(), "alt-compat.env".to_string())));
    }

    #[test]
    fn matrix_openrouter_like_sources_reject_invalid_overrides() {
        let _lock = crate::storage::lock_test_env();
        let _guard = EnvGuard::save(&[
            "JCODE_OPENROUTER_API_KEY_NAME",
            "JCODE_OPENROUTER_ENV_FILE",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME",
            "JCODE_OPENAI_COMPAT_ENV_FILE",
        ]);

        crate::env::set_var("JCODE_OPENROUTER_API_KEY_NAME", "bad-key-name");
        crate::env::set_var("JCODE_OPENROUTER_ENV_FILE", "../bad.env");
        crate::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "bad key");
        crate::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "../bad-compat.env");

        let sources = openrouter_like_api_key_sources();
        assert!(
            !sources
                .iter()
                .any(|(key, _)| key == "bad-key-name" || key == "bad key")
        );
        assert!(
            !sources
                .iter()
                .any(|(_, file)| file == "../bad.env" || file == "../bad-compat.env")
        );
    }

    #[test]
    fn matrix_openai_compatible_profile_overrides_apply_when_valid() {
        let _lock = crate::storage::lock_test_env();
        let _guard = EnvGuard::save(&[
            "JCODE_OPENAI_COMPAT_API_BASE",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME",
            "JCODE_OPENAI_COMPAT_ENV_FILE",
            "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
        ]);

        crate::env::set_var(
            "JCODE_OPENAI_COMPAT_API_BASE",
            "https://api.groq.com/openai/v1/",
        );
        crate::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "GROQ_API_KEY");
        crate::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "groq.env");
        crate::env::set_var("JCODE_OPENAI_COMPAT_DEFAULT_MODEL", "openai/gpt-oss-120b");

        let resolved = resolve_openai_compatible_profile(OPENAI_COMPAT_PROFILE);
        assert_eq!(resolved.api_base, "https://api.groq.com/openai/v1");
        assert_eq!(resolved.api_key_env, "GROQ_API_KEY");
        assert_eq!(resolved.env_file, "groq.env");
        assert_eq!(
            resolved.default_model.as_deref(),
            Some("openai/gpt-oss-120b")
        );
    }

    #[test]
    fn matrix_openai_compatible_profile_overrides_reject_invalid_values() {
        let _lock = crate::storage::lock_test_env();
        let _guard = EnvGuard::save(&[
            "JCODE_OPENAI_COMPAT_API_BASE",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME",
            "JCODE_OPENAI_COMPAT_ENV_FILE",
        ]);

        crate::env::set_var("JCODE_OPENAI_COMPAT_API_BASE", "http://example.com/v1");
        crate::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "bad-key-name");
        crate::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "../bad.env");

        let resolved = resolve_openai_compatible_profile(OPENAI_COMPAT_PROFILE);
        assert_eq!(resolved.api_base, OPENAI_COMPAT_PROFILE.api_base);
        assert_eq!(resolved.api_key_env, OPENAI_COMPAT_PROFILE.api_key_env);
        assert_eq!(resolved.env_file, OPENAI_COMPAT_PROFILE.env_file);
    }

    #[test]
    fn matrix_openai_compatible_profile_overrides_read_from_env_file() {
        let _lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let config_root = temp.path().join("config").join("jcode");
        std::fs::create_dir_all(&config_root).expect("config dir");

        let _guard = EnvGuard::save(&[
            "JCODE_HOME",
            "JCODE_OPENAI_COMPAT_API_BASE",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME",
            "JCODE_OPENAI_COMPAT_ENV_FILE",
            "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
        ]);
        crate::env::set_var("JCODE_HOME", temp.path());
        crate::env::remove_var("JCODE_OPENAI_COMPAT_API_BASE");
        crate::env::remove_var("JCODE_OPENAI_COMPAT_API_KEY_NAME");
        crate::env::remove_var("JCODE_OPENAI_COMPAT_ENV_FILE");
        crate::env::remove_var("JCODE_OPENAI_COMPAT_DEFAULT_MODEL");
        std::fs::write(
            config_root.join(OPENAI_COMPAT_PROFILE.env_file),
            concat!(
                "JCODE_OPENAI_COMPAT_API_BASE=https://api.example.com/v1\n",
                "JCODE_OPENAI_COMPAT_API_KEY_NAME=EXAMPLE_API_KEY\n",
                "JCODE_OPENAI_COMPAT_ENV_FILE=example.env\n",
                "JCODE_OPENAI_COMPAT_DEFAULT_MODEL=example/model\n",
            ),
        )
        .expect("env file");

        let resolved = resolve_openai_compatible_profile(OPENAI_COMPAT_PROFILE);
        assert_eq!(resolved.api_base, "https://api.example.com/v1");
        assert_eq!(resolved.api_key_env, "EXAMPLE_API_KEY");
        assert_eq!(resolved.env_file, "example.env");
        assert_eq!(resolved.default_model.as_deref(), Some("example/model"));
    }

    #[test]
    fn matrix_load_api_key_from_env_or_config_prefers_env() {
        let _lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let config_root = temp.path().join("config").join("jcode");
        std::fs::create_dir_all(&config_root).expect("config dir");

        let _guard = EnvGuard::save(&["JCODE_HOME", "OPENCODE_API_KEY"]);
        crate::env::set_var("JCODE_HOME", temp.path());
        crate::env::set_var("OPENCODE_API_KEY", "env-secret");
        std::fs::write(
            config_root.join("opencode.env"),
            "OPENCODE_API_KEY=file-secret\n",
        )
        .expect("env file");

        assert_eq!(
            load_api_key_from_env_or_config("OPENCODE_API_KEY", "opencode.env").as_deref(),
            Some("env-secret")
        );
    }

    #[test]
    fn matrix_load_api_key_from_env_or_config_reads_config_file() {
        let _lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let config_root = temp.path().join("config").join("jcode");
        std::fs::create_dir_all(&config_root).expect("config dir");

        let _guard = EnvGuard::save(&["JCODE_HOME", "OPENCODE_API_KEY"]);
        crate::env::set_var("JCODE_HOME", temp.path());
        crate::env::remove_var("OPENCODE_API_KEY");
        std::fs::write(
            config_root.join("opencode.env"),
            "OPENCODE_API_KEY=file-secret\n",
        )
        .expect("env file");

        assert_eq!(
            load_api_key_from_env_or_config("OPENCODE_API_KEY", "opencode.env").as_deref(),
            Some("file-secret")
        );
    }

    #[test]
    fn load_api_key_accepts_legacy_zai_key_name() {
        let _lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let config_root = temp.path().join("config").join("jcode");
        std::fs::create_dir_all(&config_root).expect("config dir");

        let _guard = EnvGuard::save(&["JCODE_HOME", "ZHIPU_API_KEY", "ZAI_API_KEY"]);
        crate::env::set_var("JCODE_HOME", temp.path());
        crate::env::remove_var("ZHIPU_API_KEY");
        crate::env::remove_var("ZAI_API_KEY");
        std::fs::write(config_root.join("zai.env"), "ZAI_API_KEY=legacy-secret\n")
            .expect("env file");

        assert_eq!(
            load_api_key_from_env_or_config("ZHIPU_API_KEY", "zai.env").as_deref(),
            Some("legacy-secret")
        );
    }
}
