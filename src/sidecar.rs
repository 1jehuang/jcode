//! Lightweight sidecar client for fast, cheap model calls (Haiku 4.5)
//!
//! Used for memory relevance verification and other quick tasks that don't
//! need the full Agent SDK infrastructure.

use crate::auth;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Haiku 3.5 model identifier (works with OAuth)
pub const HAIKU_MODEL: &str = "claude-3-5-haiku-20241022";

/// Claude Messages API endpoint (with beta=true for OAuth)
const CLAUDE_API_URL: &str = "https://api.anthropic.com/v1/messages?beta=true";

/// User-Agent for OAuth requests (must match Claude CLI format)
const CLAUDE_CLI_USER_AGENT: &str = "claude-cli/1.0.0";

/// Beta headers required for OAuth
const OAUTH_BETA_HEADERS: &str = "oauth-2025-04-20,claude-code-20250219";

/// Claude Code identity block required for OAuth direct API access
const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

/// Maximum tokens for sidecar responses (keep small for speed/cost)
const DEFAULT_MAX_TOKENS: u32 = 1024;

/// Lightweight client for Haiku sidecar calls
#[derive(Clone)]
pub struct HaikuSidecar {
    client: reqwest::Client,
    model: String,
    max_tokens: u32,
}

impl HaikuSidecar {
    /// Create a new Haiku sidecar client
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            model: HAIKU_MODEL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    /// Set custom max tokens
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Simple completion - send a prompt, get a response
    pub async fn complete(&self, system: &str, user_message: &str) -> Result<String> {
        let creds = auth::claude::load_credentials()
            .context("Failed to load Claude credentials for sidecar")?;

        let request = MessagesRequest {
            model: &self.model,
            max_tokens: self.max_tokens,
            system: build_system_param(system),
            messages: vec![Message {
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

        let result: MessagesResponse = response
            .json()
            .await
            .context("Failed to parse Claude API response")?;

        // Extract text from response
        let text = result
            .content
            .into_iter()
            .filter_map(|block| {
                if let ContentBlock::Text { text } = block {
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

/// A memory extracted by the sidecar
#[derive(Debug, Clone)]
pub struct ExtractedMemory {
    pub category: String,
    pub content: String,
    pub trust: String,
}

// API types

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<ApiSystem<'a>>,
    messages: Vec<Message<'a>>,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
#[serde(untagged)]
enum ApiSystem<'a> {
    Text(&'a str),
    Blocks(Vec<ApiSystemBlock<'a>>),
}

#[derive(Serialize)]
struct ApiSystemBlock<'a> {
    #[serde(rename = "type")]
    block_type: &'static str,
    text: &'a str,
}

fn build_system_param(system: &str) -> Option<ApiSystem<'_>> {
    let mut blocks = Vec::new();
    blocks.push(ApiSystemBlock {
        block_type: "text",
        text: CLAUDE_CODE_IDENTITY,
    });
    if !system.is_empty() {
        blocks.push(ApiSystemBlock {
            block_type: "text",
            text: system,
        });
    }
    Some(ApiSystem::Blocks(blocks))
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
    #[allow(dead_code)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct Usage {
    input_tokens: u32,
    output_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_haiku_model_name() {
        assert!(HAIKU_MODEL.contains("haiku"));
    }
}
