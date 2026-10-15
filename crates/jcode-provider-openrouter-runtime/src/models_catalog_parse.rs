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
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| object.get("name").and_then(Value::as_str))?
        .to_string();
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| object.get("display_name").and_then(Value::as_str))
        .or_else(|| object.get("displayName").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();

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
        pricing: match object.get("pricing") {
            Some(Value::Object(_)) => parse_model_pricing(object.get("pricing")),
            _ => parse_flat_model_pricing(object),
        },
        created: object.get("created").and_then(value_as_u64),
        input: parse_input_modalities(object),
    })
}

/// Read the modalities a catalog entry declares for its input.
///
/// OpenRouter nests them under `architecture.input_modalities`; other
/// gateways that speak `/v1/models` use a flat `input_modalities` or `input`
/// key. Anything else yields an empty list, which callers treat as "not
/// declared" so a missing field never becomes a capability claim.
pub(crate) fn parse_input_modalities(object: &serde_json::Map<String, Value>) -> Vec<String> {
    let declared = object
        .get("architecture")
        .and_then(Value::as_object)
        .and_then(|architecture| architecture.get("input_modalities"))
        .or_else(|| object.get("input_modalities"))
        .or_else(|| object.get("input"));

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

/// Read per-token prices published as flat top-level fields.
///
/// Requesty's `/v1/models` returns `pricing` as an array of context-length
/// tiers and repeats the base tier as `input_price`, `output_price`,
/// `cached_price` (cache read) and `caching_price` (cache write). The flat
/// fields are the base rate, which is what `ModelPricing` can represent.
pub(crate) fn parse_flat_model_pricing(object: &serde_json::Map<String, Value>) -> ModelPricing {
    let price = |key: &str| object.get(key).and_then(value_as_pricing_string);
    ModelPricing {
        prompt: price("input_price"),
        completion: price("output_price"),
        input_cache_read: price("cached_price"),
        input_cache_write: price("caching_price"),
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
    fn requesty_flat_prices_are_parsed_when_pricing_is_a_tier_array() {
        // Trimmed from a live router.requesty.ai/v1/models entry.
        let models = parse_openai_compatible_models_response(
            r#"{"object":"list","data":[{"id":"anthropic/claude-sonnet-4-5","object":"model","created":1759161676,"context_window":1000000,"input_price":0.000003,"caching_price":0.00000375,"cached_price":3e-7,"output_price":0.000015,"pricing":[{"prompt_tokens_threshold":0,"input_price":0.000003,"caching_price":0.00000375,"cached_price":3e-7,"output_price":0.000015},{"prompt_tokens_threshold":200000,"input_price":0.000006,"caching_price":0.0000075,"cached_price":6e-7,"output_price":0.0000225}],"supports_caching":true}]}"#,
        )
        .expect("Requesty catalog response should parse");

        let pricing = &models[0].pricing;
        let as_f64 = |v: Option<&str>| v.and_then(|v| v.parse::<f64>().ok());
        assert_eq!(models[0].context_length, Some(1_000_000));
        assert_eq!(as_f64(pricing.prompt.as_deref()), Some(0.000003));
        assert_eq!(as_f64(pricing.completion.as_deref()), Some(0.000015));
        assert_eq!(as_f64(pricing.input_cache_read.as_deref()), Some(3e-7));
        assert_eq!(
            as_f64(pricing.input_cache_write.as_deref()),
            Some(0.00000375)
        );
    }

    #[test]
    fn model_without_any_pricing_fields_has_default_pricing() {
        let models = parse_openai_compatible_models_response(
            r#"{"data":[{"id":"my-policy","object":"model"}]}"#,
        )
        .expect("catalog response should parse");
        assert!(models[0].pricing.prompt.is_none());
        assert!(models[0].pricing.input_cache_read.is_none());
    }
}
