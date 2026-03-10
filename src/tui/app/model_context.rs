use super::*;

impl App {
    pub(super) fn cycle_model(&mut self, direction: i8) {
        let models = self.provider.available_models();
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
        let next_model = models[next_index];

        match self.provider.set_model(next_model) {
            Ok(()) => {
                self.provider_session_id = None;
                self.session.provider_session_id = None;
                self.upstream_provider = None;
                self.connection_type = None;
                self.update_context_limit_for_model(next_model);
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
        if self.is_remote || !self.provider.supports_compaction() {
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

        let usage = manager.context_usage_with(&self.messages);
        if usage > 1.5 {
            match manager.hard_compact_with(&self.messages) {
                Ok(dropped) => {
                    let post_usage = manager.context_usage_with(&self.messages);
                    if post_usage <= 1.0 {
                        return Some(format!(
                            "⚡ Emergency compaction: dropped {} old messages (context was at {:.0}%). You can continue.",
                            dropped,
                            usage * 100.0
                        ));
                    }
                    let truncated = manager.emergency_truncate_with(&mut self.messages);
                    return Some(format!(
                        "⚡ Emergency compaction: dropped {} old messages and truncated {} tool result(s) (context was at {:.0}%). You can continue.",
                        dropped, truncated,
                        usage * 100.0
                    ));
                }
                Err(reason) => {
                    crate::logging::error(&format!(
                        "[auto_recover] hard_compact failed: {}",
                        reason
                    ));
                    let truncated = manager.emergency_truncate_with(&mut self.messages);
                    if truncated > 0 {
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
            .unwrap_or(self.context_limit as u64);
        manager.update_observed_input_tokens(observed_tokens);

        match manager.force_compact_with(&self.messages, self.provider.clone()) {
            Ok(()) => Some(
                "⚡ Auto-compaction started — summarizing old messages in background. Retry in a moment."
                    .to_string(),
            ),
            Err(reason) => {
                crate::logging::error(&format!(
                    "[auto_recover] force_compact failed: {}",
                    reason
                ));
                match manager.hard_compact_with(&self.messages) {
                    Ok(dropped) => Some(format!(
                        "⚡ Emergency compaction: dropped {} old messages. You can continue.",
                        dropped
                    )),
                    Err(_) => {
                        let truncated = manager.emergency_truncate_with(&mut self.messages);
                        if truncated > 0 {
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
                manager.update_observed_input_tokens(self.context_limit);
                let usage = manager.context_usage_with(&self.messages);
                if usage > 1.5 {
                    match manager.hard_compact_with(&self.messages) {
                        Ok(dropped) => {
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
                            return match self.run_turn_interactive(terminal, event_stream).await {
                                Ok(()) => {
                                    self.last_stream_error = None;
                                    true
                                }
                                Err(e) => {
                                    self.handle_turn_error(e.to_string());
                                    false
                                }
                            };
                        }
                        Err(_) => {
                            let truncated = manager.emergency_truncate_with(&mut self.messages);
                            if truncated > 0 {
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
                                return match self.run_turn_interactive(terminal, event_stream).await
                                {
                                    Ok(()) => {
                                        self.last_stream_error = None;
                                        true
                                    }
                                    Err(e) => {
                                        self.handle_turn_error(e.to_string());
                                        false
                                    }
                                };
                            }
                            false
                        }
                    }
                } else {
                    match manager.force_compact_with(&self.messages, self.provider.clone()) {
                        Ok(()) => true,
                        Err(_) => match manager.hard_compact_with(&self.messages) {
                            Ok(_) => {
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
                                return match self.run_turn_interactive(terminal, event_stream).await
                                {
                                    Ok(()) => {
                                        self.last_stream_error = None;
                                        true
                                    }
                                    Err(e) => {
                                        self.handle_turn_error(e.to_string());
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
                if let Some(event) = manager.poll_compaction_event() {
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
        match self.run_turn_interactive(terminal, event_stream).await {
            Ok(()) => {
                self.last_stream_error = None;
                true
            }
            Err(e) => {
                self.handle_turn_error(e.to_string());
                false
            }
        }
    }

    pub(super) fn handle_usage_report(&mut self, results: Vec<crate::usage::ProviderUsage>) {
        if results.is_empty() {
            self.push_display_message(DisplayMessage::system(
                "No providers with OAuth credentials found.\n\
                 Use `/login anthropic` or `/login openai` to authenticate."
                    .to_string(),
            ));
            return;
        }

        let mut output = String::from("## Subscription Usage\n\n");

        for (i, provider) in results.iter().enumerate() {
            if i > 0 {
                output.push_str("---\n\n");
            }
            output.push_str(&format!("### {}\n\n", provider.provider_name));

            if let Some(ref err) = provider.error {
                output.push_str(&format!("⚠ {}\n\n", err));
                continue;
            }

            if provider.limits.is_empty() && provider.extra_info.is_empty() {
                output.push_str("No usage data available\n\n");
                continue;
            }

            for limit in &provider.limits {
                let bar = crate::usage::format_usage_bar(limit.usage_percent, 15);
                let reset_info = if let Some(ref ts) = limit.resets_at {
                    let relative = crate::usage::format_reset_time(ts);
                    format!(" (resets in {})", relative)
                } else {
                    String::new()
                };
                output.push_str(&format!("- **{}**: {}{}\n", limit.name, bar, reset_info));
            }

            if !provider.limits.is_empty() {
                output.push('\n');
            }

            for (key, value) in &provider.extra_info {
                output.push_str(&format!("- {}: {}\n", key, value));
            }
            output.push('\n');
        }

        if self.total_input_tokens > 0 || self.total_output_tokens > 0 {
            output.push_str("---\n\n### Session Usage\n\n");
            output.push_str(&format!(
                "- **Input tokens:** {}\n- **Output tokens:** {}\n",
                self.total_input_tokens, self.total_output_tokens
            ));
            if self.total_cost > 0.0 {
                output.push_str(&format!("- **Cost:** ${:.4}\n", self.total_cost));
            }
            output.push('\n');
        }

        self.push_display_message(DisplayMessage::system(output));
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
                    if let Some(tokens) = observed_tokens {
                        manager.update_observed_input_tokens(tokens);
                    }
                    let usage = manager.context_usage_with(&self.messages);
                    if usage > 1.5 {
                        match manager.hard_compact_with(&self.messages) {
                            Ok(dropped) => {
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
                        let post_usage = manager.context_usage_with(&self.messages);
                        if post_usage > 1.0 {
                            let truncated = manager.emergency_truncate_with(&mut self.messages);
                            if truncated > 0 {
                                actions.push(format!(
                                    "Emergency truncation: shortened {} large tool result(s) to fit context.",
                                    truncated
                                ));
                            }
                        }
                    } else {
                        match manager.force_compact_with(&self.messages, self.provider.clone()) {
                            Ok(()) => {
                                actions.push("Started background context compaction.".to_string())
                            }
                            Err(reason) => match manager.hard_compact_with(&self.messages) {
                                Ok(dropped) => {
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
                app.connection_type = None;
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
                app.push_display_message(DisplayMessage {
                    role: "error".to_string(),
                    content: format!("Failed to switch model: {}", e),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
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
