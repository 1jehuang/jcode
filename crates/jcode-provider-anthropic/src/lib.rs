use jcode_message_types::{
    ContentBlock, Message, Role, TOOL_OUTPUT_MISSING_TEXT, ToolDefinition, sanitize_tool_id,
};
use jcode_provider_core::anthropic_map_tool_name_for_oauth as map_tool_name_for_oauth;
use serde::Serialize;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

mod history_repair;
use history_repair::*;

/// Claude Code billing attribution text observed in the official CLI's system
/// prompt blocks.
pub const OAUTH_BILLING_HEADER: &str = "cc_version=2.1.280; cc_entrypoint=sdk-cli; cch=33f85;";

const CLAUDE_CODE_IDENTITY: &str = "You are a Claude agent, built on Anthropic's Claude Agent SDK.";

/// Minimal user turn appended when a formatted conversation would otherwise end
/// on an assistant message, which Anthropic rejects on non-prefill models.
pub(crate) const CONTINUATION_USER_TURN: &str = "Continue.";

pub fn format_messages(messages: &[Message], is_oauth: bool) -> Vec<ApiMessage> {
    format_messages_with_tools(messages, is_oauth, &[])
}

/// Like [`format_messages`], rendering `ContentBlock::ToolReference` blocks as
/// native `tool_reference` blocks for tools present in `api_tools`.
///
/// A reference to a tool missing from the request's `tools` array is a hard
/// 400, so references to tools that are no longer available (server
/// disconnected, session resumed elsewhere) are dropped and the tool result
/// keeps its plain text.
pub fn format_messages_with_tools(
    messages: &[Message],
    is_oauth: bool,
    api_tools: &[ApiTool],
) -> Vec<ApiMessage> {
    format_messages_with_native(messages, is_oauth, api_tools, false)
}

/// Like [`format_messages_with_tools`], additionally controlling how stored
/// provider-native blocks ([`ContentBlock::ProviderNative`]) are sent.
///
/// With `native_replay`, Anthropic server tool blocks (`server_tool_use`,
/// `web_search_tool_result`) are replayed verbatim, which the API requires for
/// their encrypted payloads. Without it (the server tool is not attached to this
/// request, or the block came from another provider) they are downgraded to a
/// plain-text summary so the conversation stays valid.
pub fn format_messages_with_native(
    messages: &[Message],
    is_oauth: bool,
    api_tools: &[ApiTool],
    native_replay: bool,
) -> Vec<ApiMessage> {
    use std::collections::HashSet;
    let available: HashSet<&str> = api_tools.iter().map(|tool| tool.name.as_str()).collect();
    // Pre-pass: drop duplicate tool_results for the same tool_use_id.
    //
    // Anthropic rejects the whole request (400 "unexpected `tool_use_id` found
    // in `tool_result` blocks") when a tool_use_id appears twice, because after
    // same-role merging only the first result lines up with the tool_use in the
    // preceding assistant message. Duplicates are produced by the missing
    // tool-output repair racing a still-running tool: the repair inserts a
    // synthetic placeholder result, then the real result lands moments later,
    // and the conversation is permanently unsendable. Prefer the real output
    // over the synthetic placeholder, and otherwise keep the first occurrence.
    // Repair persisted provider-native Anthropic server-tool history before replay.
    // Ordinary ToolUse blocks already have a synthetic-result repair below, but
    // server_tool_use / web_search_tool_result pairs are stored as ProviderNative
    // blocks and therefore need their own recovery path.
    let repaired_messages = repair_dangling_anthropic_server_tools(messages);
    // A duplicated run of history (for example a journal replayed on top of a
    // snapshot that already contained it, #1632) repeats `tool_use` ids. Only
    // the first copy can be answered, so drop the repeats before results are
    // deduplicated; otherwise the second copy is sent with no result after it.
    let repaired_messages = dedupe_tool_uses(&repaired_messages);
    let messages = &dedupe_tool_results(&repaired_messages);
    // Anthropic requires every tool_use to be answered in the very next
    // message. A tool whose result was persisted later (after another turn was
    // written in between, e.g. a reload or a scheduled-task wake-up while tools
    // were still running) leaves the call unanswered where it matters and its
    // result stranded later on, and the request 400s permanently. Move such
    // results up to directly follow their call.
    let messages = &hoist_late_tool_results(messages);

    // First pass: collect all tool_use IDs and tool_result IDs
    let mut tool_use_ids: HashSet<String> = HashSet::new();
    let mut tool_result_ids: HashSet<String> = HashSet::new();

    for msg in messages {
        for block in &msg.content {
            match block {
                ContentBlock::ToolUse { id, .. } => {
                    tool_use_ids.insert(id.clone());
                }
                ContentBlock::ToolResult { tool_use_id, .. } => {
                    tool_result_ids.insert(tool_use_id.clone());
                }
                _ => {}
            }
        }
    }

    // Find dangling tool_uses (no matching tool_result)
    let dangling: HashSet<_> = tool_use_ids.difference(&tool_result_ids).cloned().collect();
    if !dangling.is_empty() {
        jcode_logging::info(&format!(
            "[anthropic] Repairing {} dangling tool_use(s) by injecting synthetic tool_results",
            dangling.len()
        ));
    }

    // Second pass: build messages, injecting synthetic tool_results after assistant messages
    // that have dangling tool_uses
    let mut result: Vec<ApiMessage> = Vec::new();

    for msg in messages {
        let role = match msg.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };

        let mut content = format_content_blocks_with_native(&msg.content, is_oauth, native_replay);
        apply_tool_references(&mut content, &msg.content, is_oauth, &available);

        if !content.is_empty() {
            result.push(ApiMessage {
                role: role.to_string(),
                content,
            });
        }

        // If this is an assistant message with dangling tool_uses, inject synthetic results
        if matches!(msg.role, Role::Assistant) {
            let mut synthetic_results: Vec<ApiContentBlock> = Vec::new();
            for block in &msg.content {
                if let ContentBlock::ToolUse { id, .. } = block
                    && dangling.contains(id)
                {
                    synthetic_results.push(ApiContentBlock::ToolResult {
                        tool_use_id: sanitize_tool_id(id),
                        content: ToolResultContent::Text(
                            "[Session interrupted before tool execution completed]".to_string(),
                        ),
                        is_error: true,
                    });
                }
            }
            if !synthetic_results.is_empty() {
                result.push(ApiMessage {
                    role: "user".to_string(),
                    content: synthetic_results,
                });
            }
        }
    }

    // Third pass: merge consecutive messages of the same role
    // Anthropic API requires strictly alternating user/assistant messages
    let pre_merge_count = result.len();
    let mut merged: Vec<ApiMessage> = Vec::new();
    for msg in result {
        if let Some(last) = merged.last_mut()
            && last.role == msg.role
        {
            last.content.extend(msg.content);
            continue;
        }
        merged.push(msg);
    }

    if merged.len() != pre_merge_count {
        jcode_logging::info(&format!(
            "[anthropic] Merged {} consecutive same-role messages",
            pre_merge_count - merged.len()
        ));
    }

    // Anthropic requires every tool_result answering the previous assistant
    // turn to lead the user message. Merging separate tool-result messages can
    // interleave sibling text (for example text moved out of a tool_result by
    // `apply_tool_references` for parallel tool_search calls), which makes the
    // API report later tool_use ids as missing their tool_result. Stable
    // partition so tool_results come first and other blocks keep their order.
    for msg in merged.iter_mut().filter(|m| m.role == "user") {
        let first_non_result = msg
            .content
            .iter()
            .position(|b| !matches!(b, ApiContentBlock::ToolResult { .. }));
        let needs_reorder = first_non_result.is_some_and(|start| {
            msg.content[start..]
                .iter()
                .any(|b| matches!(b, ApiContentBlock::ToolResult { .. }))
        });
        if needs_reorder {
            let (results, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut msg.content)
                .into_iter()
                .partition(|b| matches!(b, ApiContentBlock::ToolResult { .. }));
            msg.content = results;
            msg.content.extend(rest);
        }
    }

    rewrite_orphaned_tool_results(&mut merged);
    answer_unpaired_tool_uses(&mut merged);

    // Anthropic rejects a request whose final message is an assistant turn on
    // models that do not support assistant prefill ("This model does not support
    // assistant message prefill. The conversation must end with a user message.").
    // jcode never intends to prefill, so a trailing assistant turn here is always
    // an upstream accident: the reload auto-resume path starts a turn with empty
    // user content and delivers its continuation as a system reminder, leaving the
    // transcript ending on the interrupted assistant turn. Repair the shape at the
    // last formatting step. See issue #600.
    //
    // Exception: a turn that stopped with `pause_turn` mid server-tool use must
    // be resent exactly as-is so the API can resume it. Those turns end on an
    // assistant message carrying raw server tool blocks.
    if merged
        .last()
        .is_some_and(|last| last.role == "assistant" && !is_paused_server_tool_turn(last))
    {
        jcode_logging::warn(
            "[anthropic] Conversation ended with an assistant message; appending a \
             continuation user turn to avoid a model prefill rejection (400)",
        );
        merged.push(ApiMessage {
            role: "user".to_string(),
            content: vec![ApiContentBlock::Text {
                text: CONTINUATION_USER_TURN.to_string(),
                cache_control: None,
            }],
        });
    }

    // Validate: check each assistant message with tool_use has matching tool_result in next user message
    for (i, msg) in merged.iter().enumerate() {
        if msg.role == "assistant" {
            let tool_uses: Vec<&String> = msg
                .content
                .iter()
                .filter_map(|b| {
                    if let ApiContentBlock::ToolUse { id, .. } = b {
                        Some(id)
                    } else {
                        None
                    }
                })
                .collect();

            if !tool_uses.is_empty() {
                // Check next message
                if let Some(next) = merged.get(i + 1) {
                    if next.role != "user" {
                        jcode_logging::warn(&format!(
                            "[anthropic] Message {} has tool_use but next message is {} (should be user)",
                            i, next.role
                        ));
                    } else {
                        let tool_results: std::collections::HashSet<&String> = next
                            .content
                            .iter()
                            .filter_map(|b| {
                                if let ApiContentBlock::ToolResult { tool_use_id, .. } = b {
                                    Some(tool_use_id)
                                } else {
                                    None
                                }
                            })
                            .collect();

                        for tu_id in &tool_uses {
                            if !tool_results.contains(*tu_id) {
                                jcode_logging::warn(&format!(
                                    "[anthropic] Message {} has tool_use {} but no matching tool_result in message {}",
                                    i,
                                    tu_id,
                                    i + 1
                                ));
                            }
                        }
                    }
                } else {
                    jcode_logging::warn(&format!(
                        "[anthropic] Message {} has tool_use but no next message",
                        i
                    ));
                }
            }
        }
    }

    merged
}

/// Convert our ContentBlock to Anthropic API format
pub fn format_content_blocks(blocks: &[ContentBlock], is_oauth: bool) -> Vec<ApiContentBlock> {
    format_content_blocks_with_native(blocks, is_oauth, false)
}

/// True when an assistant message ends on a raw server tool block, i.e. the
/// shape of a provider-native turn paused with `pause_turn`. Anthropic resumes
/// such a turn when it is resent as-is. A turn that searched and then went on to
/// produce text (e.g. interrupted mid-answer) ends on that text instead, and
/// still needs the continuation user turn to avoid a prefill rejection.
fn is_paused_server_tool_turn(message: &ApiMessage) -> bool {
    matches!(message.content.last(), Some(ApiContentBlock::Raw(_)))
}

/// See [`format_messages_with_native`] for the meaning of `native_replay`.
pub fn format_content_blocks_with_native(
    blocks: &[ContentBlock],
    is_oauth: bool,
    native_replay: bool,
) -> Vec<ApiContentBlock> {
    let mut result: Vec<ApiContentBlock> = Vec::new();
    // Inputs of server tool calls seen so far, so a downgraded result can name
    // the query that produced it.
    let mut native_call_inputs: std::collections::HashMap<String, Value> =
        std::collections::HashMap::new();
    for block in blocks {
        match block {
            ContentBlock::ProviderNative { provider, item } => {
                use jcode_message_types::provider_native;
                let display = provider_native::provider_native_display(provider, item);
                if let Some(display) = &display
                    && display.output.is_none()
                    && let Some(input) = &display.input
                {
                    native_call_inputs.insert(display.id.clone(), input.clone());
                }
                let is_anthropic_block = provider == provider_native::PROVIDER_NATIVE_ANTHROPIC
                    && item
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(provider_native::is_anthropic_server_tool_block);
                if native_replay && is_anthropic_block {
                    result.push(ApiContentBlock::Raw(item.clone()));
                } else if let Some(text) = provider_native::provider_native_text_fallback(
                    provider,
                    item,
                    display
                        .as_ref()
                        .and_then(|display| native_call_inputs.get(&display.id)),
                ) {
                    result.push(ApiContentBlock::Text {
                        text,
                        cache_control: None,
                    });
                }
            }
            ContentBlock::Text { text, .. } => {
                // A text block that immediately follows an image-bearing tool_result is the
                // "[Attached image associated with the preceding tool result: ...]" label
                // emitted alongside image tool outputs. The Anthropic API requires every
                // tool_result for a parallel tool-call turn to be contiguous in the next user
                // message; a sibling text block wedged between tool_results makes the API
                // report later tool_use ids as missing their tool_result. Fold the label into
                // the tool_result's content blocks so the tool_results stay contiguous.
                if let Some(ApiContentBlock::ToolResult {
                    content: ToolResultContent::Blocks(blocks),
                    ..
                }) = result.last_mut()
                    && blocks
                        .iter()
                        .any(|b| matches!(b, ToolResultContentBlock::Image { .. }))
                {
                    blocks.push(ToolResultContentBlock::Text { text: text.clone() });
                } else {
                    result.push(ApiContentBlock::Text {
                        text: text.clone(),
                        cache_control: None,
                    });
                }
            }
            ContentBlock::AnthropicThinking {
                thinking,
                signature,
            } => {
                result.push(ApiContentBlock::Thinking {
                    thinking: thinking.clone(),
                    signature: signature.clone(),
                });
            }
            ContentBlock::ToolUse {
                id, name, input, ..
            } => {
                result.push(ApiContentBlock::ToolUse {
                    id: sanitize_tool_id(id),
                    name: if is_oauth {
                        map_tool_name_for_oauth(name)
                    } else {
                        name.clone()
                    },
                    input: if input.is_object() {
                        input.clone()
                    } else {
                        serde_json::json!({})
                    },
                    cache_control: None,
                });
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                result.push(ApiContentBlock::ToolResult {
                    tool_use_id: sanitize_tool_id(tool_use_id),
                    content: ToolResultContent::Text(content.clone()),
                    is_error: is_error.unwrap_or(false),
                });
            }
            ContentBlock::Image { media_type, data } => {
                let img_block = ToolResultContentBlock::Image {
                    source: ApiImageSource {
                        kind: "base64".to_string(),
                        media_type: media_type.clone(),
                        data: data.clone(),
                    },
                };
                if let Some(ApiContentBlock::ToolResult { content, .. }) = result.last_mut() {
                    match content {
                        ToolResultContent::Text(text) => {
                            let text_block = ToolResultContentBlock::Text {
                                text: std::mem::take(text),
                            };
                            *content = ToolResultContent::Blocks(vec![text_block, img_block]);
                        }
                        ToolResultContent::Blocks(blocks) => {
                            blocks.push(img_block);
                        }
                    }
                } else {
                    result.push(ApiContentBlock::Image {
                        source: ApiImageSource {
                            kind: "base64".to_string(),
                            media_type: media_type.clone(),
                            data: data.clone(),
                        },
                    });
                }
            }
            _ => {}
        }
    }
    result
}

/// Normalize a tool schema for Anthropic's `input_schema`.
///
/// Anthropic accepts JSON Schema combinators inside object properties but
/// rejects `oneOf`/`anyOf`/`allOf` at the top level, and requires an object
/// schema with a `properties` map. The subset and the rewrites live in
/// `jcode-schema-dialect` so every provider shares one implementation and one
/// set of regression tests.
///
/// Widening a top-level combiner loses the per-branch constraint, which is
/// intended: runtime tool deserialization remains the authority on which
/// combination is actually valid.
fn anthropic_input_schema(schema: &Value) -> Value {
    jcode_schema_dialect::normalize(schema, &jcode_schema_dialect::registry::ANTHROPIC)
}

/// Convert tool definitions to Anthropic API format.
///
/// Every provider and auth route advertises the same registry tools with the
/// same schemas. OAuth (subscription) only renames a few tools to their
/// Claude-Code builtin names (`bash` -> `Bash`, ...); the definition itself is
/// never replaced. Hand-curated OAuth schemas drifted from the real tools and
/// silently dropped options like `intent` (#706, #1223).
pub fn format_tools(tools: &[ToolDefinition], is_oauth: bool, cache_ttl_1h: bool) -> Vec<ApiTool> {
    let out = tools
        .iter()
        .map(|tool| ApiTool {
            name: if is_oauth {
                map_tool_name_for_oauth(&tool.name)
            } else {
                tool.name.clone()
            },
            description: tool.description.clone(),
            input_schema: anthropic_input_schema(&tool.input_schema),
            cache_control: None,
            defer_loading: tool.defer_loading,
        })
        .collect();
    finish_tool_list(out, cache_ttl_1h)
}

/// Order eager tools before deferred ones and put the prompt-cache breakpoint
/// on the last eager tool.
///
/// Deferred tools stay out of the cached system-prompt prefix, so they must
/// neither carry `cache_control` (the API rejects that with a 400) nor sit
/// between cached tools, where adding or removing one would shift the prefix.
/// The API also requires at least one non-deferred tool, so a list of only
/// deferred tools is sent eagerly instead.
fn finish_tool_list(tools: Vec<ApiTool>, cache_ttl_1h: bool) -> Vec<ApiTool> {
    let (mut out, mut deferred): (Vec<ApiTool>, Vec<ApiTool>) =
        tools.into_iter().partition(|tool| !tool.defer_loading);
    if out.is_empty() {
        for tool in &mut deferred {
            tool.defer_loading = false;
        }
        std::mem::swap(&mut out, &mut deferred);
    }
    if let Some(last) = out.last_mut() {
        last.cache_control = Some(CacheControlParam::ephemeral(cache_ttl_1h));
    }
    out.extend(deferred);
    out
}

#[derive(Serialize, Clone)]
pub struct ApiRequest {
    pub model: String,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<ApiSystem>,
    pub messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ApiToolParam>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ApiMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ApiThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<ApiOutputConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    pub stream: bool,
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApiThinking {
    Adaptive {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<&'static str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        block_binding: Option<ApiThinkingBlockBinding>,
    },
    Enabled {
        budget_tokens: u32,
    },
}

/// Permit the API to discard stale signed reasoning after compaction or a
/// changed system prompt/tool schema instead of rejecting the whole request.
#[derive(Serialize, Clone)]
pub struct ApiThinkingBlockBinding {
    pub prefix_mismatch_behavior: &'static str,
}

#[derive(Serialize, Clone)]
pub struct ApiOutputConfig {
    pub effort: String,
}

#[derive(Serialize, Clone)]
pub struct ApiMetadata {
    pub user_id: String,
}

#[derive(Serialize, Clone)]
#[serde(untagged)]
pub enum ApiSystem {
    Blocks(Vec<ApiSystemBlock>),
}

/// Cache control for prompt caching
#[derive(Serialize, Clone)]
pub struct CacheControlParam {
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<&'static str>,
}

impl CacheControlParam {
    fn ephemeral(cache_ttl_1h: bool) -> Self {
        if cache_ttl_1h {
            Self::ephemeral_1h()
        } else {
            Self {
                kind: "ephemeral",
                ttl: None,
            }
        }
    }

    fn ephemeral_1h() -> Self {
        Self {
            kind: "ephemeral",
            ttl: Some("1h"),
        }
    }
}

#[derive(Serialize, Clone)]
pub struct ApiSystemBlock {
    #[serde(rename = "type")]
    pub block_type: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlParam>,
}

pub fn build_system_param(system: &str, is_oauth: bool, cache_ttl_1h: bool) -> Option<ApiSystem> {
    build_system_param_split(system, "", is_oauth, cache_ttl_1h)
}

/// Build system param with split static/dynamic content for better caching
pub fn build_system_param_split(
    static_part: &str,
    dynamic_part: &str,
    is_oauth: bool,
    cache_ttl_1h: bool,
) -> Option<ApiSystem> {
    if is_oauth {
        let mut blocks = Vec::new();
        blocks.push(ApiSystemBlock {
            block_type: "text",
            text: format!("x-anthropic-billing-header: {}", OAUTH_BILLING_HEADER),
            cache_control: None,
        });
        blocks.push(ApiSystemBlock {
            block_type: "text",
            text: CLAUDE_CODE_IDENTITY.to_string(),
            cache_control: None,
        });
        // Static content - CACHED (instruction files, base prompt, skills)
        if !static_part.is_empty() {
            blocks.push(ApiSystemBlock {
                block_type: "text",
                text: static_part.to_string(),
                cache_control: Some(CacheControlParam::ephemeral(cache_ttl_1h)),
            });
        }
        // Dynamic content - NOT cached (date, git status, memory)
        if !dynamic_part.is_empty() {
            blocks.push(ApiSystemBlock {
                block_type: "text",
                text: dynamic_part.to_string(),
                cache_control: None,
            });
        }
        return Some(ApiSystem::Blocks(blocks));
    }

    // Non-OAuth: use block format with cache control for static part only
    let has_static = !static_part.is_empty();
    let has_dynamic = !dynamic_part.is_empty();

    if !has_static && !has_dynamic {
        None
    } else {
        let mut blocks = Vec::new();
        if has_static {
            blocks.push(ApiSystemBlock {
                block_type: "text",
                text: static_part.to_string(),
                cache_control: Some(CacheControlParam::ephemeral(cache_ttl_1h)),
            });
        }
        if has_dynamic {
            blocks.push(ApiSystemBlock {
                block_type: "text",
                text: dynamic_part.to_string(),
                cache_control: None,
            });
        }
        Some(ApiSystem::Blocks(blocks))
    }
}

pub fn format_messages_with_identity(
    messages: Vec<ApiMessage>,
    _is_oauth: bool,
    cache_ttl_1h: bool,
) -> Vec<ApiMessage> {
    let mut out = messages;

    // Add cache breakpoints for both OAuth and non-OAuth paths
    add_message_cache_breakpoint(&mut out, cache_ttl_1h);

    out
}

/// Add cache_control to messages for conversation caching.
///
/// Strategy: sliding two-marker window
///   - Second-to-last assistant message → READ marker (re-uses cache snapshot from previous turn)
///   - Last assistant message           → WRITE marker (creates new snapshot for the next turn)
///
/// This ensures each turn N+1 reads from turn N's conversation cache, paying only
/// cache_read_input_tokens for the already-cached history instead of full input tokens.
///
/// Budget: system (1) + tools (1) + messages (up to 2) = 4 total, within Anthropic's limit.
pub fn add_message_cache_breakpoint(messages: &mut [ApiMessage], cache_ttl_1h: bool) {
    jcode_logging::info(&format!(
        "Conversation caching: {} messages to process",
        messages.len()
    ));

    if messages.len() < 3 {
        // Need at least: user + assistant + user to be worth caching
        jcode_logging::info("Conversation caching: too few messages, skipping");
        return;
    }

    // Collect indices of up to 2 most recent assistant messages (newest first)
    let mut assistant_indices: Vec<usize> = Vec::with_capacity(2);
    for (i, msg) in messages.iter().enumerate().rev() {
        if msg.role == "assistant" {
            assistant_indices.push(i);
            if assistant_indices.len() == 2 {
                break;
            }
        }
    }

    if assistant_indices.is_empty() {
        jcode_logging::info("Conversation caching: no assistant message found");
        return;
    }

    // Place cache_control on both (newest = WRITE for next turn, older = READ from prev turn)
    let total = assistant_indices.len();
    for (slot, &idx) in assistant_indices.iter().enumerate() {
        let label = if slot == 0 {
            "WRITE (newest)"
        } else {
            "READ (prev-turn)"
        };
        let mut added = false;
        if let Some(msg) = messages.get_mut(idx) {
            for block in msg.content.iter_mut().rev() {
                match block {
                    ApiContentBlock::Text { cache_control, .. }
                    | ApiContentBlock::ToolUse { cache_control, .. } => {
                        *cache_control = Some(CacheControlParam::ephemeral(cache_ttl_1h));
                        added = true;
                        break;
                    }
                    _ => {}
                }
            }
        }
        if added {
            jcode_logging::info(&format!(
                "Conversation caching: breakpoint {}/{} at message {} [{}]",
                slot + 1,
                total,
                idx,
                label
            ));
        } else {
            jcode_logging::info(&format!(
                "Conversation caching: no cacheable block in assistant message {} [{}]",
                idx, label
            ));
        }
    }
}

#[derive(Serialize, Clone)]
pub struct ApiMessage {
    pub role: String,
    pub content: Vec<ApiContentBlock>,
}

#[derive(Serialize, Clone)]
#[serde(tag = "type")]
pub enum ApiContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControlParam>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControlParam>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: ToolResultContent,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    #[serde(rename = "image")]
    Image { source: ApiImageSource },
    /// Provider-native block sent back exactly as the API produced it
    /// (`server_tool_use`, `web_search_tool_result`). Its own `type` field is
    /// kept, and its encrypted payloads must not be touched.
    #[serde(untagged)]
    Raw(Value),
}

#[derive(Serialize, Clone)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(String),
    Blocks(Vec<ToolResultContentBlock>),
}

#[derive(Serialize, Clone)]
#[serde(tag = "type")]
pub enum ToolResultContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: ApiImageSource },
    /// Loads a deferred tool definition (`defer_loading: true`) into context.
    /// The API rejects a tool_result mixing references with other content.
    #[serde(rename = "tool_reference")]
    ToolReference { tool_name: String },
}

#[derive(Serialize, Clone)]
pub struct ApiImageSource {
    #[serde(rename = "type")]
    pub kind: String,
    pub media_type: String,
    pub data: String,
}

#[derive(Serialize, Clone)]
pub struct ApiTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlParam>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub defer_loading: bool,
}

/// One entry of the request `tools` array: a client tool jcode executes, or an
/// Anthropic server tool (e.g. `web_search_20250305`) the API executes itself.
#[derive(Serialize, Clone)]
#[serde(untagged)]
pub enum ApiToolParam {
    Custom(ApiTool),
    Server(Value),
}

/// Build the request `tools` array. Server tools go right after the eager client
/// tools (behind the tool-cache breakpoint) and before any deferred tools, so
/// loading or unloading deferred tools never shifts them. Returns `None` when
/// there are no tools at all.
pub fn request_tools(custom: Vec<ApiTool>, server: Vec<Value>) -> Option<Vec<ApiToolParam>> {
    let (eager, deferred): (Vec<ApiTool>, Vec<ApiTool>) =
        custom.into_iter().partition(|tool| !tool.defer_loading);
    let mut out: Vec<ApiToolParam> = eager.into_iter().map(ApiToolParam::Custom).collect();
    out.extend(server.into_iter().map(ApiToolParam::Server));
    out.extend(deferred.into_iter().map(ApiToolParam::Custom));
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
#[path = "cache_prefix_invariant_tests.rs"]
mod cache_prefix_invariant_tests;

#[cfg(test)]
#[path = "oauth_tool_schema_tests.rs"]
mod oauth_tool_schema_tests;

#[cfg(test)]
#[path = "trailing_assistant_repair_tests.rs"]
mod trailing_assistant_repair_tests;

#[cfg(test)]
#[path = "duplicate_tool_result_tests.rs"]
mod duplicate_tool_result_tests;

#[cfg(test)]
#[path = "wedge_fixture_check.rs"]
mod wedge_fixture_check;

#[cfg(test)]
#[path = "native_web_search_repair_tests.rs"]
mod native_web_search_repair_tests;

#[cfg(test)]
#[path = "deferred_tools_tests.rs"]
mod deferred_tools_tests;
