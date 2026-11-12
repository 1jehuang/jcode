use crate::tests::{ENV_LOCK, EnvVarGuard};
use crate::*;
use jcode_base::config::{NamedProviderAuth, NamedProviderConfig};
use jcode_provider_core::fingerprint::pool_index;
use std::collections::BTreeSet;

fn profile(envs: &[&str]) -> NamedProviderConfig {
    NamedProviderConfig {
        base_url: "https://gateway.example/v1".to_string(),
        auth: NamedProviderAuth::Bearer,
        api_key_envs: envs.iter().map(|env| env.to_string()).collect(),
        ..Default::default()
    }
}

/// Token and label of the credential the resolver picks now.
fn picked(auth: &AuthResolver) -> (String, String) {
    match auth().unwrap() {
        ProviderAuth::AuthorizationBearer { token, label } => (token, label),
        other => panic!("unexpected auth {other:?}"),
    }
}

/// A session id whose pick over `len` entries is `want`.
fn session_for(len: usize, want: usize) -> String {
    (0..)
        .map(|n| format!("session-{n}"))
        .find(|id| pool_index(Some(id.as_str()), len) == want)
        .unwrap()
}

/// Clears every env name that a named profile apply sets. The guards restore them on drop.
fn clear_named_profile_env() -> Vec<EnvVarGuard> {
    [
        "JCODE_NAMED_PROVIDER_PROFILE",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_API_KEY_NAMES",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_TRANSPORT_STATE",
        "JCODE_OPENROUTER_AUTH_HEADER",
    ]
    .into_iter()
    .map(EnvVarGuard::remove)
    .collect()
}

#[test]
fn one_session_keeps_one_entry_and_sessions_spread_over_the_pool() {
    let _lock = ENV_LOCK.lock();
    let _a = EnvVarGuard::set("POOL_KEY_A", "secret-a");
    let _b = EnvVarGuard::set("POOL_KEY_B", "secret-b");
    let _c = EnvVarGuard::set("POOL_KEY_C", "secret-c");
    let pool = KeyPool::new(
        "gateway",
        &profile(&["POOL_KEY_A", "POOL_KEY_B", "POOL_KEY_C"]),
    )
    .unwrap();
    pool.bind("session-one");
    let auth = pool.auth();
    let pinned = picked(&auth);
    for _ in 0..5 {
        assert_eq!(picked(&auth), pinned);
    }
    let spread: BTreeSet<String> = (0..30)
        .map(|n| {
            pool.bind(&format!("session-{n}"));
            picked(&auth).1
        })
        .collect();
    assert_eq!(spread.len(), 3);
}

#[test]
fn fork_keeps_the_pin_and_later_binds_stay_separate() {
    let _lock = ENV_LOCK.lock();
    let _a = EnvVarGuard::set("POOL_KEY_A", "secret-a");
    let _b = EnvVarGuard::set("POOL_KEY_B", "secret-b");
    let parent = KeyPool::new("gateway", &profile(&["POOL_KEY_A", "POOL_KEY_B"])).unwrap();
    parent.bind(&session_for(2, 1));
    let fork = parent.fork();
    assert_eq!(picked(&fork.auth()).0, "secret-b");
    fork.bind(&session_for(2, 0));
    assert_eq!(picked(&fork.auth()).1, "POOL_KEY_A");
    assert_eq!(picked(&parent.auth()).1, "POOL_KEY_B");
}

#[test]
fn missing_pool_entry_fails_only_its_session_and_names_it() {
    let _lock = ENV_LOCK.lock();
    let _a = EnvVarGuard::set("POOL_KEY_A", "secret-a");
    let _missing = EnvVarGuard::remove("POOL_KEY_MISSING");
    let pool = KeyPool::new("gateway", &profile(&["POOL_KEY_A", "POOL_KEY_MISSING"])).unwrap();
    let auth = pool.auth();
    pool.bind(&session_for(2, 1));
    let err = auth().unwrap_err();
    assert!(err.to_string().contains("POOL_KEY_MISSING"), "{err}");
    pool.bind(&session_for(2, 0));
    assert_eq!(picked(&auth).1, "POOL_KEY_A");
}

#[test]
fn unbound_session_uses_the_first_entry() {
    let _lock = ENV_LOCK.lock();
    let _a = EnvVarGuard::set("POOL_KEY_A", "secret-a");
    let _b = EnvVarGuard::set("POOL_KEY_B", "secret-b");
    let pool = KeyPool::new("gateway", &profile(&["POOL_KEY_B", "POOL_KEY_A"])).unwrap();
    assert_eq!(picked(&pool.auth()).1, "POOL_KEY_B");
}

#[test]
fn single_key_profile_keeps_api_key_env_and_rejects_unsafe_pool_names() {
    let _lock = ENV_LOCK.lock();
    let _a = EnvVarGuard::set("POOL_KEY_A", "secret-a");
    let single = NamedProviderConfig {
        api_key_env: Some("POOL_KEY_A".to_string()),
        ..profile(&[])
    };
    let pool = KeyPool::new("gateway", &single).unwrap();
    assert_eq!(picked(&pool.auth()).0, "secret-a");
    assert!(KeyPool::new("gateway", &profile(&["bad name"])).is_err());
}

#[test]
fn pool_profile_credential_checks_name_the_first_entry() {
    let _lock = ENV_LOCK.lock();
    let _env = clear_named_profile_env();
    let mut config = jcode_base::config::Config::default();
    config.providers.insert(
        "gateway".to_string(),
        profile(&["POOL_KEY_A", "POOL_KEY_B"]),
    );
    jcode_base::provider_catalog::apply_named_provider_profile_env_from_config("gateway", &config)
        .unwrap();
    let source = jcode_base::provider_catalog::active_named_provider_profile_credential_source();
    assert_eq!(
        source.map(|(key_env, _)| key_env).as_deref(),
        Some("POOL_KEY_A")
    );
}

#[test]
fn unset_first_pool_entry_still_counts_as_configured() {
    let _lock = ENV_LOCK.lock();
    let _env = clear_named_profile_env();
    let _a = EnvVarGuard::remove("POOL_KEY_A");
    let _b = EnvVarGuard::set("POOL_KEY_B", "secret-b");
    let mut config = jcode_base::config::Config::default();
    config.providers.insert(
        "gateway".to_string(),
        profile(&["POOL_KEY_A", "POOL_KEY_B"]),
    );
    jcode_base::provider_catalog::apply_named_provider_profile_env_from_config("gateway", &config)
        .unwrap();
    assert!(jcode_base::provider::openrouter::has_credentials());
    assert!(
        jcode_base::provider_catalog::openai_compatible_profile_is_configured(
            jcode_base::provider_catalog::OPENAI_COMPAT_PROFILE
        )
    );
}

#[test]
fn unbound_pick_takes_the_first_entry_that_resolves() {
    let _lock = ENV_LOCK.lock();
    let _a = EnvVarGuard::remove("POOL_KEY_A");
    let _b = EnvVarGuard::set("POOL_KEY_B", "secret-b");
    let pool = KeyPool::new("gateway", &profile(&["POOL_KEY_A", "POOL_KEY_B"])).unwrap();
    assert_eq!(
        picked(&pool.auth()),
        ("secret-b".to_string(), "POOL_KEY_B".to_string())
    );
}

#[test]
fn pool_credential_source_names_the_entry_that_resolves() {
    let _lock = ENV_LOCK.lock();
    let _env = clear_named_profile_env();
    let _a = EnvVarGuard::remove("POOL_KEY_A");
    let _b = EnvVarGuard::set("POOL_KEY_B", "secret-b");
    let mut config = jcode_base::config::Config::default();
    config.providers.insert(
        "gateway".to_string(),
        profile(&["POOL_KEY_A", "POOL_KEY_B"]),
    );
    jcode_base::provider_catalog::apply_named_provider_profile_env_from_config("gateway", &config)
        .unwrap();
    let source = jcode_base::provider_catalog::active_named_provider_profile_credential_source();
    assert_eq!(
        source.map(|(key_env, _)| key_env).as_deref(),
        Some("POOL_KEY_B")
    );
}
