use super::*;

impl Agent {
    pub fn session_memory_profile_snapshot(
        &mut self,
    ) -> crate::session::SessionMemoryProfileSnapshot {
        self.session.memory_profile_snapshot()
    }

    pub fn message_count(&self) -> usize {
        self.session.messages.len()
    }

    /// Number of model-visible conversation messages (excludes the immutable
    /// session-context header and internal system reminders).
    pub fn visible_conversation_message_count(&self) -> usize {
        self.session.visible_conversation_message_count()
    }

    /// Role of the most recent model-visible conversation message, if any.
    ///
    /// When this is `User` and the agent is idle, the model still owes a
    /// response for that turn (e.g. the turn errored or was interrupted before
    /// the assistant replied).
    pub fn last_visible_conversation_role(&self) -> Option<Role> {
        self.session
            .visible_conversation_messages()
            .last()
            .map(|message| message.role.clone())
    }

    pub fn last_message_role(&self) -> Option<Role> {
        self.session.messages.last().map(|m| m.role.clone())
    }

    /// Get the text content of the last message (first Text block)
    pub fn last_message_text(&self) -> Option<&str> {
        self.session.messages.last().and_then(|m| {
            m.content.iter().find_map(|block| {
                if let ContentBlock::Text { text, .. } = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
        })
    }

    /// Build a transcript string for memory extraction
    /// This is a independent method so it can be called before spawning async tasks
    pub fn build_transcript_for_extraction(&self) -> String {
        let mut transcript = String::new();
        for msg in &self.session.messages {
            let role = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
            };
            transcript.push_str(&format!("**{}:**\n", role));
            for block in &msg.content {
                match block {
                    ContentBlock::Text { text, .. } => {
                        transcript.push_str(text);
                        transcript.push('\n');
                    }
                    ContentBlock::ToolUse { name, .. } => {
                        transcript.push_str(&format!("[Used tool: {}]\n", name));
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        let preview = if content.len() > 200 {
                            format!("{}...", crate::util::truncate_str(content, 200))
                        } else {
                            content.clone()
                        };
                        transcript.push_str(&format!("[Result: {}]\n", preview));
                    }
                    ContentBlock::Reasoning { .. }
                    | ContentBlock::ReasoningTrace { .. }
                    | ContentBlock::AnthropicThinking { .. }
                    | ContentBlock::OpenAIReasoning { .. } => {}
                    ContentBlock::Image { .. } => {
                        transcript.push_str("[Image]\n");
                    }
                    ContentBlock::OpenAICompaction { .. } => {
                        transcript.push_str("[OpenAI native compaction]\n");
                    }
                }
            }
            transcript.push('\n');
        }
        transcript
    }

    pub fn last_assistant_text(&self) -> Option<String> {
        self.session
            .messages
            .iter()
            .rev()
            .find(|msg| msg.role == Role::Assistant)
            .map(|msg| {
                msg.content
                    .iter()
                    .filter_map(|c| {
                        if let ContentBlock::Text { text, .. } = c {
                            Some(text.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
    }

    /// Latest non-empty assistant text added at or after `start_index`.
    pub fn latest_assistant_text_after(&self, start_index: usize) -> Option<String> {
        self.session
            .messages
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, message)| {
                if index < start_index || !matches!(&message.role, Role::Assistant) {
                    return None;
                }

                let text = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let text = text.trim();
                (!text.is_empty()).then(|| text.to_string())
            })
    }

    pub fn last_upstream_provider(&self) -> Option<String> {
        self.last_upstream_provider
            .clone()
            .or_else(|| self.provider.preferred_provider())
    }

    pub fn last_connection_type(&self) -> Option<String> {
        self.last_connection_type.clone()
    }

    pub fn last_status_detail(&self) -> Option<String> {
        self.last_status_detail.clone()
    }

    pub fn provider_name(&self) -> String {
        // `display_name()` resolves the active runtime profile (e.g. NVIDIA NIM)
        // for the OpenRouter slot; for all other providers it equals `name()`.
        self.provider.display_name()
    }

    pub fn provider_model(&self) -> String {
        let model = self.provider.model();
        self.provider
            .explicit_provider_pin_for_current_model()
            .map(|pin| format!("{model}@{pin}"))
            .unwrap_or(model)
    }

    pub(super) fn provider_key_for_new_session(&self) -> Option<String> {
        if self
            .provider
            .explicit_provider_pin_for_current_model()
            .is_some()
        {
            // Provider pins are explicit OpenRouter route identity. Prefer that
            // over ambient runtime env state when a CLI-created Agent snapshots
            // a provider that was configured before the Agent existed.
            return crate::provider::MultiProvider::session_provider_key_for_model_request(
                &self.provider_model(),
                self.provider.name(),
            );
        }

        crate::session::derive_session_provider_key(self.provider.name())
    }

    pub(super) fn reconcile_explicit_provider_pin_route(&mut self) {
        if self
            .provider
            .explicit_provider_pin_for_current_model()
            .is_some()
        {
            self.session.model = Some(self.provider_model());
            self.session.provider_key = Some("openrouter".to_string());
            self.session.route_api_method = Some("openrouter".to_string());
        }
    }

    /// Get the short/friendly name for this session (e.g., "fox")
    pub fn session_short_name(&self) -> Option<&str> {
        self.session.short_name.as_deref()
    }
}
