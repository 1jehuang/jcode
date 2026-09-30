//! Provider-native (server-side) tool items.
//!
//! Some providers run tools on their own side, for example Anthropic's
//! `web_search` server tool or the OpenAI Responses `web_search` tool. Their
//! output comes back as opaque, provider-specific blocks (`server_tool_use`,
//! `web_search_tool_result`, `web_search_call`) that often carry encrypted
//! payloads which must be replayed **unmodified** on later turns.
//!
//! jcode stores these verbatim as [`crate::ContentBlock::ProviderNative`] and
//! only the provider that produced them replays them. This module turns the raw
//! items into a small, provider-agnostic description for transcript display.

use serde_json::Value;

/// Provider tag used for Anthropic Messages API server tool blocks.
pub const PROVIDER_NATIVE_ANTHROPIC: &str = "anthropic";
/// Provider tag used for OpenAI Responses API hosted tool items.
pub const PROVIDER_NATIVE_OPENAI: &str = "openai";
/// Display name for provider-native web search calls.
pub const PROVIDER_NATIVE_WEB_SEARCH_TOOL: &str = "web_search";

/// Anthropic content block types that belong to server tools and must be
/// stored and replayed verbatim.
pub const ANTHROPIC_SERVER_TOOL_BLOCK_TYPES: &[&str] =
    &["server_tool_use", "web_search_tool_result"];

/// Maximum number of search results listed in a transcript summary.
const MAX_DISPLAY_RESULTS: usize = 10;

/// Provider-agnostic view of a provider-native tool item.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderNativeDisplay {
    /// Tool call id shared by the call and its result.
    pub id: String,
    /// Display tool name, e.g. `web_search`.
    pub name: String,
    /// Tool input when this item starts a call.
    pub input: Option<Value>,
    /// Human-readable result when this item completes a call.
    pub output: Option<String>,
    /// True when the provider reported a failed call.
    pub is_error: bool,
}

/// True when `block_type` is an Anthropic server tool block jcode must keep.
pub fn is_anthropic_server_tool_block(block_type: &str) -> bool {
    ANTHROPIC_SERVER_TOOL_BLOCK_TYPES.contains(&block_type)
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// Describe a provider-native item for display. Returns `None` for items that
/// have nothing user-visible.
pub fn provider_native_display(provider: &str, item: &Value) -> Option<ProviderNativeDisplay> {
    match (provider, str_field(item, "type")?) {
        (PROVIDER_NATIVE_ANTHROPIC, "server_tool_use") => Some(ProviderNativeDisplay {
            id: str_field(item, "id")?.to_string(),
            name: str_field(item, "name")
                .unwrap_or(PROVIDER_NATIVE_WEB_SEARCH_TOOL)
                .to_string(),
            input: Some(item.get("input").cloned().unwrap_or(Value::Null)),
            output: None,
            is_error: false,
        }),
        (PROVIDER_NATIVE_ANTHROPIC, "web_search_tool_result") => {
            let id = str_field(item, "tool_use_id")?.to_string();
            let content = item.get("content").unwrap_or(&Value::Null);
            let (output, is_error) = match content {
                Value::Array(results) => (format_results(results.iter()), false),
                other => {
                    let code = str_field(other, "error_code").unwrap_or("unknown_error");
                    (format!("Web search failed: {code}"), true)
                }
            };
            Some(ProviderNativeDisplay {
                id,
                name: PROVIDER_NATIVE_WEB_SEARCH_TOOL.to_string(),
                input: None,
                output: Some(output),
                is_error,
            })
        }
        (PROVIDER_NATIVE_OPENAI, "web_search_call") => {
            let action = item.get("action").unwrap_or(&Value::Null);
            let query = str_field(action, "query")
                .map(str::to_string)
                .or_else(|| {
                    action
                        .get("queries")
                        .and_then(Value::as_array)
                        .map(|queries| {
                            queries
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                })
                .or_else(|| str_field(action, "url").map(str::to_string))
                .unwrap_or_default();
            let status = str_field(item, "status").unwrap_or("completed");
            let is_error = status == "failed";
            let mut output = if is_error {
                "Web search failed".to_string()
            } else {
                match str_field(action, "type") {
                    Some("open_page") => format!("Opened page: {query}"),
                    Some("find_in_page") | Some("find") => format!("Searched page: {query}"),
                    _ => format!("Searched: {query}"),
                }
            };
            if let Some(sources) = action.get("sources").and_then(Value::as_array)
                && !sources.is_empty()
            {
                output.push_str("\n\n");
                output.push_str(&format_results(sources.iter()));
            }
            Some(ProviderNativeDisplay {
                id: str_field(item, "id").unwrap_or_default().to_string(),
                name: PROVIDER_NATIVE_WEB_SEARCH_TOOL.to_string(),
                input: Some(serde_json::json!({ "query": query })),
                output: Some(output),
                is_error,
            })
        }
        _ => None,
    }
}

/// Plain-text stand-in for a provider-native item, used when the item cannot be
/// replayed natively (different provider, or the server tool is no longer
/// attached to the request). Keeps what the model learned without sending
/// provider-specific blocks another backend would reject. Returns `None` for
/// items that carry nothing worth keeping (e.g. a bare call start).
/// `call_input` is the input of the matching call start, for result items
/// that do not repeat the query themselves.
pub fn provider_native_text_fallback(
    provider: &str,
    item: &Value,
    call_input: Option<&Value>,
) -> Option<String> {
    let display = provider_native_display(provider, item)?;
    let output = display.output?;
    let query = display
        .input
        .as_ref()
        .or(call_input)
        .and_then(|input| str_field(input, "query"))
        .filter(|query| !query.is_empty());
    // Only titles/URLs survive; say so, or the model may mistake the list for
    // the full results it originally read and "correct" its earlier answer.
    const NOTE: &str =
        "(summary of an earlier provider-side search; page contents are no longer available)";
    Some(match query {
        Some(query) => format!("[{} for \"{query}\"] {NOTE}\n{output}", display.name),
        None => format!("[{} results] {NOTE}\n{output}", display.name),
    })
}

/// A provider-native tool call as seen by display consumers.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderNativeToolEvent {
    /// The provider started running a tool.
    Started {
        id: String,
        name: String,
        input: Value,
    },
    /// The provider finished running a tool. `input` is filled from the
    /// matching start when the result item does not carry it.
    Completed {
        id: String,
        name: String,
        input: Value,
        output: String,
        is_error: bool,
    },
}

/// Pairs provider-native call starts with their results so consumers can
/// render one tool row per call, the same way they render local tools.
#[derive(Debug, Default)]
pub struct ProviderNativeTracker {
    started: std::collections::HashMap<String, (String, Value)>,
}

impl ProviderNativeTracker {
    /// Observe one item and describe what happened, if anything.
    pub fn observe(&mut self, provider: &str, item: &Value) -> Option<ProviderNativeToolEvent> {
        let display = provider_native_display(provider, item)?;
        match display.output {
            None => {
                let input = display.input.unwrap_or(Value::Null);
                self.started
                    .insert(display.id.clone(), (display.name.clone(), input.clone()));
                Some(ProviderNativeToolEvent::Started {
                    id: display.id,
                    name: display.name,
                    input,
                })
            }
            Some(output) => {
                let (name, started_input) = self
                    .started
                    .remove(&display.id)
                    .unwrap_or_else(|| (display.name.clone(), Value::Null));
                Some(ProviderNativeToolEvent::Completed {
                    id: display.id,
                    name,
                    input: display.input.unwrap_or(started_input),
                    output,
                    is_error: display.is_error,
                })
            }
        }
    }

    /// True when `id` was started but has not completed yet.
    pub fn is_started(&self, id: &str) -> bool {
        self.started.contains_key(id)
    }

    /// Forget all in-flight calls (for stream retry rollbacks).
    pub fn clear(&mut self) {
        self.started.clear();
    }
}

/// Collects provider-native items streamed during one assistant response,
/// remembering where each landed in the streamed text. Interleaving them back in
/// arrival order matters: Anthropic requires a paused (`pause_turn`) turn to be
/// resent exactly as produced, and citations reference text after the results.
#[derive(Debug, Default, Clone)]
pub struct ProviderNativeItems {
    /// (byte offset into the response text at arrival, provider, item)
    items: Vec<(usize, String, Value)>,
}

impl ProviderNativeItems {
    /// Record an item that arrived after `text_len` bytes of response text.
    pub fn push(&mut self, text_len: usize, provider: String, item: Value) {
        self.items.push((text_len, provider, item));
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Input of the recorded call start with this id, for rendering its result.
    pub fn call_input(&self, id: &str) -> Value {
        self.items
            .iter()
            .filter_map(|(_, provider, item)| provider_native_display(provider, item))
            .find(|display| display.id == id && display.output.is_none())
            .and_then(|display| display.input)
            .unwrap_or(Value::Null)
    }

    /// Build the content blocks for `text` with the recorded items spliced in
    /// at their arrival offsets. Offsets past the end (or not on a char
    /// boundary) clamp to the nearest valid position. Empty text segments are
    /// skipped. With no items this is just the single text block (if any).
    pub fn interleave(&self, text: &str) -> Vec<crate::ContentBlock> {
        let mut blocks = Vec::new();
        let mut cursor = 0usize;
        let push_text = |blocks: &mut Vec<crate::ContentBlock>, segment: &str| {
            if !segment.trim().is_empty() {
                blocks.push(crate::ContentBlock::Text {
                    text: segment.to_string(),
                    cache_control: None,
                });
            }
        };
        for (offset, provider, item) in &self.items {
            let mut at = (*offset).clamp(cursor, text.len());
            while !text.is_char_boundary(at) {
                at += 1;
            }
            push_text(&mut blocks, &text[cursor..at]);
            cursor = at;
            blocks.push(crate::ContentBlock::ProviderNative {
                provider: provider.clone(),
                item: item.clone(),
            });
        }
        push_text(&mut blocks, &text[cursor..]);
        blocks
    }
}

fn format_results<'a>(results: impl Iterator<Item = &'a Value>) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    let mut total = 0usize;
    for result in results {
        total += 1;
        if count >= MAX_DISPLAY_RESULTS {
            continue;
        }
        let Some(url) = str_field(result, "url") else {
            continue;
        };
        count += 1;
        let title = str_field(result, "title").filter(|title| !title.trim().is_empty());
        match title {
            Some(title) => out.push_str(&format!("{count}. {title}\n   {url}\n")),
            None => out.push_str(&format!("{count}. {url}\n")),
        }
    }
    if count == 0 {
        return "No results".to_string();
    }
    if total > count {
        out.push_str(&format!("... and {} more\n", total - count));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn anthropic_server_tool_use_is_a_call_start() {
        let item = json!({
            "type": "server_tool_use",
            "id": "srvtoolu_1",
            "name": "web_search",
            "input": {"query": "rust 2024 edition"}
        });
        let display = provider_native_display("anthropic", &item).unwrap();
        assert_eq!(display.id, "srvtoolu_1");
        assert_eq!(display.name, "web_search");
        assert_eq!(display.input, Some(json!({"query": "rust 2024 edition"})));
        assert!(display.output.is_none());
    }

    #[test]
    fn anthropic_result_lists_titles_and_urls() {
        let item = json!({
            "type": "web_search_tool_result",
            "tool_use_id": "srvtoolu_1",
            "content": [
                {"type": "web_search_result", "url": "https://a.example", "title": "A", "encrypted_content": "x"},
                {"type": "web_search_result", "url": "https://b.example", "title": "", "encrypted_content": "y"}
            ]
        });
        let display = provider_native_display("anthropic", &item).unwrap();
        assert_eq!(display.id, "srvtoolu_1");
        assert!(!display.is_error);
        assert_eq!(
            display.output.as_deref(),
            Some("1. A\n   https://a.example\n2. https://b.example")
        );
    }

    #[test]
    fn anthropic_result_error_is_flagged() {
        let item = json!({
            "type": "web_search_tool_result",
            "tool_use_id": "srvtoolu_1",
            "content": {"type": "web_search_tool_result_error", "error_code": "max_uses_exceeded"}
        });
        let display = provider_native_display("anthropic", &item).unwrap();
        assert!(display.is_error);
        assert_eq!(
            display.output.as_deref(),
            Some("Web search failed: max_uses_exceeded")
        );
    }

    #[test]
    fn openai_web_search_call_is_call_and_result() {
        let item = json!({
            "type": "web_search_call",
            "id": "ws_1",
            "status": "completed",
            "action": {"type": "search", "query": "jcode"}
        });
        let display = provider_native_display("openai", &item).unwrap();
        assert_eq!(display.id, "ws_1");
        assert_eq!(display.input, Some(json!({"query": "jcode"})));
        assert_eq!(display.output.as_deref(), Some("Searched: jcode"));
    }

    #[test]
    fn tracker_pairs_start_with_result() {
        let mut tracker = ProviderNativeTracker::default();
        let start = json!({
            "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search",
            "input": {"query": "q"}
        });
        let result = json!({
            "type": "web_search_tool_result", "tool_use_id": "srvtoolu_1",
            "content": [{"type": "web_search_result", "url": "https://a.example", "title": "A"}]
        });
        assert!(matches!(
            tracker.observe("anthropic", &start),
            Some(ProviderNativeToolEvent::Started { .. })
        ));
        assert!(tracker.is_started("srvtoolu_1"));
        match tracker.observe("anthropic", &result) {
            Some(ProviderNativeToolEvent::Completed { id, input, .. }) => {
                assert_eq!(id, "srvtoolu_1");
                assert_eq!(input, json!({"query": "q"}));
            }
            other => panic!("expected completion, got {other:?}"),
        }
        assert!(!tracker.is_started("srvtoolu_1"));
    }

    #[test]
    fn text_fallback_keeps_query_and_results() {
        let item = json!({
            "type": "web_search_call", "id": "ws_1", "status": "completed",
            "action": {"type": "search", "query": "jcode"}
        });
        assert_eq!(
            provider_native_text_fallback("openai", &item, None).as_deref(),
            Some(
                "[web_search for \"jcode\"] (summary of an earlier provider-side search; page contents are no longer available)\nSearched: jcode"
            )
        );
        let start =
            json!({"type": "server_tool_use", "id": "s", "name": "web_search", "input": {}});
        assert!(provider_native_text_fallback("anthropic", &start, None).is_none());
        let result = json!({"type": "web_search_tool_result", "tool_use_id": "s", "content": []});
        assert_eq!(
            provider_native_text_fallback("anthropic", &result, Some(&json!({"query": "q"})))
                .as_deref(),
            Some(
                "[web_search for \"q\"] (summary of an earlier provider-side search; page contents are no longer available)\nNo results"
            )
        );
    }

    #[test]
    fn interleave_places_items_at_arrival_offsets() {
        let mut items = ProviderNativeItems::default();
        let start =
            json!({"type": "server_tool_use", "id": "s", "name": "web_search", "input": {}});
        let result = json!({"type": "web_search_tool_result", "tool_use_id": "s", "content": []});
        items.push(9, "anthropic".into(), start.clone());
        items.push(9, "anthropic".into(), result.clone());
        let blocks = items.interleave("Searching. Found it.");
        let kinds: Vec<String> = blocks
            .iter()
            .map(|block| match block {
                crate::ContentBlock::Text { text, .. } => format!("text:{text}"),
                crate::ContentBlock::ProviderNative { item, .. } => {
                    item["type"].as_str().unwrap().to_string()
                }
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "text:Searching",
                "server_tool_use",
                "web_search_tool_result",
                "text:. Found it."
            ]
        );
    }

    #[test]
    fn interleave_without_items_is_plain_text() {
        let blocks = ProviderNativeItems::default().interleave("hello");
        assert_eq!(blocks.len(), 1);
        assert!(ProviderNativeItems::default().interleave("  ").is_empty());
        let mut items = ProviderNativeItems::default();
        items.push(99, "openai".into(), json!({"type": "web_search_call"}));
        assert_eq!(items.interleave("é").len(), 2);
    }

    #[test]
    fn unknown_items_have_no_display() {
        assert!(provider_native_display("anthropic", &json!({"type": "text"})).is_none());
        assert!(provider_native_display("gemini", &json!({"type": "server_tool_use"})).is_none());
    }
}
