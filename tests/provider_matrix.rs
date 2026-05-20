use anyhow::Result;
use jcode::auth::{AuthState, AuthStatus};
use jcode::provider::Provider;
use jcode::provider::openrouter::OpenRouterProvider;
use jcode::provider_catalog::{
    OPENAI_COMPAT_PROFILE, apply_openai_compatible_profile_env, load_api_key_from_env_or_config,
    openai_compatible_profile_is_configured, openai_compatible_profiles,
    resolve_openai_compatible_profile, save_env_value_to_env_file,
};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn lock_env() -> MutexGuard<'static, ()> {
    let mutex = ENV_LOCK.get_or_init(|| Mutex::new(()));
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn tracked_env_vars() -> Vec<String> {
    let mut keys: HashSet<String> = [
        "JCODE_HOME",
        "XDG_CONFIG_HOME",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_OPENROUTER_PROVIDER",
        "JCODE_OPENROUTER_NO_FALLBACK",
        "JCODE_OPENROUTER_MODEL",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_STATIC_MODELS",
        "JCODE_OPENROUTER_AUTH_HEADER",
        "JCODE_OPENROUTER_AUTH_HEADER_NAME",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
        "JCODE_OPENROUTER_THINKING",
        "JCODE_OPENAI_COMPAT_API_BASE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
        "JCODE_OPENAI_COMPAT_SETUP_URL",
        "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
        "JCODE_OPENAI_COMPAT_LOCAL_ENABLED",
        "JCODE_NAMED_PROVIDER_PROFILE",
        "JCODE_PROVIDER_PROFILE_ACTIVE",
        "JCODE_PROVIDER_PROFILE_NAME",
        "OPENROUTER_API_KEY",
    ]
    .into_iter()
    .map(ToString::to_string)
    .collect();

    for profile in openai_compatible_profiles() {
        keys.insert(profile.api_key_env.to_string());
    }

    let mut keys: Vec<_> = keys.into_iter().collect();
    keys.sort();
    keys
}

struct TestEnv {
    _lock: MutexGuard<'static, ()>,
    saved: Vec<(String, Option<String>)>,
    temp: tempfile::TempDir,
}

impl TestEnv {
    fn new() -> Result<Self> {
        let lock = lock_env();
        let temp = tempfile::Builder::new()
            .prefix("jcode-provider-matrix-")
            .tempdir()?;
        let saved = tracked_env_vars()
            .into_iter()
            .map(|key| {
                let value = std::env::var(&key).ok();
                (key, value)
            })
            .collect::<Vec<_>>();

        for (key, _) in &saved {
            jcode::env::remove_var(key);
        }

        let config_root = temp.path().join("config").join("jcode");
        std::fs::create_dir_all(&config_root)?;
        jcode::env::set_var("JCODE_HOME", temp.path());
        apply_openai_compatible_profile_env(None);
        AuthStatus::invalidate_cache();

        Ok(Self {
            _lock: lock,
            saved,
            temp,
        })
    }

    fn config_dir(&self) -> PathBuf {
        self.temp.path().join("config").join("jcode")
    }

    fn clear_profile_keys(&self) {
        jcode::env::remove_var("OPENROUTER_API_KEY");
        for profile in openai_compatible_profiles() {
            jcode::env::remove_var(profile.api_key_env);
        }
        AuthStatus::invalidate_cache();
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        apply_openai_compatible_profile_env(None);
        AuthStatus::invalidate_cache();
        for (key, value) in &self.saved {
            if let Some(value) = value {
                jcode::env::set_var(key, value);
            } else {
                jcode::env::remove_var(key);
            }
        }
        AuthStatus::invalidate_cache();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenAiCompatibleBaseState {
    DefaultRemote,
    SavedRemote,
    SavedLocal,
}

impl OpenAiCompatibleBaseState {
    fn expected_api_base(self) -> &'static str {
        match self {
            Self::DefaultRemote => OPENAI_COMPAT_PROFILE.api_base,
            Self::SavedRemote => "https://state-space-openai-compatible.test/v1",
            Self::SavedLocal => "http://localhost:11434/v1",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::DefaultRemote => "default",
            Self::SavedRemote => "remote",
            Self::SavedLocal => "local",
        }
    }
}

fn clear_openai_compatible_runtime_env() {
    for key in [
        "JCODE_OPENAI_COMPAT_API_BASE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
        "JCODE_OPENAI_COMPAT_SETUP_URL",
        "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
        "JCODE_OPENAI_COMPAT_LOCAL_ENABLED",
        "OPENAI_COMPAT_API_KEY",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_MODEL",
        "JCODE_OPENROUTER_STATIC_MODELS",
    ] {
        jcode::env::remove_var(key);
    }
    AuthStatus::invalidate_cache();
}

#[test]
fn provider_matrix_openai_compatible_auth_state_space_material_states_preserve_login_invariants()
-> Result<()> {
    let base_states = [
        OpenAiCompatibleBaseState::DefaultRemote,
        OpenAiCompatibleBaseState::SavedRemote,
        OpenAiCompatibleBaseState::SavedLocal,
    ];

    for base_state in base_states {
        for has_key in [false, true] {
            for has_default_model in [false, true] {
                for restarted in [false, true] {
                    let env = TestEnv::new()?;
                    env.clear_profile_keys();
                    let state_label = format!(
                        "base={base_state:?}, key={has_key}, default_model={has_default_model}, restarted={restarted}"
                    );
                    let model = format!(
                        "state-space-{}-{}-{}-model",
                        base_state.label(),
                        if has_key { "key" } else { "nokey" },
                        if restarted { "restart" } else { "hot" },
                    );
                    let env_file = OPENAI_COMPAT_PROFILE.env_file;

                    match base_state {
                        OpenAiCompatibleBaseState::DefaultRemote => {}
                        OpenAiCompatibleBaseState::SavedRemote
                        | OpenAiCompatibleBaseState::SavedLocal => {
                            save_env_value_to_env_file(
                                "JCODE_OPENAI_COMPAT_API_BASE",
                                env_file,
                                Some(base_state.expected_api_base()),
                            )?;
                        }
                    }

                    if has_key {
                        save_env_value_to_env_file(
                            "OPENAI_COMPAT_API_KEY",
                            env_file,
                            Some("sk-state-space-login"),
                        )?;
                    }

                    if has_default_model {
                        save_env_value_to_env_file(
                            "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
                            env_file,
                            Some(&model),
                        )?;
                    }

                    if restarted {
                        // Simulate a new process with only persisted login/config files.
                        clear_openai_compatible_runtime_env();
                    }

                    let resolved = resolve_openai_compatible_profile(OPENAI_COMPAT_PROFILE);
                    assert_eq!(
                        resolved.api_base,
                        base_state.expected_api_base(),
                        "api base mismatch for {state_label}"
                    );
                    assert_eq!(
                        resolved.requires_api_key,
                        base_state != OpenAiCompatibleBaseState::SavedLocal,
                        "requires_api_key mismatch for {state_label}"
                    );
                    assert_eq!(
                        resolved.default_model.as_deref(),
                        has_default_model.then_some(model.as_str()),
                        "default model mismatch for {state_label}"
                    );

                    let loaded_key =
                        load_api_key_from_env_or_config(&resolved.api_key_env, &resolved.env_file);
                    assert_eq!(
                        loaded_key.as_deref(),
                        has_key.then_some("sk-state-space-login"),
                        "saved key mismatch for {state_label}"
                    );

                    let expected_configured =
                        has_key || matches!(base_state, OpenAiCompatibleBaseState::SavedLocal);
                    assert_eq!(
                        openai_compatible_profile_is_configured(OPENAI_COMPAT_PROFILE),
                        expected_configured,
                        "configured predicate mismatch for {state_label}"
                    );

                    apply_openai_compatible_profile_env(Some(OPENAI_COMPAT_PROFILE));
                    AuthStatus::invalidate_cache();
                    assert_eq!(
                        std::env::var("JCODE_OPENROUTER_API_BASE").ok().as_deref(),
                        Some(resolved.api_base.as_str()),
                        "runtime api base mismatch for {state_label}"
                    );
                    assert_eq!(
                        std::env::var("JCODE_OPENROUTER_API_KEY_NAME")
                            .ok()
                            .as_deref(),
                        Some(resolved.api_key_env.as_str()),
                        "runtime api key env mismatch for {state_label}"
                    );
                    assert_eq!(
                        std::env::var("JCODE_OPENROUTER_ENV_FILE").ok().as_deref(),
                        Some(resolved.env_file.as_str()),
                        "runtime env file mismatch for {state_label}"
                    );
                    assert_eq!(
                        std::env::var("JCODE_OPENROUTER_ALLOW_NO_AUTH")
                            .ok()
                            .as_deref(),
                        (base_state == OpenAiCompatibleBaseState::SavedLocal).then_some("1"),
                        "runtime no-auth flag mismatch for {state_label}"
                    );
                    assert_eq!(
                        OpenRouterProvider::has_credentials(),
                        expected_configured,
                        "runtime credentials mismatch for {state_label}"
                    );

                    let provider = OpenRouterProvider::new();
                    if expected_configured {
                        let provider = provider.unwrap_or_else(|err| {
                            panic!("provider should construct for {state_label}: {err}")
                        });
                        provider.set_model(&model)?;
                        assert_eq!(
                            provider.model(),
                            model,
                            "selected model mismatch for {state_label}"
                        );
                        assert!(
                            provider
                                .available_models_display()
                                .iter()
                                .any(|available| available == &model),
                            "configured model should be immediately visible for {state_label}"
                        );
                        let routes = provider.model_routes();
                        assert!(
                            routes.iter().any(|route| {
                                route.provider == "OpenAI-compatible"
                                    && route.api_method == "openai-compatible:openai-compatible"
                                    && route.model == model
                                    && route.available
                            }),
                            "configured model route should be immediately visible for {state_label}; routes: {routes:?}"
                        );
                    } else {
                        assert!(
                            provider.is_err(),
                            "provider should not construct without credentials for {state_label}"
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

#[test]
fn provider_matrix_env_credentials_activate_openrouter_runtime() -> Result<()> {
    let env = TestEnv::new()?;

    for &profile in openai_compatible_profiles() {
        env.clear_profile_keys();
        apply_openai_compatible_profile_env(Some(profile));
        let resolved = resolve_openai_compatible_profile(profile);
        jcode::env::set_var(&resolved.api_key_env, "matrix-env-secret");
        AuthStatus::invalidate_cache();

        assert_eq!(
            std::env::var("JCODE_OPENROUTER_API_BASE").ok().as_deref(),
            Some(resolved.api_base.as_str())
        );
        assert_eq!(
            std::env::var("JCODE_OPENROUTER_API_KEY_NAME")
                .ok()
                .as_deref(),
            Some(resolved.api_key_env.as_str())
        );
        assert_eq!(
            std::env::var("JCODE_OPENROUTER_ENV_FILE").ok().as_deref(),
            Some(resolved.env_file.as_str())
        );
        assert_eq!(
            std::env::var("JCODE_OPENROUTER_CACHE_NAMESPACE")
                .ok()
                .as_deref(),
            Some(resolved.id.as_str())
        );
        assert_eq!(
            std::env::var("JCODE_OPENROUTER_PROVIDER_FEATURES")
                .ok()
                .as_deref(),
            Some("0")
        );
        assert!(
            OpenRouterProvider::has_credentials(),
            "expected credentials for {}",
            resolved.id
        );
        OpenRouterProvider::new()?;
        assert_eq!(AuthStatus::check().openrouter, AuthState::Available);

        jcode::env::remove_var(&resolved.api_key_env);
    }

    Ok(())
}

#[test]
fn provider_matrix_file_credentials_activate_openrouter_runtime() -> Result<()> {
    let env = TestEnv::new()?;

    for &profile in openai_compatible_profiles() {
        env.clear_profile_keys();
        apply_openai_compatible_profile_env(Some(profile));
        let resolved = resolve_openai_compatible_profile(profile);
        let env_file = env.config_dir().join(&resolved.env_file);
        std::fs::write(
            &env_file,
            format!("{}=matrix-file-secret\n", resolved.api_key_env),
        )?;
        AuthStatus::invalidate_cache();

        assert!(
            OpenRouterProvider::has_credentials(),
            "expected file credentials for {}",
            resolved.id
        );
        OpenRouterProvider::new()?;
        assert_eq!(AuthStatus::check().openrouter, AuthState::Available);

        std::fs::remove_file(env_file)?;
    }

    Ok(())
}

#[test]
fn provider_matrix_custom_compat_overrides_flow_into_runtime() -> Result<()> {
    let env = TestEnv::new()?;
    env.clear_profile_keys();

    jcode::env::set_var(
        "JCODE_OPENAI_COMPAT_API_BASE",
        "https://api.groq.com/openai/v1/",
    );
    jcode::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "GROQ_API_KEY");
    jcode::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "groq.env");
    jcode::env::set_var("JCODE_OPENAI_COMPAT_DEFAULT_MODEL", "openai/gpt-oss-120b");

    apply_openai_compatible_profile_env(Some(OPENAI_COMPAT_PROFILE));
    let resolved = resolve_openai_compatible_profile(OPENAI_COMPAT_PROFILE);
    let env_file = env.config_dir().join(&resolved.env_file);
    std::fs::write(
        &env_file,
        format!("{}=matrix-file-secret\n", resolved.api_key_env),
    )?;
    AuthStatus::invalidate_cache();

    assert_eq!(resolved.api_base, "https://api.groq.com/openai/v1");
    assert_eq!(resolved.api_key_env, "GROQ_API_KEY");
    assert_eq!(resolved.env_file, "groq.env");
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_BASE").ok().as_deref(),
        Some("https://api.groq.com/openai/v1")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_KEY_NAME")
            .ok()
            .as_deref(),
        Some("GROQ_API_KEY")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_ENV_FILE").ok().as_deref(),
        Some("groq.env")
    );
    assert!(OpenRouterProvider::has_credentials());
    OpenRouterProvider::new()?;
    assert_eq!(AuthStatus::check().openrouter, AuthState::Available);

    Ok(())
}

#[test]
fn provider_matrix_custom_local_compat_without_api_key_activates_openrouter_runtime() -> Result<()>
{
    let env = TestEnv::new()?;
    env.clear_profile_keys();

    jcode::env::set_var("JCODE_OPENAI_COMPAT_API_BASE", "http://localhost:11434/v1");

    apply_openai_compatible_profile_env(Some(OPENAI_COMPAT_PROFILE));
    let resolved = resolve_openai_compatible_profile(OPENAI_COMPAT_PROFILE);
    AuthStatus::invalidate_cache();

    assert_eq!(resolved.api_base, "http://localhost:11434/v1");
    assert!(!resolved.requires_api_key);
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_ALLOW_NO_AUTH")
            .ok()
            .as_deref(),
        Some("1")
    );
    assert!(OpenRouterProvider::has_credentials());
    OpenRouterProvider::new()?;
    assert_eq!(AuthStatus::check().openrouter, AuthState::Available);

    Ok(())
}
