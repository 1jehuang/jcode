//! Regression tests for issue #1824: the `/model` picker for an
//! OpenAI-compatible provider must union the models declared in config.toml
//! (`[[providers.<name>.models]]`) with the endpoint's fetched `/models`
//! catalog, and must never trust a disk cache that was fetched from a different
//! endpoint.
//!
//! Two failure modes are covered:
//!
//! 1. MiniMax serves `MiniMax-M3.1-Flash-Preview` on chat completions but omits
//!    it from `GET /v1/models`, so a declared model was invisible in the picker
//!    even though the config explicitly names it.
//! 2. The cache namespace is process-global, so `~/.kcode/cache/minimax_models.json`
//! could hold another provider's catalog (the report found NVIDIA NIM models
//! under `source_api_base`) and silently decide the picker.

use super::*;

/// Write `config.toml` into the hermetic test home and invalidate the cache.
fn write_test_config(contents: &str) {
    let jcode_home = std::env::var_os("KCODE_HOME").expect("test KCODE_HOME should be set");
    std::fs::write(
        std::path::PathBuf::from(jcode_home).join("config.toml"),
        contents,
    )
    .expect("write test config.toml");
    crate::config::invalidate_config_cache();
}

/// Config for the MiniMax profile keyed the way the issue reports it: the
/// config key is the profile id, so the block augments the built-in endpoint
/// with a model the `/models` list omits.
const MINIMAX_CONFIG_WITH_DECLARED_MODEL: &str = r#"
[providers.minimax]
type = "openai-compatible"
base_url = "https://api.minimax.io/v1"
auth = "Bearer"
api_key_env = "MINIMAX_API_KEY"
default_model = "MiniMax-M3"

[[providers.minimax.models]]
id = "MiniMax-M3.1-Flash-Preview"
context_window = 1000000
"#;

/// A model declared in config.toml but absent from the endpoint's `/models`
/// list must still appear in that provider's picker routes.
#[test]
fn config_declared_model_absent_from_fetched_list_reaches_the_picker() {
    with_clean_provider_test_env(|| {
        crate::env::set_var("MINIMAX_API_KEY", "test-minimax-key");
        write_test_config(MINIMAX_CONFIG_WITH_DECLARED_MODEL);
        // The endpoint's live catalog, which omits the declared model.
        save_test_openrouter_model_cache(
            "minimax",
            "https://api.minimax.io/v1",
            &["MiniMax-M3", "MiniMax-M2.7", "MiniMax-M2"],
        );

        let profile = crate::provider_catalog::openai_compatible_profile_by_id("minimax")
            .expect("MiniMax profile exists");
        let routes = super::super::direct_openai_compatible_profile_routes(profile);
        let declared = routes
            .iter()
            .find(|route| route.model == "MiniMax-M3.1-Flash-Preview")
            .unwrap_or_else(|| {
                panic!(
                    "config-declared model must reach the picker even when the endpoint \
                     omits it from /models, routes: {routes:?}"
                )
            });
        assert_eq!(declared.provider, "MiniMax");
        assert_eq!(declared.api_method, "openai-compatible:minimax");
        assert!(declared.available);

        // Fetched entries keep their live-catalog detail and ordering.
        let fetched: Vec<&str> = routes
            .iter()
            .map(|route| route.model.as_str())
            .filter(|model| !model.starts_with("MiniMax-M3.1"))
            .collect();
        assert_eq!(
            fetched,
            vec!["MiniMax-M3", "MiniMax-M2.7", "MiniMax-M2"],
            "fetched models must keep their order and set: {routes:?}"
        );
    });
}

/// Matching a default model cannot authorize a foreign endpoint declaration.
#[test]
fn matching_default_model_does_not_allow_foreign_endpoint_declarations() {
    with_clean_provider_test_env(|| {
        write_test_config(
            &MINIMAX_CONFIG_WITH_DECLARED_MODEL
                .replace(
                    "https://api.minimax.io/v1",
                    "https://integrate.api.nvidia.com/v1",
                )
                .replace(
                    "MiniMax-M3.1-Flash-Preview",
                    "foreign-config-only-negative-control",
                ),
        );
        let profile = crate::provider_catalog::openai_compatible_profile_by_id("minimax")
            .expect("MiniMax profile exists");
        let routes = super::super::direct_openai_compatible_profile_routes(profile);
        assert!(
            !routes
                .iter()
                .any(|route| route.model == "foreign-config-only-negative-control")
        );
    });
}

/// A declared model that the endpoint already advertises must not appear twice.
#[test]
fn config_declared_model_duplicate_of_fetched_model_is_not_listed_twice() {
    with_clean_provider_test_env(|| {
        crate::env::set_var("MINIMAX_API_KEY", "test-minimax-key");
        write_test_config(MINIMAX_CONFIG_WITH_DECLARED_MODEL);
        // The endpoint already lists the declared model.
        save_test_openrouter_model_cache(
            "minimax",
            "https://api.minimax.io/v1",
            &["MiniMax-M3", "MiniMax-M3.1-Flash-Preview", "MiniMax-M2.7"],
        );

        let profile = crate::provider_catalog::openai_compatible_profile_by_id("minimax")
            .expect("MiniMax profile exists");
        let routes = super::super::direct_openai_compatible_profile_routes(profile);
        let occurrences = routes
            .iter()
            .filter(|route| route.model == "MiniMax-M3.1-Flash-Preview")
            .count();
        assert_eq!(
            occurrences, 1,
            "a declared model already in the fetched list must not be duplicated: {routes:?}"
        );
    });
}

/// Declared models go through the same validation as fetched ones, so a
/// declared embedding/speech model stays out of the picker.
#[test]
fn config_declared_non_listable_model_is_still_filtered_out() {
    with_clean_provider_test_env(|| {
        crate::env::set_var("MINIMAX_API_KEY", "test-minimax-key");
        write_test_config(
            r#"
[providers.minimax]
type = "openai-compatible"
base_url = "https://api.minimax.io/v1"
auth = "Bearer"
api_key_env = "MINIMAX_API_KEY"

[[providers.minimax.models]]
id = "MiniMax-M3.1-Flash-Preview"

[[providers.minimax.models]]
id = "minimax-embedding"
"#,
        );
        save_test_openrouter_model_cache("minimax", "https://api.minimax.io/v1", &["MiniMax-M3"]);

        let profile = crate::provider_catalog::openai_compatible_profile_by_id("minimax")
            .expect("MiniMax profile exists");
        let routes = super::super::direct_openai_compatible_profile_routes(profile);
        assert!(
            routes
                .iter()
                .any(|route| route.model == "MiniMax-M3.1-Flash-Preview"),
            "declared chat model should be listed: {routes:?}"
        );
        assert!(
            !routes
                .iter()
                .any(|route| route.model == "minimax-embedding"),
            "declared non-chat model must be filtered like a fetched one: {routes:?}"
        );
    });
}

/// A cache in the profile namespace that was fetched from a different endpoint
/// is not usable: the picker must not present another provider's models as this
/// provider's, and the background sweeper must want to refetch.
#[test]
fn profile_cache_from_a_different_endpoint_is_rejected() {
    with_clean_provider_test_env(|| {
        let profile = crate::provider_catalog::openai_compatible_profile_by_id("minimax")
            .expect("MiniMax profile exists");
        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
        assert_eq!(resolved.api_base, "https://api.minimax.io/v1");

        // Cross-provider pollution: NVIDIA NIM's catalog under the MiniMax key.
        save_test_openrouter_model_cache(
            "minimax",
            "https://integrate.api.nvidia.com/v1",
            &["yi-large", "gemma-3-27b-it"],
        );

        assert!(
            super::super::cached_live_models_for_openai_compatible_profile(&resolved).is_none(),
            "a cache fetched from another endpoint must not be usable for this profile"
        );
        assert!(
            super::super::catalog_scheduler::profile_catalog_cache_needs_refresh(profile),
            "an unusable cache must leave the profile due for a refresh"
        );

        let routes = super::super::direct_openai_compatible_profile_routes(profile);
        assert!(
            !routes.iter().any(|route| route.model == "yi-large"),
            "another provider's models must never be relabeled as this profile: {routes:?}"
        );
    });
}

/// The same discipline on the native (non-direct) read path: the `openrouter`
/// namespace is the process default for a direct compatible runtime, so a
/// foreign catalog there must read as "unknown" rather than as an authoritative
/// answer about the public OpenRouter catalog.
#[test]
fn native_openrouter_namespace_rejects_a_foreign_source_api_base() {
    with_clean_provider_test_env(|| {
        save_test_openrouter_model_cache(
            "openrouter",
            "https://integrate.api.nvidia.com/v1",
            &["yi-large", "gemma-3-27b-it"],
        );
        assert_eq!(
            openrouter::standard_catalog_lists_model("yi-large"),
            None,
            "a cross-provider-polluted cache must not answer OpenRouter catalog questions"
        );
        assert!(
            super::super::configured_standard_openrouter_profile_routes().is_empty(),
            "a cross-provider-polluted cache must not produce standard OpenRouter routes"
        );

        // A cache genuinely from openrouter.ai is still authoritative.
        save_test_openrouter_model_cache(
            "openrouter",
            "https://openrouter.ai/api/v1",
            &["anthropic/claude-sonnet-4-6"],
        );
        assert_eq!(
            openrouter::standard_catalog_lists_model("anthropic/claude-sonnet-4-6"),
            Some(true)
        );
        assert_eq!(
            openrouter::standard_catalog_lists_model("openai/gpt-5.3-codex-spark"),
            Some(false)
        );
    });
}
