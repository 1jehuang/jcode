use anyhow::Result;
use jcode::auth::{AuthState, AuthStatus};
use jcode::cli::provider_init::{
    ProviderChoice, apply_login_provider_profile_env, choice_for_login_provider,
    init_provider_for_validation,
};
use jcode::provider::Provider;
use jcode::provider::openrouter::OpenRouterProvider;
use jcode::provider_catalog::{
    LoginProviderDescriptor, LoginProviderTarget, OPENAI_COMPAT_PROFILE, OpenAiCompatibleProfile,
    apply_openai_compatible_profile_env, load_api_key_from_env_or_config, login_providers,
    openai_compatible_profile_is_configured, openai_compatible_profiles,
    resolve_openai_compatible_profile, save_env_value_to_env_file,
    server_bootstrap_login_providers,
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
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "COPILOT_GITHUB_TOKEN",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "CURSOR_API_KEY",
        "GEMINI_API_KEY",
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
        jcode::config::invalidate_config_cache();
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

    fn config_file(&self) -> PathBuf {
        self.temp.path().join("config.toml")
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
        jcode::config::invalidate_config_cache();
        for (key, value) in &self.saved {
            if let Some(value) = value {
                jcode::env::set_var(key, value);
            } else {
                jcode::env::remove_var(key);
            }
        }
        AuthStatus::invalidate_cache();
        jcode::config::invalidate_config_cache();
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
        "JCODE_PROVIDER_PROFILE_ACTIVE",
        "JCODE_PROVIDER_PROFILE_NAME",
        "JCODE_NAMED_PROVIDER_PROFILE",
    ] {
        jcode::env::remove_var(key);
    }
    AuthStatus::invalidate_cache();
}

#[derive(Debug, Clone, Copy)]
enum CompetingCompatibleState {
    None,
    OtherSavedFiles,
    OtherSavedFilesAndConfigDefault,
}

fn openai_compatible_login_providers(
    providers: impl IntoIterator<Item = LoginProviderDescriptor>,
) -> Vec<LoginProviderDescriptor> {
    providers
        .into_iter()
        .filter(|provider| matches!(provider.target, LoginProviderTarget::OpenAiCompatible(_)))
        .collect()
}

fn non_compatible_login_providers(
    providers: impl IntoIterator<Item = LoginProviderDescriptor>,
) -> Vec<LoginProviderDescriptor> {
    providers
        .into_iter()
        .filter(|provider| {
            !matches!(
                provider.target,
                LoginProviderTarget::OpenAiCompatible(_)
                    | LoginProviderTarget::AutoImport
                    | LoginProviderTarget::Google
            )
        })
        .collect()
}

fn login_provider_profile(provider: LoginProviderDescriptor) -> OpenAiCompatibleProfile {
    match provider.target {
        LoginProviderTarget::OpenAiCompatible(profile) => profile,
        _ => panic!("{} is not an OpenAI-compatible login provider", provider.id),
    }
}

fn write_profile_api_key_file(
    env: &TestEnv,
    profile: OpenAiCompatibleProfile,
    value: &str,
) -> Result<()> {
    let resolved = resolve_openai_compatible_profile(profile);
    let path = env.config_dir().join(&resolved.env_file);
    std::fs::create_dir_all(env.config_dir())?;
    std::fs::write(&path, format!("{}={value}\n", resolved.api_key_env))?;
    jcode::env::remove_var(&resolved.api_key_env);
    AuthStatus::invalidate_cache();
    Ok(())
}

fn write_profile_login_material(
    env: &TestEnv,
    profile: OpenAiCompatibleProfile,
    label: &str,
) -> Result<()> {
    let resolved = resolve_openai_compatible_profile(profile);
    if resolved.requires_api_key {
        write_profile_api_key_file(env, profile, &format!("sk-{label}-{}", resolved.id))?;
    }
    Ok(())
}

fn competing_remote_profiles(selected: OpenAiCompatibleProfile) -> Vec<OpenAiCompatibleProfile> {
    openai_compatible_profiles()
        .iter()
        .copied()
        .filter(|profile| profile.id != selected.id)
        .filter(|profile| resolve_openai_compatible_profile(*profile).requires_api_key)
        .take(2)
        .collect()
}

fn apply_competing_compatible_state(
    env: &TestEnv,
    selected: OpenAiCompatibleProfile,
    state: CompetingCompatibleState,
) -> Result<()> {
    match state {
        CompetingCompatibleState::None => {}
        CompetingCompatibleState::OtherSavedFiles
        | CompetingCompatibleState::OtherSavedFilesAndConfigDefault => {
            let competitors = competing_remote_profiles(selected);
            assert!(
                !competitors.is_empty(),
                "provider catalog should have at least one competitor for {}",
                selected.id
            );
            for (index, competitor) in competitors.iter().copied().enumerate() {
                write_profile_api_key_file(
                    env,
                    competitor,
                    &format!("sk-competing-{index}-{}", competitor.id),
                )?;
            }
            if matches!(
                state,
                CompetingCompatibleState::OtherSavedFilesAndConfigDefault
            ) {
                let default_provider = competitors[0].id;
                std::fs::write(
                    env.config_file(),
                    format!("[provider]\ndefault_provider = \"{default_provider}\"\n"),
                )?;
                jcode::config::invalidate_config_cache();
            }
        }
    }
    AuthStatus::invalidate_cache();
    Ok(())
}

fn assert_runtime_profile_env(profile: OpenAiCompatibleProfile, context: &str) {
    let resolved = resolve_openai_compatible_profile(profile);
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_BASE").ok().as_deref(),
        Some(resolved.api_base.as_str()),
        "runtime api base mismatch for {context}"
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_KEY_NAME")
            .ok()
            .as_deref(),
        Some(resolved.api_key_env.as_str()),
        "runtime api key env mismatch for {context}"
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_ENV_FILE").ok().as_deref(),
        Some(resolved.env_file.as_str()),
        "runtime env file mismatch for {context}"
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_CACHE_NAMESPACE")
            .ok()
            .as_deref(),
        Some(resolved.id.as_str()),
        "runtime cache namespace mismatch for {context}"
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_ALLOW_NO_AUTH")
            .ok()
            .as_deref(),
        (!resolved.requires_api_key).then_some("1"),
        "runtime no-auth flag mismatch for {context}"
    );
}

fn assert_no_compatible_runtime_profile_env(context: &str) {
    for key in [
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_OPENROUTER_STATIC_MODELS",
        "JCODE_PROVIDER_PROFILE_ACTIVE",
        "JCODE_PROVIDER_PROFILE_NAME",
        "JCODE_NAMED_PROVIDER_PROFILE",
    ] {
        assert!(
            std::env::var_os(key).is_none(),
            "{key} should be cleared for {context}; value={:?}",
            std::env::var_os(key)
        );
    }
}

fn assert_no_active_compatible_profile_lock(context: &str) {
    for key in [
        "JCODE_PROVIDER_PROFILE_ACTIVE",
        "JCODE_PROVIDER_PROFILE_NAME",
    ] {
        assert!(
            std::env::var_os(key).is_none(),
            "{key} should not stay locked for {context}; value={:?}",
            std::env::var_os(key)
        );
    }
}

fn seed_non_compatible_auto_auth(provider: LoginProviderDescriptor) -> bool {
    match provider.target {
        LoginProviderTarget::Claude => {
            jcode::env::set_var("ANTHROPIC_API_KEY", "test-anthropic-key");
            true
        }
        LoginProviderTarget::OpenAiApiKey => {
            jcode::env::set_var("OPENAI_API_KEY", "sk-test-openai-key");
            true
        }
        LoginProviderTarget::OpenRouter => {
            jcode::env::set_var("OPENROUTER_API_KEY", "sk-test-openrouter-key");
            true
        }
        LoginProviderTarget::Copilot => {
            jcode::env::set_var("COPILOT_GITHUB_TOKEN", "gho_test-copilot-token");
            true
        }
        LoginProviderTarget::Cursor => {
            jcode::env::set_var("CURSOR_API_KEY", "sk-test-cursor-key");
            true
        }
        _ => false,
    }
}

fn assert_model_picker_has_profile_route(
    provider: &dyn Provider,
    profile: OpenAiCompatibleProfile,
    context: &str,
) {
    let resolved = resolve_openai_compatible_profile(profile);
    let expected_api_method = format!("openai-compatible:{}", resolved.id);
    let routes = provider.model_routes();
    assert!(
        routes.iter().any(|route| {
            route.available
                && route.api_method == expected_api_method
                && !route.model.trim().is_empty()
        }),
        "model picker missing selected compatible provider route for {context}; expected_api_method={expected_api_method}, routes={routes:?}"
    );
}

#[tokio::test]
async fn provider_matrix_bootstrap_login_profile_survives_auto_daemon_state_space() -> Result<()> {
    let providers = openai_compatible_login_providers(server_bootstrap_login_providers());
    assert!(
        !providers.is_empty(),
        "server bootstrap login surface should include compatible providers"
    );

    for provider in providers {
        let selected = login_provider_profile(provider);
        for competing_state in [
            CompetingCompatibleState::None,
            CompetingCompatibleState::OtherSavedFiles,
            CompetingCompatibleState::OtherSavedFilesAndConfigDefault,
        ] {
            let env = TestEnv::new()?;
            env.clear_profile_keys();
            write_profile_login_material(&env, selected, "selected-bootstrap")?;
            apply_competing_compatible_state(&env, selected, competing_state)?;

            // Client-side bootstrap after a successful interactive login. The
            // actual daemon is spawned as `--provider auto`, so the selected
            // profile must survive the process boundary and auto-init path.
            apply_login_provider_profile_env(provider);
            AuthStatus::invalidate_cache();

            let context = format!(
                "bootstrap provider={} selected={} competing={competing_state:?}",
                provider.id, selected.id
            );
            assert_runtime_profile_env(selected, &context);

            let runtime = init_provider_for_validation(&ProviderChoice::Auto, None)
                .await
                .unwrap_or_else(|err| panic!("auto daemon init failed for {context}: {err}"));

            assert_runtime_profile_env(selected, &context);
            assert_model_picker_has_profile_route(runtime.as_ref(), selected, &context);
        }
    }

    Ok(())
}

#[tokio::test]
async fn provider_matrix_non_compatible_bootstrap_login_clears_stale_compatible_profile_state_space()
-> Result<()> {
    let stale_provider = openai_compatible_login_providers(server_bootstrap_login_providers())
        .into_iter()
        .next()
        .expect("compatible bootstrap provider");
    let stale_profile = login_provider_profile(stale_provider);
    let providers = non_compatible_login_providers(server_bootstrap_login_providers());
    assert!(
        !providers.is_empty(),
        "server bootstrap login surface should include non-compatible providers"
    );

    for provider in providers {
        let env = TestEnv::new()?;
        env.clear_profile_keys();
        write_profile_login_material(&env, stale_profile, "stale-non-compatible")?;
        apply_login_provider_profile_env(stale_provider);
        let context = format!(
            "non-compatible bootstrap provider={} stale_profile={}",
            provider.id, stale_profile.id
        );
        assert_runtime_profile_env(stale_profile, &context);

        apply_login_provider_profile_env(provider);
        AuthStatus::invalidate_cache();
        assert_no_compatible_runtime_profile_env(&context);

        if seed_non_compatible_auto_auth(provider) {
            AuthStatus::invalidate_cache();
            let runtime = init_provider_for_validation(&ProviderChoice::Auto, None)
                .await
                .unwrap_or_else(|err| panic!("auto init failed for {context}: {err}"));
            // Auto-init may legitimately rediscover the stale compatible key file
            // as an additional available route. The bug-prone invariant is that a
            // non-compatible selection must clear the active compatible-profile
            // lock so it cannot force the daemon/model picker to stay on that
            // previous profile.
            assert_no_active_compatible_profile_lock(&context);
            assert!(
                !runtime.model_routes().is_empty(),
                "model picker routes should remain renderable for {context}"
            );
        }
    }

    Ok(())
}

#[tokio::test]
async fn provider_matrix_concurrent_auto_init_preserves_bootstrap_compatible_profile() -> Result<()>
{
    let providers = openai_compatible_login_providers(server_bootstrap_login_providers());
    assert!(!providers.is_empty(), "compatible bootstrap providers");

    for provider in providers {
        let selected = login_provider_profile(provider);
        let env = TestEnv::new()?;
        env.clear_profile_keys();
        write_profile_login_material(&env, selected, "concurrent-auto")?;
        apply_login_provider_profile_env(provider);
        AuthStatus::invalidate_cache();

        let context = format!(
            "concurrent auto init provider={} selected={}",
            provider.id, selected.id
        );
        let (a, b, c, d) = tokio::join!(
            init_provider_for_validation(&ProviderChoice::Auto, None),
            init_provider_for_validation(&ProviderChoice::Auto, None),
            init_provider_for_validation(&ProviderChoice::Auto, None),
            init_provider_for_validation(&ProviderChoice::Auto, None),
        );
        for result in [a, b, c, d] {
            result.unwrap_or_else(|err| panic!("auto init failed for {context}: {err}"));
        }
        assert_runtime_profile_env(selected, &context);
    }

    Ok(())
}

#[tokio::test]
async fn provider_matrix_explicit_compatible_choice_overrides_stale_active_profile_state_space()
-> Result<()> {
    let providers = openai_compatible_login_providers(login_providers().iter().copied());
    assert!(
        providers.len() > 1,
        "provider catalog should include multiple compatible providers"
    );

    for provider in providers.iter().copied() {
        let selected = login_provider_profile(provider);
        let stale_provider = providers
            .iter()
            .copied()
            .find(|candidate| login_provider_profile(*candidate).id != selected.id)
            .expect("stale compatible provider");
        let stale = login_provider_profile(stale_provider);
        let choice = choice_for_login_provider(provider)
            .unwrap_or_else(|| panic!("{} should map to a ProviderChoice", provider.id));

        let env = TestEnv::new()?;
        env.clear_profile_keys();
        write_profile_login_material(&env, selected, "selected-direct")?;
        write_profile_login_material(&env, stale, "stale-direct")?;
        apply_competing_compatible_state(
            &env,
            selected,
            CompetingCompatibleState::OtherSavedFilesAndConfigDefault,
        )?;

        apply_login_provider_profile_env(stale_provider);
        AuthStatus::invalidate_cache();
        let context = format!(
            "explicit choice={} selected={} stale_active={}",
            provider.id, selected.id, stale.id
        );
        assert_runtime_profile_env(stale, &context);

        let runtime = init_provider_for_validation(&choice, None)
            .await
            .unwrap_or_else(|err| panic!("explicit compatible init failed for {context}: {err}"));

        assert_runtime_profile_env(selected, &context);
        assert_model_picker_has_profile_route(runtime.as_ref(), selected, &context);
    }

    Ok(())
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
