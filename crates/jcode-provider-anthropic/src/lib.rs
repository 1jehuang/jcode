use jcode_message_types::{ContentBlock, Message, Role, ToolDefinition, sanitize_tool_id};
use jcode_provider_core::anthropic_map_tool_name_for_oauth as map_tool_name_for_oauth;
use serde::Serialize;
use serde_json::{Value, json};

/// Claude Code billing attribution text observed in the official CLI's system
/// prompt blocks.
pub const OAUTH_BILLING_HEADER: &str = "cc_version=2.1.123; cc_entrypoint=sdk-cli; cch=33f85;";

const CLAUDE_CODE_IDENTITY: &str = "You are a Claude agent, built on Anthropic's Claude Agent SDK.";

pub fn format_messages(messages: &[Message], is_oauth: bool) -> Vec<ApiMessage> {
    use std::collections::HashSet;

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

        let content = format_content_blocks(&msg.content, is_oauth);

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
    let mut result: Vec<ApiContentBlock> = Vec::new();
    for block in blocks {
        match block {
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

/// Convert tool definitions to Anthropic API format
/// Adds cache_control to the last tool for prompt caching
/// Local tool names that are represented by the curated Claude-Code builtin
/// definitions in OAuth mode. These keep their hand-tuned schemas/descriptions
/// (which the Anthropic subscription endpoint expects) instead of the raw
/// registry definitions; every other tool is forwarded as-is (see #409).
const OAUTH_BUILTIN_LOCAL_TOOLS: &[&str] = &[
    "subagent",
    "bash",
    "edit",
    "glob",
    "grep",
    "read",
    "schedule",
    "skill_manage",
    "write",
];

pub fn format_tools(tools: &[ToolDefinition], is_oauth: bool, cache_ttl_1h: bool) -> Vec<ApiTool> {
    if is_oauth {
        // Curated Claude-Code builtin tool definitions. These remain hand-tuned
        // because the Anthropic OAuth (subscription) endpoint expects the
        // builtin names with compatible schemas. Anything not represented here
        // is appended from the real registry below so OAuth users keep the full
        // toolset (websearch, webfetch, browser, codesearch, memory, ...).
        let mut out = vec![
            ApiTool {
                name: "Agent".to_string(),
                description: "Launch a new agent to handle complex, multi-step tasks.".to_string(),
                input_schema: json!({"type":"object","properties":{"description":{"type":"string"},"prompt":{"type":"string"},"subagent_type":{"type":"string"},"run_in_background":{"type":"boolean"}},"required":["description","prompt"],"additionalProperties":false}),
                cache_control: None,
            },
            ApiTool {
                name: "Bash".to_string(),
                description: "Executes a given bash command and returns its output.".to_string(),
                input_schema: json!({"type":"object","properties":{"command":{"type":"string"},"timeout":{"type":"integer"},"run_in_background":{"type":"boolean"}},"required":["command"],"additionalProperties":false}),
                cache_control: None,
            },
            ApiTool {
                name: "Edit".to_string(),
                description: "Performs exact string replacements in files.".to_string(),
                input_schema: json!({"type":"object","properties":{"file_path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean","default":false}},"required":["file_path","old_string","new_string"],"additionalProperties":false}),
                cache_control: None,
            },
            ApiTool {
                name: "Glob".to_string(),
                description: "Fast file pattern matching tool.".to_string(),
                input_schema: json!({"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"],"additionalProperties":false}),
                cache_control: None,
            },
            ApiTool {
                name: "Grep".to_string(),
                description: "A powerful search tool built on ripgrep.".to_string(),
                input_schema: json!({"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"},"glob":{"type":"string"},"output_mode":{"type":"string","enum":["content","files_with_matches","count"]},"-B":{"type":"number"},"-A":{"type":"number"},"-C":{"type":"number"},"context":{"type":"number"},"-n":{"type":"boolean"},"-i":{"type":"boolean"},"type":{"type":"string"},"head_limit":{"type":"number"},"offset":{"type":"number"},"multiline":{"type":"boolean"}},"required":["pattern"],"additionalProperties":false}),
                cache_control: None,
            },
            ApiTool {
                name: "Read".to_string(),
                description: "Reads a file from the local filesystem.".to_string(),
                input_schema: json!({"type":"object","properties":{"file_path":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","exclusiveMinimum":0},"pages":{"type":"string"}},"required":["file_path"],"additionalProperties":false}),
                cache_control: None,
            },
            ApiTool {
                name: "ScheduleWakeup".to_string(),
                description: "Schedule when to resume work in /loop dynamic mode.".to_string(),
                input_schema: json!({"type":"object","properties":{"delaySeconds":{"type":"number"},"reason":{"type":"string"},"prompt":{"type":"string"}},"required":["delaySeconds","reason","prompt"],"additionalProperties":false}),
                cache_control: None,
            },
            ApiTool {
                name: "Skill".to_string(),
                description: "Execute a skill within the main conversation".to_string(),
                input_schema: json!({"type":"object","properties":{"skill":{"type":"string"},"args":{"type":"string"}},"required":["skill"],"additionalProperties":false}),
                cache_control: None,
            },
            ApiTool {
                name: "Write".to_string(),
                description: "Writes a file to the local filesystem.".to_string(),
                input_schema: json!({"type":"object","properties":{"file_path":{"type":"string"},"content":{"type":"string"}},"required":["file_path","content"],"additionalProperties":false}),
                cache_control: None,
            },
        ];

        // Forward every other registered tool, remapping its name to the
        // OAuth-accepted form. This restores websearch/webfetch/browser/
        // codesearch/memory/swarm/multiedit/open/etc. for subscription users,
        // matching the documented "remap names, keep the full toolset" behavior
        // and the (deprecated) Claude CLI transport.
        for tool in tools {
            if OAUTH_BUILTIN_LOCAL_TOOLS.contains(&tool.name.as_str()) {
                continue;
            }
            out.push(ApiTool {
                name: map_tool_name_for_oauth(&tool.name),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
                cache_control: None,
            });
        }

        // Move the prompt-cache breakpoint to the final tool in the list.
        if let Some(last) = out.last_mut() {
            last.cache_control = Some(CacheControlParam::ephemeral(cache_ttl_1h));
        }

        return out;
    }

    let len = tools.len();
    tools
        .iter()
        .enumerate()
        .map(|(i, tool)| ApiTool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            cache_control: if i == len - 1 {
                Some(CacheControlParam::ephemeral(cache_ttl_1h))
            } else {
                None
            },
        })
        .collect()
}

#[derive(Serialize, Clone)]
pub struct ApiRequest {
    pub model: String,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<ApiSystem>,
    pub messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ApiTool>>,
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
    },
    Enabled {
        budget_tokens: u32,
    },
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
}

#[cfg(test)]
mod cache_prefix_invariant_tests {
    //! Deterministic proof that injecting a trailing memory message can never move
    //! the Anthropic prefix-cache breakpoints off the stable assistant prefix.
    //!
    //! Anthropic caching is strict-prefix: a `cache_control` breakpoint caches every
    //! token up to and including the block it sits on. `add_message_cache_breakpoint`
    //! always anchors the two breakpoints on the two most recent *assistant* messages.
    //! Memory is injected by the agent as a trailing *user* message (see
    //! `turn_loops.rs` / `turn_streaming_mpsc.rs`). Therefore the breakpoint anchors,
    //! and every token they cache, are identical with or without the memory suffix.
    //! These tests pin that invariant so a refactor cannot silently break the cache.

    use super::*;
    use jcode_message_types::{ContentBlock, Message, Role};

    fn text_msg(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        }
    }

    /// A realistic warm conversation: user/assistant turns ending on a user message.
    fn base_conversation() -> Vec<Message> {
        vec![
            text_msg(Role::User, "Q1"),
            text_msg(Role::Assistant, "A1"),
            text_msg(Role::User, "Q2"),
            text_msg(Role::Assistant, "A2"),
            text_msg(Role::User, "Q3"),
        ]
    }

    /// Returns the indices of ApiMessages that carry a cache_control breakpoint,
    /// paired with the role of that message.
    fn breakpoint_anchors(messages: &[ApiMessage]) -> Vec<(usize, String)> {
        messages
            .iter()
            .enumerate()
            .filter_map(|(i, msg)| {
                let has_bp = msg.content.iter().any(|block| {
                    matches!(
                        block,
                        ApiContentBlock::Text {
                            cache_control: Some(_),
                            ..
                        } | ApiContentBlock::ToolUse {
                            cache_control: Some(_),
                            ..
                        }
                    )
                });
                has_bp.then(|| (i, msg.role.clone()))
            })
            .collect()
    }

    /// Serialize only the prefix up to and including the last breakpoint. This is the
    /// exact span Anthropic caches; if it is byte-identical across two requests, the
    /// cache is guaranteed to be reused.
    fn cached_prefix_json(messages: &[ApiMessage]) -> String {
        let last_bp = breakpoint_anchors(messages)
            .last()
            .map(|(idx, _)| *idx)
            .expect("expected at least one cache breakpoint");
        serde_json::to_string(&messages[..=last_bp]).expect("serialize cached prefix")
    }

    fn formatted_with_breakpoints(messages: &[Message]) -> Vec<ApiMessage> {
        let mut api = format_messages(messages, false);
        add_message_cache_breakpoint(&mut api, false);
        api
    }

    #[test]
    fn breakpoints_anchor_on_assistant_messages_only() {
        let api = formatted_with_breakpoints(&base_conversation());
        let anchors = breakpoint_anchors(&api);
        assert!(!anchors.is_empty(), "expected breakpoints to be placed");
        for (idx, role) in &anchors {
            assert_eq!(
                role, "assistant",
                "breakpoint at message {idx} must be on an assistant message, got {role}"
            );
        }
    }

    #[test]
    fn trailing_memory_message_does_not_move_breakpoints() {
        let base = base_conversation();
        let mut with_memory = base.clone();
        with_memory.push(text_msg(
            Role::User,
            "<memory>relevant recall injected for this turn</memory>",
        ));

        let base_api = formatted_with_breakpoints(&base);
        let mem_api = formatted_with_breakpoints(&with_memory);

        let base_anchors = breakpoint_anchors(&base_api);
        let mem_anchors = breakpoint_anchors(&mem_api);

        assert_eq!(
            base_anchors, mem_anchors,
            "memory suffix moved the cache breakpoints: {base_anchors:?} -> {mem_anchors:?}"
        );
    }

    #[test]
    fn cached_prefix_is_byte_identical_with_and_without_memory() {
        let base = base_conversation();
        let mut with_memory = base.clone();
        with_memory.push(text_msg(
            Role::User,
            "<memory>turn-specific recall</memory>",
        ));

        let base_prefix = cached_prefix_json(&formatted_with_breakpoints(&base));
        let mem_prefix = cached_prefix_json(&formatted_with_breakpoints(&with_memory));

        assert_eq!(
            base_prefix, mem_prefix,
            "the cached prefix span differs once memory is appended; cache would be invalidated"
        );
    }

    #[test]
    fn different_memory_each_turn_keeps_identical_cached_prefix() {
        // The memory content changes every turn. Because it is a trailing user message
        // placed *after* the newest assistant breakpoint, the cached prefix must remain
        // identical regardless of what memory is injected.
        let base = base_conversation();
        let cached = cached_prefix_json(&formatted_with_breakpoints(&base));

        for memory in [
            "<memory>recall A</memory>",
            "<memory>completely different recall B with more text</memory>",
            "",
        ] {
            let mut msgs = base.clone();
            if !memory.is_empty() {
                msgs.push(text_msg(Role::User, memory));
            }
            let candidate = cached_prefix_json(&formatted_with_breakpoints(&msgs));
            assert_eq!(
                cached, candidate,
                "memory variant {memory:?} changed the cached prefix span"
            );
        }
    }

    fn tool_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("{name} description"),
            input_schema: json!({"type":"object","properties":{}}),
        }
    }

    #[test]
    fn oauth_format_tools_keeps_full_custom_toolset() {
        // Registry includes builtins (remapped) plus extra tools that must survive.
        let registry = vec![
            tool_def("bash"),
            tool_def("read"),
            tool_def("subagent"),
            tool_def("websearch"),
            tool_def("webfetch"),
            tool_def("browser"),
            tool_def("codesearch"),
            tool_def("memory"),
        ];

        let formatted = format_tools(&registry, true, false);
        let names: Vec<&str> = formatted.iter().map(|t| t.name.as_str()).collect();

        // Curated builtins are present under their OAuth names.
        for builtin in ["Bash", "Read", "Agent", "Write", "Edit", "Glob", "Grep"] {
            assert!(
                names.contains(&builtin),
                "missing builtin {builtin} in {names:?}"
            );
        }
        // The previously-dropped custom tools are now forwarded.
        for custom in ["websearch", "webfetch", "browser", "codesearch", "memory"] {
            assert!(
                names.contains(&custom),
                "custom tool {custom} was dropped on OAuth; got {names:?}"
            );
        }
        // No duplicate Agent/Bash/Read from the registry remap.
        assert_eq!(names.iter().filter(|n| **n == "Agent").count(), 1);
        assert_eq!(names.iter().filter(|n| **n == "Bash").count(), 1);
        assert_eq!(names.iter().filter(|n| **n == "Read").count(), 1);
    }

    #[test]
    fn oauth_format_tools_places_single_cache_breakpoint_on_last_tool() {
        let registry = vec![tool_def("bash"), tool_def("websearch")];
        let formatted = format_tools(&registry, true, false);
        let with_cache: Vec<&str> = formatted
            .iter()
            .filter(|t| t.cache_control.is_some())
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(with_cache.len(), 1, "expected exactly one cache breakpoint");
        assert_eq!(
            formatted.last().map(|t| t.name.as_str()),
            with_cache.first().copied(),
            "cache breakpoint must be on the final tool"
        );
    }
}
