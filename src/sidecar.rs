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

/// Preferred sidecar model (fast + cheap) when OpenAI creds are available.
pub const SIDECAR_OPENAI_MODEL: &str = "gpt-5.3-codex-spark";
const SIDECAR_OPENAI_CHATGPT_MODEL: &str = "gpt-5.3-codex";

/// Fallback sidecar model when only Claude creds are available.
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
pub struct HaikuSidecar {
    client: reqwest::Client,
    model: String,
    max_tokens: u32,
    backend: SidecarBackend,
}

impl HaikuSidecar {
    /// Create a new sidecar client, auto-selecting the best available backend.
    /// Prefers OpenAI (codex-spark) if creds exist and are in direct API mode.
    /// Falls back to Claude if OpenAI creds are ChatGPT mode (requires streaming).
    pub fn new() -> Self {
        let use_openai = if let Ok(creds) = auth::codex::load_credentials() {
            // ChatGPT mode (refresh_token or id_token) requires streaming, which
            // the sidecar doesn't support. Only use OpenAI for direct API keys.
            let is_chatgpt_mode = !creds.refresh_token.is_empty() || creds.id_token.is_some();
            !is_chatgpt_mode
        } else {
            false
        };

        let (backend, model) = if use_openai {
            (SidecarBackend::OpenAI, SIDECAR_OPENAI_MODEL.to_string())
        } else if auth::claude::load_credentials().is_ok() {
            (SidecarBackend::Claude, SIDECAR_CLAUDE_MODEL.to_string())
        } else {
            // Default to Claude - will fail on use with a clear error
            (SidecarBackend::Claude, SIDECAR_CLAUDE_MODEL.to_string())
        };

        Self {
            client: reqwest::Client::new(),
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

    /// Complete via OpenAI Responses API
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

        let mut instructions = String::new();
        if !system.is_empty() {
            instructions.push_str(system);
        }

        let request = serde_json::json!({
            "model": if is_chatgpt_mode && self.model == SIDECAR_OPENAI_MODEL {
                SIDECAR_OPENAI_CHATGPT_MODEL
            } else {
                &self.model
            },
            "instructions": instructions,
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": user_message,
                }],
            }],
            "stream": false,
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

        let result: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse OpenAI API response")?;

        // Extract text from Responses API output
        let mut text = String::new();
        if let Some(output) = result.get("output").and_then(|v| v.as_array()) {
            for item in output {
                let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if item_type == "message" {
                    if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
                        for block in content {
                            let block_type =
                                block.get("type").and_then(|v| v.as_str()).unwrap_or("");
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
        let system = r#"You are a memory extraction assistant. Extract important learnings from the conversation that should be remembered for future sessions.

Focus on:
1. Facts about the codebase (architecture, patterns, dependencies)
2. User preferences (coding style, conventions, tool preferences)
3. Corrections made by the user (things that were wrong)
4. Lessons learned from debugging or mistakes

For each memory, output in this format (one per line):
CATEGORY|CONTENT|TRUST

Where:
- CATEGORY is one of: fact, preference, correction, observation
- CONTENT is a concise statement (1-2 sentences max)
- TRUST is one of: high (user stated), medium (observed), low (inferred)

Output ONLY the formatted lines, no other text. If no memories worth extracting, output nothing."#;

        let response = self.complete(system, transcript).await?;

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

impl Default for HaikuSidecar {
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
        // If both creds exist, OpenAI should be preferred
        let has_openai = crate::auth::codex::load_credentials().is_ok();
        let has_claude = crate::auth::claude::load_credentials().is_ok();

        let sidecar = HaikuSidecar::new();
        if has_openai {
            assert_eq!(sidecar.backend, SidecarBackend::OpenAI);
            assert_eq!(sidecar.model, SIDECAR_OPENAI_MODEL);
        } else if has_claude {
            assert_eq!(sidecar.backend, SidecarBackend::Claude);
            assert_eq!(sidecar.model, SIDECAR_CLAUDE_MODEL);
        }
    }
}
