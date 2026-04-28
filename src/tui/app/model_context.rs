use super::*;

impl App {
    fn format_failover_count(value: usize) -> String {
        match value {
            0..=999 => value.to_string(),
            1_000..=999_999 => format!("{:.1}k", value as f64 / 1_000.0),
            _ => format!("{:.1}M", value as f64 / 1_000_000.0),
        }
    }

    fn format_failover_input_summary(prompt: &crate::provider::ProviderFailoverPrompt) -> String {
        format!(
            "about **{} input tokens** (~{} chars)",
            Self::format_failover_count(prompt.estimated_input_tokens),
            Self::format_failover_count(prompt.estimated_input_chars),
        )
    }

    fn failover_config_hint() -> &'static str {
        "To turn this off, set `[provider].cross_provider_failover = \"manual\"` in `~/.jcode/config.toml` or export `JCODE_CROSS_PROVIDER_FAILOVER=manual`."
    }

    fn apply_provider_switch_for_failover(
        &mut self,
        prompt: &crate::provider::ProviderFailoverPrompt,
    ) -> anyhow::Result<String> {
        self.provider
            .switch_active_provider_to(&prompt.to_provider)?;
        self.provider_session_id = None;
        self.session.provider_session_id = None;
        self.upstream_provider = None;
        self.status_detail = None;
        let active_model = self.provider.model();
        self.update_context_limit_for_model(&active_model);
        self.session.model = Some(active_model.clone());
        let _ = self.session.save();
        Ok(active_model)
    }

    pub(super) fn cancel_pending_provider_failover(&mut self, notice: impl Into<String>) {
        let Some(pending) = self.pending_provider_failover.take() else {
            return;
        };
        self.push_display_message(DisplayMessage::system(format!(
            "⏸ **Canceled provider auto-switch** — kept **{}** active.\n\nYou can switch manually with `/model`, then resend. {}",
            pending.prompt.from_label,
            Self::failover_config_hint(),
        )));
        self.set_status_notice(notice);
    }

    pub(super) fn maybe_progress_provider_failover_countdown(&mut self) -> bool {
        let Some(pending) = self.pending_provider_failover.clone() else {
            return false;
        };
        if self.is_processing {
            return false;
        }
        let now = Instant::now();
        if now < pending.deadline {
            let remaining = pending.deadline.saturating_duration_since(now).as_secs() + 1;
            self.set_status_notice(format!(
                "Provider auto-switch → {} in {}s (Esc to cancel)",
                pending.prompt.to_label, remaining
            ));
            return true;
        }

        self.pending_provider_failover = None;
        match self.apply_provider_switch_for_failover(&pending.prompt) {
            Ok(active_model) => {
                self.push_display_message(DisplayMessage::system(format!(
                    "⚡ **Auto-switched provider** after countdown: **{}** → **{}**.\n\nResending {} on model `{}`.\n\n{}",
                    pending.prompt.from_label,
                    pending.prompt.to_label,
                    Self::format_failover_input_summary(&pending.prompt),
                    active_model,
                    Self::failover_config_hint(),
                )));
                self.set_status_notice(format!(
                    "Provider → {} (retrying)",
                    pending.prompt.to_label
                ));
                self.pending_turn = true;
                true
            }
            Err(error) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to switch provider to {}: {}",
                    pending.prompt.to_label, error
                )));
                self.set_status_notice("Provider switch failed");
                true
            }
        }
    }

    fn handle_provider_failover_prompt(&mut self, prompt: crate::provider::ProviderFailoverPrompt) {
        let input_summary = Self::format_failover_input_summary(&prompt);
        let manual_message = format!(
            "⚠ **{} became unavailable** — jcode did **not** resend your prompt to **{}** automatically.\n\nReason: {}\n\nRetrying elsewhere would send {}.\n\nTo switch manually now, use `/model` and pick a model from **{}**, then resend. {}",
            prompt.from_label,
            prompt.to_label,
            prompt.reason,
            input_summary,
            prompt.to_label,
            Self::failover_config_hint(),
        );

        match crate::config::Config::load()
            .provider
            .cross_provider_failover
        {
            crate::config::CrossProviderFailoverMode::Manual if !self.is_remote => {
                self.push_display_message(DisplayMessage::system(manual_message));
                self.set_status_notice(format!(
                    "{} unavailable; switch manually if desired",
                    prompt.from_label
                ));
            }
            crate::config::CrossProviderFailoverMode::Countdown if !self.is_remote => {
                self.pending_provider_failover = Some(super::PendingProviderFailover {
                    prompt: prompt.clone(),
                    deadline: Instant::now() + Duration::from_secs(3),
                });
                self.push_display_message(DisplayMessage::system(format!(
                    "⚠ **{} became unavailable** — jcode will switch to **{}** in **3 seconds** unless you cancel.\n\nReason: {}\n\nRetrying would send {}. Press **Esc** to cancel.\n\n{}",
                    prompt.from_label,
                    prompt.to_label,
                    prompt.reason,
                    input_summary,
                    Self::failover_config_hint(),
                )));
                self.set_status_notice(format!(
                    "Provider auto-switch → {} in 3s (Esc to cancel)",
                    prompt.to_label
                ));
            }
            _ => {
                self.push_display_message(DisplayMessage::system(format!(
                    "{}\n\n_Automatic countdown switching is only available in local sessions right now._",
                    manual_message,
                )));
                self.set_status_notice(format!(
                    "{} unavailable; manual switch suggested",
                    prompt.from_label
                ));
            }
        }
    }

    pub(super) fn cycle_model(&mut self, direction: i8) {
        let models = self.provider.available_models_for_switching();
        if models.is_empty() {
            self.push_display_message(DisplayMessage::error(
                "Model switching is not available for this provider.",
            ));
            self.set_status_notice("Model switching not available");
            return;
        }

        let current = self.provider.model();
        let current_index = models.iter().position(|m| *m == current).unwrap_or(0);

        let len = models.len();
        let next_index = if direction >= 0 {
            (current_index + 1) % len
        } else {
            (current_index + len - 1) % len
        };
        let next_model = models[next_index].clone();

        match self.provider.set_model(&next_model) {
            Ok(()) => {
                self.provider_session_id = None;
                self.session.provider_session_id = None;
                self.upstream_provider = None;
                self.status_detail = None;
                self.update_context_limit_for_model(&next_model);
                self.session.model = Some(self.provider.model());
                let _ = self.session.save();
                self.push_display_message(DisplayMessage::system(format!(
                    "✓ Switched to model: {}",
                    next_model
                )));
                self.set_status_notice(format!("Model → {}", next_model));
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to switch model: {}",
                    e
                )));
                self.set_status_notice("Model switch failed");
            }
        }
    }

    pub(super) fn cycle_effort(&mut self, direction: i8) {
        let efforts = self.provider.available_efforts();
        if efforts.is_empty() {
            self.set_status_notice("Reasoning effort not available for this provider");
            return;
        }

        let current = self.provider.reasoning_effort();
        let current_index = current
            .as_ref()
            .and_then(|c| efforts.iter().position(|e| *e == c.as_str()))
            .unwrap_or(efforts.len() - 1); // default to last (xhigh)

        let len = efforts.len();
        let next_index = if direction > 0 {
            if current_index + 1 >= len {
                current_index // already at max
            } else {
                current_index + 1
            }
        } else if current_index == 0 {
            0 // already at min
        } else {
            current_index - 1
        };

        let next_effort = efforts[next_index];
        if Some(next_effort.to_string()) == current {
            let label = effort_display_label(next_effort);
            self.set_status_notice(format!(
                "Effort: {} (already at {})",
                label,
                if direction > 0 { "max" } else { "min" }
            ));
            return;
        }

        match self.provider.set_reasoning_effort(next_effort) {
            Ok(()) => {
                let label = effort_display_label(next_effort);
                let bar = effort_bar(next_index, len);
                self.set_status_notice(format!("Effort: {} {}", label, bar));
            }
            Err(e) => {
                self.set_status_notice(format!("Effort switch failed: {}", e));
            }
        }
    }

    pub(super) fn update_context_limit_for_model(&mut self, model: &str) {
        let limit = if self.is_remote {
            crate::provider::context_limit_for_model_with_provider(
                model,
                self.remote_provider_name.as_deref(),
            )
            .unwrap_or(self.provider.context_window())
        } else {
            self.provider.context_window()
        };
        self.context_limit = limit as u64;
        self.context_warning_shown = false;

        // Also update compaction manager's budget
        {
            let compaction = self.registry.compaction();
            if let Ok(mut manager) = compaction.try_write() {
                manager.set_budget(limit);
            };
        }
    }

    pub(super) fn effective_context_tokens_from_usage(
        &self,
        input_tokens: u64,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    ) -> u64 {
        if input_tokens == 0 {
            return 0;
        }
        let cache_read = cache_read_input_tokens.unwrap_or(0);
        let cache_creation = cache_creation_input_tokens.unwrap_or(0);
        let provider_name = if self.is_remote {
            self.remote_provider_name.clone().unwrap_or_default()
        } else {
            self.provider.name().to_string()
        }
        .to_lowercase();

        // Some providers report cache tokens as separate counters, others report them as subsets.
        // When in doubt, avoid over-counting unless we have strong evidence of split accounting.
        let split_cache_accounting = provider_name.contains("anthropic")
            || provider_name.contains("claude")
            || cache_creation > 0
            || cache_read > input_tokens;

        if split_cache_accounting {
            input_tokens
                .saturating_add(cache_read)
                .saturating_add(cache_creation)
        } else {
            input_tokens
        }
    }

    pub(super) fn current_stream_context_tokens(&self) -> Option<u64> {
        if self.streaming_input_tokens == 0 {
            return None;
        }
        Some(self.effective_context_tokens_from_usage(
            self.streaming_input_tokens,
            self.streaming_cache_read_tokens,
            self.streaming_cache_creation_tokens,
        ))
    }

    pub(super) fn update_compaction_usage_from_stream(&mut self) {
        if self.is_remote || !self.provider.uses_jcode_compaction() {
            return;
        }
        let Some(tokens) = self.current_stream_context_tokens() else {
            return;
        };
        let compaction = self.registry.compaction();
        if let Ok(mut manager) = compaction.try_write() {
            manager.update_observed_input_tokens(tokens);
        };
    }

    pub(super) fn handle_turn_error(&mut self, error: impl Into<String>) {
        let error = error.into();
        self.last_stream_error = Some(error.clone());

        if let Some(prompt) = crate::provider::parse_failover_prompt_message(&error) {
            self.handle_provider_failover_prompt(prompt);
            return;
        }

        if is_context_limit_error(&error) {
            let recovery = self.auto_recover_context_limit();
            let hint = match recovery {
                Some(msg) => format!(" {}", msg),
                None => " Context limit exceeded but auto-recovery failed. Run `/fix` to try manual recovery.".to_string(),
            };
            self.push_display_message(DisplayMessage::error(format!("Error: {}{}", error, hint)));
        } else {
            self.push_display_message(DisplayMessage::error(format!(
                "Error: {} Run `/fix` to attempt recovery.",
                error
            )));
        }
    }

    pub(super) fn auto_recover_context_limit(&mut self) -> Option<String> {
        if self.is_remote || !self.provider.supports_compaction() {
            return None;
        }
        let compaction = self.registry.compaction();
        let mut manager = compaction.try_write().ok()?;
        let mut provider_messages = self.materialized_provider_messages();

        let usage = manager.context_usage_with(&provider_messages);
        if usage > 1.5 {
            match manager.hard_compact_with(&provider_messages) {
                Ok(dropped) => {
                    self.sync_session_compaction_state_from_manager(&manager);
                    let post_usage = manager.context_usage_with(&provider_messages);
                    if post_usage <= 1.0 {
                        return Some(format!(
                            "⚡ Emergency compaction: dropped {} old messages (context was at {:.0}%). You can continue.",
                            dropped,
                            usage * 100.0
                        ));
                    }
                    let truncated = manager.emergency_truncate_with(&mut provider_messages);
                    self.messages = provider_messages;
                    return Some(format!(
                        "⚡ Emergency compaction: dropped {} old messages and truncated {} tool result(s) (context was at {:.0}%). You can continue.",
                        dropped,
                        truncated,
                        usage * 100.0
                    ));
                }
                Err(reason) => {
                    crate::logging::error(&format!(
                        "[auto_recover] hard_compact failed: {}",
                        reason
                    ));
                    let truncated = manager.emergency_truncate_with(&mut provider_messages);
                    if truncated > 0 {
                        self.messages = provider_messages;
                        return Some(format!(
                            "⚡ Emergency truncation: shortened {} large tool result(s) to fit context. You can continue.",
                            truncated
                        ));
                    }
                }
            }
        }

        let observed_tokens = self
            .current_stream_context_tokens()
            .unwrap_or(self.context_limit);
        manager.update_observed_input_tokens(observed_tokens);

        match manager.force_compact_with(&provider_messages, self.provider.clone()) {
            Ok(()) => Some(
                "⚡ Auto-compaction started — summarizing old messages in background. Retry in a moment."
                    .to_string(),
            ),
            Err(reason) => {
                crate::logging::error(&format!(
                    "[auto_recover] force_compact failed: {}",
                    reason
                ));
                match manager.hard_compact_with(&provider_messages) {
                    Ok(dropped) => {
                        self.sync_session_compaction_state_from_manager(&manager);
                        Some(format!(
                            "⚡ Emergency compaction: dropped {} old messages. You can continue.",
                            dropped
                        ))
                    }
                    Err(_) => {
                        let truncated = manager.emergency_truncate_with(&mut provider_messages);
                        if truncated > 0 {
                            self.messages = provider_messages;
                            Some(format!(
                                "⚡ Emergency truncation: shortened {} large tool result(s) to fit context. You can continue.",
                                truncated
                            ))
                        } else {
                            None
                        }
                    }
                }
            }
        }
    }

    /// Attempt automatic compaction and retry when context limit is exceeded.
    /// Returns true if the retry succeeded.
    pub(super) async fn try_auto_compact_and_retry(
        &mut self,
        terminal: &mut DefaultTerminal,
        event_stream: &mut EventStream,
    ) -> bool {
        if self.is_remote || !self.provider.supports_compaction() {
            return false;
        }

        self.push_display_message(DisplayMessage::system(
            "⚠️ Context limit exceeded — auto-compacting and retrying...".to_string(),
        ));

        // Force the compaction manager to think we're at the limit
        let compaction = self.registry.compaction();
        let compact_started = match compaction.try_write() {
            Ok(mut manager) => {
                let mut provider_messages = self.materialized_provider_messages();
                manager.update_observed_input_tokens(self.context_limit);
                let usage = manager.context_usage_with(&provider_messages);
                if usage > 1.5 {
                    match manager.hard_compact_with(&provider_messages) {
                        Ok(dropped) => {
                            self.sync_session_compaction_state_from_manager(&manager);
                            self.push_display_message(DisplayMessage::system(
                                format!(
                                    "⚡ Emergency compaction: dropped {} old messages (context was at {:.0}%).",
                                    dropped,
                                    usage * 100.0
                                ),
                            ));
                            drop(manager);
                            self.provider_session_id = None;
                            self.session.provider_session_id = None;
                            self.context_warning_shown = false;
                            self.clear_streaming_render_state();
                            self.stream_buffer.clear();
                            self.streaming_tool_calls.clear();
                            self.streaming_input_tokens = 0;
                            self.streaming_output_tokens = 0;
                            self.streaming_cache_read_tokens = None;
                            self.streaming_cache_creation_tokens = None;
                            self.thought_line_inserted = false;
                            self.thinking_prefix_emitted = false;
                            self.thinking_buffer.clear();
                            self.status = ProcessingStatus::Sending;

                            self.push_display_message(DisplayMessage::system(
                                "✓ Context compacted. Retrying...".to_string(),
                            ));
                            let retry_result =
                                self.run_turn_interactive(terminal, event_stream).await;
                            self.messages.clear();
                            return match retry_result {
                                Ok(()) => {
                                    self.last_stream_error = None;
                                    true
                                }
                                Err(e) => {
                                    self.handle_turn_error(crate::util::format_error_chain(&e));
                                    false
                                }
                            };
                        }
                        Err(_) => {
                            let truncated = manager.emergency_truncate_with(&mut provider_messages);
                            if truncated > 0 {
                                self.messages = provider_messages;
                                drop(manager);
                                self.provider_session_id = None;
                                self.session.provider_session_id = None;
                                self.context_warning_shown = false;
                                self.clear_streaming_render_state();
                                self.stream_buffer.clear();
                                self.streaming_tool_calls.clear();
                                self.streaming_input_tokens = 0;
                                self.streaming_output_tokens = 0;
                                self.streaming_cache_read_tokens = None;
                                self.streaming_cache_creation_tokens = None;
                                self.thought_line_inserted = false;
                                self.thinking_prefix_emitted = false;
                                self.thinking_buffer.clear();
                                self.status = ProcessingStatus::Sending;

                                self.push_display_message(DisplayMessage::system(
                                    format!("⚡ Emergency truncation: shortened {} large tool result(s). Retrying...", truncated),
                                ));
                                let retry_result =
                                    self.run_turn_interactive(terminal, event_stream).await;
                                self.messages.clear();
                                return match retry_result {
                                    Ok(()) => {
                                        self.last_stream_error = None;
                                        true
                                    }
                                    Err(e) => {
                                        self.handle_turn_error(crate::util::format_error_chain(&e));
                                        false
                                    }
                                };
                            }
                            false
                        }
                    }
                } else {
                    match manager.force_compact_with(&provider_messages, self.provider.clone()) {
                        Ok(()) => true,
                        Err(_) => match manager.hard_compact_with(&provider_messages) {
                            Ok(_) => {
                                self.sync_session_compaction_state_from_manager(&manager);
                                drop(manager);
                                self.provider_session_id = None;
                                self.session.provider_session_id = None;
                                self.context_warning_shown = false;
                                self.clear_streaming_render_state();
                                self.stream_buffer.clear();
                                self.streaming_tool_calls.clear();
                                self.streaming_input_tokens = 0;
                                self.streaming_output_tokens = 0;
                                self.streaming_cache_read_tokens = None;
                                self.streaming_cache_creation_tokens = None;
                                self.thought_line_inserted = false;
                                self.thinking_prefix_emitted = false;
                                self.thinking_buffer.clear();
                                self.status = ProcessingStatus::Sending;

                                self.push_display_message(DisplayMessage::system(
                                    "✓ Context compacted (emergency). Retrying...".to_string(),
                                ));
                                let retry_result =
                                    self.run_turn_interactive(terminal, event_stream).await;
                                self.messages.clear();
                                return match retry_result {
                                    Ok(()) => {
                                        self.last_stream_error = None;
                                        true
                                    }
                                    Err(e) => {
                                        self.handle_turn_error(crate::util::format_error_chain(&e));
                                        false
                                    }
                                };
                            }
                            Err(_) => false,
                        },
                    }
                }
            }
            Err(_) => false,
        };

        if !compact_started {
            return false;
        }

        // Wait for compaction to finish (up to 60s), reacting to Bus event
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        self.status = ProcessingStatus::RunningTool("compacting context...".to_string());
        let mut bus_rx = Bus::global().subscribe();

        loop {
            if std::time::Instant::now() >= deadline {
                self.push_display_message(DisplayMessage::error(
                    "Auto-compaction timed out.".to_string(),
                ));
                return false;
            }

            // Redraw UI while we wait
            let _ = terminal.draw(|frame| crate::tui::ui::draw(frame, self));

            let compaction = self.registry.compaction();
            let done = if let Ok(mut manager) = compaction.try_write() {
                let provider_messages = self.materialized_provider_messages();
                if let Some(event) = manager.poll_compaction_event_with(&provider_messages) {
                    self.sync_session_compaction_state_from_manager(&manager);
                    self.handle_compaction_event(event);
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if done {
                break;
            }

            // Wait for Bus notification or timeout (instead of sleep-polling)
            let timeout = tokio::time::sleep(Duration::from_secs(1));
            tokio::select! {
                _ = bus_rx.recv() => {}
                _ = timeout => {}
            }
        }

        self.push_display_message(DisplayMessage::system(
            "✓ Context compacted. Retrying...".to_string(),
        ));

        // Reset provider session since context changed
        self.provider_session_id = None;
        self.session.provider_session_id = None;
        self.context_warning_shown = false;

        // Clear streaming state for the retry
        self.clear_streaming_render_state();
        self.stream_buffer.clear();
        self.streaming_tool_calls.clear();
        self.streaming_input_tokens = 0;
        self.streaming_output_tokens = 0;
        self.streaming_cache_read_tokens = None;
        self.streaming_cache_creation_tokens = None;
        self.thought_line_inserted = false;
        self.thinking_prefix_emitted = false;
        self.thinking_buffer.clear();
        self.status = ProcessingStatus::Sending;

        // Retry the turn
        let result = self.run_turn_interactive(terminal, event_stream).await;
        self.messages.clear();
        match result {
            Ok(()) => {
                self.last_stream_error = None;
                true
            }
            Err(e) => {
                self.handle_turn_error(crate::util::format_error_chain(&e));
                false
            }
        }
    }

    pub(super) fn handle_usage_report(&mut self, results: Vec<crate::usage::ProviderUsage>) {
        self.usage_report_refreshing = false;
        self.clear_usage_transient_ui();
        self.upsert_usage_display_card(Self::format_usage_display_card(
            &results,
            false,
            results.len(),
            results.len(),
            false,
        ));
        if results.is_empty() {
            self.set_status_notice("Usage → no connected providers");
        } else {
            self.set_status_notice("Usage → updated");
        }
    }

    pub(super) fn handle_usage_report_progress(
        &mut self,
        progress: crate::usage::ProviderUsageProgress,
    ) {
        self.usage_report_refreshing = !progress.done;
        self.clear_usage_transient_ui();
        self.upsert_usage_display_card(Self::format_usage_display_card(
            &progress.results,
            !progress.done,
            progress.completed,
            progress.total,
            progress.from_cache,
        ));

        if progress.done {
            if progress.results.is_empty() {
                self.set_status_notice("Usage → no connected providers");
            } else {
                self.set_status_notice("Usage → updated");
            }
        } else if progress.from_cache && progress.total == 0 {
            self.set_status_notice("Usage → showing cached data, refreshing");
        } else if progress.total > 0 {
            self.set_status_notice(format!(
                "Usage → refreshing {}/{}",
                progress.completed.min(progress.total),
                progress.total
            ));
        } else {
            self.set_status_notice("Usage → refreshing");
        }
    }

    pub(super) fn push_usage_loading_card(&mut self) {
        self.clear_usage_transient_ui();
        self.push_display_message(DisplayMessage::usage(Self::format_usage_display_card(
            &[],
            true,
            0,
            0,
            false,
        )));
    }

    fn clear_usage_transient_ui(&mut self) {
        self.inline_view_state = None;
        self.usage_overlay = None;
        if self
            .inline_interactive_state
            .as_ref()
            .map(|picker| picker.kind == crate::tui::PickerKind::Usage)
            .unwrap_or(false)
        {
            self.inline_interactive_state = None;
        }
    }

    fn upsert_usage_display_card(&mut self, content: String) {
        let existing = self.display_messages.iter().rposition(|message| {
            message.role == "usage" && message.title.as_deref() == Some("Usage")
        });
        if let Some(idx) = existing {
            self.replace_display_message_title_and_content(idx, Some("Usage".to_string()), content);
        } else {
            self.push_display_message(DisplayMessage::usage(content));
        }
    }

    fn format_usage_display_card(
        reports: &[crate::usage::ProviderUsage],
        refreshing: bool,
        completed: usize,
        total: usize,
        from_cache: bool,
    ) -> String {
        let mut lines = Vec::new();

        if refreshing {
            if total > 0 {
                lines.push(format!(
                    "# Refreshing usage ({}/{})",
                    completed.min(total),
                    total
                ));
            } else if from_cache {
                lines.push("# Showing cached usage while refreshing".to_string());
            } else {
                lines.push("# Refreshing usage".to_string());
            }
            lines.push("Checking connected provider limits...".to_string());
            if !reports.is_empty() {
                lines.push(String::new());
            }
        } else if reports.is_empty() {
            lines.push("# No connected providers".to_string());
            lines.push(
                "Use `/login claude` or `/login openai`, then run `/usage` again.".to_string(),
            );
            return lines.join("\n");
        } else {
            lines.push(format!("# Usage updated · {} source(s)", reports.len()));
            lines.push(String::new());
        }

        for (idx, provider) in reports.iter().enumerate() {
            if idx > 0 {
                lines.push(String::new());
            }
            lines.push(Self::format_usage_provider_summary(provider));

            if let Some(error) = &provider.error {
                lines.push(format!("  error: {}", error));
                continue;
            }

            if provider.hard_limit_reached {
                lines.push("  hard limit reached".to_string());
            }

            if provider.limits.is_empty() && provider.extra_info.is_empty() {
                lines.push("  no usage data available".to_string());
                continue;
            }

            for limit in &provider.limits {
                let reset = limit
                    .resets_at
                    .as_deref()
                    .map(crate::usage::format_reset_time)
                    .map(|value| format!(" · resets in {}", value))
                    .unwrap_or_default();
                lines.push(format!(
                    "  {}: {}{}",
                    limit.name,
                    crate::usage::format_usage_bar(limit.usage_percent, 14),
                    reset
                ));
            }

            for (key, value) in &provider.extra_info {
                lines.push(format!("  {}: {}", key, value));
            }
        }

        lines.join("\n")
    }

    fn format_usage_provider_summary(provider: &crate::usage::ProviderUsage) -> String {
        if provider.error.is_some() {
            return format!("! {} — error", provider.provider_name);
        }
        if provider.hard_limit_reached {
            return format!("! {} — hard limit", provider.provider_name);
        }

        let max_percent = provider
            .limits
            .iter()
            .map(|limit| limit.usage_percent)
            .fold(0.0_f32, f32::max);
        if max_percent >= 90.0 {
            format!("! {} — {:.0}% used", provider.provider_name, max_percent)
        } else if max_percent >= 70.0 {
            format!("~ {} — {:.0}% used", provider.provider_name, max_percent)
        } else if provider.limits.is_empty() && provider.extra_info.is_empty() {
            format!("{} — no data", provider.provider_name)
        } else if max_percent > 0.0 {
            format!("+ {} — {:.0}% used", provider.provider_name, max_percent)
        } else {
            format!("+ {} — available", provider.provider_name)
        }
    }

    pub(super) fn run_fix_command(&mut self) {
        let mut actions: Vec<String> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        let last_error = self.last_stream_error.clone();
        let context_error = last_error
            .as_deref()
            .map(is_context_limit_error)
            .unwrap_or(false);

        let repaired = self.repair_missing_tool_outputs();
        if repaired > 0 {
            actions.push(format!("Recovered {} missing tool output(s).", repaired));
        }

        if self.summarize_tool_results_missing().is_some() {
            self.recover_session_without_tools();
            actions.push("Created a recovery session with text-only history.".to_string());
        }

        if self.provider_session_id.is_some() || self.session.provider_session_id.is_some() {
            self.provider_session_id = None;
            self.session.provider_session_id = None;
            actions.push("Reset provider session resume state.".to_string());
        }

        if !self.is_remote && self.provider.supports_compaction() {
            let observed_tokens = self
                .current_stream_context_tokens()
                .or_else(|| context_error.then_some(self.context_limit));
            let compaction = self.registry.compaction();
            match compaction.try_write() {
                Ok(mut manager) => {
                    let mut provider_messages = self.materialized_provider_messages();
                    if let Some(tokens) = observed_tokens {
                        manager.update_observed_input_tokens(tokens);
                    }
                    let usage = manager.context_usage_with(&provider_messages);
                    if usage > 1.5 {
                        match manager.hard_compact_with(&provider_messages) {
                            Ok(dropped) => {
                                self.sync_session_compaction_state_from_manager(&manager);
                                actions.push(format!(
                                    "Emergency compaction: dropped {} old messages (context was at {:.0}%).",
                                    dropped,
                                    usage * 100.0
                                ));
                            }
                            Err(reason) => {
                                notes.push(format!("Hard compaction failed: {}", reason));
                            }
                        }
                        let post_usage = manager.context_usage_with(&provider_messages);
                        if post_usage > 1.0 {
                            let truncated = manager.emergency_truncate_with(&mut provider_messages);
                            if truncated > 0 {
                                self.messages = provider_messages.clone();
                                actions.push(format!(
                                    "Emergency truncation: shortened {} large tool result(s) to fit context.",
                                    truncated
                                ));
                            }
                        }
                    } else {
                        match manager.force_compact_with(&provider_messages, self.provider.clone())
                        {
                            Ok(()) => {
                                actions.push("Started background context compaction.".to_string())
                            }
                            Err(reason) => match manager.hard_compact_with(&provider_messages) {
                                Ok(dropped) => {
                                    self.sync_session_compaction_state_from_manager(&manager);
                                    actions.push(format!(
                                            "Emergency compaction: dropped {} old messages (normal compaction failed: {}).",
                                            dropped, reason
                                        ));
                                }
                                Err(hard_reason) => {
                                    notes.push(format!(
                                        "Compaction not started: {}. Emergency fallback: {}",
                                        reason, hard_reason
                                    ));
                                }
                            },
                        }
                    }
                }
                Err(_) => notes.push("Could not access compaction manager (busy).".to_string()),
            };
        } else {
            notes.push("Compaction is unavailable for this provider.".to_string());
        }

        self.context_warning_shown = false;
        self.last_stream_error = None;
        self.set_status_notice("Fix applied");

        let mut content = String::from("**Fix Results:**\n");
        if actions.is_empty() {
            content.push_str("• No structural issues detected.\n");
        } else {
            for action in &actions {
                content.push_str(&format!("• {}\n", action));
            }
        }
        for note in &notes {
            content.push_str(&format!("• {}\n", note));
        }
        if let Some(last_error) = &last_error {
            content.push_str(&format!(
                "\nLast error: `{}`",
                crate::util::truncate_str(last_error, 200)
            ));
        }
        self.push_display_message(DisplayMessage::system(content));
    }
}

pub(super) fn handle_model_command(app: &mut App, trimmed: &str) -> bool {
    if is_refresh_model_list_command(trimmed) {
        app.set_status_notice("Refreshing model list...");
        let provider = app.provider.clone();
        let result = if let Ok(handle) = tokio::runtime::Handle::try_current() {
            tokio::task::block_in_place(|| handle.block_on(provider.refresh_model_catalog()))
        } else {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime.block_on(provider.refresh_model_catalog()),
                Err(error) => Err(anyhow::anyhow!(error)),
            }
        };

        match result {
            Ok(summary) => {
                app.push_display_message(DisplayMessage::system(format_model_refresh_summary(
                    &summary,
                )));
                app.set_status_notice(format!(
                    "Model list refreshed: +{} models, +{} routes, ~{} changed",
                    summary.models_added, summary.routes_added, summary.routes_changed
                ));
            }
            Err(error) => {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to refresh model list: {}",
                    error
                )));
                app.set_status_notice("Model list refresh failed");
            }
        }
        return true;
    }

    if trimmed == "/model" || trimmed == "/models" {
        app.open_model_picker();
        return true;
    }

    if let Some(model_name) = trimmed.strip_prefix("/model ") {
        let model_name = model_name.trim();
        match app.provider.set_model(model_name) {
            Ok(()) => {
                app.provider_session_id = None;
                app.session.provider_session_id = None;
                app.upstream_provider = None;
                app.invalidate_model_picker_cache();
                let active_model = app.provider.model();
                app.update_context_limit_for_model(&active_model);
                app.session.model = Some(active_model.clone());
                let _ = app.session.save();
                app.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: format!("✓ Switched to model: {}", active_model),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
                app.set_status_notice(format!("Model → {}", model_name));
            }
            Err(e) => {
                app.push_display_message(DisplayMessage::error(model_switch_failure_message(
                    &e.to_string(),
                    app.is_remote,
                )));
                app.set_status_notice("Model switch failed");
            }
        }
        return true;
    }

    if trimmed == "/effort" {
        let current = app.provider.reasoning_effort();
        let efforts = app.provider.available_efforts();
        if efforts.is_empty() {
            app.push_display_message(DisplayMessage::system(
                "Reasoning effort not available for this provider.".to_string(),
            ));
        } else {
            let current_label = current
                .as_deref()
                .map(effort_display_label)
                .unwrap_or("default");
            let list: Vec<String> = efforts
                .iter()
                .map(|e| {
                    if Some(e.to_string()) == current {
                        format!("**{}** ← current", effort_display_label(e))
                    } else {
                        effort_display_label(e).to_string()
                    }
                })
                .collect();
            app.push_display_message(DisplayMessage::system(format!(
                "Reasoning effort: {}\nAvailable: {}\nUse `/effort <level>` or Alt+←/→ to change.",
                current_label,
                list.join(" · ")
            )));
        }
        return true;
    }

    if let Some(level) = trimmed.strip_prefix("/effort ") {
        let level = level.trim();
        match app.provider.set_reasoning_effort(level) {
            Ok(()) => {
                let new_effort = app.provider.reasoning_effort();
                let label = new_effort
                    .as_deref()
                    .map(effort_display_label)
                    .unwrap_or("default");
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Reasoning effort → {}",
                    label
                )));
                let efforts = app.provider.available_efforts();
                let idx = new_effort
                    .as_ref()
                    .and_then(|e| efforts.iter().position(|x| *x == e.as_str()))
                    .unwrap_or(0);
                let bar = effort_bar(idx, efforts.len());
                app.set_status_notice(format!("Effort: {} {}", label, bar));
            }
            Err(e) => {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set effort: {}",
                    e
                )));
            }
        }
        return true;
    }

    if matches!(trimmed, "/fast default" | "/fast default status") {
        let default_tier = crate::config::Config::load().provider.openai_service_tier;
        let default_enabled = default_tier.as_deref() == Some("priority");
        let default_label = default_tier
            .as_deref()
            .map(service_tier_display_label)
            .unwrap_or("Standard");
        app.push_display_message(DisplayMessage::system(fast_mode_default_message(
            default_enabled,
            default_label,
        )));
        return true;
    }

    if let Some(mode) = trimmed.strip_prefix("/fast default ") {
        let mode = mode.trim().to_ascii_lowercase();
        match mode.as_str() {
            "on" => super::auth::save_openai_fast_setting_local(app, true),
            "off" => super::auth::save_openai_fast_setting_local(app, false),
            "status" => {
                let default_tier = crate::config::Config::load().provider.openai_service_tier;
                let default_enabled = default_tier.as_deref() == Some("priority");
                let default_label = default_tier
                    .as_deref()
                    .map(service_tier_display_label)
                    .unwrap_or("Standard");
                app.push_display_message(DisplayMessage::system(fast_mode_default_message(
                    default_enabled,
                    default_label,
                )));
            }
            _ => {
                app.push_display_message(DisplayMessage::error(
                    "Usage: /fast default [on|off|status]".to_string(),
                ));
            }
        }
        return true;
    }

    if matches!(trimmed, "/fast" | "/fast status") {
        let current = app.provider.service_tier();
        let status = if current.as_deref() == Some("priority") {
            "on"
        } else {
            "off"
        };
        let current_label = current
            .as_deref()
            .map(service_tier_display_label)
            .unwrap_or("Standard");
        let default_tier = crate::config::Config::load().provider.openai_service_tier;
        let default_enabled = default_tier.as_deref() == Some("priority");
        let default_label = default_tier
            .as_deref()
            .map(service_tier_display_label)
            .unwrap_or("Standard");
        app.push_display_message(DisplayMessage::system(fast_mode_overview_message(
            status == "on",
            current_label,
            default_enabled,
            default_label,
        )));
        return true;
    }

    if let Some(mode) = trimmed.strip_prefix("/fast ") {
        let mode = mode.trim().to_ascii_lowercase();
        let target = match mode.as_str() {
            "on" => "priority",
            "off" => "off",
            "status" => {
                let current = app.provider.service_tier();
                let enabled = current.as_deref() == Some("priority");
                let current_label = current
                    .as_deref()
                    .map(service_tier_display_label)
                    .unwrap_or("Standard");
                let default_tier = crate::config::Config::load().provider.openai_service_tier;
                let default_enabled = default_tier.as_deref() == Some("priority");
                let default_label = default_tier
                    .as_deref()
                    .map(service_tier_display_label)
                    .unwrap_or("Standard");
                app.push_display_message(DisplayMessage::system(fast_mode_overview_message(
                    enabled,
                    current_label,
                    default_enabled,
                    default_label,
                )));
                return true;
            }
            _ => {
                app.push_display_message(DisplayMessage::error(
                    "Usage: /fast [on|off|status|default ...]".to_string(),
                ));
                return true;
            }
        };

        match app.provider.set_service_tier(target) {
            Ok(()) => {
                let current = app.provider.service_tier();
                let enabled = current.as_deref() == Some("priority");
                let label = current
                    .as_deref()
                    .map(service_tier_display_label)
                    .unwrap_or("Standard");
                let applies_next_request = app.is_processing;
                app.push_display_message(DisplayMessage::system(fast_mode_success_message(
                    enabled,
                    label,
                    applies_next_request,
                )));
                app.set_status_notice(fast_mode_status_notice(enabled, applies_next_request));
            }
            Err(e) => {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set fast mode: {}",
                    e
                )));
            }
        }
        return true;
    }

    if trimmed == "/transport" {
        let current = app.provider.transport();
        let transports = app.provider.available_transports();
        if transports.is_empty() {
            app.push_display_message(DisplayMessage::system(
                "Transport switching is not available for this provider.".to_string(),
            ));
        } else {
            let current_label = current.as_deref().unwrap_or("unknown");
            let list: Vec<String> = transports
                .iter()
                .map(|t| {
                    if Some(*t) == current.as_deref() {
                        format!("**{}** ← current", t)
                    } else {
                        t.to_string()
                    }
                })
                .collect();
            app.push_display_message(DisplayMessage::system(format!(
                "Transport: {}\nAvailable: {}\nUse `/transport <mode>` to change.",
                current_label,
                list.join(" · ")
            )));
        }
        return true;
    }

    if let Some(mode) = trimmed.strip_prefix("/transport ") {
        let mode = mode.trim();
        match app.provider.set_transport(mode) {
            Ok(()) => {
                let new_transport = app.provider.transport().unwrap_or_else(|| mode.to_string());
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Transport → {}",
                    new_transport
                )));
                app.set_status_notice(format!("Transport → {}", new_transport));
            }
            Err(e) => {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set transport: {}",
                    e
                )));
            }
        }
        return true;
    }

    false
}

pub(super) fn is_refresh_model_list_command(trimmed: &str) -> bool {
    trimmed == "/refresh-model-list"
}

pub(super) fn format_model_refresh_summary(
    summary: &crate::provider::ModelCatalogRefreshSummary,
) -> String {
    format!(
        "**Model List Refresh Complete**\n\nModels: {} → {}  (+{} / -{})\nRoutes: {} → {}  (+{} / -{} / ~{})",
        summary.model_count_before,
        summary.model_count_after,
        summary.models_added,
        summary.models_removed,
        summary.route_count_before,
        summary.route_count_after,
        summary.routes_added,
        summary.routes_removed,
        summary.routes_changed,
    )
}

pub(super) fn no_models_available_message(is_remote: bool) -> String {
    let mut lines = vec![
        "No models are available right now.".to_string(),
        String::new(),
        "Next steps:".to_string(),
        "- Run `/login` to connect or refresh a provider".to_string(),
        "- Run `/account` to inspect or switch credentials".to_string(),
        "- If you just logged in, wait a moment and try `/model` again".to_string(),
    ];

    if is_remote {
        lines.push(
            "- If this is a remote session, reconnect if the server model list looks stale"
                .to_string(),
        );
    }

    lines.join("\n")
}

pub(super) fn model_switch_failure_message(error: &str, is_remote: bool) -> String {
    let mut lines = vec![
        format!("Failed to switch model: {}", error),
        String::new(),
        "Next steps:".to_string(),
        "- Use `/model` to choose another available route".to_string(),
        "- Run `/login` to add or refresh credentials".to_string(),
        "- Run `/account` to inspect or switch accounts".to_string(),
    ];

    if is_remote {
        lines.push(
            "- If this is a remote session and the list looks stale, reconnect and try again"
                .to_string(),
        );
    }

    lines.join("\n")
}

pub(super) fn unavailable_model_route_message(
    model: &str,
    provider: &str,
    detail: &str,
    is_remote: bool,
) -> String {
    let reason = if detail.trim().is_empty() {
        "This route is not currently available.".to_string()
    } else {
        format!("This route is not currently available: {}", detail.trim())
    };

    let mut lines = vec![
        format!("Cannot use `{}` via **{}** right now.", model, provider),
        String::new(),
        reason,
        String::new(),
        "Next steps:".to_string(),
        "- Pick another available row in `/model`".to_string(),
        "- Run `/login` to add or refresh credentials".to_string(),
        "- Run `/account` to inspect or switch accounts".to_string(),
    ];

    if is_remote {
        lines.push(
            "- If this is a remote session, wait a moment or reconnect if the catalog looks stale"
                .to_string(),
        );
    }

    lines.join("\n")
}
