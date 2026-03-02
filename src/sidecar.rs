//! Lightweight sidecar client for fast, cheap model calls.
//!
//! Used for memory relevance verification and other quick tasks that don't
//! need the full Agent SDK infrastructure.
//!
//! Automatically selects the best available backend:
//! - OpenAI (gpt-5.3-codex-spark) if Codex credentials are available
//! - Claude (claude-haiku-4-5-20241022) if Claude credentials are available

use crate::auth;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Fast/cheap OpenAI model used when Codex credentials are available.
pub const SIDECAR_OPENAI_MODEL: &str = "gpt-5.3-codex-spark";
const SIDECAR_OPENAI_CHATGPT_MODEL: &str = "gpt-5.3-codex";

/// Fast/cheap Claude model used when only Claude credentials are available.
const SIDECAR_CLAUDE_MODEL: &str = "claude-haiku-4-5-20241022";

/// OpenAI Responses API
const OPENAI_API_BASE: &str = "https://api.openai.com/v1";
const CHATGPT_API_BASE: &str = "https://chatgpt.com/backend-api/codex";
const OPENAI_RESPONSES_PATH: &str = "responses";
const OPENAI_ORIGINATOR: &str = "codex_cli_rs";

/// Claude Messages API endpoint (with beta=true for OAuth)
const CLAUDE_API_URL: &str = "https://api.anthropic.com/v1/messages?beta=true";

/// User-Agent for OAuth requests (must match Claude CLI format)
const CLAUDE_CLI_USER_AGENT: &str = "claude-cli/1.0.0";

/// Beta headers required for OAuth
const OAUTH_BETA_HEADERS: &str = "oauth-2025-04-20,claude-code-20250219";

/// Claude Code identity block required for OAuth direct API access
const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const CLAUDE_CODE_JCODE_NOTICE: &str =
    "You are jcode, powered by Claude Code. You are a third-party CLI, not the official Claude Code CLI.";

/// Maximum tokens for sidecar responses (keep small for speed/cost)
const DEFAULT_MAX_TOKENS: u32 = 1024;

/// Which backend the sidecar is using
#[derive(Debug, Clone, Copy, PartialEq)]
enum SidecarBackend {
    OpenAI,
    Claude,
}

/// Lightweight client for fast sidecar calls
#[derive(Clone)]
pub struct Sidecar {
    client: reqwest::Client,
    model: String,
    max_tokens: u32,
    backend: SidecarBackend,
}

impl Sidecar {
    /// Create a new sidecar client, auto-selecting the best available backend.
    /// Prefers OpenAI (codex-spark) if creds exist, falls back to Claude.
    pub fn new() -> Self {
        let (backend, model) = if auth::codex::load_credentials().is_ok() {
            (SidecarBackend::OpenAI, SIDECAR_OPENAI_MODEL.to_string())
        } else if auth::claude::load_credentials().is_ok() {
            (SidecarBackend::Claude, SIDECAR_CLAUDE_MODEL.to_string())
        } else {
            // Default to Claude - will fail on use with a clear error
            (SidecarBackend::Claude, SIDECAR_CLAUDE_MODEL.to_string())
        };

        Self {
            client: crate::provider::shared_http_client(),
            model,
            max_tokens: DEFAULT_MAX_TOKENS,
            backend,
        }
    }

    /// Set custom max tokens
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Simple completion - send a prompt, get a response.
    /// Routes to the correct API based on the detected backend.
    pub async fn complete(&self, system: &str, user_message: &str) -> Result<String> {
        match self.backend {
            SidecarBackend::OpenAI => self.complete_openai(system, user_message).await,
            SidecarBackend::Claude => self.complete_claude(system, user_message).await,
        }
    }

    /// Complete via OpenAI Responses API.
    ///
    /// - Direct API key mode: non-streaming, simple JSON response.
    /// - ChatGPT OAuth mode: streaming SSE (required by chatgpt.com endpoint).
    async fn complete_openai(&self, system: &str, user_message: &str) -> Result<String> {
        let creds = auth::codex::load_credentials()
            .context("Failed to load OpenAI/Codex credentials for sidecar")?;

        let is_chatgpt_mode = !creds.refresh_token.is_empty() || creds.id_token.is_some();
        let base = if is_chatgpt_mode {
            CHATGPT_API_BASE
        } else {
            OPENAI_API_BASE
        };
        let url = format!("{}/{}", base.trim_end_matches('/'), OPENAI_RESPONSES_PATH);

        let model = if is_chatgpt_mode && self.model == SIDECAR_OPENAI_MODEL {
            SIDECAR_OPENAI_CHATGPT_MODEL
        } else {
            &self.model
        };

        let mut instructions = String::new();
        if !system.is_empty() {
            instructions.push_str(system);
        }

        // ChatGPT endpoint requires stream:true; direct API supports stream:false.
        let request = serde_json::json!({
            "model": model,
            "instructions": instructions,
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": user_message,
                }],
            }],
            "stream": is_chatgpt_mode,
            "store": false,
        });

        let mut builder = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", creds.access_token))
            .header("Content-Type", "application/json");

        if is_chatgpt_mode {
            builder = builder.header("originator", OPENAI_ORIGINATOR);
            if let Some(ref account_id) = creds.account_id {
                builder = builder.header("chatgpt-account-id", account_id);
            }
        }

        let response = builder
            .json(&request)
            .send()
            .await
            .context("Failed to send request to OpenAI API")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI API error ({}): {}", status, error_text);
        }

        if is_chatgpt_mode {
            collect_openai_sse_text(response).await
        } else {
            let result: serde_json::Value = response
                .json()
                .await
                .context("Failed to parse OpenAI API response")?;
            extract_openai_response_text(&result)
        }
    }

    /// Complete via Claude Messages API
    async fn complete_claude(&self, system: &str, user_message: &str) -> Result<String> {
        let creds = auth::claude::load_credentials()
            .context("Failed to load Claude credentials for sidecar")?;

        let request = ClaudeMessagesRequest {
            model: &self.model,
            max_tokens: self.max_tokens,
            system: build_claude_system_param(system),
            messages: vec![ClaudeMessage {
                role: "user",
                content: user_message,
            }],
        };

        let response = self
            .client
            .post(CLAUDE_API_URL)
            .header("Authorization", format!("Bearer {}", creds.access_token))
            .header("User-Agent", CLAUDE_CLI_USER_AGENT)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", OAUTH_BETA_HEADERS)
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send request to Claude API")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Claude API error ({}): {}", status, error_text);
        }

        let result: ClaudeMessagesResponse = response
            .json()
            .await
            .context("Failed to parse Claude API response")?;

        let text = result
            .content
            .into_iter()
            .filter_map(|block| {
                if let ClaudeContentBlock::Text { text } = block {
                    Some(text)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");

        Ok(text)
    }

    /// Check if a memory is relevant to the current context
    /// Returns (is_relevant, explanation)
    pub async fn check_relevance(
        &self,
        memory_content: &str,
        current_context: &str,
    ) -> Result<(bool, String)> {
        let system = r#"You are a memory relevance checker. Your job is to determine if a stored memory is relevant to the current context.

Respond in this exact format:
RELEVANT: yes/no
REASON: <brief explanation>

Be conservative - only say "yes" if the memory would actually be useful for the current task."#;

        let prompt = format!(
            "## Stored Memory\n{}\n\n## Current Context\n{}\n\nIs this memory relevant to the current context?",
            memory_content, current_context
        );

        let response = self.complete(system, &prompt).await?;

        // Parse response
        let mut is_relevant = false;
        for line in response.lines() {
            let line = line.trim();
            if line.len() >= 9 && line[..9].eq_ignore_ascii_case("relevant:") {
                let value = line[9..].trim();
                is_relevant = value.eq_ignore_ascii_case("yes") || value.starts_with("yes");
                break;
            }
        }
        let reason = response
            .lines()
            .find(|line| line.to_lowercase().starts_with("reason:"))
            .map(|line| line.trim_start_matches(|c: char| !c.is_alphabetic()).trim())
            .unwrap_or(&response)
            .to_string();

        Ok((is_relevant, reason))
    }

    /// Check if new information contradicts existing information
    /// Returns true if the two statements are contradictory
    pub async fn check_contradiction(
        &self,
        new_content: &str,
        existing_content: &str,
    ) -> Result<bool> {
        let system = "You are a contradiction detector. Given two statements, determine if the new information directly contradicts the existing information. Reply with exactly YES or NO.";

        let prompt = format!(
            "## Existing Information\n{}\n\n## New Information\n{}\n\nDoes the new information contradict the existing information?",
            existing_content, new_content
        );

        let response = self.complete(system, &prompt).await?;
        let trimmed = response.trim().to_uppercase();
        Ok(trimmed.starts_with("YES"))
    }

    /// Extract memories from a session transcript
    pub async fn extract_memories(&self, transcript: &str) -> Result<Vec<ExtractedMemory>> {
        self.extract_memories_with_existing(transcript, &[]).await
    }

    /// Extract memories from a session transcript, aware of what's already stored.
    pub async fn extract_memories_with_existing(
        &self,
        transcript: &str,
        existing: &[String],
    ) -> Result<Vec<ExtractedMemory>> {
        let mut system = String::from(
            r#"You are a memory extraction assistant. Extract important NEW learnings from the conversation that should be remembered for future sessions.

Categories (use EXACTLY one of these):
- fact: Technical facts about the codebase, architecture, patterns, dependencies, tools, environment
- preference: User preferences, workflow habits, UX expectations, coding style, conventions, how they want the assistant to behave
- correction: Mistakes that were corrected, bugs found and fixed, wrong assumptions, things the user corrected
- entity: Named entities worth tracking - people, projects, services, repos, teams

Categorization rules:
- If it describes what the USER WANTS or HOW THEY LIKE THINGS, it is "preference", not "fact"
- If it describes a BUG FIX or MISTAKE, it is "correction", not "fact"
- "fact" is for objective technical information about code/systems, not user behavior
- Do NOT extract transient debugging details, compile errors, or intermediate steps

For each memory, output in this format (one per line):
CATEGORY|CONTENT|TRUST

Where:
- CATEGORY is one of: fact, preference, correction, entity
- CONTENT is a concise statement (1-2 sentences max)
- TRUST is one of: high (user stated), medium (observed), low (inferred)

Output ONLY the formatted lines, no other text. If no NEW memories worth extracting, output nothing."#,
        );

        if !existing.is_empty() {
            system.push_str("\n\nAlready known (do NOT re-extract these):\n");
            for mem in existing.iter().take(40) {
                system.push_str("- ");
                system.push_str(mem);
                system.push('\n');
            }
        }

        let response = self.complete(&system, transcript).await?;

        let memories = response
            .lines()
            .filter(|line| line.contains('|'))
            .filter_map(|line| {
                let parts: Vec<&str> = line.split('|').collect();
                if parts.len() >= 3 {
                    Some(ExtractedMemory {
                        category: parts[0].trim().to_lowercase(),
                        content: parts[1].trim().to_string(),
                        trust: parts[2].trim().to_lowercase(),
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(memories)
    }
}

impl Default for Sidecar {
    fn default() -> Self {
        Self::new()
    }
}

/// The public model constant for backward compatibility
pub const SIDECAR_FAST_MODEL: &str = SIDECAR_OPENAI_MODEL;

/// A memory extracted by the sidecar
#[derive(Debug, Clone)]
pub struct ExtractedMemory {
    pub category: String,
    pub content: String,
    pub trust: String,
}

/// Collect text from an OpenAI Responses API SSE stream.
///
/// Parses `data: <json>` lines and accumulates text deltas from
/// `response.output_text.delta` events, stopping on completion/done.
async fn collect_openai_sse_text(response: reqwest::Response) -> Result<String> {
    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    let mut text = String::new();
    let mut buf = String::new();

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.context("Error reading SSE stream")?;
        buf.push_str(&String::from_utf8_lossy(&bytes));

        // Process all complete lines in the buffer
        while let Some(newline_pos) = buf.find('\n') {
            let line = buf[..newline_pos].trim_end_matches('\r').to_string();
            buf = buf[newline_pos + 1..].to_string();

            if let Some(data) = line.strip_prefix("data: ") {
                if data == "[DONE]" {
                    return Ok(text);
                }
                if let Ok(event) = serde_json::from_str::<SseEvent>(data) {
                    match event.kind.as_str() {
                        "response.output_text.delta" => {
                            if let Some(delta) = event.delta {
                                text.push_str(&delta);
                            }
                        }
                        "response.completed" | "response.incomplete" => {
                            return Ok(text);
                        }
                        "response.failed" | "error" => {
                            let msg = event
                                .error
                                .as_ref()
                                .and_then(|e| e.as_str())
                                .unwrap_or("unknown error");
                            anyhow::bail!("OpenAI SSE error: {}", msg);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(text)
}

/// Extract text from a non-streaming OpenAI Responses API JSON response.
fn extract_openai_response_text(result: &serde_json::Value) -> Result<String> {
    let mut text = String::new();
    if let Some(output) = result.get("output").and_then(|v| v.as_array()) {
        for item in output {
            let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if item_type == "message" {
                if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
                    for block in content {
                        let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        if block_type == "output_text" || block_type == "text" {
                            if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                                text.push_str(t);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(text)
}

#[derive(Deserialize)]
struct SseEvent {
    #[serde(rename = "type")]
    kind: String,
    delta: Option<String>,
    error: Option<serde_json::Value>,
}

// Claude API types

#[derive(Serialize)]
struct ClaudeMessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<ClaudeApiSystem<'a>>,
    messages: Vec<ClaudeMessage<'a>>,
}

#[derive(Serialize)]
struct ClaudeMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
#[serde(untagged)]
enum ClaudeApiSystem<'a> {
    #[allow(dead_code)]
    Text(&'a str),
    Blocks(Vec<ClaudeApiSystemBlock<'a>>),
}

#[derive(Serialize)]
struct ClaudeApiSystemBlock<'a> {
    #[serde(rename = "type")]
    block_type: &'static str,
    text: &'a str,
}

fn build_claude_system_param(system: &str) -> Option<ClaudeApiSystem<'_>> {
    let mut blocks = Vec::new();
    blocks.push(ClaudeApiSystemBlock {
        block_type: "text",
        text: CLAUDE_CODE_IDENTITY,
    });
    blocks.push(ClaudeApiSystemBlock {
        block_type: "text",
        text: CLAUDE_CODE_JCODE_NOTICE,
    });
    if !system.is_empty() {
        blocks.push(ClaudeApiSystemBlock {
            block_type: "text",
            text: system,
        });
    }
    Some(ClaudeApiSystem::Blocks(blocks))
}

#[derive(Deserialize)]
struct ClaudeMessagesResponse {
    content: Vec<ClaudeContentBlock>,
    #[allow(dead_code)]
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClaudeContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct ClaudeUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sidecar_fast_model() {
        assert_eq!(SIDECAR_FAST_MODEL, "gpt-5.3-codex-spark");
    }

    #[test]
    fn test_backend_selection_prefers_openai() {
        // OpenAI is preferred when creds exist (both direct API and ChatGPT mode).
        // ChatGPT mode uses streaming SSE; direct API uses non-streaming JSON.
        let has_openai = crate::auth::codex::load_credentials().is_ok();
        let has_claude = crate::auth::claude::load_credentials().is_ok();

        let sidecar = Sidecar::new();
        if has_openai {
            assert_eq!(sidecar.backend, SidecarBackend::OpenAI);
            assert_eq!(sidecar.model, SIDECAR_OPENAI_MODEL);
        } else if has_claude {
            assert_eq!(sidecar.backend, SidecarBackend::Claude);
            assert_eq!(sidecar.model, SIDECAR_CLAUDE_MODEL);
        }
    }
}
