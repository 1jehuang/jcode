pub mod claude;
pub mod openai;

use crate::auth;
use crate::message::{Message, StreamEvent, ToolDefinition};
use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;
use std::sync::RwLock;

/// Stream of events from a provider
pub type EventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>;

/// Provider trait for LLM backends
#[async_trait]
pub trait Provider: Send + Sync {
    /// Send messages and get a streaming response
    /// resume_session_id: Optional session ID to resume a previous conversation (provider-specific)
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream>;

    /// Get the provider name
    fn name(&self) -> &str;

    /// Get the model identifier being used
    fn model(&self) -> String {
        "unknown".to_string()
    }

    /// Set the model to use (returns error if model not supported)
    fn set_model(&self, _model: &str) -> Result<()> {
        Err(anyhow::anyhow!(
            "This provider does not support model switching"
        ))
    }

    /// List available models for this provider
    fn available_models(&self) -> Vec<&'static str> {
        vec![]
    }

    /// Get the reasoning effort level (if applicable, e.g., OpenAI)
    fn reasoning_effort(&self) -> Option<String> {
        None
    }

    /// Returns true if the provider executes tools internally (e.g., Claude Agent SDK).
    /// When true, jcode should NOT execute tools locally - just record the tool calls.
    fn handles_tools_internally(&self) -> bool {
        false
    }
}

/// Available models (shown in /model list)
pub const ALL_CLAUDE_MODELS: &[&str] = &["claude-opus-4-5-20251101"];

pub const ALL_OPENAI_MODELS: &[&str] = &["gpt-5.2-codex"];

/// Detect which provider a model belongs to
pub fn provider_for_model(model: &str) -> Option<&'static str> {
    if ALL_CLAUDE_MODELS.contains(&model) {
        Some("claude")
    } else if ALL_OPENAI_MODELS.contains(&model) {
        Some("openai")
    } else {
        None
    }
}

/// MultiProvider wraps multiple providers and allows seamless model switching
pub struct MultiProvider {
    claude: Option<claude::ClaudeProvider>,
    openai: Option<openai::OpenAIProvider>,
    active: RwLock<ActiveProvider>,
    has_claude_creds: bool,
    has_openai_creds: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum ActiveProvider {
    Claude,
    OpenAI,
}

impl MultiProvider {
    /// Create a new MultiProvider, detecting available credentials
    pub fn new() -> Self {
        let has_claude_creds = auth::claude::load_credentials().is_ok();
        let has_openai_creds = auth::codex::load_credentials().is_ok();

        // Initialize providers based on available credentials
        let claude = if has_claude_creds {
            Some(claude::ClaudeProvider::new())
        } else {
            None
        };

        let openai = if has_openai_creds {
            auth::codex::load_credentials()
                .ok()
                .map(openai::OpenAIProvider::new)
        } else {
            None
        };

        // Default to Claude if available, otherwise OpenAI
        let active = if claude.is_some() {
            ActiveProvider::Claude
        } else if openai.is_some() {
            ActiveProvider::OpenAI
        } else {
            // No credentials - default to Claude (will fail on use)
            ActiveProvider::Claude
        };

        Self {
            claude,
            openai,
            active: RwLock::new(active),
            has_claude_creds,
            has_openai_creds,
        }
    }

    /// Create with explicit initial provider preference
    pub fn with_preference(prefer_openai: bool) -> Self {
        let provider = Self::new();
        if prefer_openai && provider.openai.is_some() {
            *provider.active.write().unwrap() = ActiveProvider::OpenAI;
        }
        provider
    }

    fn active_provider(&self) -> ActiveProvider {
        *self.active.read().unwrap()
    }
}

impl Default for MultiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for MultiProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        match self.active_provider() {
            ActiveProvider::Claude => {
                if let Some(ref claude) = self.claude {
                    claude
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "Claude credentials not available. Run `claude` to log in."
                    ))
                }
            }
            ActiveProvider::OpenAI => {
                if let Some(ref openai) = self.openai {
                    openai
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!("OpenAI credentials not available. Run `jcode login --provider openai` to log in."))
                }
            }
        }
    }

    fn name(&self) -> &str {
        match self.active_provider() {
            ActiveProvider::Claude => "Claude",
            ActiveProvider::OpenAI => "OpenAI",
        }
    }

    fn model(&self) -> String {
        match self.active_provider() {
            ActiveProvider::Claude => self
                .claude
                .as_ref()
                .map(|c| c.model())
                .unwrap_or_else(|| "claude-opus-4-5-20251101".to_string()),
            ActiveProvider::OpenAI => self
                .openai
                .as_ref()
                .map(|o| o.model())
                .unwrap_or_else(|| "gpt-5.2-codex".to_string()),
        }
    }

    fn set_model(&self, model: &str) -> Result<()> {
        // Detect which provider this model belongs to
        let target_provider = provider_for_model(model);

        if target_provider == Some("claude") {
            if self.claude.is_none() {
                return Err(anyhow::anyhow!(
                    "Claude credentials not available. Run `claude` to log in first."
                ));
            }
            // Switch active provider to Claude
            *self.active.write().unwrap() = ActiveProvider::Claude;
            if let Some(ref claude) = self.claude {
                claude.set_model(model)
            } else {
                Ok(())
            }
        } else if target_provider == Some("openai") {
            if self.openai.is_none() {
                return Err(anyhow::anyhow!(
                    "OpenAI credentials not available. Run `jcode login --provider openai` first."
                ));
            }
            // Switch active provider to OpenAI
            *self.active.write().unwrap() = ActiveProvider::OpenAI;
            if let Some(ref openai) = self.openai {
                openai.set_model(model)
            } else {
                Ok(())
            }
        } else {
            // Unknown model - try current provider
            match self.active_provider() {
                ActiveProvider::Claude => {
                    if let Some(ref claude) = self.claude {
                        claude.set_model(model)
                    } else {
                        Err(anyhow::anyhow!("Unknown model: {}", model))
                    }
                }
                ActiveProvider::OpenAI => {
                    if let Some(ref openai) = self.openai {
                        openai.set_model(model)
                    } else {
                        Err(anyhow::anyhow!("Unknown model: {}", model))
                    }
                }
            }
        }
    }

    fn available_models(&self) -> Vec<&'static str> {
        let mut models = Vec::new();
        models.extend_from_slice(ALL_CLAUDE_MODELS);
        models.extend_from_slice(ALL_OPENAI_MODELS);
        models
    }

    fn handles_tools_internally(&self) -> bool {
        match self.active_provider() {
            ActiveProvider::Claude => self
                .claude
                .as_ref()
                .map(|c| c.handles_tools_internally())
                .unwrap_or(false),
            ActiveProvider::OpenAI => self
                .openai
                .as_ref()
                .map(|o| o.handles_tools_internally())
                .unwrap_or(false),
        }
    }

    fn reasoning_effort(&self) -> Option<String> {
        match self.active_provider() {
            ActiveProvider::Claude => None,
            ActiveProvider::OpenAI => self.openai.as_ref().and_then(|o| o.reasoning_effort()),
        }
    }
}
