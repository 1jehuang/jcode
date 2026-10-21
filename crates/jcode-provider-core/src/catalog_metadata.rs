use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelMetadata {
    pub context_window: Option<usize>,
    pub max_input_tokens: Option<usize>,
    pub max_output_tokens: Option<usize>,
    pub max_non_streaming_output_tokens: Option<usize>,
    pub reasoning_efforts: Vec<String>,
    /// Provider labels for the selectable reasoning values, such as ChatGPT's
    /// `none` = Instant and `max` = Pro. Older catalogs omit these labels.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub reasoning_effort_labels: HashMap<String, String>,
    pub supports_image_input: Option<bool>,
    pub supports_tool_calls: Option<bool>,
    pub supports_parallel_tool_calls: Option<bool>,
    pub supports_streaming: Option<bool>,
    pub supports_structured_outputs: Option<bool>,
    pub supports_adaptive_thinking: Option<bool>,
    pub supported_endpoints: Vec<String>,
}

static COPILOT_MODEL_METADATA: LazyLock<RwLock<HashMap<String, ModelMetadata>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn register_copilot_model_metadata(metadata: HashMap<String, ModelMetadata>) {
    *COPILOT_MODEL_METADATA
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = metadata;
}

pub fn copilot_model_metadata(model: &str) -> Option<ModelMetadata> {
    let model = crate::strip_own_model_prefix(model, "copilot:");
    COPILOT_MODEL_METADATA
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(model)
        .cloned()
}

pub fn register_copilot_route_metadata(routes: &[crate::ModelRoute]) {
    let metadata = routes
        .iter()
        .filter(|route| crate::provider_key_from_hint(Some(&route.api_method)) == Some("copilot"))
        .filter_map(|route| {
            route
                .catalog_metadata
                .clone()
                .map(|metadata| (route.model.clone(), metadata))
        })
        .collect();
    register_copilot_model_metadata(metadata);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_round_trip_distinct_prompt_and_context_limits() {
        let metadata = ModelMetadata {
            context_window: Some(400_000),
            max_input_tokens: Some(272_000),
            max_output_tokens: Some(128_000),
            supports_image_input: Some(true),
            reasoning_efforts: vec!["low".into(), "max".into()],
            reasoning_effort_labels: HashMap::from([("max".into(), "Pro".into())]),
            ..ModelMetadata::default()
        };
        let encoded = serde_json::to_vec(&metadata).unwrap();
        assert_eq!(
            serde_json::from_slice::<ModelMetadata>(&encoded).unwrap(),
            metadata
        );
    }

    #[test]
    fn should_keep_legacy_routes_compatible_without_metadata() {
        let route: crate::ModelRoute = serde_json::from_value(serde_json::json!({
            "model": "legacy", "provider": "Copilot", "api_method": "copilot",
            "available": true, "detail": ""
        }))
        .unwrap();
        assert_eq!(route.catalog_metadata, None);
    }

    #[test]
    fn should_use_remote_catalog_for_copilot_reasoning_controls() {
        let route = crate::ModelRoute {
            model: "metadata-test-model".into(),
            provider: "Copilot".into(),
            api_method: "copilot".into(),
            available: true,
            detail: String::new(),
            catalog_metadata: Some(ModelMetadata {
                reasoning_efforts: vec!["low".into(), "medium".into(), "high".into()],
                ..ModelMetadata::default()
            }),
            cheapness: None,
            usage: None,
        };
        register_copilot_route_metadata(&[route]);
        assert_eq!(
            crate::inferred_reasoning_efforts(Some("Copilot"), Some("metadata-test-model")),
            vec!["low", "medium", "high"]
        );
    }
}
