use anyhow::Result;
use jcode_message_types::{ContentBlock, Message, Role, ToolCall, ToolDefinition};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;

pub mod interactions;
pub mod stream;

// Non-dated server-side alias: always resolves to the newest stable pro
// model, so the default can never go stale or 404 into a downgrade chain
// (2026-09-19: a transient NOT_FOUND on gemini-3.1-pro-preview silently
// pinned every session to gemini-2.5-pro for days).
pub const DEFAULT_MODEL: &str = "gemini-pro-latest";
pub const AVAILABLE_MODELS: &[&str] = &[
    "gemini-pro-latest",
    "gemini-flash-latest",
    "gemini-3.1-pro-preview",
    "gemini-3-pro-preview",
    "gemini-3-flash-preview",
    "gemini-2.5-pro",
    "gemini-2.5-flash",
    "gemini-2.0-flash",
    "gemini-1.5-pro",
    "gemini-1.5-flash",
];
pub const FALLBACK_MODELS: &[&str] = &[
    "gemini-pro-latest",
    "gemini-3.1-pro-preview",
    "gemini-3-pro-preview",
    "gemini-2.5-pro",
    "gemini-3-flash-preview",
    "gemini-2.5-flash",
    "gemini-2.0-flash",
];
pub const CODE_ASSIST_ENDPOINT: &str = "https://cloudcode-pa.googleapis.com";
pub const CODE_ASSIST_API_VERSION: &str = "v1internal";
/// Official Gemini Developer API (Google AI Studio) endpoint. Used when an API
/// key is configured instead of OAuth Code Assist credentials.
pub const GEMINI_API_ENDPOINT: &str = "https://generativelanguage.googleapis.com";
pub const GEMINI_API_VERSION: &str = "v1beta";
pub const USER_TIER_FREE: &str = "free-tier";
pub const USER_TIER_LEGACY: &str = "legacy-tier";

#[derive(Debug, Clone)]
pub struct GeminiRuntimeState {
    pub project_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientMetadata {
    pub ide_type: &'static str,
    pub platform: &'static str,
    pub plugin_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duet_project: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadCodeAssistRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloudaicompanion_project: Option<String>,
    pub metadata: ClientMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<&'static str>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadCodeAssistResponse {
    #[serde(default)]
    pub current_tier: Option<GeminiUserTier>,
    #[serde(default)]
    pub allowed_tiers: Option<Vec<GeminiUserTier>>,
    #[serde(default)]
    pub ineligible_tiers: Option<Vec<IneligibleTier>>,
    #[serde(default)]
    pub cloudaicompanion_project: Option<String>,
    #[serde(default)]
    pub paid_tier: Option<GeminiUserTier>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiUserTier {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub is_default: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IneligibleTier {
    #[serde(default)]
    pub reason_code: Option<String>,
    #[serde(default)]
    pub reason_message: Option<String>,
    #[serde(default)]
    pub validation_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardUserRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloudaicompanion_project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ClientMetadata>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LongRunningOperationResponse {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub done: Option<bool>,
    #[serde(default)]
    pub response: Option<OnboardUserResponse>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardUserResponse {
    #[serde(default)]
    pub cloudaicompanion_project: Option<ProjectRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectRef {
    #[serde(default)]
    pub id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodeAssistGenerateRequest {
    pub model: String,
    pub project: String,
    pub user_prompt_id: String,
    pub request: VertexGenerateContentRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_config: Option<GeminiThinkingConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GeminiThinkingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_thoughts: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
}

/// Progressive fallback level for requesting Gemini thought summaries.
///
/// On Antigravity (Cloud Code), Gemini 2.5/3.x Flash models default their
/// thinking budget to `0` when `thinkingConfig` sets only `includeThoughts`,
/// emitting empty thought parts and zero thought tokens unless a dynamic
/// budget (`thinkingBudget: -1`) or explicit `thinkingLevel` is supplied.
/// If a backend rejects `thinkingBudget`/`thinkingLevel`, callers downgrade to
/// `IncludeThoughtsOnly`, and finally to `Disabled` if `thinkingConfig` itself
/// is unsupported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeminiThinkingRequestMode {
    Full,
    IncludeThoughtsOnly,
    Disabled,
}

impl GeminiThinkingRequestMode {
    /// Step down to a less strict thinking configuration after a backend 400
    /// rejecting the current `thinkingConfig` for `model`.
    ///
    /// When the backend specifically rejects `thinkingBudget` or
    /// `thinkingLevel`, downgrade from `Full` to `IncludeThoughtsOnly` so
    /// thought summaries remain enabled. When the backend rejects
    /// `thinkingConfig` / `includeThoughts` / thinking as a whole, jump
    /// straight to `Disabled` without an extra round-trip.
    pub fn downgrade_for_error(self, model: &str, error_message: &str) -> Option<Self> {
        if !is_thinking_config_rejected_error(error_message) {
            return None;
        }
        let lowered = error_message.to_ascii_lowercase();
        let budget_or_level_only = (lowered.contains("thinking_budget")
            || lowered.contains("thinkingbudget")
            || lowered.contains("thinking_level")
            || lowered.contains("thinkinglevel"))
            && !lowered.contains("thinking_config")
            && !lowered.contains("thinkingconfig")
            && !lowered.contains("include_thoughts")
            && !lowered.contains("includethoughts")
            && !lowered.contains("thinking is not supported")
            && !lowered.contains("thinking mode is not supported");
        match self {
            Self::Full => {
                if budget_or_level_only {
                    let full = build_generation_config_for_thoughts_with_mode(model, Self::Full);
                    let only = build_generation_config_for_thoughts_with_mode(
                        model,
                        Self::IncludeThoughtsOnly,
                    );
                    if full == only {
                        Some(Self::Disabled)
                    } else {
                        Some(Self::IncludeThoughtsOnly)
                    }
                } else {
                    Some(Self::Disabled)
                }
            }
            Self::IncludeThoughtsOnly => Some(Self::Disabled),
            Self::Disabled => None,
        }
    }
}

fn explicit_gemini_thinking_level(lower_model: &str) -> Option<&'static str> {
    if lower_model.ends_with("-minimal") {
        Some("MINIMAL")
    } else if lower_model.ends_with("-low") {
        Some("LOW")
    } else if lower_model.ends_with("-medium") {
        Some("MEDIUM")
    } else if lower_model.ends_with("-high") {
        Some("HIGH")
    } else {
        None
    }
}

/// Whether `model` accepts `generationConfig.thinkingConfig.includeThoughts`.
///
/// Gemini 2.5+ (including 3.x, 4.x, future generations, and `-latest` aliases)
/// and Antigravity's hosted Claude / GPT-OSS routes accept `includeThoughts`
/// and stream `thought: true` parts when enabled. Older Gemini 1.x / 2.0
/// non-thinking models and non-text media/embedding models reject
/// `thinkingConfig` with HTTP 400.
pub fn model_supports_thought_summaries(model: &str) -> bool {
    let lower = model
        .trim()
        .trim_start_matches("models/")
        .to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }
    if lower.contains("embedding")
        || lower.contains("-tts")
        || lower.contains("-image")
        || lower.starts_with("imagen")
        || lower.starts_with("veo")
        || lower.starts_with("aqa")
    {
        return false;
    }
    if lower.starts_with("gemini-") {
        if lower.contains("thinking") {
            return true;
        }
        if lower.starts_with("gemini-1.")
            || lower.starts_with("gemini-1-")
            || lower.starts_with("gemini-2.0")
        {
            return false;
        }
        return true;
    }
    true
}

/// Build `generationConfig` requesting thought summaries for `model` at the
/// given fallback `mode`, or `None` when disabled/unsupported.
pub fn build_generation_config_for_thoughts_with_mode(
    model: &str,
    mode: GeminiThinkingRequestMode,
) -> Option<GeminiGenerationConfig> {
    if mode == GeminiThinkingRequestMode::Disabled || !model_supports_thought_summaries(model) {
        return None;
    }
    let lower = model
        .trim()
        .trim_start_matches("models/")
        .to_ascii_lowercase();
    let thinking_config = match mode {
        GeminiThinkingRequestMode::Full if lower.starts_with("gemini-") => {
            if let Some(level) = explicit_gemini_thinking_level(&lower) {
                GeminiThinkingConfig {
                    include_thoughts: Some(true),
                    thinking_budget: None,
                    thinking_level: Some(level.to_string()),
                }
            } else {
                GeminiThinkingConfig {
                    include_thoughts: Some(true),
                    thinking_budget: Some(-1),
                    thinking_level: None,
                }
            }
        }
        GeminiThinkingRequestMode::Full | GeminiThinkingRequestMode::IncludeThoughtsOnly => {
            GeminiThinkingConfig {
                include_thoughts: Some(true),
                thinking_budget: None,
                thinking_level: None,
            }
        }
        GeminiThinkingRequestMode::Disabled => unreachable!(),
    };
    Some(GeminiGenerationConfig {
        thinking_config: Some(thinking_config),
    })
}

/// Build `generationConfig` requesting thought summaries when supported by
/// `model`, or `None` when the model does not support `thinkingConfig`.
pub fn build_generation_config_for_thoughts(model: &str) -> Option<GeminiGenerationConfig> {
    build_generation_config_for_thoughts_with_mode(model, GeminiThinkingRequestMode::Full)
}

/// Whether an HTTP error indicates the backend rejected `thinkingConfig` /
/// `includeThoughts` / `thinkingBudget` / `thinkingLevel` for this model, so
/// the caller can safely downgrade or omit `generationConfig`.
pub fn is_thinking_config_rejected_error(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("thinking_config")
        || lowered.contains("thinkingconfig")
        || lowered.contains("include_thoughts")
        || lowered.contains("includethoughts")
        || lowered.contains("thinking_budget")
        || lowered.contains("thinkingbudget")
        || lowered.contains("thinking_level")
        || lowered.contains("thinkinglevel")
        || lowered.contains("thinking is not supported")
        || lowered.contains("thinking mode is not supported")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VertexGenerateContentRequest {
    pub contents: Vec<GeminiContent>,
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<GeminiTool>>,
    #[serde(rename = "toolConfig", skip_serializing_if = "Option::is_none")]
    pub tool_config: Option<GeminiToolConfig>,
    #[serde(rename = "generationConfig", skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GeminiGenerationConfig>,
    #[serde(rename = "session_id", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiContent {
    // Requests always set `role` (see `build_contents`), but `generateContent`
    // responses occasionally omit it on a candidate's `content` (observed on
    // Antigravity/Cloud Code Gemini-3 turns). The response-side value is never
    // read, so default it rather than failing the whole decode with
    // "missing field `role`".
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub parts: Vec<GeminiPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Whether this part contains model thought summaries rather than answer text.
    /// Returned when `generationConfig.thinkingConfig.includeThoughts` is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_data: Option<InlineData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call: Option<GeminiFunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_response: Option<GeminiFunctionResponse>,
    /// Gemini 3 thought signature for this part. Must be replayed verbatim on
    /// the `functionCall` part in later turns or the Cloud Code backend rejects
    /// the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

impl GeminiPart {
    pub fn is_thought(&self) -> bool {
        self.thought == Some(true)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineData {
    pub mime_type: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiFunctionCall {
    pub name: String,
    #[serde(default)]
    pub args: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiFunctionResponse {
    pub name: String,
    pub response: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

pub fn build_system_instruction(system: &str) -> Option<GeminiContent> {
    let trimmed = system.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(GeminiContent {
            role: "user".to_string(),
            parts: vec![GeminiPart {
                text: Some(trimmed.to_string()),
                ..Default::default()
            }],
        })
    }
}

/// Prevention guidance appended to the Gemini system prompt when tools are
/// advertised. Gemini-3 "thinking" models intermittently emit Python-style
/// pseudo-code (e.g. `print(default_api.read(...))`) instead of a clean
/// `functionCall`, which the backend rejects with `MALFORMED_FUNCTION_CALL` and
/// empty content. Explicitly forbidding code/namespaces measurably reduces that
/// failure mode at no latency cost (see the Gemini function-calling guidance and
/// field reports of this exact behavior).
const GEMINI_FUNCTION_CALL_GUARD: &str = "\n\n## Function calling\n\
     - When you call a tool, emit a native function call, not code. Never write \
     Python (or any language) that calls the tool, and never wrap a call in \
     print(...) or a code block.\n\
     - Use the function name exactly as defined. Do not prepend `default_api.` \
     or any other namespace to the function name.";

/// Build the Gemini `system_instruction`, appending [`GEMINI_FUNCTION_CALL_GUARD`]
/// when tools are advertised so the model is steered away from the
/// `MALFORMED_FUNCTION_CALL` pseudo-code failure mode.
pub fn build_system_instruction_with_tool_guard(
    system: &str,
    has_tools: bool,
) -> Option<GeminiContent> {
    if !has_tools {
        return build_system_instruction(system);
    }
    let mut combined = system.trim().to_string();
    combined.push_str(GEMINI_FUNCTION_CALL_GUARD);
    build_system_instruction(&combined)
}

pub fn build_contents(messages: &[Message]) -> Vec<GeminiContent> {
    build_contents_with_signature_policy(messages, SignaturePolicy::ReplayCarriedForward)
}

/// How [`build_contents_with_signature_policy`] handles Gemini-3 function-call
/// `thoughtSignature` replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignaturePolicy {
    /// Normal path: replay each call's own signature, carrying the most recent
    /// real signature forward onto calls that lack one.
    ReplayCarriedForward,
    /// Recovery path for a `missing a thought_signature` 400. The backend has
    /// told us it will not accept this history's function-call parts, so
    /// downgrade every tool call and tool result to plain text. This loses the
    /// structured form but keeps the conversation's *content*, turning a hard
    /// dead-end into a turn that completes.
    DowngradeToolCallsToText,
}

/// Whether a provider error body is the Cloud Code / Antigravity backend
/// rejecting a turn because its function calls lack a usable thought signature.
pub fn is_missing_thought_signature_error(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("thought_signature") || lowered.contains("thoughtsignature")
}

pub fn build_contents_with_signature_policy(
    messages: &[Message],
    policy: SignaturePolicy,
) -> Vec<GeminiContent> {
    // Gemini-3 attaches an opaque `thoughtSignature` to function-call parts, and
    // the Cloud Code / Antigravity backend rejects an assistant turn whose
    // function calls are ALL unsigned with `Function call is missing a
    // thought_signature in functionCall parts` (HTTP 400, issue #339). This
    // happens because:
    //   * a parallel multi-call turn only signs its FIRST call (siblings persist
    //     unsigned), and
    //   * locally synthesized tool calls (batch sub-calls, manual tool use,
    //     auto-poke continuations, recovery) and pre-signature/imported sessions
    //     carry no signature at all.
    //
    // Live-verified backend rule: a turn is accepted as long as *at least one*
    // of its function calls carries a (valid) signature; a fully-unsigned turn
    // 400s. All calls in a session share the same opaque reasoning channel and
    // the backend accepts a previously-emitted signature replayed on later
    // calls, so we carry the most recent real signature forward across the whole
    // conversation onto any function call that lacks one. This keeps multi-call
    // turns and synthesized/imported histories replayable instead of hard-failing.
    let mut last_signature: Option<String> = None;
    messages
        .iter()
        .filter_map(|message| {
            let role = match message.role {
                Role::User => "user",
                Role::Assistant => "model",
            };
            let mut parts = Vec::new();
            for block in &message.content {
                match block {
                    ContentBlock::Text { text, .. } => {
                        parts.push(GeminiPart {
                            text: Some(text.clone()),
                            ..Default::default()
                        });
                    }
                    ContentBlock::Reasoning { .. }
                    | ContentBlock::ReasoningTrace { .. }
                    | ContentBlock::AnthropicThinking { .. }
                    | ContentBlock::OpenAIReasoning { .. } => {}
                    ContentBlock::ToolUse {
                        id,
                        name,
                        input,
                        thought_signature,
                    } => {
                        if policy == SignaturePolicy::DowngradeToolCallsToText {
                            // The backend rejected this history's function-call
                            // parts. Preserve the call as readable text so the
                            // model keeps its context instead of 400ing forever.
                            parts.push(GeminiPart {
                                text: Some(format!(
                                    "[previous tool call] {name}({})",
                                    ToolCall::input_as_object(input)
                                )),
                                ..Default::default()
                            });
                            continue;
                        }
                        let own_signature = thought_signature
                            .as_ref()
                            .filter(|sig| !sig.is_empty())
                            .cloned();
                        if own_signature.is_some() {
                            last_signature = own_signature.clone();
                        }
                        let signature = own_signature.or_else(|| last_signature.clone());
                        parts.push(GeminiPart {
                            function_call: Some(GeminiFunctionCall {
                                name: name.clone(),
                                args: ToolCall::input_as_object(input),
                                id: Some(id.clone()),
                            }),
                            thought_signature: signature,
                            ..Default::default()
                        });
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        if policy == SignaturePolicy::DowngradeToolCallsToText {
                            // A `functionResponse` with no matching signed
                            // `functionCall` is also rejected, so downgrade it too.
                            let label = if is_error.unwrap_or(false) {
                                "previous tool error"
                            } else {
                                "previous tool result"
                            };
                            parts.push(GeminiPart {
                                text: Some(format!(
                                    "[{label}] {}: {content}",
                                    tool_name_from_tool_result(tool_use_id, messages)
                                )),
                                ..Default::default()
                            });
                            continue;
                        }
                        parts.push(GeminiPart {
                            function_response: Some(GeminiFunctionResponse {
                                name: tool_name_from_tool_result(tool_use_id, messages),
                                response: if is_error.unwrap_or(false) {
                                    json!({ "error": content })
                                } else {
                                    json!({ "content": content })
                                },
                                id: Some(tool_use_id.clone()),
                            }),
                            ..Default::default()
                        });
                    }
                    ContentBlock::Image { media_type, data } => {
                        parts.push(GeminiPart {
                            inline_data: Some(InlineData {
                                mime_type: media_type.clone(),
                                data: data.clone(),
                            }),
                            ..Default::default()
                        });
                    }
                    ContentBlock::OpenAICompaction { .. }
                    | ContentBlock::ToolReference { .. }
                    | ContentBlock::ProviderNative { .. } => {}
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(GeminiContent {
                    role: role.to_string(),
                    parts,
                })
            }
        })
        .collect()
}

fn tool_name_from_tool_result(tool_use_id: &str, messages: &[Message]) -> String {
    for message in messages.iter().rev() {
        for block in &message.content {
            if let ContentBlock::ToolUse { id, name, .. } = block
                && id == tool_use_id
            {
                return name.clone();
            }
        }
    }
    "tool".to_string()
}

pub fn build_tools(tools: &[ToolDefinition]) -> Option<Vec<GeminiTool>> {
    if tools.is_empty() {
        return None;
    }

    Some(vec![GeminiTool {
        function_declarations: tools
            .iter()
            .map(|tool| GeminiFunctionDeclaration {
                name: tool.name.clone(),
                // Prompt-visible. Approximate token cost for this field:
                // tool.description_token_estimate().
                description: tool.description.clone(),
                parameters: gemini_compatible_schema(&tool.input_schema),
            })
            .collect(),
    }])
}

/// Normalize a tool-parameter schema for Gemini `generateContent`, which
/// accepts only an OpenAPI 3.0 subset and rejects the whole request with
/// HTTP 400 ("Unknown name ... Cannot find field") over a single draft-style
/// keyword anywhere in any tool.
///
/// The accepted subset, the recursion, and the structural rewrites (dangling
/// `required` pruning for #655, `const` -> `enum`) all live in
/// `jcode-schema-dialect` so that every provider shares one implementation and
/// one set of regression tests. See that crate's docs for why this is an
/// allow-list: a deny-list can only contain keywords that have already taken
/// the provider down for somebody.
pub fn gemini_compatible_schema(schema: &Value) -> Value {
    jcode_schema_dialect::normalize(schema, &jcode_schema_dialect::registry::GEMINI)
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiTool {
    #[serde(rename = "functionDeclarations")]
    pub function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiFunctionDeclaration {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiToolConfig {
    pub function_calling_config: GeminiFunctionCallingConfig,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiFunctionCallingConfig {
    pub mode: &'static str,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeAssistGenerateResponse {
    #[serde(default)]
    pub trace_id: Option<String>,
    #[serde(default)]
    pub response: Option<VertexGenerateContentResponse>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VertexGenerateContentResponse {
    /// Actual upstream model identifier, when supplied by the response.
    #[serde(default)]
    pub model_version: Option<String>,
    #[serde(default)]
    pub candidates: Option<Vec<GeminiCandidate>>,
    #[serde(default)]
    pub prompt_feedback: Option<GeminiPromptFeedback>,
    #[serde(default)]
    pub usage_metadata: Option<GeminiUsageMetadata>,
}

impl CodeAssistGenerateResponse {
    /// Never infer attribution from a requested model or a catalog alias.
    pub fn resolved_model_event(&self) -> Option<jcode_message_types::StreamEvent> {
        let model = self.response.as_ref()?.model_version.as_ref()?;
        if model.trim().is_empty() {
            return None;
        }
        Some(jcode_message_types::StreamEvent::ResolvedModel {
            model: model.clone(),
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiCandidate {
    #[serde(default)]
    pub content: Option<GeminiContent>,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub finish_message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiPromptFeedback {
    #[serde(default)]
    pub block_reason: Option<String>,
    #[serde(default)]
    pub block_reason_message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiUsageMetadata {
    #[serde(default)]
    pub prompt_token_count: Option<u64>,
    #[serde(default)]
    pub tool_use_prompt_token_count: Option<u64>,
    #[serde(default)]
    pub candidates_token_count: Option<u64>,
    #[serde(default)]
    pub thoughts_token_count: Option<u64>,
    #[serde(default)]
    pub cached_content_token_count: Option<u64>,
}

impl GeminiUsageMetadata {
    /// Tool-use prompts are input tokens, separate from the ordinary prompt.
    pub fn input_token_count(&self) -> Option<u64> {
        match (self.prompt_token_count, self.tool_use_prompt_token_count) {
            (Some(prompt), Some(tool_use)) => Some(prompt.saturating_add(tool_use)),
            (prompt, tool_use) => prompt.or(tool_use),
        }
    }

    /// Billable output includes candidates and thinking tokens.
    /// Do not infer output from totalTokenCount, which includes tool-use prompts.
    pub fn output_token_count(&self) -> Option<u64> {
        match (self.candidates_token_count, self.thoughts_token_count) {
            (Some(candidates), Some(thoughts)) => Some(candidates.saturating_add(thoughts)),
            (candidates, thoughts) => candidates.or(thoughts),
        }
    }
}

pub fn gemini_fallback_models(current_model: &str) -> Vec<&'static str> {
    FALLBACK_MODELS
        .iter()
        .copied()
        .filter(|candidate| !candidate.eq_ignore_ascii_case(current_model))
        .collect()
}

pub fn google_cloud_project_from_env() -> Option<String> {
    std::env::var("GOOGLE_CLOUD_PROJECT")
        .ok()
        .or_else(|| std::env::var("GOOGLE_CLOUD_PROJECT_ID").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn load_code_assist_request(
    project_id: Option<String>,
    metadata: ClientMetadata,
) -> LoadCodeAssistRequest {
    LoadCodeAssistRequest {
        cloudaicompanion_project: project_id,
        metadata,
        mode: None,
    }
}

pub fn merge_gemini_model_lists(models: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut preferred = Vec::new();

    for known in AVAILABLE_MODELS {
        if models.iter().any(|model| model == known) && seen.insert((*known).to_string()) {
            preferred.push((*known).to_string());
        }
    }

    let mut extras: Vec<String> = models
        .into_iter()
        .map(|model| model.trim().to_string())
        .filter(|model| is_gemini_model_id(model) && seen.insert(model.clone()))
        .collect();
    extras.sort();
    preferred.extend(extras);
    preferred
}

pub fn extract_gemini_model_ids(value: &Value) -> Vec<String> {
    let mut found = HashSet::new();
    collect_gemini_model_ids(value, &mut found);
    merge_gemini_model_lists(found.into_iter().collect())
}

fn collect_gemini_model_ids(value: &Value, found: &mut HashSet<String>) {
    match value {
        Value::String(raw) => {
            let trimmed = raw.trim();
            if is_gemini_model_id(trimmed) {
                found.insert(trimmed.to_string());
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_gemini_model_ids(item, found);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                collect_gemini_model_ids(item, found);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

pub fn is_gemini_model_id(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.starts_with("gemini-")
        && trimmed
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_'))
}

pub fn client_metadata(project_id: Option<String>) -> ClientMetadata {
    ClientMetadata {
        ide_type: "IDE_UNSPECIFIED",
        platform: "PLATFORM_UNSPECIFIED",
        plugin_type: "GEMINI",
        duet_project: project_id,
    }
}

pub fn validate_load_code_assist_response(res: &LoadCodeAssistResponse) -> Result<()> {
    if res.current_tier.is_none()
        && let Some(validation) = res.ineligible_tiers.as_ref().and_then(|tiers| {
            tiers.iter().find(|tier| {
                tier.reason_code.as_deref() == Some("VALIDATION_REQUIRED")
                    && tier.validation_url.is_some()
            })
        })
    {
        let description = validation
            .reason_message
            .clone()
            .unwrap_or_else(|| "Account validation required".to_string());
        let url = validation.validation_url.clone().unwrap_or_default();
        anyhow::bail!("{description}. Complete account validation: {url}");
    }
    Ok(())
}

/// Message shown when Google refuses Code Assist for individuals and points the
/// user at Antigravity instead.
pub const CODE_ASSIST_INDIVIDUALS_RETIRED_HINT: &str = "Google no longer serves Gemini Code Assist for individuals to third-party clients. Personal Google accounts (including Google AI Pro/Ultra) should run `jcode login --provider antigravity`. For Gemini API access, run `jcode login --provider gemini` and choose the API key option. Paid Code Assist (Standard/Enterprise) users can set GOOGLE_CLOUD_PROJECT.";

/// Detects the `UNSUPPORTED_CLIENT` rejection of the individuals free tier.
/// Without an explicit Cloud project, the remaining paid tier cannot be
/// onboarded, so fail early with a helpful hint instead of an opaque error.
pub fn retired_individual_tier_error(
    res: &LoadCodeAssistResponse,
    has_cloud_project: bool,
) -> Option<anyhow::Error> {
    if res.current_tier.is_some() || has_cloud_project {
        return None;
    }
    let tier = res
        .ineligible_tiers
        .as_ref()?
        .iter()
        .find(|tier| tier.reason_code.as_deref() == Some("UNSUPPORTED_CLIENT"))?;
    let reason = tier
        .reason_message
        .as_deref()
        .unwrap_or("This client is no longer supported for Gemini Code Assist for individuals");
    Some(anyhow::anyhow!(
        "{reason}\n{CODE_ASSIST_INDIVIDUALS_RETIRED_HINT}"
    ))
}

pub fn ineligible_or_project_error(res: &LoadCodeAssistResponse) -> anyhow::Error {
    if let Some(reasons) = res
        .ineligible_tiers
        .as_ref()
        .filter(|tiers| !tiers.is_empty())
    {
        let joined = reasons
            .iter()
            .filter_map(|tier| tier.reason_message.as_deref())
            .collect::<Vec<_>>()
            .join(", ");
        return anyhow::anyhow!(joined);
    }

    anyhow::anyhow!(
        "This Google account requires setting GOOGLE_CLOUD_PROJECT or GOOGLE_CLOUD_PROJECT_ID. See Gemini Code Assist Workspace auth docs."
    )
}

pub fn choose_onboard_tier(res: &LoadCodeAssistResponse) -> GeminiUserTier {
    if let Some(default_tier) = res.allowed_tiers.as_ref().and_then(|tiers| {
        tiers
            .iter()
            .find(|tier| tier.is_default.unwrap_or(false))
            .cloned()
    }) {
        return default_tier;
    }

    GeminiUserTier {
        id: Some(USER_TIER_LEGACY.to_string()),
        name: Some(String::new()),
        is_default: None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn unsupported_client_individual_tier_points_to_antigravity() {
        // Shape observed live from cloudcode-pa on 2026-11-21.
        let res: super::LoadCodeAssistResponse = serde_json::from_value(serde_json::json!({
            "allowedTiers": [{"id": "standard-tier", "isDefault": true}],
            "ineligibleTiers": [{
                "reasonCode": "UNSUPPORTED_CLIENT",
                "reasonMessage": "This client is no longer supported.",
                "tierId": "free-tier"
            }]
        }))
        .unwrap();
        let err = super::retired_individual_tier_error(&res, false)
            .expect("retired tier detected")
            .to_string();
        assert!(err.contains("no longer supported"));
        assert!(err.contains("--provider antigravity"));
        assert!(super::retired_individual_tier_error(&res, true).is_none());

        let onboarded: super::LoadCodeAssistResponse = serde_json::from_value(
            serde_json::json!({"currentTier": {"id": "standard-tier"}, "ineligibleTiers": [{"reasonCode": "UNSUPPORTED_CLIENT"}]}),
        )
        .unwrap();
        assert!(super::retired_individual_tier_error(&onboarded, false).is_none());
    }

    #[test]
    fn resolved_model_only_uses_returned_model_version() {
        use jcode_message_types::StreamEvent;
        let response: super::CodeAssistGenerateResponse = serde_json::from_value(
            serde_json::json!({"response": {"modelVersion": "gemini-actual-001"}}),
        )
        .unwrap();
        assert!(matches!(response.resolved_model_event(),
            Some(StreamEvent::ResolvedModel { model }) if model == "gemini-actual-001"));
        for value in [
            serde_json::json!({}),
            serde_json::json!({"response": {}}),
            serde_json::json!({"response": {"model": "request-alias"}}),
            serde_json::json!({"response": {"modelVersion": null}}),
            serde_json::json!({"response": {"modelVersion": " "}}),
        ] {
            let response: super::CodeAssistGenerateResponse =
                serde_json::from_value(value).unwrap();
            assert!(response.resolved_model_event().is_none());
        }
    }

    use super::*;
    use serde_json::json;

    #[test]
    fn response_usage_counts_tool_use_prompts_without_inventing_input() {
        for (metadata, expected) in [
            (json!({}), None),
            (json!({"totalTokenCount": 99}), None),
            (
                json!({"promptTokenCount": null, "toolUsePromptTokenCount": null}),
                None,
            ),
            (json!({"promptTokenCount": 0}), Some(0)),
            (json!({"toolUsePromptTokenCount": 0}), Some(0)),
            (json!({"promptTokenCount": 10}), Some(10)),
            (json!({"toolUsePromptTokenCount": 90}), Some(90)),
            (
                json!({"promptTokenCount": 10, "toolUsePromptTokenCount": 90, "cachedContentTokenCount": 4}),
                Some(100),
            ),
            (
                json!({"promptTokenCount": u64::MAX, "toolUsePromptTokenCount": 1}),
                Some(u64::MAX),
            ),
        ] {
            let response: VertexGenerateContentResponse = serde_json::from_value(json!({
                "usageMetadata": metadata
            }))
            .unwrap();
            let usage = response.usage_metadata.unwrap();
            assert_eq!(usage.input_token_count(), expected, "{metadata}");
            assert_eq!(
                usage.cached_content_token_count,
                metadata["cachedContentTokenCount"].as_u64()
            );
        }
    }

    #[test]
    fn response_usage_counts_thinking_without_inventing_output() {
        for (metadata, expected) in [
            (json!({"thoughtsTokenCount": 13}), Some(13)),
            (
                json!({"candidatesTokenCount": 7, "thoughtsTokenCount": 13}),
                Some(20),
            ),
            (json!({"candidatesTokenCount": 7}), Some(7)),
            (json!({}), None),
            (
                json!({"candidatesTokenCount": null, "thoughtsTokenCount": null}),
                None,
            ),
            (json!({"thoughtsTokenCount": 0}), Some(0)),
            (json!({"candidatesTokenCount": 0}), Some(0)),
            (
                json!({"candidatesTokenCount": 0, "thoughtsTokenCount": 0}),
                Some(0),
            ),
            (
                json!({"candidatesTokenCount": u64::MAX, "thoughtsTokenCount": 1}),
                Some(u64::MAX),
            ),
            (
                json!({"promptTokenCount": 10, "totalTokenCount": 100, "toolUsePromptTokenCount": 90}),
                None,
            ),
            (
                json!({"candidatesTokenCount": 7, "thoughtsTokenCount": 13, "totalTokenCount": 120, "toolUsePromptTokenCount": 90}),
                Some(20),
            ),
        ] {
            let response: VertexGenerateContentResponse = serde_json::from_value(json!({
                "usageMetadata": metadata
            }))
            .unwrap();
            assert_eq!(
                response.usage_metadata.unwrap().output_token_count(),
                expected,
                "{metadata}"
            );
        }
        let response: VertexGenerateContentResponse = serde_json::from_value(json!({})).unwrap();
        assert!(response.usage_metadata.is_none());
    }

    #[test]
    fn compatible_schema_strips_nested_property_names() {
        let schema = json!({
            "type": "object",
            "properties": {
                "data": {
                    "type": "object",
                    "propertyNames": { "type": "string" },
                    "additionalProperties": { "type": "string" }
                }
            }
        });
        let out = gemini_compatible_schema(&schema);
        assert!(out["properties"]["data"].get("propertyNames").is_none());
        // Existing Gemini normalization still drops additionalProperties.
        assert!(
            out["properties"]["data"]
                .get("additionalProperties")
                .is_none()
        );
    }

    #[test]
    fn fallback_models_skip_current_model() {
        assert_eq!(
            gemini_fallback_models("gemini-2.5-flash"),
            vec![
                "gemini-pro-latest",
                "gemini-3.1-pro-preview",
                "gemini-3-pro-preview",
                "gemini-2.5-pro",
                "gemini-3-flash-preview",
                "gemini-2.0-flash",
            ]
        );
    }

    #[test]
    fn extract_gemini_model_ids_discovers_nested_models() {
        let response = json!({
            "routing": {
                "manual": {
                    "models": [
                        {"id": "gemini-3-pro-preview"},
                        {"name": "gemini-3.1-pro-preview"}
                    ]
                },
                "auto": ["gemini-3-flash-preview", "not-a-model"]
            }
        });

        assert_eq!(
            extract_gemini_model_ids(&response),
            vec![
                "gemini-3.1-pro-preview".to_string(),
                "gemini-3-pro-preview".to_string(),
                "gemini-3-flash-preview".to_string(),
            ]
        );
    }

    #[test]
    fn candidate_content_decodes_without_role() {
        // Antigravity/Cloud Code Gemini-3 responses occasionally omit `role` on
        // a candidate's `content` (and sometimes `parts` entirely). The whole
        // generateContent decode used to fail with "missing field `role`",
        // which aborted the turn; assert the response now decodes and the
        // function call survives.
        let raw = json!({
            "response": {
                "candidates": [{
                    "content": {
                        "parts": [{
                            "functionCall": {"name": "read", "args": {"file_path": "/tmp/x"}},
                            "thoughtSignature": "SIG_XYZ"
                        }]
                    },
                    "finishReason": "STOP"
                }]
            }
        })
        .to_string();

        let decoded: CodeAssistGenerateResponse =
            serde_json::from_str(&raw).expect("decode response with role-less content");
        let candidates = decoded.response.unwrap().candidates.unwrap();
        let part = &candidates[0].content.as_ref().unwrap().parts[0];
        assert_eq!(part.function_call.as_ref().unwrap().name, "read");
        assert_eq!(part.thought_signature.as_deref(), Some("SIG_XYZ"));
    }

    #[test]
    fn candidate_content_decodes_without_parts() {
        // A bare `content: {}` (no `role`, no `parts`) must not abort the decode.
        let raw = json!({
            "response": {
                "candidates": [{ "content": {}, "finishReason": "STOP" }]
            }
        })
        .to_string();

        let decoded: CodeAssistGenerateResponse =
            serde_json::from_str(&raw).expect("decode response with empty content");
        let content = decoded.response.unwrap().candidates.unwrap()[0]
            .content
            .clone()
            .unwrap();
        assert!(content.role.is_empty());
        assert!(content.parts.is_empty());
    }
}
