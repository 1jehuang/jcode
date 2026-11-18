use super::*;

#[cfg(test)]
#[path = "dispatch_estimation_tests.rs"]
mod estimation_tests;

#[derive(Clone, Copy)]
pub(super) enum CompletionMode<'a> {
    Unified {
        system: &'a str,
    },
    Split {
        system_static: &'a str,
        system_dynamic: &'a str,
    },
}

impl CompletionMode<'_> {
    pub(super) fn log_suffix(self) -> &'static str {
        match self {
            CompletionMode::Unified { .. } => "",
            CompletionMode::Split { .. } => " (split)",
        }
    }

    pub(super) fn switch_log_prefix(self) -> &'static str {
        match self {
            CompletionMode::Unified { .. } => "Auto-fallback",
            CompletionMode::Split { .. } => "Auto-fallback (split)",
        }
    }
}

impl MultiProvider {
    pub(super) fn estimate_request_input(
        messages: &[Message],
        tools: &[ToolDefinition],
        mode: CompletionMode<'_>,
    ) -> (usize, usize) {
        // Count model context, not transport JSON. In particular, base64 images
        // and opaque native compaction payloads are not tokenized as text.
        let mut chars: usize = messages
            .iter()
            .map(jcode_compaction_core::message_char_count)
            .sum();
        let referenced_tools: std::collections::HashSet<&str> = messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|block| match block {
                crate::message::ContentBlock::ToolReference { tool_name, .. } => {
                    Some(tool_name.as_str())
                }
                _ => None,
            })
            .collect();
        // Native deferred catalogs travel over the wire but enter model context
        // only when referenced. Non-native routes already strip deferred tools
        // before dispatch, so do not charge their full unloaded catalogs here.
        chars += tools
            .iter()
            .filter(|tool| !tool.defer_loading || referenced_tools.contains(tool.name.as_str()))
            .map(ToolDefinition::prompt_chars)
            .sum::<usize>();
        match mode {
            CompletionMode::Unified { system } => {
                chars += system.len();
            }
            CompletionMode::Split {
                system_static,
                system_dynamic,
            } => {
                chars += system_static.len() + system_dynamic.len();
            }
        }
        let tokens = chars / 4;
        (chars, tokens)
    }

    pub(super) async fn complete_on_provider(
        &self,
        provider: ActiveProvider,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.reconcile_auth_if_provider_missing(provider);
        match provider {
            ActiveProvider::Claude => {
                if let Some(anthropic) = self.anthropic_provider() {
                    self.bind_provider_session(anthropic.as_ref());
                    anthropic
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "Claude credentials not available. Run `jcode login --provider claude` to log in."
                    ))
                }
            }
            ActiveProvider::OpenAI => {
                if let Some(openai) = self.openai_provider() {
                    self.bind_provider_session(openai.as_ref());
                    openai
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "OpenAI credentials not available. Run `jcode login --provider openai` to log in."
                    ))
                }
            }
            ActiveProvider::Copilot => {
                let copilot = self
                    .copilot_api
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                if let Some(copilot) = copilot {
                    self.bind_provider_session(copilot.as_ref());
                    copilot
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "GitHub Copilot is not available. Run `jcode login --provider copilot`."
                    ))
                }
            }
            ActiveProvider::Antigravity => {
                let antigravity = self.antigravity_provider();
                if let Some(antigravity) = antigravity {
                    self.bind_provider_session(antigravity.as_ref());
                    antigravity
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "Antigravity is not available. Run `jcode login --provider antigravity`."
                    ))
                }
            }
            ActiveProvider::Gemini => {
                let gemini = self
                    .gemini
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                if let Some(gemini) = gemini {
                    self.bind_provider_session(gemini.as_ref());
                    gemini
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "Gemini is not available. Run `jcode login --provider gemini`."
                    ))
                }
            }
            ActiveProvider::Cursor => {
                let cursor = self
                    .cursor
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                if let Some(cursor) = cursor {
                    self.bind_provider_session(cursor.as_ref());
                    cursor
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "Cursor is not available. Run `jcode login --provider cursor`."
                    ))
                }
            }
            ActiveProvider::Bedrock => {
                if let Some(bedrock) = self.bedrock_provider() {
                    self.bind_provider_session(bedrock.as_ref());
                    bedrock
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "AWS Bedrock is not available. Configure AWS credentials and region, or set AWS_PROFILE/AWS_REGION."
                    ))
                }
            }
            ActiveProvider::OpenRouter => {
                let openrouter = self.active_openrouter_execution_provider();
                if let Some(openrouter) = openrouter {
                    self.bind_provider_session(openrouter.as_ref());
                    openrouter
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "OpenRouter credentials not available. Set OPENROUTER_API_KEY environment variable."
                    ))
                }
            }
        }
    }

    pub(super) async fn complete_split_on_provider(
        &self,
        provider: ActiveProvider,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_static: &str,
        system_dynamic: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.reconcile_auth_if_provider_missing(provider);
        match provider {
            ActiveProvider::Claude => {
                if let Some(anthropic) = self.anthropic_provider() {
                    self.bind_provider_session(anthropic.as_ref());
                    anthropic
                        .complete_split(
                            messages,
                            tools,
                            system_static,
                            system_dynamic,
                            resume_session_id,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "Claude credentials not available. Run `jcode login --provider claude` to log in."
                    ))
                }
            }
            ActiveProvider::OpenAI => {
                if let Some(openai) = self.openai_provider() {
                    self.bind_provider_session(openai.as_ref());
                    openai
                        .complete_split(
                            messages,
                            tools,
                            system_static,
                            system_dynamic,
                            resume_session_id,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "OpenAI credentials not available. Run `jcode login --provider openai` to log in."
                    ))
                }
            }
            ActiveProvider::Copilot => {
                let copilot = self
                    .copilot_api
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                if let Some(copilot) = copilot {
                    self.bind_provider_session(copilot.as_ref());
                    copilot
                        .complete_split(
                            messages,
                            tools,
                            system_static,
                            system_dynamic,
                            resume_session_id,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "GitHub Copilot is not available. Run `jcode login --provider copilot`."
                    ))
                }
            }
            ActiveProvider::Antigravity => {
                let antigravity = self.antigravity_provider();
                if let Some(antigravity) = antigravity {
                    self.bind_provider_session(antigravity.as_ref());
                    antigravity
                        .complete_split(
                            messages,
                            tools,
                            system_static,
                            system_dynamic,
                            resume_session_id,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "Antigravity is not available. Run `jcode login --provider antigravity`."
                    ))
                }
            }
            ActiveProvider::Gemini => {
                let gemini = self
                    .gemini
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                if let Some(gemini) = gemini {
                    self.bind_provider_session(gemini.as_ref());
                    gemini
                        .complete_split(
                            messages,
                            tools,
                            system_static,
                            system_dynamic,
                            resume_session_id,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "Gemini is not available. Run `jcode login --provider gemini`."
                    ))
                }
            }
            ActiveProvider::Cursor => {
                let cursor = self
                    .cursor
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                if let Some(cursor) = cursor {
                    self.bind_provider_session(cursor.as_ref());
                    cursor
                        .complete_split(
                            messages,
                            tools,
                            system_static,
                            system_dynamic,
                            resume_session_id,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "Cursor is not available. Run `jcode login --provider cursor`."
                    ))
                }
            }
            ActiveProvider::Bedrock => {
                if let Some(bedrock) = self.bedrock_provider() {
                    self.bind_provider_session(bedrock.as_ref());
                    bedrock
                        .complete_split(
                            messages,
                            tools,
                            system_static,
                            system_dynamic,
                            resume_session_id,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "AWS Bedrock is not available. Configure AWS credentials and region, or set AWS_PROFILE/AWS_REGION."
                    ))
                }
            }
            ActiveProvider::OpenRouter => {
                let openrouter = self.active_openrouter_execution_provider();
                if let Some(openrouter) = openrouter {
                    self.bind_provider_session(openrouter.as_ref());
                    openrouter
                        .complete_split(
                            messages,
                            tools,
                            system_static,
                            system_dynamic,
                            resume_session_id,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "OpenRouter credentials not available. Set OPENROUTER_API_KEY environment variable."
                    ))
                }
            }
        }
    }
}
