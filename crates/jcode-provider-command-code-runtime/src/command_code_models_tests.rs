//! Plan 02 tests: discovery fallback, alias mapping, reasoning-recovery
//! single-retry semantics (all offline; live network not exercised).

use crate::efforts::{CommandCodeReasoningCapability, is_reasoning_effort_rejection};
use crate::models::{
    CommandCodeCatalog, canonicalize_command_code_model, parse_command_code_models,
};
use jcode_provider_command_code::CURATED_MODELS;
use std::time::Duration;

fn fresh_catalog() -> CommandCodeCatalog {
    CommandCodeCatalog::new()
}

#[test]
fn command_code_catalog_success_replaces_curated_fallback() {
    let catalog = fresh_catalog();
    assert!(
        catalog
            .model_ids()
            .iter()
            .all(|model| CURATED_MODELS.contains(&model.as_str()))
    );
    let replaced = [
        "zai-org/GLM-5.3",
        "deepseek/deepseek-v4-flash",
        "new-model/x",
    ];
    let stored = replaced.iter().map(|model| model.to_string()).collect();
    let replaced_catalog = replaced
        .iter()
        .map(|model| model.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        stored,
        vec![
            "zai-org/GLM-5.3".to_string(),
            "deepseek/deepseek-v4-flash".to_string(),
            "new-model/x".to_string()
        ]
    );
    assert!(catalog.refresh_with(move || Ok(stored)).expect("refresh"));
    assert_eq!(catalog.model_ids().len(), replaced_catalog.len());
    assert!(catalog.model_ids().contains(&"new-model/x".to_string()));
    assert!(catalog.is_recent(Duration::from_secs(60)));
}

#[test]
fn command_code_catalog_failure_keeps_curated_fallback() {
    let catalog = fresh_catalog();
    let before = catalog.model_ids();
    assert!(
        catalog
            .refresh_with(|| Err(anyhow::anyhow!("discovery offline")))
            .is_err()
    );
    assert_eq!(catalog.model_ids(), before);
    assert!(!catalog.is_recent(Duration::from_secs(1)));
    assert!(catalog.observed_at().is_none());
    // Even an empty live payload must not wipe the fallback.
    assert!(catalog.refresh_with(|| Ok(Vec::new())).is_err());
    assert_eq!(catalog.model_ids(), before);
}

#[test]
fn command_code_catalog_parses_wrapped_and_bare_shapes() {
    let wrapped: serde_json::Value = serde_json::json!({
        "data": [
            {"id": "zai-org/GLM-5.3"},
            {"id": "deepseek/deepseek-v4-flash"},
            {"id": "deepseek/deepseek-v4-flash"}
        ]
    });
    let ids = parse_command_code_models(&wrapped);
    assert_eq!(ids.len(), 2, "duplicates collapse, case preserved");
    assert_eq!(ids[0], "zai-org/GLM-5.3");

    let bare: serde_json::Value = serde_json::json!([{"model": "moonshotai/Kimi-K3"}]);
    let ids = parse_command_code_models(&bare);
    assert_eq!(ids, vec!["moonshotai/Kimi-K3".to_string()]);
}

#[test]
fn command_code_alias_resolves_to_case_sensitive_canonical_ids() {
    let catalog = fresh_catalog();
    assert_eq!(
        canonicalize_command_code_model("glm-5.3", &catalog).as_deref(),
        Some("zai-org/GLM-5.3")
    );
    assert_eq!(
        canonicalize_command_code_model("GLM-5.3", &catalog).as_deref(),
        Some("zai-org/GLM-5.3")
    );
    assert_eq!(
        canonicalize_command_code_model("kimi", &catalog).as_deref(),
        Some("moonshotai/Kimi-K3")
    );
    assert_eq!(
        canonicalize_command_code_model("deepseek-v4-flash", &catalog).as_deref(),
        Some("deepseek/deepseek-v4-flash")
    );
    // Canonical ids stay case-sensitive on the wire.
    assert_eq!(
        canonicalize_command_code_model("zai-org/glm-5.3", &catalog),
        None
    );
    assert_eq!(canonicalize_command_code_model("", &catalog), None);
    assert_eq!(
        canonicalize_command_code_model("totally-unknown", &catalog),
        None
    );
}

#[test]
fn command_code_reasoning_rejection_matches_only_reasoning_errors() {
    assert!(is_reasoning_effort_rejection(
        400,
        "reasoning_effort unsupported: ultra"
    ));
    assert!(is_reasoning_effort_rejection(422, "Invalid effort value"));
    assert!(is_reasoning_effort_rejection(
        400,
        "Unsupported effort for model"
    ));
    // Unrelated client errors must not classify as reasoning rejections.
    assert!(!is_reasoning_effort_rejection(400, "malformed json body"));
    assert!(!is_reasoning_effort_rejection(401, "unauthorized"));
    assert!(!is_reasoning_effort_rejection(
        500,
        "reasoning_effort mismatch"
    ));
    assert!(!is_reasoning_effort_rejection(429, "rate limited"));
}

#[test]
fn command_code_reasoning_retry_is_once_and_cached() {
    let cache = CommandCodeReasoningCapability::default();
    let model = "zai-org/GLM-5.3";
    let parsed_body = serde_json::json!({
        "model": model,
        "params": {"reasoning_effort": "ultra", "stream": true}
    });
    assert!(!cache.reasoning_denied(model));
    let rejection = cache
        .classify_pre_stream_rejection(model, 400, "error: reasoning_effort ultra not allowed")
        .expect("reasoning rejection classified");
    assert_eq!(rejection.model, model);
    assert!(cache.reasoning_denied(model));
    let degraded = retry_body_without_reasoning(&parsed_body).expect("degraded body");
    assert!(degraded.pointer("/params/reasoning_effort").is_none());
    // A second identical rejection must not mutate anything new.
    cache.classify_pre_stream_rejection(model, 400, "reasoning_effort again");
    assert!(cache.reasoning_denied(model));
    // Ladder still exposes supported choices for user visibility checks.
    assert!(!cache.visible_efforts(model).is_empty());
    // Unrelated errors never write the cache (D-18).
    let unrelated_cache = CommandCodeReasoningCapability::default();
    unrelated_cache.classify_pre_stream_rejection(model, 422, "malformed body");
    assert!(!unrelated_cache.reasoning_denied(model));
}

fn retry_body_without_reasoning(body: &serde_json::Value) -> Option<serde_json::Value> {
    let mut body = body.clone();
    let _ = body
        .get_mut("params")
        .and_then(|params| params.as_object_mut())
        .map(|params| params.remove("reasoning_effort"))?;
    Some(body)
}
