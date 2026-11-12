//! Context-window resolution for Ollama endpoints.
//!
//! Split out of `openrouter_tests.rs` so the Ollama serving-window rules live
//! next to the module that implements them, and so the shared OpenRouter test
//! file does not keep growing.

use super::tests::{ENV_LOCK, EnvVarGuard};
use super::*;
use jcode_provider_core::Provider;

#[test]
fn ollama_context_window_does_not_over_report_before_the_catalog_is_probed() {
    // Ollama serves min(trained window, OLLAMA_CONTEXT_LENGTH) and silently
    // truncates anything longer, so before the native-API probe has populated
    // the catalog there is no evidence for a large window. Reporting a model's
    // advertised window here would over-budget the request and make the
    // conversation look like it lost its history.
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let mut config = jcode_base::config::NamedProviderConfig {
        base_url: "http://localhost:11434/v1".to_string(),
        auth: jcode_base::config::NamedProviderAuth::None,
        // A model id whose family heuristics advertise a very large window.
        default_model: Some("qwen3:35b".to_string()),
        ..Default::default()
    };
    config.model_catalog = false;
    config.requires_api_key = Some(false);

    let provider =
        OpenRouterProvider::new_named_openai_compatible("ollama", &config).expect("provider");

    assert_eq!(
        provider.context_window(),
        4096,
        "a cold Ollama cache must fall back to Ollama's conservative serving \
         default instead of the model's advertised window"
    );
}

#[test]
fn explicit_context_window_still_wins_over_the_ollama_clamp() {
    // The clamp is a fallback for missing evidence, not a ceiling. A user who
    // has raised OLLAMA_CONTEXT_LENGTH and declared the matching window must be
    // believed, otherwise the fix for over-reporting becomes under-reporting.
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let mut config = jcode_base::config::NamedProviderConfig {
        base_url: "http://localhost:11434/v1".to_string(),
        auth: jcode_base::config::NamedProviderAuth::None,
        default_model: Some("qwen3:35b".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "qwen3:35b".to_string(),
            context_window: Some(65_536),
            reasoning: None,
            reasoning_effort: None,
            input: Vec::new(),
        }],
        ..Default::default()
    };
    config.model_catalog = false;
    config.requires_api_key = Some(false);

    let provider =
        OpenRouterProvider::new_named_openai_compatible("ollama", &config).expect("provider");

    assert_eq!(provider.context_window(), 65_536);
}

#[test]
fn ollama_cloud_model_is_not_clamped_to_the_local_runner_default() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let mut config = jcode_base::config::NamedProviderConfig {
        base_url: "http://localhost:11434/v1".to_string(),
        auth: jcode_base::config::NamedProviderAuth::None,
        default_model: Some("glm-5.2:cloud".to_string()),
        ..Default::default()
    };
    config.model_catalog = false;
    config.requires_api_key = Some(false);

    let provider =
        OpenRouterProvider::new_named_openai_compatible("ollama", &config).expect("provider");

    assert_eq!(provider.context_window(), 1_000_000);
}
