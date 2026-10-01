//! OpenAI Responses hosted `web_search` tool (`websearch.engine = "native"`).
//!
//! When enabled, the request carries the hosted `web_search` tool instead of
//! jcode's local `websearch` tool, so searches run on OpenAI's side. The
//! resulting `web_search_call` output items are stored verbatim and replayed in
//! later requests, the same way Codex does.
//!
//! Docs: <https://platform.openai.com/docs/guides/tools-web-search>

use jcode_base::config::WebSearchConfig;
use jcode_message_types::ToolDefinition;
use serde_json::{Value, json};

/// Name of jcode's local scraping search tool, replaced by the hosted tool.
const LOCAL_WEBSEARCH_TOOL: &str = "websearch";

/// Whether the hosted `web_search` tool can be attached for `model_id`.
///
/// Mirrors the `image_generation` rule: `*-codex*` models reject unknown hosted
/// tools, so they keep the local tool.
pub(crate) fn model_supports_web_search(model_id: &str) -> bool {
    !model_id.to_ascii_lowercase().contains("codex")
}

/// The hosted tool definition built from config.
pub(crate) fn hosted_tool(config: &WebSearchConfig) -> Value {
    let mut tool = json!({ "type": "web_search" });
    let allowed: Vec<&str> = config
        .native_allowed_domains
        .iter()
        .map(|domain| domain.trim())
        .filter(|domain| !domain.is_empty())
        .collect();
    if !allowed.is_empty() {
        tool["filters"] = json!({ "allowed_domains": allowed });
    }
    tool
}

/// Hosted tools to attach, if native search is on and the model supports it.
pub(crate) fn hosted_tools_with_config(config: &WebSearchConfig, model_id: &str) -> Vec<Value> {
    if config.native_enabled() && model_supports_web_search(model_id) {
        vec![hosted_tool(config)]
    } else {
        Vec::new()
    }
}

pub(crate) fn hosted_tools_for_request(model_id: &str) -> Vec<Value> {
    hosted_tools_with_config(&jcode_base::config::config().websearch, model_id)
}

/// Drop jcode's local search tool when the hosted tool replaces it.
pub(crate) fn without_local_websearch<'a>(
    tools: &'a [ToolDefinition],
    hosted_tools: &[Value],
) -> std::borrow::Cow<'a, [ToolDefinition]> {
    if hosted_tools.is_empty() {
        return std::borrow::Cow::Borrowed(tools);
    }
    std::borrow::Cow::Owned(
        tools
            .iter()
            .filter(|tool| tool.name != LOCAL_WEBSEARCH_TOOL)
            .cloned()
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_base::config::WebSearchEngine;

    fn native_config() -> WebSearchConfig {
        WebSearchConfig {
            engine: WebSearchEngine::Native,
            ..WebSearchConfig::default()
        }
    }

    #[test]
    fn hosted_tool_only_when_native_and_not_codex() {
        assert!(hosted_tools_with_config(&WebSearchConfig::default(), "gpt-5.4").is_empty());
        assert!(hosted_tools_with_config(&native_config(), "gpt-5.3-codex").is_empty());
        assert_eq!(
            hosted_tools_with_config(&native_config(), "gpt-5.4"),
            vec![json!({"type": "web_search"})]
        );
    }

    #[test]
    fn allowed_domains_become_filters() {
        let mut config = native_config();
        config.native_allowed_domains = vec!["docs.rs".to_string(), " ".to_string()];
        assert_eq!(
            hosted_tool(&config),
            json!({"type": "web_search", "filters": {"allowed_domains": ["docs.rs"]}})
        );
    }

    #[test]
    fn local_websearch_dropped_only_with_hosted_tool() {
        let tools = vec![
            ToolDefinition::new("bash", "", json!({"type": "object"})),
            ToolDefinition::new("websearch", "", json!({"type": "object"})),
        ];
        assert_eq!(without_local_websearch(&tools, &[]).len(), 2);
        let kept = without_local_websearch(&tools, &[json!({"type": "web_search"})]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "bash");
    }
}
