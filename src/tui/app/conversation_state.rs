use super::*;

impl App {
    pub(super) fn format_compaction_strategy_label(trigger: &str) -> &'static str {
        match trigger {
            "manual" => "manual",
            "proactive" => "proactive",
            "semantic" => "semantic",
            "reactive" => "reactive",
            "auto_recovery" => "automatic recovery",
            "hard_compact" => "emergency",
            _ => "automatic",
        }
    }

    pub(super) fn format_compaction_started_message(trigger: &str) -> String {
        let strategy = Self::format_compaction_strategy_label(trigger);
        format!(
            "📦 **Compacting context** ({}) — summarizing older messages in the background to stay within the context window.",
            strategy
        )
    }

    pub(super) fn format_compaction_complete_message(
        trigger: &str,
        pre_tokens: Option<u64>,
    ) -> String {
        if trigger == "hard_compact" {
            let mut message =
                "📦 **Emergency compaction** — older messages were dropped to recover from context pressure. Recent context was kept.".to_string();
            if let Some(tokens) = pre_tokens {
                message.push_str(&format!(
                    " Previous size: ~{} tokens.",
                    Self::format_compaction_number(tokens)
                ));
            }
            return message;
        }

        let reason = match trigger {
            "auto_recovery" => "after the context window filled up",
            _ => "to stay within the context window",
        };
        let strategy = Self::format_compaction_strategy_label(trigger);
        let mut message = format!(
            "📦 **Context compacted** ({}) — older messages were summarized {}.",
            strategy, reason
        );
        if let Some(tokens) = pre_tokens {
            message.push_str(&format!(
                " Previous size: ~{} tokens.",
                Self::format_compaction_number(tokens)
            ));
        }
        message
    }

    pub(super) fn format_emergency_compaction_message(dropped: usize) -> String {
        let noun = if dropped == 1 { "message" } else { "messages" };
        format!(
            "📦 **Emergency compaction** — dropped {} oldest {} because the context window was full. Recent context was kept.",
            Self::format_compaction_number(dropped as u64),
            noun,
        )
    }

    pub(super) fn format_compaction_number(value: u64) -> String {
        let digits = value.to_string();
        let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
        for (idx, ch) in digits.chars().rev().enumerate() {
            if idx > 0 && idx % 3 == 0 {
                formatted.push(',');
            }
            formatted.push(ch);
        }
        formatted.chars().rev().collect()
    }

    pub(super) fn add_provider_message(&mut self, message: Message) {
        self.messages.push(message);
        if self.is_remote || !self.provider.supports_compaction() {
            return;
        }
        let compaction = self.registry.compaction();
        if let Ok(mut manager) = compaction.try_write() {
            manager.notify_message_added();
        };
    }

    pub(super) fn replace_provider_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.last_injected_memory_signature = None;
        self.rebuild_tool_result_index();
        self.reseed_compaction_from_provider_messages();
    }

    pub(super) fn clear_provider_messages(&mut self) {
        self.messages.clear();
        self.last_injected_memory_signature = None;
        self.tool_result_ids.clear();
        self.reseed_compaction_from_provider_messages();
    }

    pub(super) fn rebuild_tool_result_index(&mut self) {
        self.tool_result_ids.clear();
        for msg in &self.messages {
            if let Role::User = msg.role {
                for block in &msg.content {
                    if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                        self.tool_result_ids.insert(tool_use_id.clone());
                    }
                }
            }
        }
    }

    pub(super) fn reseed_compaction_from_provider_messages(&mut self) {
        if self.is_remote || !self.provider.supports_compaction() {
            return;
        }
        let compaction = self.registry.compaction();
        if let Ok(mut manager) = compaction.try_write() {
            manager.reset();
            manager.set_budget(self.context_limit as usize);
            if let Some(state) = self.session.compaction.as_ref() {
                manager.restore_persisted_state(state, self.messages.len());
            } else {
                manager.seed_restored_messages(self.messages.len());
            }
        };
    }

    pub(super) fn sync_session_compaction_state_from_manager(
        &mut self,
        manager: &crate::compaction::CompactionManager,
    ) {
        let new_state = manager.persisted_state();
        if self.session.compaction != new_state {
            self.session.compaction = new_state;
            if let Err(err) = self.session.save() {
                crate::logging::error(&format!(
                    "Failed to persist compaction state for session {}: {}",
                    self.session.id, err
                ));
            }
        }
    }

    pub(super) fn messages_for_provider(&mut self) -> (Vec<Message>, Option<CompactionEvent>) {
        if self.is_remote || !self.provider.supports_compaction() {
            return (self.messages.clone(), None);
        }
        let compaction = self.registry.compaction();
        let result = match compaction.try_write() {
            Ok(mut manager) => {
                let action = manager.ensure_context_fits(&self.messages, self.provider.clone());
                match action {
                    crate::compaction::CompactionAction::BackgroundStarted { trigger } => {
                        self.push_display_message(DisplayMessage::system(
                            Self::format_compaction_started_message(&trigger),
                        ));
                        self.set_status_notice("Compacting context");
                    }
                    crate::compaction::CompactionAction::HardCompacted(dropped) => {
                        self.push_display_message(DisplayMessage::system(
                            Self::format_emergency_compaction_message(dropped),
                        ));
                        self.set_status_notice("Emergency compaction");
                    }
                    crate::compaction::CompactionAction::None => {}
                }
                let messages = manager.messages_for_api_with(&self.messages);
                let event = manager.take_compaction_event();
                if event.is_some() {
                    self.sync_session_compaction_state_from_manager(&manager);
                }
                (messages, event)
            }
            Err(_) => (self.messages.clone(), None),
        };
        result
    }

    pub(super) fn poll_compaction_completion(&mut self) {
        if self.is_remote || !self.provider.supports_compaction() {
            return;
        }
        let compaction = self.registry.compaction();
        if let Ok(mut manager) = compaction.try_write() {
            if let Some(event) = manager.poll_compaction_event() {
                self.sync_session_compaction_state_from_manager(&manager);
                self.handle_compaction_event(event);
            }
        };
    }

    pub(super) fn handle_compaction_event(&mut self, event: CompactionEvent) {
        self.provider_session_id = None;
        self.session.provider_session_id = None;
        self.context_warning_shown = false;
        let message = if let Some(dropped) = event.messages_dropped {
            self.set_status_notice("Emergency compaction");
            Self::format_emergency_compaction_message(dropped)
        } else {
            self.set_status_notice("Context compacted");
            Self::format_compaction_complete_message(&event.trigger, event.pre_tokens)
        };
        self.push_display_message(DisplayMessage::system(message));
    }

    pub fn set_status_notice(&mut self, text: impl Into<String>) {
        self.status_notice = Some((text.into(), Instant::now()));
    }

    pub(super) fn set_memory_feature_enabled(&mut self, enabled: bool) {
        self.memory_enabled = enabled;
        if !enabled {
            crate::memory::clear_pending_memory(&self.session.id);
            crate::memory::clear_activity();
            crate::memory_agent::reset();
            self.last_injected_memory_signature = None;
        }
    }

    pub(super) fn memory_prompt_signature(prompt: &str) -> String {
        prompt
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<String>>()
            .join("\n")
    }

    pub(super) fn should_inject_memory_context(&mut self, prompt: &str) -> bool {
        let signature = Self::memory_prompt_signature(prompt);
        let now = Instant::now();
        if let Some((last_signature, last_injected_at)) =
            self.last_injected_memory_signature.as_ref()
        {
            if *last_signature == signature
                && now.duration_since(*last_injected_at).as_secs()
                    < MEMORY_INJECTION_SUPPRESSION_SECS
            {
                return false;
            }
        }
        self.last_injected_memory_signature = Some((signature, now));
        true
    }

    pub(super) fn set_swarm_feature_enabled(&mut self, enabled: bool) {
        self.swarm_enabled = enabled;
        if !enabled {
            self.remote_swarm_members.clear();
        }
    }

    pub(super) fn extract_thought_line(text: &str) -> Option<String> {
        let trimmed = text.trim();
        if trimmed.starts_with("Thought for ") && trimmed.ends_with('s') {
            Some(trimmed.to_string())
        } else {
            None
        }
    }

    /// Handle quit request (Ctrl+C/Ctrl+D). Returns true if should actually quit.
    pub(super) fn handle_quit_request(&mut self) -> bool {
        const QUIT_TIMEOUT: Duration = Duration::from_secs(2);

        if let Some(pending_time) = self.quit_pending {
            if pending_time.elapsed() < QUIT_TIMEOUT {
                // Second press within timeout - actually quit
                // Mark session as closed and save
                self.session.provider_session_id = self.provider_session_id.clone();
                crate::telemetry::end_session_with_reason(
                    self.provider.name(),
                    &self.provider.model(),
                    crate::telemetry::SessionEndReason::NormalExit,
                );
                self.session.mark_closed();
                let _ = self.session.save();
                self.should_quit = true;
                return true;
            }
        }

        // First press or timeout expired - show warning
        self.quit_pending = Some(Instant::now());
        self.set_status_notice("Press Ctrl+C again to quit");
        false
    }

    pub(super) fn missing_tool_result_ids(&self) -> Vec<String> {
        let mut tool_calls = HashSet::new();
        let mut tool_results = HashSet::new();

        for msg in &self.messages {
            match msg.role {
                Role::Assistant => {
                    for block in &msg.content {
                        if let ContentBlock::ToolUse { id, .. } = block {
                            tool_calls.insert(id.clone());
                        }
                    }
                }
                Role::User => {
                    for block in &msg.content {
                        if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                            tool_results.insert(tool_use_id.clone());
                        }
                    }
                }
            }
        }

        tool_calls
            .difference(&tool_results)
            .cloned()
            .collect::<Vec<_>>()
    }

    pub(super) fn summarize_tool_results_missing(&self) -> Option<String> {
        let missing = self.missing_tool_result_ids();
        if missing.is_empty() {
            return None;
        }
        let sample = missing
            .iter()
            .take(3)
            .map(|id| format!("`{}`", id))
            .collect::<Vec<_>>()
            .join(", ");
        let count = missing.len();
        let suffix = if count > 3 { "..." } else { "" };
        Some(format!(
            "Missing tool outputs for {} call(s): {}{}",
            count, sample, suffix
        ))
    }

    pub(super) fn repair_missing_tool_outputs(&mut self) -> usize {
        let mut known_results = HashSet::new();
        for msg in &self.messages {
            if let Role::User = msg.role {
                for block in &msg.content {
                    if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                        known_results.insert(tool_use_id.clone());
                    }
                }
            }
        }

        let mut repaired = 0usize;
        let mut index = 0usize;
        while index < self.messages.len() {
            let mut missing_for_message: Vec<String> = Vec::new();
            if let Role::Assistant = self.messages[index].role {
                for block in &self.messages[index].content {
                    if let ContentBlock::ToolUse { id, .. } = block {
                        if !known_results.contains(id) {
                            known_results.insert(id.clone());
                            missing_for_message.push(id.clone());
                        }
                    }
                }
            }

            if !missing_for_message.is_empty() {
                for (offset, id) in missing_for_message.iter().enumerate() {
                    let tool_block = ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: TOOL_OUTPUT_MISSING_TEXT.to_string(),
                        is_error: Some(true),
                    };
                    let inserted_message = Message {
                        role: Role::User,
                        content: vec![tool_block.clone()],
                        timestamp: None,
                    };
                    let stored_message = crate::session::StoredMessage {
                        id: id::new_id("message"),
                        role: Role::User,
                        content: vec![tool_block],
                        timestamp: Some(chrono::Utc::now()),
                        tool_duration_ms: None,
                        token_usage: None,
                    };
                    self.messages.insert(index + 1 + offset, inserted_message);
                    self.session
                        .messages
                        .insert(index + 1 + offset, stored_message);
                    self.tool_result_ids.insert(id.clone());
                    repaired += 1;
                }
                index += missing_for_message.len();
            }

            index += 1;
        }

        if repaired > 0 {
            self.reseed_compaction_from_provider_messages();
            let _ = self.session.save();
        }

        repaired
    }

    /// Rebuild current session into a new one without tool calls
    pub(super) fn recover_session_without_tools(&mut self) {
        let old_session = self.session.clone();
        let old_messages = old_session.messages.clone();

        let new_session_id = format!("session_recovery_{}", id::new_id("rec"));
        let mut new_session =
            Session::create_with_id(new_session_id, Some(old_session.id.clone()), None);
        new_session.title = old_session.title.clone();
        new_session.provider_session_id = old_session.provider_session_id.clone();
        new_session.model = old_session.model.clone();
        new_session.is_canary = old_session.is_canary;
        new_session.testing_build = old_session.testing_build.clone();
        new_session.is_debug = old_session.is_debug;
        new_session.saved = old_session.saved;
        new_session.save_label = old_session.save_label.clone();
        new_session.working_dir = old_session.working_dir.clone();

        self.clear_provider_messages();
        self.clear_display_messages();
        self.queued_messages.clear();
        self.pasted_contents.clear();
        self.pending_images.clear();
        self.active_skill = None;
        self.provider_session_id = None;
        self.session = new_session;

        for msg in old_messages {
            let role = msg.role.clone();
            let kept_blocks: Vec<ContentBlock> = msg
                .content
                .into_iter()
                .filter(|block| matches!(block, ContentBlock::Text { .. }))
                .collect();
            if kept_blocks.is_empty() {
                continue;
            }
            self.add_provider_message(Message {
                role: role.clone(),
                content: kept_blocks.clone(),
                timestamp: None,
            });
            self.push_display_message(DisplayMessage {
                role: match role {
                    Role::User => "user".to_string(),
                    Role::Assistant => "assistant".to_string(),
                },
                content: kept_blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            let _ = self.session.add_message(role, kept_blocks);
        }
        let _ = self.session.save();

        self.push_display_message(DisplayMessage::system(format!(
            "Recovery complete. New session: {}. Tool calls stripped; context preserved.",
            self.session.id
        )));
        self.set_status_notice("Recovered session");
    }
}
