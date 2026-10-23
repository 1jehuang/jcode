//! Parsing of OpenAI-compatible `/v1/models` catalog responses.
//!
//! Kept separate from the provider runtime so the shape-tolerance rules for the
//! many gateways jcode speaks to (vLLM, llama.cpp, Ollama, LM Studio, vendor
//! clouds) live in one small, testable place.

use anyhow::{Context, Result};
use jcode_provider_openrouter::{ModelInfo, ModelPricing};
use serde_json::Value;

pub(crate) fn parse_openai_compatible_models_response(raw_body: &str) -> Result<Vec<ModelInfo>> {
    let value: Value = serde_json::from_str(raw_body)?;
    let items = match &value {
        Value::Array(items) => items,
        Value::Object(object) => object
            .get("data")
            .or_else(|| object.get("models"))
            .and_then(Value::as_array)
            .context("missing model array")?,
        _ => anyhow::bail!("model catalog response must be an object or array"),
    };

    let mut models = Vec::new();
    for item in items {
        if let Some(model) = parse_model_info_value(item) {
            models.push(model);
        }
    }

    if models.is_empty() {
        anyhow::bail!("model catalog response did not contain any valid model objects");
    }

    Ok(models)
}

pub(crate) fn parse_model_info_value(value: &Value) -> Option<ModelInfo> {
    let object = value.as_object()?;
    // Native Muse subscription catalog hides non-user-facing entries via
    // `metadata["muse-code"].is_hidden`. The field only appears on the native
    // shape; ordinary providers never set it, so filtering here cannot regress
    // them. Source: observed schema gh api repos/danzilberdan/opencode-muse/contents/src/oauth.ts
    if is_muse_hidden_model(object) {
        return None;
    }
    // Muse native entries that do not emit text (e.g. image-only output) are
    // not chat-completable and are hidden from the picker. The check only
    // applies when the native `metadata["muse-code"].modalities.output` array
    // is present; ordinary providers never set it.
    if is_muse_non_text_output(object) {
        return None;
    }
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| object.get("name").and_then(Value::as_str))?
        .to_string();
    // The native shape puts the display label in `metadata["muse-code"].name`
    // rather than a top-level `name`/`display_name`. Preserve it.
    let muse_display_name = object
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|meta| meta.get("muse-code"))
        .and_then(Value::as_object)
        .and_then(|mc| mc.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let name = muse_display_name.unwrap_or_else(|| {
        object
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| object.get("display_name").and_then(Value::as_str))
            .or_else(|| object.get("displayName").and_then(Value::as_str))
            .unwrap_or("")
            .to_string()
    });

    Some(ModelInfo {
        id,
        name,
        context_length: first_u64_field(
            object,
            &[
                "context_length",
                "context_window",
                "context_size",
                "contextLength",
                "max_context_length",
                "maxModelLength",
                "max_model_len",
                "trainingContextLength",
            ],
        )
        // Native Muse shape nests the live window in
        // `metadata["muse-code"].limit.context` (and .output). Preserve it
        // verbatim; no fallback to `output` and no guessed default like
        // Wonder/Full (1_007_997) is synthesized here.
        .or_else(|| parse_muse_context_limit(object))
        // llama.cpp's /v1/models reports the serving context only inside
        // `meta` (`n_ctx`, with `n_ctx_train` as the trained maximum). Without
        // this, local llama.cpp models fall back to the generic 200K default
        // and the context gauge overstates the real window (issue #447).
        .or_else(|| {
            object
                .get("meta")
                .and_then(Value::as_object)
                .and_then(|meta| first_u64_field(meta, &["n_ctx", "n_ctx_train"]))
        }),
        pricing: parse_model_pricing(object.get("pricing")),
        created: object.get("created").and_then(value_as_u64),
        input: parse_input_modalities(object),
    })
}

fn is_muse_hidden_model(object: &serde_json::Map<String, Value>) -> bool {
    object
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|meta| meta.get("muse-code"))
        .and_then(Value::as_object)
        .and_then(|mc| mc.get("is_hidden"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn is_muse_non_text_output(object: &serde_json::Map<String, Value>) -> bool {
    let Some(outputs) = object
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|meta| meta.get("muse-code"))
        .and_then(Value::as_object)
        .and_then(|mc| mc.get("modalities"))
        .and_then(Value::as_object)
        .and_then(|m| m.get("output"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    if outputs.is_empty() {
        return false;
    }
    !outputs.iter().any(|value| {
        value
            .as_str()
            .map(|text| text.trim().eq_ignore_ascii_case("text"))
            .unwrap_or(false)
    })
}

fn parse_muse_context_limit(object: &serde_json::Map<String, Value>) -> Option<u64> {
    let mc = object
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|meta| meta.get("muse-code"))
        .and_then(Value::as_object)?;
    let limit = mc.get("limit")?.as_object()?;
    value_as_u64(limit.get("context")?)
}

/// Read the modalities a catalog entry declares for its input.
///
/// OpenRouter nests them under `architecture.input_modalities`; other
/// gateways that speak `/v1/models` use a flat `input_modalities` or `input`
/// key. The native Muse shape uses `metadata["muse-code"].modalities.input`
/// and `.output` (arrays of strings such as "text", "image"). Anything else
/// yields an empty list, which callers treat as "not declared" so a missing
/// field never becomes a capability claim.
pub(crate) fn parse_input_modalities(object: &serde_json::Map<String, Value>) -> Vec<String> {
    let declared = object
        .get("architecture")
        .and_then(Value::as_object)
        .and_then(|architecture| architecture.get("input_modalities"))
        .or_else(|| object.get("input_modalities"))
        .or_else(|| object.get("input"))
        .or_else(|| {
            object
                .get("metadata")
                .and_then(Value::as_object)
                .and_then(|meta| meta.get("muse-code"))
                .and_then(Value::as_object)
                .and_then(|mc| mc.get("modalities"))
                .and_then(Value::as_object)
                .and_then(|m| m.get("input"))
        });

    let Some(Value::Array(items)) = declared else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|modality| !modality.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

pub(crate) fn first_u64_field(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Option<u64> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(value_as_u64))
}

pub(crate) fn value_as_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse::<u64>().ok(),
        _ => None,
    }
}

pub(crate) fn value_as_pricing_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

pub(crate) fn parse_model_pricing(value: Option<&Value>) -> ModelPricing {
    let Some(Value::Object(object)) = value else {
        return ModelPricing::default();
    };

    ModelPricing {
        prompt: object
            .get("prompt")
            .or_else(|| object.get("input"))
            .and_then(value_as_pricing_string),
        completion: object
            .get("completion")
            .or_else(|| object.get("output"))
            .and_then(value_as_pricing_string),
        input_cache_read: object
            .get("input_cache_read")
            .or_else(|| object.get("cached_input"))
            .and_then(value_as_pricing_string),
        input_cache_write: object
            .get("input_cache_write")
            .and_then(value_as_pricing_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn novita_model_catalog_preserves_names_and_context_limits() {
        let models = parse_openai_compatible_models_response(
            r#"{"data":[{"id":"zai-org/glm-5.3","object":"model","owned_by":"novita","display_name":"GLM 5.3","context_size":1048576,"created":1787105676}]}"#,
        )
        .expect("Novita catalog response should parse");
        assert_eq!(models[0].id, "zai-org/glm-5.3");
        assert_eq!(models[0].name, "GLM 5.3");
        assert_eq!(models[0].context_length, Some(1_048_576));
        assert_eq!(models[0].created, Some(1_787_105_676));
    }

    #[test]
    fn conifer_context_window_and_pricing_are_parsed() {
        let models = parse_openai_compatible_models_response(
            r#"{"data":[{"id":"gpt-5.6-sol","context_window":1000000,"pricing":{"input":"0.1","output":"0.2","cached_input":"0.01"}}]}"#,
        )
        .expect("Conifer catalog response should parse");

        assert_eq!(models[0].context_length, Some(1_000_000));
        assert_eq!(models[0].pricing.prompt.as_deref(), Some("0.1"));
        assert_eq!(models[0].pricing.completion.as_deref(), Some("0.2"));
        assert_eq!(models[0].pricing.input_cache_read.as_deref(), Some("0.01"));
    }

    #[test]
    fn muse_native_shape_preserves_nested_context_and_filters_hidden() {
        // Source: GH api repos/danzilberdan/opencode-muse/contents/src/oauth.ts
        // data[] { id, metadata: { "muse-code": { name, is_hidden, modalities:{input,output}, limit:{context,output}, tool_call, ... } } }
        let models = parse_openai_compatible_models_response(
            r#"{
                "data": [
                    {"id":"muse-spark-1.3","metadata":{"muse-code":{"name":"Muse Spark 1.3","is_hidden":false,"modalities":{"input":["text","image"],"output":["text"]},"limit":{"context":1048576,"output":65536},"tool_call":true}}},
                    {"id":"muse-spark-contrib","metadata":{"muse-code":{"name":"Hidden","is_hidden":true,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":1048576,"output":65536}}}},
                    {"id":"muse-wonder-1.0","metadata":{"muse-code":{"name":"Muse Wonder 1.0","is_hidden":false,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":1007997,"output":65536}}}}
                ]
            }"#,
        )
        .expect("Muse native catalog should parse");
        // Hidden entry filtered.
        assert_eq!(models.len(), 2, "is_hidden models must be dropped");
        assert!(!models.iter().any(|m| m.id == "muse-spark-contrib"));
        // Nested limit.context preserved verbatim, not guessed Wonder Full 1_007_997.
        let spark = models.iter().find(|m| m.id == "muse-spark-1.3").unwrap();
        assert_eq!(spark.name, "Muse Spark 1.3");
        assert_eq!(spark.context_length, Some(1_048_576));
        assert!(spark.input.contains(&"text".to_string()));
        assert!(spark.input.contains(&"image".to_string()));
        let wonder = models.iter().find(|m| m.id == "muse-wonder-1.0").unwrap();
        assert_eq!(wonder.context_length, Some(1_007_997));
        // Top-level context_length would win over nested one; no synthesis when absent.
    }

    #[test]
    fn muse_native_missing_context_is_none_not_synthesized() {
        let models = parse_openai_compatible_models_response(
            r#"{"data":[{"id":"muse-wonder-1.0","metadata":{"muse-code":{"name":"Muse Wonder","is_hidden":false,"modalities":{"input":["text"],"output":["text"]},"limit":{"output":65536}}}}]}"#,
        )
        .unwrap();
        assert_eq!(
            models[0].context_length, None,
            "must not synthesize a Wonder Full id when context absent"
        );
    }

    #[test]
    fn ordinary_provider_without_muse_metadata_not_filtered() {
        // Regression guard: providers without metadata["muse-code"] must not be dropped.
        let models = parse_openai_compatible_models_response(
            r#"{"data":[{"id":"novita/zai-org-glm-5.3","context_size":1048576}]}"#,
        )
        .unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].context_length, Some(1_048_576));
    }

    #[test]
    fn muse_native_non_text_output_is_filtered() {
        // Image-only or other non-text output entries are not chat
        // completable and must be dropped; ordinary providers without
        // muse-code metadata must not be affected (regression guard below).
        let models = parse_openai_compatible_models_response(
            r#"{
                "data": [
                    {"id":"muse-text","metadata":{"muse-code":{"name":"Text","is_hidden":false,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":1024,"output":256}}}},
                    {"id":"muse-image-only","metadata":{"muse-code":{"name":"Image","is_hidden":false,"modalities":{"input":["text"],"output":["image"]},"limit":{"context":1024,"output":256}}}},
                    {"id":"muse-mixed","metadata":{"muse-code":{"name":"Mixed","is_hidden":false,"modalities":{"input":["text"],"output":["text","image"]},"limit":{"context":1024,"output":256}}}}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(models.len(), 2, "image-only output must be filtered");
        assert!(models.iter().any(|m| m.id == "muse-text"));
        assert!(models.iter().any(|m| m.id == "muse-mixed"));
        assert!(!models.iter().any(|m| m.id == "muse-image-only"));
    }

    #[test]
    fn muse_native_no_output_field_means_not_filtered() {
        let models = parse_openai_compatible_models_response(
            r#"{"data":[{"id":"muse-no-output","metadata":{"muse-code":{"name":"No output","is_hidden":false,"modalities":{"input":["text"]},"limit":{"context":1024,"output":256}}}}]}"#,
        )
        .unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "muse-no-output");
    }

    #[test]
    fn ordinary_provider_image_without_native_metadata_not_filtered() {
        // Ordinary catalogs never carry muse-code; they must not be dropped
        // even if they look image-ish. The native filter is scoped to
        // metadata["muse-code"].modalities.output only.
        let models = parse_openai_compatible_models_response(
            r#"{"data":[
                {"id":"openai/dall-e-3","name":"DALL-E 3"},
                {"id":"novita/zai-org-glm-5.3","context_size":1048576}
            ]}"#,
        )
        .unwrap();
        assert_eq!(models.len(), 2);
    }

    #[test]
    fn muse_native_human_name_prefers_metadata_name() {
        // Native name lives in metadata; top-level display_name must not win.
        let models = parse_openai_compatible_models_response(
            r#"{"data":[{"id":"muse-spark-1.3","display_name":"Wrong","metadata":{"muse-code":{"name":"Muse Spark 1.3","is_hidden":false,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":1024,"output":256}}}}]}"#,
        )
        .unwrap();
        assert_eq!(models[0].name, "Muse Spark 1.3");
    }

    #[test]
    fn muse_native_top_level_context_wins_over_nested() {
        // When both exist, the generic top-level window is preserved first.
        let models = parse_openai_compatible_models_response(
            r#"{"data":[{"id":"muse-spark-1.3","context_length":999,"metadata":{"muse-code":{"name":"Muse Spark","is_hidden":false,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":1048576,"output":256}}}}]}"#,
        )
        .unwrap();
        assert_eq!(models[0].context_length, Some(999));
    }

    #[test]
    fn muse_catalog_fixture_matches_observed_schema() {
        // Local fixture approximating the observed opencode-muse oauth.ts shape;
        // exercises hidden + text-only + nested limit + name in one pass.
        // No network, no credentials.
        let fixture = r#"{
            "data": [
                {"id":"muse-spark-1.3","metadata":{"muse-code":{"name":"Muse Spark 1.3","is_hidden":false,"modalities":{"input":["text","image"],"output":["text"]},"limit":{"context":1048576,"output":65536},"tool_call":true}}},
                {"id":"muse-spark-internal","metadata":{"muse-code":{"name":"Internal","is_hidden":true,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":1048576,"output":65536}}}},
                {"id":"muse-image-gen","metadata":{"muse-code":{"name":"ImageGen","is_hidden":false,"modalities":{"input":["text"],"output":["image"]},"limit":{"context":1048576,"output":65536}}}},
                {"id":"muse-wonder-1.0","metadata":{"muse-code":{"name":"Muse Wonder 1.0","is_hidden":false,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":1007997,"output":65536}}}}
            ]
        }"#;
        let models = parse_openai_compatible_models_response(fixture).unwrap();
        // hidden + non-text filtered -> 2 chat models
        assert_eq!(models.len(), 2);
        assert!(models.iter().any(|m| m.id == "muse-spark-1.3"));
        assert!(models.iter().any(|m| m.id == "muse-wonder-1.0"));
        // Wonder context is verbatim 1_007_997, not a guessed Wonder Full id.
        assert_eq!(
            models
                .iter()
                .find(|m| m.id == "muse-wonder-1.0")
                .unwrap()
                .context_length,
            Some(1_007_997)
        );
    }
}
