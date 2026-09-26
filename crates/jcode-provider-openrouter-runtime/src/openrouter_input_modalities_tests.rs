//! Regression tests for input modalities declared in the model catalog.
//!
//! The OpenRouter catalog states which modalities each model accepts under
//! `architecture.input_modalities`. Before this was parsed, a vision-capable
//! model on the native OpenRouter route still fell through to the per-provider
//! fallback, which is `false` when `supports_provider_features` is true, so the
//! outbound image clamp replaced the image with a text marker.

use crate::tests::{ENV_LOCK, EnvVarGuard};
use crate::*;

/// The catalogue declares image support, so the model must be treated as
/// vision-capable even though the provider-level fallback says otherwise.
#[test]
fn catalog_declared_image_modality_enables_image_input() {
    let _lock = ENV_LOCK.lock();
    let _key = EnvVarGuard::set("OPENROUTER_API_KEY", "test-key");

    let provider = OpenRouterProvider::new_openrouter_api_key_runtime()
        .expect("openrouter runtime should initialize with a test key");
    provider
        .set_model("vendor/vision-model")
        .expect("openrouter accepts any model id");

    {
        let mut cache = provider.models_cache.blocking_write();
        cache.models = vec![jcode_provider_openrouter::ModelInfo {
            id: "vendor/vision-model".to_string(),
            name: "vision model".to_string(),
            context_length: Some(128_000),
            input: vec!["text".to_string(), "image".to_string()],
            ..Default::default()
        }];
        cache.fetched = true;
    }

    assert!(
        provider.supports_image_input(),
        "a model the catalog declares image-capable must not be clamped to text"
    );
}

/// Negative control: a catalog entry that says text only must keep the existing
/// behaviour. This is the safety property, so a missing `image` entry must never
/// be read as permission.
#[test]
fn catalog_without_image_modality_keeps_legacy_behaviour() {
    let _lock = ENV_LOCK.lock();
    let _key = EnvVarGuard::set("OPENROUTER_API_KEY", "test-key");

    let provider = OpenRouterProvider::new_openrouter_api_key_runtime()
        .expect("openrouter runtime should initialize with a test key");
    provider
        .set_model("vendor/text-only-model")
        .expect("openrouter accepts any model id");

    {
        let mut cache = provider.models_cache.blocking_write();
        cache.models = vec![jcode_provider_openrouter::ModelInfo {
            id: "vendor/text-only-model".to_string(),
            name: "text only model".to_string(),
            context_length: Some(128_000),
            input: vec!["text".to_string()],
            ..Default::default()
        }];
        cache.fetched = true;
    }

    assert!(
        !provider.supports_image_input(),
        "a text-only catalog entry must not be treated as vision-capable"
    );
}

/// A model absent from the catalog keeps the provider-level fallback.
#[test]
fn absent_catalog_entry_keeps_provider_fallback() {
    let _lock = ENV_LOCK.lock();
    let _key = EnvVarGuard::set("OPENROUTER_API_KEY", "test-key");

    let provider = OpenRouterProvider::new_openrouter_api_key_runtime()
        .expect("openrouter runtime should initialize with a test key");
    provider
        .set_model("vendor/never-listed")
        .expect("openrouter accepts any model id");

    assert!(
        !provider.supports_image_input(),
        "an unlisted model must fall back to the provider default"
    );
}

/// The nested OpenRouter shape is what the provider actually serves.
#[test]
fn openrouter_architecture_modalities_are_parsed() {
    let raw = r#"{
        "id": "vendor/vision-model",
        "architecture": { "input_modalities": ["text", "image", "file"] }
    }"#;
    let value: serde_json::Value = serde_json::from_str(raw).expect("valid json");
    let model = crate::models_catalog_parse::parse_model_info_value(&value)
        .expect("catalog entry should parse");

    assert_eq!(
        model.input,
        vec!["text".to_string(), "image".to_string(), "file".to_string()],
        "declared input modalities must survive parsing"
    );
}

/// A catalog with no modality information must produce an empty list rather
/// than a guess, so absence stays absence.
#[test]
fn missing_architecture_yields_no_modalities() {
    let value: serde_json::Value =
        serde_json::from_str(r#"{"id": "vendor/plain"}"#).expect("valid json");
    let model = crate::models_catalog_parse::parse_model_info_value(&value)
        .expect("catalog entry should parse");

    assert!(
        model.input.is_empty(),
        "an entry that declares nothing must not become a capability claim"
    );
}

/// Gateways that flatten the field are accepted too, not only the nested shape.
#[test]
fn flat_input_modalities_are_parsed() {
    let value: serde_json::Value =
        serde_json::from_str(r#"{"id": "m", "input_modalities": ["text", "image"]}"#)
            .expect("valid json");
    let model = crate::models_catalog_parse::parse_model_info_value(&value)
        .expect("catalog entry should parse");

    assert_eq!(
        model.input,
        vec!["text".to_string(), "image".to_string()],
        "a flat input_modalities key must be honoured"
    );
}

/// Cold start: the in-memory catalog is empty and the catalog is only on disk.
///
/// The in-memory cache is initialised empty and the catalog is normally already
/// persisted from the previous run, so this is the state of the very first
/// request after startup. An earlier version of the fix consulted memory only,
/// which meant the first request still clamped images even though the disk
/// catalog declared image input, and the defect only showed up after a refresh
/// had populated memory.
#[test]
fn cold_start_uses_the_persisted_catalog_for_image_support() {
    let _lock = ENV_LOCK.lock();
    let temp = tempfile::tempdir().expect("temp jcode home");
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path().to_str().expect("utf8 path"));
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _key = EnvVarGuard::set("TEST_COLD_START_KEY", "test-key");

    let api_base = "https://coldstart.models.test/v1";
    let profile = jcode_base::config::NamedProviderConfig {
        base_url: api_base.to_string(),
        api_key_env: Some("TEST_COLD_START_KEY".to_string()),
        model_catalog: true,
        ..Default::default()
    };
    let provider = OpenRouterProvider::new_named_openai_compatible("coldstart", &profile)
        .expect("named profile should initialize");

    let cache_dir = temp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).expect("create cache dir");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    // The persisted shape is the serialized ModelInfo, so the declared
    // modalities live under `input` rather than the API's `architecture` wrapper.
    let cache = serde_json::json!({
        "cached_at": now,
        "source_api_base": api_base,
        "models": [{
            "id": "vendor/cold-vision",
            "name": "cold vision",
            "context_length": null,
            "input": ["text", "image"],
        }],
    });
    std::fs::write(
        cache_dir.join("coldstart_models.json"),
        serde_json::to_string(&cache).expect("serialize cache"),
    )
    .expect("write cache");

    assert!(
        provider.load_usable_model_disk_cache_entry().is_some(),
        "test setup: the disk cache must load, or this test would pass vacuously"
    );
    {
        let memory = provider.models_cache.blocking_read();
        assert!(
            memory.models.is_empty(),
            "test setup: the in-memory catalog must start cold"
        );
    }

    assert!(
        provider.catalog_declares_image_input("vendor/cold-vision"),
        "a cold provider must still honour the persisted image declaration"
    );
}
