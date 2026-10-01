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
/// OpenAI's own Responses API base. Custom gateways may not implement hosted
/// tools, so they keep the local tool.
const FIRST_PARTY_API_BASE: &str = "https://api.openai.com/v1";

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

/// True when requests go to an OpenAI backend that implements hosted tools:
/// the ChatGPT/Codex OAuth backend, or the first-party API-key endpoint.
pub(crate) fn first_party_backend(is_chatgpt_mode: bool) -> bool {
    is_chatgpt_mode
        || jcode_base::provider::openai::resolve_api_base().trim_end_matches('/')
            == FIRST_PARTY_API_BASE
}

/// True when the session offers search at all. The hosted tool only replaces
/// the local `websearch` tool, so a session whose tool policy (allowed /
/// disabled tools, tool profile, SDK config) excludes `websearch` never gets
/// provider-side search either.
pub(crate) fn session_offers_websearch(tools: &[ToolDefinition]) -> bool {
    tools.iter().any(|tool| tool.name == LOCAL_WEBSEARCH_TOOL)
}

/// Hosted tools to attach: native search preferred, the model and backend
/// support it, and the session's tool policy allows search.
pub(crate) fn hosted_tools_with_config(
    config: &WebSearchConfig,
    model_id: &str,
    first_party: bool,
    tools: &[ToolDefinition],
) -> Vec<Value> {
    if config.native_enabled()
        && first_party
        && model_supports_web_search(model_id)
        && session_offers_websearch(tools)
    {
        vec![hosted_tool(config)]
    } else {
        Vec::new()
    }
}

pub(crate) fn hosted_tools_for_request(
    model_id: &str,
    is_chatgpt_mode: bool,
    tools: &[ToolDefinition],
) -> Vec<Value> {
    hosted_tools_with_config(
        &jcode_base::config::config().websearch,
        model_id,
        first_party_backend(is_chatgpt_mode),
        tools,
    )
}

pub(crate) use jcode_provider_openai::downgrade_web_search_calls;

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
        let tools = vec![ToolDefinition::new(
            "websearch",
            "",
            json!({"type": "object"}),
        )];
        let off = WebSearchConfig {
            prefer_native: false,
            ..WebSearchConfig::default()
        };
        assert!(hosted_tools_with_config(&off, "gpt-5.4", true, &tools).is_empty());
        assert!(
            hosted_tools_with_config(&native_config(), "gpt-5.3-codex", true, &tools).is_empty()
        );
        assert_eq!(
            hosted_tools_with_config(&native_config(), "gpt-5.4", true, &tools),
            vec![json!({"type": "web_search"})]
        );
        // Native is the default wherever the provider supports it.
        assert_eq!(
            hosted_tools_with_config(&WebSearchConfig::default(), "gpt-5.4", true, &tools).len(),
            1
        );
    }

    #[test]
    fn hosted_tool_respects_session_policy_and_gateway() {
        let bash_only = vec![ToolDefinition::new("bash", "", json!({"type": "object"}))];
        assert!(hosted_tools_with_config(&native_config(), "gpt-5.4", true, &bash_only).is_empty());
        let tools = vec![ToolDefinition::new(
            "websearch",
            "",
            json!({"type": "object"}),
        )];
        assert!(hosted_tools_with_config(&native_config(), "gpt-5.4", false, &tools).is_empty());
    }

    #[test]
    fn web_search_calls_downgrade_to_text() {
        let mut input = vec![
            json!({"type": "web_search_call", "id": "ws_1", "status": "completed",
                "action": {"type": "search", "query": "jcode"}}),
            json!({"type": "message", "role": "user", "content": []}),
        ];
        downgrade_web_search_calls(&mut input);
        assert_eq!(input[0]["type"], "message");
        assert!(
            input[0]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("jcode")
        );
        assert_eq!(input[1]["role"], "user");
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
