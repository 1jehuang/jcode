//! Anthropic server-side `web_search` tool (`websearch.engine = "native"`).
//!
//! When enabled, the request carries Anthropic's server tool instead of jcode's
//! local `websearch` tool, so searches run on Anthropic's side. The resulting
//! `server_tool_use` / `web_search_tool_result` blocks are parsed in the SSE
//! handler and stored verbatim for replay (see
//! `jcode_message_types::provider_native`).
//!
//! Docs: <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/web-search-tool>

use jcode_base::config::WebSearchConfig;
use jcode_message_types::ToolDefinition;
use serde_json::{Value, json};

/// Name of jcode's local scraping search tool, replaced by the server tool.
const LOCAL_WEBSEARCH_TOOL: &str = "websearch";
/// First tool version; later versions default `allowed_callers` to code
/// execution and must be pinned to direct calls.
const BASIC_TOOL_VERSION: &str = "web_search_20250305";

/// Build the server tool definition from config.
pub(crate) fn server_tool(config: &WebSearchConfig) -> Value {
    let version = config.native_anthropic_tool_version.trim();
    let version = if version.is_empty() {
        BASIC_TOOL_VERSION
    } else {
        version
    };
    let mut tool = json!({ "type": version, "name": "web_search" });
    if let Some(max_uses) = config.native_max_uses.filter(|max| *max > 0) {
        tool["max_uses"] = json!(max_uses);
    }
    // The API rejects a request that sets both lists, so allowed wins.
    let allowed = clean_domains(&config.native_allowed_domains);
    let blocked = clean_domains(&config.native_blocked_domains);
    if !allowed.is_empty() {
        if !blocked.is_empty() {
            jcode_base::logging::warn(
                "websearch: native_allowed_domains and native_blocked_domains are both set; \
                 Anthropic accepts only one, using native_allowed_domains",
            );
        }
        tool["allowed_domains"] = json!(allowed);
    } else if !blocked.is_empty() {
        tool["blocked_domains"] = json!(blocked);
    }
    // Newer versions call through code execution by default, which returns 400
    // on models without programmatic tool calling. Direct calls work everywhere.
    if version != BASIC_TOOL_VERSION {
        tool["allowed_callers"] = json!(["direct"]);
    }
    tool
}

fn clean_domains(domains: &[String]) -> Vec<String> {
    domains
        .iter()
        .map(|domain| domain.trim())
        .filter(|domain| !domain.is_empty())
        .map(str::to_string)
        .collect()
}

/// Server tools to attach to this request. Empty unless native search is
/// preferred, the request goes to Anthropic's first-party API (custom gateways
/// may not implement server tools), and the session's tool policy offers
/// `websearch` at all (the server tool only replaces it, never adds search to a
/// session that excluded it).
pub(crate) fn server_tools_for_request(first_party: bool, tools: &[ToolDefinition]) -> Vec<Value> {
    let config = jcode_base::config::config();
    if !session_offers_websearch(tools) {
        return Vec::new();
    }
    server_tools_with_config(&config.websearch, first_party)
}

/// True when the session's filtered tool list includes the local search tool.
pub(crate) fn session_offers_websearch(tools: &[ToolDefinition]) -> bool {
    tools.iter().any(|tool| tool.name == LOCAL_WEBSEARCH_TOOL)
}

pub(crate) fn server_tools_with_config(config: &WebSearchConfig, first_party: bool) -> Vec<Value> {
    if config.native_enabled() && first_party {
        vec![server_tool(config)]
    } else {
        Vec::new()
    }
}

/// Drop jcode's local search tool when the server tool replaces it, so the
/// model has exactly one way to search. Done before formatting so the prompt
/// cache breakpoint still lands on the last remaining client tool.
pub(crate) fn without_local_websearch<'a>(
    tools: &'a [ToolDefinition],
    server_tools: &[Value],
) -> std::borrow::Cow<'a, [ToolDefinition]> {
    if server_tools.is_empty() {
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

/// Outcome of a `content_block_start` whose type the typed parser did not
/// recognize.
pub(crate) enum ServerBlockStart {
    /// A server tool call whose input still streams in via `input_json_delta`.
    Streaming(ServerBlockAccumulator),
    /// A server tool block delivered whole (e.g. `web_search_tool_result`).
    Complete(Value),
    /// Not a server tool block.
    Other,
}

/// Accumulates a streamed `server_tool_use` block.
pub(crate) struct ServerBlockAccumulator {
    block: Value,
    input_json: String,
}

impl ServerBlockAccumulator {
    pub(crate) fn push_input(&mut self, partial_json: &str) {
        self.input_json.push_str(partial_json);
    }

    /// The finished block, with its streamed input filled in. The rest of the
    /// block is left exactly as the API sent it.
    pub(crate) fn finish(mut self) -> Value {
        if !self.input_json.trim().is_empty() {
            match serde_json::from_str::<Value>(&self.input_json) {
                Ok(input) => self.block["input"] = input,
                Err(err) => jcode_base::logging::warn(&format!(
                    "Anthropic server tool input was not valid JSON ({err}); keeping start input"
                )),
            }
        }
        self.block
    }
}

/// Classify the raw `content_block_start` payload.
pub(crate) fn server_block_start(data: &str) -> ServerBlockStart {
    let Some(block) = serde_json::from_str::<Value>(data)
        .ok()
        .and_then(|mut event| event.get_mut("content_block").map(Value::take))
    else {
        return ServerBlockStart::Other;
    };
    match block.get("type").and_then(Value::as_str) {
        Some("server_tool_use") => ServerBlockStart::Streaming(ServerBlockAccumulator {
            block,
            input_json: String::new(),
        }),
        Some(kind)
            if jcode_message_types::provider_native::is_anthropic_server_tool_block(kind) =>
        {
            ServerBlockStart::Complete(block)
        }
        _ => ServerBlockStart::Other,
    }
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
    fn default_tool_is_basic_version_with_max_uses() {
        let tool = server_tool(&native_config());
        assert_eq!(
            tool,
            json!({"type": "web_search_20250305", "name": "web_search", "max_uses": 5})
        );
    }

    #[test]
    fn newer_versions_pin_direct_callers() {
        let mut config = native_config();
        config.native_anthropic_tool_version = "web_search_20260209".to_string();
        config.native_max_uses = None;
        let tool = server_tool(&config);
        assert_eq!(tool["type"], "web_search_20260209");
        assert_eq!(tool["allowed_callers"], json!(["direct"]));
        assert!(tool.get("max_uses").is_none());
    }

    #[test]
    fn allowed_domains_win_over_blocked() {
        let mut config = native_config();
        config.native_allowed_domains = vec![" docs.rs ".to_string(), String::new()];
        config.native_blocked_domains = vec!["example.com".to_string()];
        let tool = server_tool(&config);
        assert_eq!(tool["allowed_domains"], json!(["docs.rs"]));
        assert!(tool.get("blocked_domains").is_none());

        config.native_allowed_domains.clear();
        let tool = server_tool(&config);
        assert_eq!(tool["blocked_domains"], json!(["example.com"]));
    }

    #[test]
    fn server_tool_only_for_native_engine_on_first_party_api() {
        let off = WebSearchConfig {
            prefer_native: false,
            ..WebSearchConfig::default()
        };
        assert!(server_tools_with_config(&off, true).is_empty());
        // Native is the default wherever the provider supports it.
        assert_eq!(
            server_tools_with_config(&WebSearchConfig::default(), true).len(),
            1
        );
        assert!(server_tools_with_config(&native_config(), false).is_empty());
        assert_eq!(server_tools_with_config(&native_config(), true).len(), 1);
    }

    #[test]
    fn server_tool_requires_session_to_offer_websearch() {
        let tool = |name: &str| ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            input_schema: json!({"type": "object", "properties": {}}),
            defer_loading: false,
        };
        assert!(!session_offers_websearch(&[tool("bash")]));
        assert!(server_tools_for_request(true, &[tool("bash")]).is_empty());
        assert!(session_offers_websearch(&[tool("bash"), tool("websearch")]));
    }

    #[test]
    fn local_websearch_is_dropped_only_when_server_tool_attached() {
        let tool = |name: &str| ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            input_schema: json!({"type": "object", "properties": {}}),
            defer_loading: false,
        };
        let tools = vec![tool("bash"), tool("websearch")];
        assert_eq!(without_local_websearch(&tools, &[]).len(), 2);
        let kept = without_local_websearch(&tools, &[json!({"type": "web_search_20250305"})]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "bash");
    }
}
