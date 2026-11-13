use anyhow::Result;
use jcode_message_types::{ContentBlock, Message, ToolDefinition};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

mod normalize;

#[cfg(test)]
mod tests;

pub use normalize::{
    SignaturePolicy, build_contents, build_contents_with_signature_policy,
    is_missing_thought_signature_error,
};

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
    #[serde(default)]
    pub candidates: Option<Vec<GeminiCandidate>>,
    #[serde(default)]
    pub prompt_feedback: Option<GeminiPromptFeedback>,
    #[serde(default)]
    pub usage_metadata: Option<GeminiUsageMetadata>,
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
