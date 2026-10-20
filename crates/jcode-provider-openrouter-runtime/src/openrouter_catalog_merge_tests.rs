//! Regression tests for static-model / live-catalog merge behavior
//! across built-in and user-declared OpenAI-compatible provider profiles.

use crate::tests::{ENV_LOCK, EnvVarGuard};

/// Minimal one-shot `/models` endpoint: serves `body` to the first request.
fn spawn_models_server(body: &'static str) -> String {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake provider server");
    let addr = listener.local_addr().expect("fake provider addr");
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
        let mut buf = vec![0u8; 8192];
        let _ = stream.read(&mut buf);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
    });
    format!("http://{addr}/v1")
}

use crate::*;

#[test]
fn named_profile_static_models_survive_live_catalog_refresh() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _key = EnvVarGuard::set("TEST_NAMED_MERGE_KEY", "test-key");

    let profile = jcode_base::config::NamedProviderConfig {
        base_url: "https://llm.example.test/v1".to_string(),
        api_key_env: Some("TEST_NAMED_MERGE_KEY".to_string()),
        model_catalog: true,
        default_model: Some("my-custom-model".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "my-custom-model".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };

    let provider = OpenRouterProvider::new_named_openai_compatible("user-compat", &profile)
        .expect("named profile should initialize");
    assert!(provider.is_user_named_profile());
    assert!(provider.should_merge_static_models_with_live_catalog());

    // Simulate a completed background `/models` catalog refresh that does not
    // include the user's config-declared model.
    {
        let mut cache = provider.models_cache.blocking_write();
        cache.models = vec![jcode_provider_openrouter::ModelInfo {
            id: "vendor-live-model".to_string(),
            name: "vendor live model".to_string(),
            context_length: Some(128_000),
            pricing: Default::default(),
            created: None,
            ..Default::default()
        }];
        cache.fetched = true;
        cache.cached_at = Some(1);
    }

    let models = provider.available_models_display();
    assert!(
        models.iter().any(|m| m == "my-custom-model"),
        "config-declared model should survive live catalog refresh: {models:?}"
    );
    assert!(models.iter().any(|m| m == "vendor-live-model"));
}

/// A built-in profile keeps replace-after-fetch semantics: its `static_models`
/// are a pre-catalog fallback, not a user-authored list.
#[test]
fn builtin_profile_is_not_treated_as_user_named() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _key = EnvVarGuard::set("CEREBRAS_API_KEY", "test-key");

    let profile = jcode_base::config::NamedProviderConfig {
        base_url: "https://api.cerebras.ai/v1".to_string(),
        api_key_env: Some("CEREBRAS_API_KEY".to_string()),
        model_catalog: true,
        ..Default::default()
    };

    let provider = OpenRouterProvider::new_named_openai_compatible("cerebras", &profile)
        .expect("builtin-shaped profile should initialize");
    assert!(!provider.is_user_named_profile());
    assert!(!provider.should_merge_static_models_with_live_catalog());
}

/// A `[providers.cerebras]` block that shadows a built-in name but points at a
/// different endpoint is still user-declared, so its models must be preserved.
#[test]
fn profile_shadowing_builtin_name_with_other_base_is_user_named() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _key = EnvVarGuard::set("TEST_SHADOW_KEY", "test-key");

    let profile = jcode_base::config::NamedProviderConfig {
        base_url: "https://proxy.internal.test/v1".to_string(),
        api_key_env: Some("TEST_SHADOW_KEY".to_string()),
        model_catalog: true,
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "shadowed-model".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };

    let provider = OpenRouterProvider::new_named_openai_compatible("cerebras", &profile)
        .expect("shadowing profile should initialize");
    assert!(provider.is_user_named_profile());
    assert!(provider.should_merge_static_models_with_live_catalog());
}

/// End-to-end guard for issue #579: parse a real `config.toml` snippet, build
/// the provider from it, perform a live `/models` fetch whose catalog omits the
/// user's declared model, and assert the picker still offers it.
#[tokio::test]
async fn config_toml_models_survive_a_real_catalog_fetch() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");

    let api_base = spawn_models_server(
        r#"{
            "object": "list",
            "data": [
                {"id": "vendor-live-model", "object": "model", "context_length": 131072}
            ]
        }"#,
    );

    // Exactly the shape a user writes in ~/.jcode/config.toml.
    let toml_src = format!(
        r#"
base_url = "{api_base}"
auth = "none"
model_catalog = true
default_model = "vendor-live-model"

[[models]]
id = "my-custom-model"
context_window = 128000
"#
    );
    let profile: jcode_base::config::NamedProviderConfig =
        toml::from_str(&toml_src).expect("config.toml profile should parse");

    let provider = OpenRouterProvider::new_named_openai_compatible("mylocal", &profile)
        .expect("named profile should initialize");

    // Real HTTP catalog fetch, mirroring the background refresh that used to
    // drop config-declared models.
    provider
        .fetch_models()
        .await
        .expect("live catalog fetch should succeed");

    let models = provider.available_models_display();
    assert!(
        models.iter().any(|m| m == "my-custom-model"),
        "config.toml model must survive a live catalog fetch (and must not rely \
         on the current-model fallback): {models:?}"
    );
    assert!(
        models.iter().any(|m| m == "vendor-live-model"),
        "live catalog models must still be offered: {models:?}"
    );
}

/// A pinned `<model>@<Provider>` variant is routing syntax, not the catalog id.
/// OpenRouter's `/models` lists `stealth/space-bunny-alpha` while the pinned runtime
/// model is `stealth/space-bunny-alpha@Stealth`. Matching only the pinned form missed
/// every catalog entry, so `context_window()` fell through to the 200K default even
/// though the catalog entry carries `context_length` 1000000. Measured live: the panel
/// reported 200K while the on-disk catalog already carried the correct number.
#[tokio::test]
async fn pinned_variant_suffix_still_matches_the_catalog_entry() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let temp = tempfile::tempdir().expect("temp jcode home");
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path().to_str().expect("utf8 temp path"));

    let api_base = spawn_models_server(
        r#"{
            "object": "list",
            "data": [
                {"id": "stealth/space-bunny-alpha", "object": "model", "context_length": 1000000}
            ]
        }"#,
    );

    let toml_src = format!(
        r#"
base_url = "{api_base}"
auth = "none"
model_catalog = true
default_model = "stealth/space-bunny-alpha"
"#
    );
    let profile: jcode_base::config::NamedProviderConfig =
        toml::from_str(&toml_src).expect("config.toml profile should parse");

    let provider = OpenRouterProvider::new_named_openai_compatible("orvariant", &profile)
        .expect("named profile should initialize");

    provider
        .fetch_models()
        .await
        .expect("live catalog fetch should succeed");

    // The pinned form the runtime actually carries.
    {
        let mut model = provider.model.try_write().expect("model lock");
        *model = "stealth/space-bunny-alpha@Stealth".to_string();
    }

    assert_eq!(
        provider.context_window(),
        1_000_000,
        "a pinned @Provider suffix must not hide the model from the catalog"
    );
}

/// Regression test for issue #607.
///
/// Every `new_named_openai_compatible()` constructor sets the process-global
/// `JCODE_OPENROUTER_CACHE_NAMESPACE` env var, so with several named profiles
/// in one process the last one constructed wins and every profile's foreground
/// cache read/write collides on a single `<last-profile>_models.json`. The
/// picker then shows another provider's catalog, or refetches and clobbers it,
/// which is why the model list did not survive a relaunch.
#[test]
fn named_profiles_keep_distinct_disk_cache_namespaces() {
    let _lock = ENV_LOCK.lock();
    let temp = tempfile::tempdir().expect("temp jcode home");
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path().to_str().expect("utf8 temp path"));
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _key = EnvVarGuard::set("TEST_NS_KEY", "test-key");

    let make_profile = |base_url: &str| jcode_base::config::NamedProviderConfig {
        base_url: base_url.to_string(),
        api_key_env: Some("TEST_NS_KEY".to_string()),
        model_catalog: true,
        ..Default::default()
    };

    // Construct "first" then "second"; the env var now points at "second".
    let first = OpenRouterProvider::new_named_openai_compatible(
        "first",
        &make_profile("https://first.models.test/v1"),
    )
    .expect("first profile");
    let second = OpenRouterProvider::new_named_openai_compatible(
        "second",
        &make_profile("https://second.models.test/v1"),
    )
    .expect("second profile");

    assert!(first.is_user_named_profile());
    assert!(second.is_user_named_profile());
    assert_eq!(
        first.foreground_cache_namespace().as_deref(),
        Some("first"),
        "each named profile must use its own namespace, not the env var winner"
    );
    assert_eq!(
        second.foreground_cache_namespace().as_deref(),
        Some("second")
    );

    // Write a distinct on-disk cache for each namespace.
    let cache_dir = temp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).expect("create cache dir");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    for (namespace, api_base, model_id) in [
        ("first", "https://first.models.test/v1", "first-only-model"),
        (
            "second",
            "https://second.models.test/v1",
            "second-only-model",
        ),
    ] {
        let cache = serde_json::json!({
            "cached_at": now,
            "source_api_base": api_base,
            "models": [{ "id": model_id }],
        });
        std::fs::write(
            cache_dir.join(format!("{namespace}_models.json")),
            serde_json::to_string(&cache).expect("serialize cache"),
        )
        .expect("write cache");
    }

    // Each provider must read only its own cache, even though the env var
    // points at "second".
    let first_entry = first
        .load_usable_model_disk_cache_entry()
        .expect("first profile should find its own cache");
    assert_eq!(first_entry.models.len(), 1);
    assert_eq!(first_entry.models[0].id, "first-only-model");

    let second_entry = second
        .load_usable_model_disk_cache_entry()
        .expect("second profile should find its own cache");
    assert_eq!(second_entry.models[0].id, "second-only-model");
}

/// Built-in runtimes must not follow another profile's process-global cache
/// namespace. Otherwise an NVIDIA fetch can overwrite minimax_models.json
/// with NVIDIA provenance even though readers correctly reject that mismatch.
#[test]
fn builtin_profiles_keep_distinct_disk_cache_namespaces() {
    let _lock = ENV_LOCK.lock();
    let temp = tempfile::tempdir().expect("temp jcode home");
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path().to_str().expect("utf8 temp path"));
    let _namespace = EnvVarGuard::set("JCODE_OPENROUTER_CACHE_NAMESPACE", "minimax");
    let _minimax_key = EnvVarGuard::set("MINIMAX_API_KEY", "test-key");
    let _nvidia_key = EnvVarGuard::set("NVIDIA_API_KEY", "test-key");

    for id in ["minimax", "nvidia-nim"] {
        let profile = openai_compatible_profile_by_id(id).expect("built-in profile");
        let provider = OpenRouterProvider::new_openai_compatible_profile_runtime(profile)
            .expect("built-in runtime");
        assert!(!provider.is_user_named_profile());
        assert_eq!(provider.foreground_cache_namespace().as_deref(), Some(id));

        let model = ModelInfo {
            id: format!("{id}-only-model"),
            ..Default::default()
        };
        save_disk_cache_with_source_for_namespace(id, &[model], Some(&provider.api_base));

        // Simulate another provider changing the env after construction. Even
        // a matching-source cache in the wrong namespace must not be selected.
        let _other_namespace = EnvVarGuard::set("JCODE_OPENROUTER_CACHE_NAMESPACE", "other");
        save_disk_cache_with_source_for_namespace(
            "other",
            &[ModelInfo {
                id: "wrong-namespace-model".to_string(),
                ..Default::default()
            }],
            Some(&provider.api_base),
        );
        let cache = provider
            .load_usable_model_disk_cache_entry()
            .expect("own cache");
        assert_eq!(cache.models[0].id, format!("{id}-only-model"));
    }
}

/// Exercise the real fetch/write path with a built-in runtime and a different
/// global namespace, without making any requests to an actual provider.
#[tokio::test]
async fn builtin_fetch_models_writes_own_namespace() {
    let _lock = ENV_LOCK.lock();
    let temp = tempfile::tempdir().expect("temp jcode home");
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path().to_str().expect("utf8 temp path"));
    let _namespace = EnvVarGuard::set("JCODE_OPENROUTER_CACHE_NAMESPACE", "other");
    let _key = EnvVarGuard::set("MINIMAX_API_KEY", "test-key");
    let profile = openai_compatible_profile_by_id("minimax").expect("built-in profile");
    let mut provider = OpenRouterProvider::new_openai_compatible_profile_runtime(profile)
        .expect("built-in runtime");
    assert!(!provider.is_user_named_profile());

    // Keep the real constructor/profile identity, but send catalog HTTP only
    // to this local server, never to MiniMax or another actual provider.
    provider.api_base = spawn_models_server(r#"{"data":[{"id":"minimax-live-model"}]}"#);
    save_disk_cache_with_source_for_namespace(
        "other",
        &[ModelInfo {
            id: "other-model".to_string(),
            ..Default::default()
        }],
        Some("https://other.example.test/v1"),
    );
    let other_path = temp.path().join("cache/other_models.json");
    let other_before = std::fs::read(&other_path).expect("other cache before fetch");
    assert!(jcode_provider_openrouter::load_disk_cache_entry_for_namespace("minimax").is_none());

    let models = tokio::time::timeout(std::time::Duration::from_secs(5), provider.fetch_models())
        .await
        .expect("catalog request timeout")
        .expect("catalog fetch");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "minimax-live-model");
    let own_cache = jcode_provider_openrouter::load_disk_cache_entry_for_namespace("minimax")
        .expect("fetch must write the builtin profile cache");
    assert_eq!(
        own_cache.source_api_base.as_deref(),
        Some(provider.api_base.as_str())
    );
    assert_eq!(own_cache.models[0].id, "minimax-live-model");
    assert_eq!(
        std::fs::read(&other_path).expect("other cache after fetch"),
        other_before,
        "fetch must not overwrite the process-global namespace's cache"
    );
}

/// The provenance guard already exists: a fresh but cross-endpoint cache
/// must be ignored, fetched again, and replaced with the current source.
#[tokio::test]
async fn mismatched_catalog_source_is_refetched() {
    let _lock = ENV_LOCK.lock();
    let temp = tempfile::tempdir().expect("temp jcode home");
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path().to_str().expect("utf8 temp path"));
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let api_base = spawn_models_server(r#"{"data":[{"id":"correct-live-model"}]}"#);
    let profile = jcode_base::config::NamedProviderConfig {
        base_url: api_base.clone(),
        auth: jcode_base::config::NamedProviderAuth::None,
        model_catalog: true,
        ..Default::default()
    };
    let provider = OpenRouterProvider::new_named_openai_compatible("source-test", &profile)
        .expect("named profile");
    save_disk_cache_with_source_for_namespace(
        "source-test",
        &[ModelInfo {
            id: "foreign-model".to_string(),
            ..Default::default()
        }],
        Some("https://integrate.api.nvidia.com/v1"),
    );
    assert!(provider.load_usable_model_disk_cache_entry().is_none());
    let models = tokio::time::timeout(std::time::Duration::from_secs(5), provider.fetch_models())
        .await
        .expect("catalog request timeout")
        .expect("catalog fetch");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "correct-live-model");
    let cache = provider
        .load_usable_model_disk_cache_entry()
        .expect("refetched cache");
    assert_eq!(cache.source_api_base.as_deref(), Some(api_base.as_str()));
    assert_eq!(cache.models[0].id, "correct-live-model");
}
