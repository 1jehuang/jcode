use super::*;

fn assistant_output_tail_after(messages: &[StoredMessage], start_index: usize) -> Option<String> {
    messages
        .iter()
        .skip(start_index)
        .rev()
        .filter(|message| message.role == Role::Assistant)
        .find_map(|message| {
            let text = message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    // Never include tool activity, private reasoning, or results.
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            jcode_swarm_core::normalize_completion_report(Some(text))
        })
}

impl Agent {
    /// Read-only source for splitting a new session before its first persistence.
    pub(crate) fn session_for_split(&self) -> &Session {
        &self.session
    }

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
                    | ContentBlock::OpenAIReasoning { .. }
                    | ContentBlock::ToolReference { .. }
                    | ContentBlock::ProviderNative { .. } => {}
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

    /// Tail of the latest non-empty assistant text response at or after `start_index`.
    /// Earlier commentary, tool activity, reasoning, and results are excluded.
    pub fn recent_assistant_output_tail_after(&self, start_index: usize) -> Option<String> {
        assistant_output_tail_after(&self.session.messages, start_index)
    }

    /// The final visible assistant prose of this turn (its answer), used for
    /// notification previews. Tool calls and reasoning are excluded.
    pub fn final_assistant_text_after(&self, start_index: usize) -> Option<String> {
        self.session
            .messages
            .iter()
            .skip(start_index)
            .rev()
            .filter(|message| message.role == Role::Assistant)
            .find_map(|message| {
                let text = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text, .. } => Some(text.trim()),
                        _ => None,
                    })
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                (!text.is_empty()).then_some(text)
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

    /// Reasoning effort the active provider is running with, if any.
    pub fn provider_reasoning_effort(&self) -> Option<String> {
        self.provider.reasoning_effort()
    }

    pub fn provider_model(&self) -> String {
        let model = self.provider.model();
        self.provider
            .explicit_provider_pin_for_current_model()
            .map(|pin| format!("{model}@{pin}"))
            .unwrap_or(model)
    }

    pub(super) fn provider_key_for_new_session(&self) -> Option<String> {
        if let Some((method, label)) = self.observed_oauth_account_route() {
            return Some(format!("{method}@{label}"));
        }
        if self.provider.credential_mode() == jcode_provider_core::CredentialMode::ApiKey
            && self.provider.active_resolved_credential()
                == Some(jcode_provider_core::ResolvedCredential::ApiKey)
        {
            match self.provider.name().to_ascii_lowercase().as_str() {
                "openai" => return Some("openai-api".to_string()),
                "claude" | "anthropic" => return Some("claude-api".to_string()),
                _ => {}
            }
        }
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

    /// Re-apply the persisted session model on resume (or attach). A stale
    /// credential pin (the deployment's credential changed type since the
    /// session was created) is recovered onto the usable credential and the
    /// session's route metadata is rewritten to match, so swarm workers and
    /// later resumes inherit the route this session actually runs on.
    pub(crate) fn restore_session_model_from_session(&mut self) {
        crate::session_accounts::refresh_bound_account_route(&mut self.session);
        let Some(model) = self.session.model.clone() else {
            self.session.model = Some(self.provider_model());
            return;
        };
        match crate::provider::restore_session_model(
            self.provider.as_ref(),
            &model,
            self.session.provider_key.as_deref(),
            self.session.route_api_method.as_deref(),
        ) {
            Ok(restore) => {
                if let Some(healed) = restore.healed_route {
                    logging::warn(&format!(
                        "Session {} route healed: '{}' -> '{}' (provider_key {:?} -> {:?})",
                        self.session.id,
                        healed.stale_request,
                        restore.request,
                        self.session.provider_key,
                        healed.provider_key,
                    ));
                    self.session.provider_key = healed.provider_key;
                    self.session.route_api_method = healed.route_api_method;
                }
                self.reconcile_explicit_provider_pin_route();
            }
            Err(e) => {
                logging::error(&format!(
                    "Failed to restore session model '{}': {}",
                    model, e
                ));
            }
        }
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

#[cfg(test)]
mod output_tail_tests {
    use super::*;

    fn message(role: &str, content: serde_json::Value) -> StoredMessage {
        serde_json::from_value(serde_json::json!({
            "id": "message", "role": role, "content": content,
        }))
        .unwrap()
    }

    #[test]
    fn automatic_output_tail_excludes_tools_and_private_content() {
        let messages = vec![
            message(
                "assistant",
                serde_json::json!([{"type":"text", "text":"previous turn"}]),
            ),
            message(
                "user",
                serde_json::json!([{"type":"text", "text":"private user input"}]),
            ),
            message(
                "assistant",
                serde_json::json!([
                    {"type":"reasoning", "text":"private reasoning must not leak"},
                    {"type":"reasoning_trace", "text":"private trace must not leak"},
                    {"type":"text", "text":"Implemented the parser fix."},
                    {"type":"tool_result", "tool_use_id":"prior", "content":"private result must not leak"},
                    {"type":"tool_use", "id":"test", "name":"bash", "input":{"intent":"Run parser tests"}}
                ]),
            ),
            message(
                "user",
                serde_json::json!([{"type":"text", "text":"tool stdout must not leak"}]),
            ),
            message("assistant", serde_json::json!([])),
        ];
        assert_eq!(
            assistant_output_tail_after(&messages, 1).as_deref(),
            Some("Implemented the parser fix.")
        );
        assert_eq!(assistant_output_tail_after(&messages, messages.len()), None);
        assert_eq!(assistant_output_tail_after(&messages, usize::MAX), None);
    }

    #[test]
    fn automatic_output_tail_uses_only_latest_nonempty_assistant_response() {
        let messages = vec![
            message(
                "assistant",
                serde_json::json!([{"type":"text", "text":"first commentary"}]),
            ),
            message(
                "assistant",
                serde_json::json!([
                    {"type":"text", "text":"final commentary"},
                    {"type":"text", "text":"second paragraph"}
                ]),
            ),
            message(
                "assistant",
                serde_json::json!([
                    {"type":"text", "text":" \n\t"},
                    {"type":"tool_use", "id":"test", "name":"bash", "input":{}}
                ]),
            ),
        ];
        assert_eq!(
            assistant_output_tail_after(&messages, 0).as_deref(),
            Some("final commentary\n\nsecond paragraph")
        );
        assert_eq!(assistant_output_tail_after(&messages, 2), None);
    }

    #[test]
    fn automatic_output_tail_preserves_utf8_when_truncating_latest_response() {
        let long = format!("{}\nLATEST RESULT", "界🦀".repeat(3000));
        let messages = [message(
            "assistant",
            serde_json::json!([{"type":"text", "text":long}]),
        )];
        let tail = assistant_output_tail_after(&messages, 0).unwrap();
        assert_eq!(
            tail.chars().count(),
            jcode_swarm_core::MAX_SWARM_COMPLETION_REPORT_CHARS
        );
        assert!(tail.starts_with("[Earlier response text truncated by jcode.]"));
        assert!(tail.ends_with("LATEST RESULT"));
    }
}
