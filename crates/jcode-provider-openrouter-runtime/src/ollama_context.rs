//! Ollama serving-context discovery for the OpenAI-compatible runtime.
//!
//! Ollama's `/v1/models` response carries no `context_length`, so jcode used to
//! fall back to a generic large default (or the model's advertised trained
//! window) and reported, for example, a 262K window for `qwen3:35b` while the
//! server was actually serving a 4K window. Ollama silently truncates the
//! prompt to that serving window, which looks exactly like "the model forgot
//! the conversation" (issue: Ollama conversations appear stateless in jcode).
//!
//! The serving window is `min(trained context, server default)` where the
//! server default comes from `OLLAMA_CONTEXT_LENGTH` (Ollama >= 0.6 defaults to
//! 4096) and cannot be overridden per-request through the OpenAI-compatible
//! endpoint. We therefore read both numbers from Ollama's native API:
//!
//! - `GET /api/ps` reports `context_length` for currently loaded models, which
//!   is the real serving window the server chose.
//! - `POST /api/show` reports `model_info["<family>.context_length"]`, the
//!   model's trained maximum.

use anyhow::{Context, Result};
use jcode_provider_openrouter::ModelInfo;
use reqwest::Client;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

/// Ollama's built-in default serving context when `OLLAMA_CONTEXT_LENGTH` is
/// unset. Kept as a last resort so an unreachable native API still yields a
/// truthful (conservative) window instead of a fabricated 262K one.
pub(crate) const OLLAMA_DEFAULT_SERVING_CONTEXT: u64 = 4096;

/// Ollama's `:cloud` tag identifies models executed by Ollama's cloud service,
/// not by the local runner. Local `OLLAMA_CONTEXT_LENGTH` and `/api/ps` limits
/// therefore do not constrain these models.
pub(crate) fn is_cloud_model(model: &str) -> bool {
    model
        .trim()
        .rsplit_once(':')
        .is_some_and(|(_, tag)| tag.eq_ignore_ascii_case("cloud"))
}

/// Native-API probes are best-effort metadata enrichment on a catalog fetch, so
/// keep them short enough that a wedged local server cannot stall model listing.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// True when `api_base` looks like an Ollama OpenAI-compatible endpoint.
pub(crate) fn is_ollama_api_base(api_base: &str, profile_id: Option<&str>) -> bool {
    if profile_id.is_some_and(|id| id.eq_ignore_ascii_case("ollama")) {
        return true;
    }
    let lower = api_base.to_ascii_lowercase();
    lower.contains(":11434")
}

/// Strip the trailing OpenAI API version segment so native `/api/*` routes can
/// be addressed on the same host.
pub(crate) fn ollama_native_root(api_base: &str) -> String {
    let trimmed = api_base.trim_end_matches('/');
    trimmed
        .strip_suffix("/v1")
        .unwrap_or(trimmed)
        .trim_end_matches('/')
        .to_string()
}

/// Serving window for one model: the trained window clamped by the server's
/// configured default.
///
/// When the server default is unknown (nothing loaded yet, so `/api/ps` is
/// empty) we must not fall back to the trained window: over-budgeting is
/// exactly the failure this module exists to prevent, because Ollama truncates
/// silently and the conversation appears to lose its history. Assume Ollama's
/// own default instead; the value self-corrects on the next catalog refresh
/// once a model is loaded and `/api/ps` reports the real window.
pub(crate) fn effective_serving_context(trained: Option<u64>, server_default: Option<u64>) -> u64 {
    let default = server_default.unwrap_or(OLLAMA_DEFAULT_SERVING_CONTEXT);
    trained.map_or(default, |trained| trained.min(default))
}

/// Resolve a model's effective window without applying local-runner limits to
/// Ollama cloud models. For cloud routes, `/api/show` metadata describes the
/// remote serving window and is authoritative when available.
pub(crate) fn effective_context_for_model(
    model: &str,
    trained: Option<u64>,
    server_default: Option<u64>,
) -> u64 {
    if is_cloud_model(model) {
        trained
            .or(server_default)
            .unwrap_or(OLLAMA_DEFAULT_SERVING_CONTEXT)
    } else {
        effective_serving_context(trained, server_default)
    }
}

/// Server default serving window, parsed from `/api/ps`. Ollama applies the
/// same `OLLAMA_CONTEXT_LENGTH` to every model it loads, so any loaded model
/// reveals it. Models whose trained window is smaller are clamped below the
/// server default, so take the maximum across loaded models.
fn parse_server_default_from_ps(body: &str) -> Result<Option<u64>> {
    let value: Value = serde_json::from_str(body).context("/api/ps response was not valid JSON")?;
    let Some(models) = value.get("models").and_then(Value::as_array) else {
        return Ok(None);
    };
    Ok(models
        .iter()
        .filter_map(|m| m.get("context_length").and_then(Value::as_u64))
        .max())
}

/// Trained context window from an `/api/show` response, e.g. the
/// `qwen3.context_length` entry under `model_info`.
fn parse_trained_context_from_show(body: &str) -> Result<Option<u64>> {
    let value: Value =
        serde_json::from_str(body).context("/api/show response was not valid JSON")?;
    let Some(model_info) = value.get("model_info").and_then(Value::as_object) else {
        return Ok(None);
    };
    Ok(model_info
        .iter()
        .filter(|(key, _)| key.ends_with(".context_length"))
        .filter_map(|(_, value)| value.as_u64())
        .max())
}

async fn fetch_server_default(client: &Client, root: &str) -> Result<Option<u64>> {
    let body = client
        .get(format!("{root}/api/ps"))
        .timeout(PROBE_TIMEOUT)
        .send()
        .await
        .context("GET /api/ps failed")?
        .text()
        .await
        .context("reading /api/ps body failed")?;
    parse_server_default_from_ps(&body)
}

async fn fetch_trained_context(client: &Client, root: &str, model: &str) -> Result<Option<u64>> {
    let body = client
        .post(format!("{root}/api/show"))
        .timeout(PROBE_TIMEOUT)
        .json(&serde_json::json!({ "model": model }))
        .send()
        .await
        .context("POST /api/show failed")?
        .text()
        .await
        .context("reading /api/show body failed")?;
    parse_trained_context_from_show(&body)
}

/// Log-and-degrade wrapper: a failed probe must never fail catalog listing, but
/// it should be visible in the log rather than silently producing a wrong window.
fn probe_or_log<T>(what: &str, result: Result<Option<T>>) -> Option<T> {
    match result {
        Ok(value) => value,
        Err(error) => {
            jcode_base::logging::info(&format!(
                "Ollama context probe ({what}) failed, falling back to the default serving window: {error:#}"
            ));
            None
        }
    }
}

/// Fill in `context_length` for Ollama models when `api_base` is an Ollama
/// endpoint. Ollama's OpenAI-compatible `/v1/models` omits `context_length`
/// entirely, so without this every model would inherit a generic large default
/// while the server silently truncates prompts to a far smaller window.
pub(crate) async fn maybe_enrich(
    client: &Client,
    api_base: &str,
    profile_id: Option<&str>,
    models: &mut [ModelInfo],
) {
    if !is_ollama_api_base(api_base, profile_id) {
        return;
    }
    enrich_ollama_context_lengths(client, api_base, models).await;
}

/// Fill in `context_length` for Ollama models using the native API.
///
/// Best-effort: models whose metadata cannot be read keep whatever the
/// OpenAI-compatible response provided. When the server default is known and a
/// model reports a much larger trained window, a one-line hint is logged so the
/// user knows to raise `OLLAMA_CONTEXT_LENGTH` instead of assuming jcode lost
/// the conversation.
async fn enrich_ollama_context_lengths(client: &Client, api_base: &str, models: &mut [ModelInfo]) {
    let root = ollama_native_root(api_base);
    let server_default = probe_or_log("/api/ps", fetch_server_default(client, &root).await);

    let mut trained_by_model: HashMap<String, Option<u64>> = HashMap::new();
    for model in models.iter() {
        if trained_by_model.contains_key(&model.id) {
            continue;
        }
        let trained = probe_or_log(
            "/api/show",
            fetch_trained_context(client, &root, &model.id).await,
        );
        trained_by_model.insert(model.id.clone(), trained);
    }

    for model in models.iter_mut() {
        let trained = trained_by_model.get(&model.id).copied().flatten();
        let effective = effective_context_for_model(&model.id, trained, server_default);
        model.context_length = Some(effective);

        if let Some(trained) = trained
            && trained > effective
        {
            jcode_base::logging::info(&format!(
                "Ollama model {} serves {} tokens of context but is trained for {}. \
                 Ollama caps this server-side and ignores per-request overrides on \
                 /v1/chat/completions; restart with OLLAMA_CONTEXT_LENGTH={} (or set \
                 num_ctx in a Modelfile) to use the full window.",
                model.id, effective, trained, trained
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_ollama_endpoints() {
        assert!(is_ollama_api_base("http://localhost:11434/v1", None));
        assert!(is_ollama_api_base("http://127.0.0.1:11434/v1", None));
        assert!(is_ollama_api_base(
            "http://box.local:8080/v1",
            Some("ollama")
        ));
        assert!(!is_ollama_api_base("http://127.0.0.1:1234/v1", None));
        assert!(!is_ollama_api_base("https://openrouter.ai/api/v1", None));
    }

    #[test]
    fn strips_openai_version_segment_for_native_routes() {
        assert_eq!(
            ollama_native_root("http://localhost:11434/v1"),
            "http://localhost:11434"
        );
        assert_eq!(
            ollama_native_root("http://localhost:11434/v1/"),
            "http://localhost:11434"
        );
        assert_eq!(
            ollama_native_root("http://localhost:11434"),
            "http://localhost:11434"
        );
    }

    #[test]
    fn serving_context_is_clamped_by_server_default() {
        assert_eq!(effective_serving_context(Some(262_144), Some(4096)), 4096);
        assert_eq!(effective_serving_context(Some(4096), Some(262_144)), 4096);
        assert_eq!(effective_serving_context(None, Some(16_384)), 16_384);
    }

    #[test]
    fn cloud_model_uses_remote_trained_context_instead_of_local_clamp() {
        assert!(is_cloud_model("glm-5.2:cloud"));
        assert!(is_cloud_model("GLM-5.2:CLOUD"));
        assert!(!is_cloud_model("glm-5.2"));
        assert!(!is_cloud_model("cloud-model:latest"));
        assert_eq!(
            effective_context_for_model("glm-5.2:cloud", Some(1_000_000), Some(4096)),
            1_000_000
        );
        assert_eq!(
            effective_context_for_model("glm-5.2", Some(1_000_000), Some(4096)),
            4096
        );
    }

    #[test]
    fn unknown_server_default_never_trusts_the_trained_window() {
        // Over-budgeting here is the actual bug: Ollama would silently truncate.
        assert_eq!(
            effective_serving_context(Some(262_144), None),
            OLLAMA_DEFAULT_SERVING_CONTEXT
        );
        assert_eq!(
            effective_serving_context(None, None),
            OLLAMA_DEFAULT_SERVING_CONTEXT
        );
        // A model trained smaller than the default still reports its own limit.
        assert_eq!(effective_serving_context(Some(2048), None), 2048);
    }

    #[test]
    fn parses_server_default_from_ps_payload() {
        let body = r#"{"models":[{"model":"qwen3:0.6b","context_length":16384},
                       {"model":"llama3.2","context_length":4096}]}"#;
        assert_eq!(parse_server_default_from_ps(body).unwrap(), Some(16_384));
    }

    #[test]
    fn missing_loaded_models_yields_no_server_default() {
        assert_eq!(
            parse_server_default_from_ps(r#"{"models":[]}"#).unwrap(),
            None
        );
        assert_eq!(parse_server_default_from_ps(r#"{}"#).unwrap(), None);
        assert!(parse_server_default_from_ps("not json").is_err());
    }

    #[test]
    fn parses_trained_context_from_show_payload() {
        let body = r#"{"model_info":{"general.architecture":"qwen3",
                        "qwen3.context_length":40960,
                        "qwen3.embedding_length":1024}}"#;
        assert_eq!(parse_trained_context_from_show(body).unwrap(), Some(40_960));
        assert_eq!(
            parse_trained_context_from_show(r#"{"model_info":{}}"#).unwrap(),
            None
        );
        assert!(parse_trained_context_from_show("not json").is_err());
    }
}
