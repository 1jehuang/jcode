use super::*;

impl App {
    fn append_current_turn_system_reminder(&self, split: &mut crate::prompt::SplitSystemPrompt) {
        let Some(reminder) = self
            .current_turn_system_reminder
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        else {
            return;
        };

        if !split.dynamic_part.is_empty() {
            split.dynamic_part.push_str("\n\n");
        }
        split.dynamic_part.push_str("# System Reminder\n\n");
        split.dynamic_part.push_str(reminder);
    }

    async fn run_turn(&mut self) -> Result<()> {
        loop {
            let repaired = self.repair_missing_tool_outputs();
            if repaired > 0 {
                let message = format!(
                    "Recovered {} missing tool output(s) from an interrupted turn.",
                    repaired
                );
                self.push_display_message(DisplayMessage::system(message));
                self.set_status_notice("Recovered missing tool outputs");
            }
            if let Some(summary) = self.summarize_tool_results_missing() {
                let message = format!(
                    "Tool outputs are missing for this turn. {}\n\nPress Ctrl+R to recover into a new session with context copied.",
                    summary
                );
                self.push_display_message(DisplayMessage::error(message));
                self.set_status_notice("Recovery needed");
                return Ok(());
            }

            let (provider_messages, compaction_event) = self.messages_for_provider();
            if let Some(event) = compaction_event {
                self.handle_compaction_event(event);
            }

            let tools = self.registry.definitions(None).await;
            // Non-blocking memory: uses pending result from last turn, spawns check for next turn
            let memory_pending = self.build_memory_prompt_nonblocking(&provider_messages);
            // Use split prompt for better caching - static content cached, dynamic not
            let split_prompt =
                self.build_system_prompt_split(memory_pending.as_ref().map(|p| p.prompt.as_str()));
            if let Some(pending) = &memory_pending {
                let age_ms = pending.computed_at.elapsed().as_millis() as u64;
                self.show_injected_memory_context(
                    &pending.prompt,
                    pending.display_prompt.as_deref(),
                    pending.count,
                    age_ms,
                    pending.memory_ids.clone(),
                );
            }

            self.status = ProcessingStatus::Sending;
            let stamped;
            let request_messages = provider_messages;
            let send_messages = if crate::config::config().features.message_timestamps {
                stamped = Message::with_timestamps(&request_messages);
                &stamped
            } else {
                &request_messages
            };
            let mut stream = self
                .provider
                .complete_split(
                    send_messages,
                    &tools,
                    &split_prompt.static_part,
                    &split_prompt.dynamic_part,
                    self.provider_session_id.as_deref(),
                )
                .await?;

            let mut text_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut first_event = true;
            let mut saw_message_end = false;
            let mut call_output_tokens_seen: u64 = 0;
            let store_reasoning_content = self.provider.name() == "openrouter";
            let mut reasoning_content = String::new();
            // Track tool results from provider (already executed by Claude Code CLI)
            let mut sdk_tool_results: std::collections::HashMap<String, (String, bool)> =
                std::collections::HashMap::new();
            let mut openai_native_compaction: Option<(String, usize)> = None;

            while let Some(event) = stream.next().await {
                // Track activity for status display
                self.last_stream_activity = Some(Instant::now());

                // Poll for background compaction completion during streaming
                self.poll_compaction_completion();

                if first_event {
                    first_event = false;
                }
                match event? {
                    StreamEvent::TextDelta(text) => {
                        self.status = ProcessingStatus::Streaming;
                        text_content.push_str(&text);
                        self.resume_streaming_tps();
                        if let Some(chunk) = self.stream_buffer.push(&text) {
                            self.streaming_text.push_str(&chunk);
                        }
                    }
                    StreamEvent::ToolUseStart { id, name } => {
                        self.status = ProcessingStatus::Streaming;
                        self.pause_streaming_tps(false);
                        self.commit_pending_streaming_assistant_message();
                        current_tool = Some(ToolCall {
                            id,
                            name,
                            input: serde_json::Value::Null,
                            intent: None,
                        });
                        current_tool_input.clear();
                    }
                    StreamEvent::ToolInputDelta(delta) => {
                        current_tool_input.push_str(&delta);
                    }
                    StreamEvent::ToolUseEnd => {
                        self.pause_streaming_tps(false);
                        if let Some(mut tool) = current_tool.take() {
                            tool.input = serde_json::from_str(&current_tool_input)
                                .unwrap_or(serde_json::Value::Null);
                            self.commit_pending_streaming_assistant_message();

                            // Add tool call as its own display message
                            self.push_display_message(DisplayMessage {
                                role: "tool".to_string(),
                                content: tool.name.clone(),
                                tool_calls: vec![],
                                duration_secs: None,
                                title: None,
                                tool_data: Some(tool.clone()),
                            });

                            tool_calls.push(tool);
                            current_tool_input.clear();
                        }
                    }
                    StreamEvent::TokenUsage {
                        input_tokens,
                        output_tokens,
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    } => {
                        let mut usage_changed = false;
                        if let Some(input) = input_tokens {
                            self.streaming_input_tokens = input;
                            usage_changed = true;
                        }
                        if let Some(output) = output_tokens {
                            self.streaming_output_tokens = output;
                            self.accumulate_streaming_output_tokens(
                                output,
                                &mut call_output_tokens_seen,
                            );
                        }
                        if cache_read_input_tokens.is_some() {
                            self.streaming_cache_read_tokens = cache_read_input_tokens;
                            usage_changed = true;
                        }
                        if cache_creation_input_tokens.is_some() {
                            self.streaming_cache_creation_tokens = cache_creation_input_tokens;
                            usage_changed = true;
                        }
                        if usage_changed {
                            self.update_compaction_usage_from_stream();
                            if let Some(context_tokens) = self.current_stream_context_tokens() {
                                self.check_context_warning(context_tokens);
                            }
                        }
                    }
                    StreamEvent::ConnectionType { connection } => {
                        self.connection_type = Some(connection);
                        self.update_terminal_title();
                    }
                    StreamEvent::ConnectionPhase { phase } => {
                        self.status = if matches!(phase, crate::message::ConnectionPhase::Streaming)
                        {
                            ProcessingStatus::Streaming
                        } else {
                            ProcessingStatus::Connecting(phase)
                        };
                    }
                    StreamEvent::StatusDetail { detail } => {
                        self.status_detail = Some(detail);
                    }
                    StreamEvent::MessageEnd { .. } => {
                        self.pause_streaming_tps(true);
                        saw_message_end = true;
                    }
                    StreamEvent::SessionId(sid) => {
                        self.provider_session_id = Some(sid);
                        if saw_message_end {
                            break;
                        }
                    }
                    StreamEvent::Error {
                        message,
                        retry_after_secs,
                    } => {
                        // Check if this is a rate limit error
                        // First try the explicit retry_after_secs, then fall back to parsing message
                        let reset_duration = retry_after_secs
                            .map(Duration::from_secs)
                            .or_else(|| parse_rate_limit_error(&message));

                        if let Some(reset_duration) = reset_duration {
                            let reset_time = Instant::now() + reset_duration;
                            self.rate_limit_reset = Some(reset_time);
                            // Don't return error - the queued message will retry
                            let queued_info = if !self.queued_messages.is_empty() {
                                format!(" ({} messages queued)", self.queued_messages.len())
                            } else {
                                String::new()
                            };
                            self.push_display_message(DisplayMessage::system(format!(
                                "⏳ Rate limit hit. Will auto-retry in {} seconds...{}",
                                reset_duration.as_secs(),
                                queued_info
                            )));
                            self.status = ProcessingStatus::Idle;
                            self.clear_streaming_render_state();
                            return Ok(());
                        }
                        return Err(anyhow::anyhow!("Stream error: {}", message));
                    }
                    StreamEvent::ThinkingStart => {
                        // Track start and update status for real-time indicator
                        let start = Instant::now();
                        self.thinking_start = Some(start);
                        self.thinking_buffer.clear();
                        self.thinking_prefix_emitted = false;
                        // Always show Thinking in status bar (even when thinking content is visible)
                        self.status = ProcessingStatus::Thinking(start);
                    }
                    StreamEvent::ThinkingDelta(thinking_text) => {
                        // Buffer thinking content and emit with prefix only once
                        self.thinking_buffer.push_str(&thinking_text);
                        // Flush any pending text first
                        if let Some(chunk) = self.stream_buffer.flush() {
                            self.streaming_text.push_str(&chunk);
                        }
                        // Only show thinking content if enabled in config
                        if config().display.show_thinking {
                            // Only emit the prefix once at the start of thinking
                            if !self.thinking_prefix_emitted
                                && !self.thinking_buffer.trim().is_empty()
                            {
                                self.insert_thought_line(format!(
                                    "💭 {}",
                                    self.thinking_buffer.trim_start()
                                ));
                                self.thinking_prefix_emitted = true;
                                self.thinking_buffer.clear();
                            } else if self.thinking_prefix_emitted {
                                // After prefix is emitted, append subsequent chunks directly
                                self.streaming_text.push_str(&thinking_text);
                            }
                        }
                        if store_reasoning_content {
                            reasoning_content.push_str(&thinking_text);
                        }
                    }
                    StreamEvent::ThinkingEnd => {
                        // Don't display here - ThinkingDone has accurate timing
                        self.thinking_start = None;
                        self.thinking_buffer.clear();
                    }
                    StreamEvent::ThinkingDone { duration_secs } => {
                        // Flush any pending buffered text first
                        if let Some(chunk) = self.stream_buffer.flush() {
                            self.streaming_text.push_str(&chunk);
                        }
                        // Bridge provides accurate wall-clock timing
                        let thinking_msg = format!("*Thought for {:.1}s*", duration_secs);
                        self.insert_thought_line(thinking_msg);
                        self.thinking_prefix_emitted = false;
                        self.thinking_buffer.clear();
                    }
                    StreamEvent::Compaction {
                        trigger,
                        pre_tokens,
                        openai_encrypted_content,
                    } => {
                        if let Some(encrypted_content) = openai_encrypted_content {
                            openai_native_compaction
                                .get_or_insert_with(|| (encrypted_content, self.messages.len()));
                        }
                        // Flush any pending buffered text first
                        if let Some(chunk) = self.stream_buffer.flush() {
                            self.streaming_text.push_str(&chunk);
                        }
                        let tokens_str = pre_tokens
                            .map(|t| format!(" (was {} tokens)", t))
                            .unwrap_or_default();
                        let compact_msg = format!(
                            "📦 **Compaction complete** — context summarized ({}){}\n\n",
                            trigger, tokens_str
                        );
                        self.streaming_text.push_str(&compact_msg);
                        // Reset warning so it can appear again
                        self.context_warning_shown = false;
                    }
                    StreamEvent::UpstreamProvider { provider } => {
                        // Store the upstream provider (e.g., Fireworks, Together)
                        self.upstream_provider = Some(provider);
                    }
                    StreamEvent::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        // SDK already executed this tool, store result for later
                        self.tool_result_ids.insert(tool_use_id.clone());
                        sdk_tool_results.insert(tool_use_id, (content, is_error));
                    }
                    StreamEvent::NativeToolCall {
                        request_id,
                        tool_name,
                        input,
                    } => {
                        // Execute native tool and send result back to SDK bridge
                        let ctx = crate::tool::ToolContext {
                            session_id: self.session_id().to_string(),
                            message_id: self.session_id().to_string(),
                            tool_call_id: request_id.clone(),
                            working_dir: self.session.working_dir.as_deref().map(PathBuf::from),
                            stdin_request_tx: None,
                            graceful_shutdown_signal: None,
                            execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
                        };
                        let tool_result = self.registry.execute(&tool_name, input, ctx).await;
                        let native_result = match tool_result {
                            Ok(output) => crate::provider::NativeToolResult::success(
                                request_id,
                                output.output,
                            ),
                            Err(e) => {
                                crate::provider::NativeToolResult::error(request_id, e.to_string())
                            }
                        };
                        if let Some(sender) = self.provider.native_result_sender() {
                            let _ = sender.send(native_result).await;
                        }
                    }
                }
            }

            // Add assistant message to history
            let mut content_blocks = Vec::new();
            if !text_content.is_empty() {
                content_blocks.push(ContentBlock::Text {
                    text: text_content.clone(),
                    cache_control: None,
                });
            }
            if store_reasoning_content && !reasoning_content.is_empty() {
                content_blocks.push(ContentBlock::Reasoning {
                    text: reasoning_content.clone(),
                });
            }
            for tc in &tool_calls {
                content_blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.input.clone(),
                });
            }

            let assistant_message_id = if !content_blocks.is_empty() {
                crate::telemetry::record_assistant_response();
                let content_clone = content_blocks.clone();
                self.add_provider_message(Message {
                    role: Role::Assistant,
                    content: content_blocks,
                    timestamp: Some(chrono::Utc::now()),
                    tool_duration_ms: None,
                });
                let message_id = self.session.add_message(Role::Assistant, content_clone);
                let _ = self.session.save();
                for tc in &tool_calls {
                    self.tool_result_ids.insert(tc.id.clone());
                }
                Some(message_id)
            } else {
                None
            };

            if let Some((encrypted_content, compacted_count)) = openai_native_compaction.take() {
                self.apply_openai_native_compaction(encrypted_content, compacted_count)?;
            }

            // Add remaining text to display
            let duration = self.processing_started.map(|t| t.elapsed().as_secs_f32());

            // Flush any remaining buffered text
            if let Some(chunk) = self.stream_buffer.flush() {
                self.streaming_text.push_str(&chunk);
            }

            if tool_calls.is_empty() {
                // No tool calls - display full text_content
                if !text_content.is_empty() {
                    self.push_display_message(DisplayMessage {
                        role: "assistant".to_string(),
                        content: text_content.clone(),
                        tool_calls: vec![],
                        duration_secs: duration,
                        title: None,
                        tool_data: None,
                    });
                    self.push_turn_footer(duration);
                }
            } else {
                // Had tool calls - only display text that came AFTER the last tool
                // (text before each tool was already committed in ToolUseEnd handler)
                if !self.streaming_text.is_empty() {
                    self.push_display_message(DisplayMessage {
                        role: "assistant".to_string(),
                        content: self.streaming_text.clone(),
                        tool_calls: vec![],
                        duration_secs: duration,
                        title: None,
                        tool_data: None,
                    });
                    self.push_turn_footer(duration);
                }
            }
            self.clear_streaming_render_state();
            self.stream_buffer.clear();
            self.streaming_tool_calls.clear();

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                break;
            }

            // Execute tools - SDK may have executed some, but custom tools need local execution
            // Note: handles_tools_internally() means SDK handled KNOWN tools, but custom tools like
            // selfdev are not known to the SDK and need to be executed locally.
            for tc in tool_calls {
                self.status = ProcessingStatus::RunningTool(tc.name.clone());
                self.observe_tool_call(&tc);
                if matches!(tc.name.as_str(), "memory") {
                    crate::memory::set_state(crate::tui::info_widget::MemoryState::Embedding);
                }
                let message_id = assistant_message_id
                    .clone()
                    .unwrap_or_else(|| self.session.id.clone());

                // Check if SDK already executed this tool
                let (output, is_error, tool_title, tool_duration_ms) =
                    if let Some((sdk_content, sdk_is_error)) = sdk_tool_results.remove(&tc.id) {
                        // Use SDK result
                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: if sdk_is_error {
                                ToolStatus::Error
                            } else {
                                ToolStatus::Completed
                            },
                            title: None,
                        }));
                        (sdk_content, sdk_is_error, None, None)
                    } else {
                        // Execute locally
                        let ctx = ToolContext {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            working_dir: self.session.working_dir.as_deref().map(PathBuf::from),
                            stdin_request_tx: None,
                            graceful_shutdown_signal: None,
                            execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
                        };

                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: ToolStatus::Running,
                            title: None,
                        }));

                        let tool_start = Instant::now();
                        let result = self.registry.execute(&tc.name, tc.input.clone(), ctx).await;
                        let tool_duration_ms = tool_start.elapsed().as_millis() as u64;
                        crate::telemetry::record_tool_call();
                        match result {
                            Ok(o) => {
                                Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                                    session_id: self.session.id.clone(),
                                    message_id: message_id.clone(),
                                    tool_call_id: tc.id.clone(),
                                    tool_name: tc.name.clone(),
                                    status: ToolStatus::Completed,
                                    title: o.title.clone(),
                                }));
                                (o.output, false, o.title, Some(tool_duration_ms))
                            }
                            Err(e) => {
                                crate::telemetry::record_tool_failure();
                                Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                                    session_id: self.session.id.clone(),
                                    message_id: message_id.clone(),
                                    tool_call_id: tc.id.clone(),
                                    tool_name: tc.name.clone(),
                                    status: ToolStatus::Error,
                                    title: None,
                                }));
                                (format!("Error: {}", e), true, None, Some(tool_duration_ms))
                            }
                        }
                    };

                // Update the tool's DisplayMessage with the output
                let _ = self.replace_latest_tool_display_message(
                    &tc.id,
                    tool_title.clone(),
                    output.clone(),
                );

                self.add_provider_message(Message::tool_result_with_duration(
                    &tc.id,
                    &output,
                    is_error,
                    tool_duration_ms,
                ));
                self.session.add_message_with_duration(
                    Role::User,
                    vec![ContentBlock::ToolResult {
                        tool_use_id: tc.id.clone(),
                        content: output.clone(),
                        is_error: if is_error { Some(true) } else { None },
                    }],
                    tool_duration_ms,
                );
                self.observe_tool_result(&tc, &output, is_error, tool_title.as_deref());
                let _ = self.session.save();
            }
        }

        Ok(())
    }

    /// Run turn with interactive input handling (redraws UI, accepts input during streaming)
    pub(super) async fn run_turn_interactive(
        &mut self,
        terminal: &mut DefaultTerminal,
        event_stream: &mut EventStream,
    ) -> Result<()> {
        let mut redraw_period = crate::tui::redraw_interval(self);
        let mut redraw_interval = interval(redraw_period);

        loop {
            let desired_redraw = crate::tui::redraw_interval(self);
            if desired_redraw != redraw_period {
                redraw_period = desired_redraw;
                redraw_interval = interval(redraw_period);
            }

            self.status = ProcessingStatus::Sending;
            terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;
            self.flush_pending_session_save();

            let repaired = self.repair_missing_tool_outputs();
            if repaired > 0 {
                let message = format!(
                    "Recovered {} missing tool output(s) from an interrupted turn.",
                    repaired
                );
                self.push_display_message(DisplayMessage::system(message));
                self.set_status_notice("Recovered missing tool outputs");
            }
            if let Some(summary) = self.summarize_tool_results_missing() {
                let message = format!(
                    "Tool outputs are missing for this turn. {}\n\nPress Ctrl+R to recover into a new session with context copied.",
                    summary
                );
                self.push_display_message(DisplayMessage::error(message));
                self.set_status_notice("Recovery needed");
                return Ok(());
            }

            let (provider_messages, compaction_event) = self.messages_for_provider();
            if let Some(event) = compaction_event {
                self.handle_compaction_event(event);
            }

            let tools = self.registry.definitions(None).await;
            // Non-blocking memory: uses pending result from last turn, spawns check for next turn
            let memory_pending = self.build_memory_prompt_nonblocking(&provider_messages);
            // Use split prompt for better caching - static content cached, dynamic not
            let split_prompt =
                self.build_system_prompt_split(memory_pending.as_ref().map(|p| p.prompt.as_str()));
            if let Some(pending) = &memory_pending {
                let age_ms = pending.computed_at.elapsed().as_millis() as u64;
                self.show_injected_memory_context(
                    &pending.prompt,
                    pending.display_prompt.as_deref(),
                    pending.count,
                    age_ms,
                    pending.memory_ids.clone(),
                );
            }

            crate::logging::info(&format!(
                "TUI: API call starting ({} messages)",
                provider_messages.len()
            ));
            let api_start = std::time::Instant::now();

            // Clone data needed for the API call to avoid borrow issues
            // The future would hold references across the select! which conflicts with handle_key
            let provider = self.provider.clone();
            let request_messages = provider_messages;
            let messages_clone = if crate::config::config().features.message_timestamps {
                Message::with_timestamps(&request_messages)
            } else {
                request_messages.clone()
            };
            let session_id_clone = self.provider_session_id.clone();
            let static_part = split_prompt.static_part.clone();
            let dynamic_part = split_prompt.dynamic_part.clone();

            // Make API call non-blocking - poll it in select! so we can handle input while waiting
            let mut api_future = std::pin::pin!(provider.complete_split(
                &messages_clone,
                &tools,
                &static_part,
                &dynamic_part,
                session_id_clone.as_deref()
            ));

            let mut stream = loop {
                tokio::select! {
                    biased;
                    // Handle keyboard input while waiting for API
                    event = event_stream.next() => {
                        match event {
                            Some(Ok(Event::Key(key))) => {
                                self.update_copy_badge_key_event(key);
                                if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                                    let scroll_only = super::input::is_scroll_only_key(self, key.code, key.modifiers);
                                    let _ = self.handle_key_press_event(key);
                                    if self.cancel_requested {
                                        self.cancel_requested = false;
                                        self.interleave_message = None;
                                        self.pending_soft_interrupts.clear();
                                        self.clear_streaming_render_state();
                                        self.stream_buffer.clear();
                                        self.streaming_tool_calls.clear();
                                        self.schedule_queued_dispatch_after_interrupt();
                                        self.push_display_message(DisplayMessage::system("Interrupted"));
                                        return Ok(());
                                    }
                                    if !scroll_only {
                                        self.redraw_now(terminal)?;
                                    }
                                }
                            }
                            Some(Ok(Event::Paste(text))) => {
                                self.handle_paste(text);
                                self.redraw_now(terminal)?;
                            }
                            Some(Ok(Event::Mouse(mouse))) => {
                                let scroll_only = self.handle_mouse_event(mouse);
                                if !scroll_only {
                                    self.redraw_now(terminal)?;
                                }
                            }
                            Some(Ok(Event::Resize(_, _))) => {
                                if self.should_redraw_after_resize() {
                                    self.redraw_now(terminal)?;
                                }
                            }
                            _ => {}
                        }
                    }
                    // Redraw periodically
                    _ = redraw_interval.tick() => {
                        terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;
                    }
                    // Poll API call
                    result = &mut api_future => {
                        break result?;
                    }
                }
            };

            crate::logging::info(&format!(
                "TUI: API stream opened in {:.2}s",
                api_start.elapsed().as_secs_f64()
            ));

            let mut text_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut first_event = true;
            let mut saw_message_end = false;
            let mut call_output_tokens_seen: u64 = 0;
            let mut interleaved = false; // Track if we interleaved a message mid-stream
            // Track tool results from provider (already executed by Claude Code CLI)
            let mut sdk_tool_results: std::collections::HashMap<String, (String, bool)> =
                std::collections::HashMap::new();
            let store_reasoning_content = self.provider.name() == "openrouter";
            let mut reasoning_content = String::new();
            let mut openai_native_compaction: Option<(String, usize)> = None;

            // Stream with input handling
            loop {
                tokio::select! {
                    // Redraw periodically
                    _ = redraw_interval.tick() => {
                        if let Some(chunk) = self.stream_buffer.flush() {
                            self.streaming_text.push_str(&chunk);
                        }
                        // Poll for background compaction completion during streaming
                        self.poll_compaction_completion();
                        terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;
                    }
                    // Handle keyboard input
                    event = event_stream.next() => {
                        match event {
                            Some(Ok(Event::Key(key))) => {
                                self.update_copy_badge_key_event(key);
                                if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                                    let scroll_only = super::input::is_scroll_only_key(self, key.code, key.modifiers);
                                    let _ = self.handle_key_press_event(key);
                                    // Check for cancel request
                                    if self.cancel_requested {
                                        self.cancel_requested = false;
                                        self.interleave_message = None;
                                        self.pending_soft_interrupts.clear();
                                        // Save partial assistant response before clearing
                                        if let Some(tool) = current_tool.take() {
                                            tool_calls.push(tool);
                                        }
                                        if !text_content.is_empty() || !tool_calls.is_empty() {
                                            let mut content_blocks = Vec::new();
                                            if !text_content.is_empty() {
                                                content_blocks.push(ContentBlock::Text {
                                                    text: format!("{}\n\n[generation interrupted by user]", text_content),
                                                    cache_control: None,
                                                });
                                            }
                                            if store_reasoning_content && !reasoning_content.is_empty() {
                                                content_blocks.push(ContentBlock::Reasoning {
                                                    text: reasoning_content.clone(),
                                                });
                                            }
                                            for tc in &tool_calls {
                                                content_blocks.push(ContentBlock::ToolUse {
                                                    id: tc.id.clone(),
                                                    name: tc.name.clone(),
                                                    input: tc.input.clone(),
                                                });
                                            }
                                            if !content_blocks.is_empty() {
                                                let content_clone = content_blocks.clone();
                                                self.add_provider_message(Message {
                                                    role: Role::Assistant,
                                                    content: content_blocks,
                                                    timestamp: Some(chrono::Utc::now()),
                                                    tool_duration_ms: None,
                                                });
                                                self.session.add_message(Role::Assistant, content_clone);
                                                let _ = self.session.save();
                                            }
                                            // Flush buffer and show partial response
                                            if let Some(chunk) = self.stream_buffer.flush() {
                                                self.streaming_text.push_str(&chunk);
                                            }
                                            if !self.streaming_text.is_empty() {
                                                let content = self.take_streaming_text();
                                                self.push_display_message(DisplayMessage {
                                                    role: "assistant".to_string(),
                                                    content,
                                                    tool_calls: tool_calls.iter().map(|t| t.name.clone()).collect(),
                                                    duration_secs: self.processing_started.map(|t| t.elapsed().as_secs_f32()),
                                                    title: None,
                                                    tool_data: None,
                                                });
                                            }
                                        }
                                        self.clear_streaming_render_state();
                                        self.stream_buffer.clear();
                                        self.streaming_tool_calls.clear();
                                        self.schedule_queued_dispatch_after_interrupt();
                                        self.push_display_message(DisplayMessage::system("Interrupted"));
                                        return Ok(());
                                    }
                                    // Check for interleave request (Shift+Enter)
                                    if let Some(interleave_msg) = self.interleave_message.take() {
                                        // Save partial assistant response if any
                                        if !text_content.is_empty() || !tool_calls.is_empty() {
                                            // Complete any pending tool
                                            if let Some(tool) = current_tool.take() {
                                                tool_calls.push(tool);
                                            }
                                            // Build content blocks for partial response
                                            let mut content_blocks = Vec::new();
                                            if !text_content.is_empty() {
                                                content_blocks.push(ContentBlock::Text {
                                                    text: text_content.clone(),
                                                    cache_control: None,
                                                });
                                            }
                                            if store_reasoning_content && !reasoning_content.is_empty() {
                                                content_blocks.push(ContentBlock::Reasoning {
                                                    text: reasoning_content.clone(),
                                                });
                                            }
                                            for tc in &tool_calls {
                                                content_blocks.push(ContentBlock::ToolUse {
                                                    id: tc.id.clone(),
                                                    name: tc.name.clone(),
                                                    input: tc.input.clone(),
                                                });
                                            }
                                            // Add partial assistant response to messages
                                            if !content_blocks.is_empty() {
                                                self.add_provider_message(Message {
                                                    role: Role::Assistant,
                                                    content: content_blocks,
                                                    timestamp: Some(chrono::Utc::now()),
                                                    tool_duration_ms: None,
                                                });
                                            }
                                            // Add display message for partial response
                                            if !self.streaming_text.is_empty() {
                                                let content = self.take_streaming_text();
                                                self.push_display_message(DisplayMessage {
                                                    role: "assistant".to_string(),
                                                    content,
                                                    tool_calls: tool_calls.iter().map(|t| t.name.clone()).collect(),
                                                    duration_secs: None,
                                                    title: None,
                                                    tool_data: None,
                                                });
                                            }
                                        }
                                        // Add user's interleaved message
                                        self.add_provider_message(Message::user(&interleave_msg));
                                        self.push_display_message(DisplayMessage {
                                            role: "user".to_string(),
                                            content: interleave_msg,
                                            tool_calls: vec![],
                                            duration_secs: None,
                                            title: None,
                                            tool_data: None,
                                        });
                                        // Clear streaming state and continue with new turn
                                        self.clear_streaming_render_state();
                                        self.streaming_tool_calls.clear();
                                        self.stream_buffer = StreamBuffer::new();
                                        reasoning_content.clear();
                                        interleaved = true;
                                        // Continue to next iteration of outer loop (new API call)
                                        break;
                                    }

                                    if !scroll_only {
                                        self.redraw_now(terminal)?;
                                    }
                                }
                            }
                            Some(Ok(Event::Paste(text))) => {
                                self.handle_paste(text);
                                self.redraw_now(terminal)?;
                            }
                            Some(Ok(Event::Mouse(mouse))) => {
                                let scroll_only = self.handle_mouse_event(mouse);
                                if !scroll_only {
                                    self.redraw_now(terminal)?;
                                }
                            }
                            Some(Ok(Event::Resize(_, _))) => {
                                if self.should_redraw_after_resize() {
                                    self.redraw_now(terminal)?;
                                }
                            }
                            _ => {}
                        }
                    }
                    // Handle stream events
                    stream_event = stream.next() => {
                        match stream_event {
                            Some(Ok(event)) => {
                                // Track activity for status display
                                self.last_stream_activity = Some(Instant::now());

                                if first_event {
                                    first_event = false;
                                }
                                match event {
                                    StreamEvent::TextDelta(text) => {
                                        self.status = ProcessingStatus::Streaming;
                                        text_content.push_str(&text);
                                        self.resume_streaming_tps();
                                        if let Some(chunk) = self.stream_buffer.push(&text) {
                                            self.streaming_text.push_str(&chunk);
                                            self.broadcast_debug(crate::tui::backend::DebugEvent::TextDelta {
                                                text: chunk.clone()
                                            });
                                        }
                                    }
                                    StreamEvent::ToolUseStart { id, name } => {
                                        self.pause_streaming_tps(false);
                                        self.broadcast_debug(crate::tui::backend::DebugEvent::ToolStart {
                                            id: id.clone(),
                                            name: name.clone(),
                                        });
                                        self.commit_pending_streaming_assistant_message();
                                        // Update status to show tool in progress
                                        self.status = ProcessingStatus::RunningTool(name.clone());
                                        if matches!(name.as_str(), "memory") {
                                            crate::memory::set_state(
                                                crate::tui::info_widget::MemoryState::Embedding,
                                            );
                                        }
                                        self.streaming_tool_calls.push(ToolCall {
                                            id: id.clone(),
                                            name: name.clone(),
                                            input: serde_json::Value::Null,
                                            intent: None,
                                        });
                                        current_tool = Some(ToolCall {
                                            id,
                                            name,
                                            input: serde_json::Value::Null,
                                            intent: None,
                                        });
                                        current_tool_input.clear();
                                    }
                                    StreamEvent::ToolInputDelta(delta) => {
                                        self.broadcast_debug(crate::tui::backend::DebugEvent::ToolInput {
                                            delta: delta.clone()
                                        });
                                        current_tool_input.push_str(&delta);
                                    }
                                    StreamEvent::ToolUseEnd => {
                                        self.pause_streaming_tps(false);
                                        if let Some(mut tool) = current_tool.take() {
                                            tool.input = serde_json::from_str(&current_tool_input)
                                                .unwrap_or(serde_json::Value::Null);
                                            self.broadcast_debug(crate::tui::backend::DebugEvent::ToolExec {
                                                id: tool.id.clone(),
                                                name: tool.name.clone(),
                                            });
                                            self.commit_pending_streaming_assistant_message();

                                            // Add tool call as its own display message
                                            self.push_display_message(DisplayMessage {
                                                role: "tool".to_string(),
                                                content: tool.name.clone(),
                                                tool_calls: vec![],
                                                duration_secs: None,
                                                title: None,
                                                tool_data: Some(tool.clone()),
                                            });

                                            tool_calls.push(tool);
                                            current_tool_input.clear();
                                        }
                                    }
                                    StreamEvent::TokenUsage {
                                        input_tokens,
                                        output_tokens,
                                        cache_read_input_tokens,
                                        cache_creation_input_tokens,
                                    } => {
                                        let mut usage_changed = false;
                                        if let Some(input) = input_tokens {
                                            self.streaming_input_tokens = input;
                                            usage_changed = true;
                                        }
                                        if let Some(output) = output_tokens {
                                            self.streaming_output_tokens = output;
                                            self.accumulate_streaming_output_tokens(
                                                output,
                                                &mut call_output_tokens_seen,
                                            );
                                        }
                                        if cache_read_input_tokens.is_some() {
                                            self.streaming_cache_read_tokens = cache_read_input_tokens;
                                            usage_changed = true;
                                        }
                                        if cache_creation_input_tokens.is_some() {
                                            self.streaming_cache_creation_tokens =
                                                cache_creation_input_tokens;
                                            usage_changed = true;
                                        }
                                        if usage_changed {
                                            self.update_compaction_usage_from_stream();
                                            if let Some(context_tokens) = self.current_stream_context_tokens() {
                                                self.check_context_warning(context_tokens);
                                            }
                                        }
                                        self.broadcast_debug(crate::tui::backend::DebugEvent::TokenUsage {
                                            input_tokens: self.streaming_input_tokens,
                                            output_tokens: self.streaming_output_tokens,
                                            cache_read_input_tokens: self.streaming_cache_read_tokens,
                                            cache_creation_input_tokens: self
                                                .streaming_cache_creation_tokens,
                                        });
                                    }
                                    StreamEvent::ConnectionType { connection } => {
                                        self.connection_type = Some(connection);
                                        self.update_terminal_title();
                                    }
                                    StreamEvent::ConnectionPhase { phase } => {
                                        self.status = if matches!(phase, crate::message::ConnectionPhase::Streaming) {
                                            ProcessingStatus::Streaming
                                        } else {
                                            ProcessingStatus::Connecting(phase)
                                        };
                                    }
                                    StreamEvent::StatusDetail { detail } => {
                                        self.status_detail = Some(detail);
                                    }
                                    StreamEvent::MessageEnd { .. } => {
                                        self.pause_streaming_tps(true);
                                        saw_message_end = true;
                                    }
                                    StreamEvent::SessionId(sid) => {
                                        self.provider_session_id = Some(sid);
                                        if saw_message_end {
                                            break;
                                        }
                                    }
                                    StreamEvent::Error { message, .. } => {
                                        return Err(anyhow::anyhow!("Stream error: {}", message));
                                    }
                                    StreamEvent::ThinkingStart => {
                                        let start = Instant::now();
                                        self.thinking_start = Some(start);
                                        self.thinking_buffer.clear();
                                        self.thinking_prefix_emitted = false;
                                        // Always show Thinking in status bar
                                        self.status = ProcessingStatus::Thinking(start);
                                        self.broadcast_debug(crate::tui::backend::DebugEvent::ThinkingStart);
                                    }
                                    StreamEvent::ThinkingDelta(thinking_text) => {
                                        // Buffer thinking content and emit with prefix only once
                                        self.thinking_buffer.push_str(&thinking_text);
                                        // Display reasoning/thinking content from OpenAI
                                        if let Some(chunk) = self.stream_buffer.flush() {
                                            self.streaming_text.push_str(&chunk);
                                        }
                                        // Only show thinking content if enabled in config
                                        if config().display.show_thinking {
                                            // Only emit the prefix once at the start of thinking
                                            if !self.thinking_prefix_emitted && !self.thinking_buffer.trim().is_empty() {
                                                self.insert_thought_line(format!("💭 {}", self.thinking_buffer.trim_start()));
                                                self.thinking_prefix_emitted = true;
                                                self.thinking_buffer.clear();
                                            } else if self.thinking_prefix_emitted {
                                                // After prefix is emitted, append subsequent chunks directly
                                                self.streaming_text.push_str(&thinking_text);
                                            }
                                        }
                                        if store_reasoning_content {
                                            reasoning_content.push_str(&thinking_text);
                                        }
                                    }
                                    StreamEvent::ThinkingEnd => {
                                        self.thinking_start = None;
                                        self.thinking_buffer.clear();
                                        self.broadcast_debug(crate::tui::backend::DebugEvent::ThinkingEnd);
                                    }
                                    StreamEvent::ThinkingDone { duration_secs } => {
                                        // Flush any pending buffered text first
                                        if let Some(chunk) = self.stream_buffer.flush() {
                                            self.streaming_text.push_str(&chunk);
                                        }
                                        let thinking_msg = format!("*Thought for {:.1}s*", duration_secs);
                                        self.insert_thought_line(thinking_msg);
                                        self.thinking_prefix_emitted = false;
                                        self.thinking_buffer.clear();
                                    }
                                    StreamEvent::Compaction {
                                        trigger,
                                        pre_tokens,
                                        openai_encrypted_content,
                                    } => {
                                        if let Some(encrypted_content) = openai_encrypted_content {
                                            openai_native_compaction
                                                .get_or_insert_with(|| (encrypted_content, self.messages.len()));
                                        }
                                        // Flush any pending buffered text first
                                        if let Some(chunk) = self.stream_buffer.flush() {
                                            self.streaming_text.push_str(&chunk);
                                        }
                                        let tokens_str = pre_tokens
                                            .map(|t| format!(" (was {} tokens)", t))
                                            .unwrap_or_default();
                                        let compact_msg = format!(
                                            "📦 **Compaction complete** — context summarized ({}){}\n\n",
                                            trigger, tokens_str
                                        );
                                        self.streaming_text.push_str(&compact_msg);
                                        self.context_warning_shown = false;
                                    }
                                    StreamEvent::UpstreamProvider { provider } => {
                                        // Store the upstream provider (e.g., Fireworks, Together)
                                        self.upstream_provider = Some(provider);
                                    }
                                    StreamEvent::ToolResult { tool_use_id, content, is_error } => {
                                        // SDK already executed this tool
                                        self.tool_result_ids.insert(tool_use_id.clone());
                                        // Find the tool name from our tracking
                                        let tool_name = self.streaming_tool_calls
                                            .iter()
                                            .find(|tc| tc.id == tool_use_id)
                                            .map(|tc| tc.name.clone())
                                            .unwrap_or_default();

                                        self.broadcast_debug(crate::tui::backend::DebugEvent::ToolDone {
                                            id: tool_use_id.clone(),
                                            name: tool_name.clone(),
                                            output: content.clone(),
                                            is_error,
                                        });

                                        // Update the tool's DisplayMessage with the output (if it exists)
                                        if let Some(dm) = self.display_messages.iter_mut().rev().find(|dm| {
                                            dm.tool_data.as_ref().map(|td| &td.id) == Some(&tool_use_id)
                                        }) {
                                            dm.content = content.clone();
                                            self.bump_display_messages_version();
                                        }

                                        // Clear this tool from streaming_tool_calls
                                        self.streaming_tool_calls.retain(|tc| tc.id != tool_use_id);

                                        // Reset status back to Streaming
                                        self.status = ProcessingStatus::Streaming;

                                        sdk_tool_results.insert(tool_use_id, (content, is_error));
                                    }
                                    StreamEvent::NativeToolCall {
                                        request_id,
                                        tool_name,
                                        input,
                                    } => {
                                        // Execute native tool and send result back to SDK bridge
                                        let ctx = crate::tool::ToolContext {
                                            session_id: self.session_id().to_string(),
                                            message_id: self.session_id().to_string(),
                                            tool_call_id: request_id.clone(),
                                            working_dir: self.session.working_dir.as_deref().map(PathBuf::from),
                                            stdin_request_tx: None,
                                            graceful_shutdown_signal: None,
                                            execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
                                        };
                                        let tool_result = self.registry.execute(&tool_name, input, ctx).await;
                                        crate::telemetry::record_tool_call();
                                        if tool_result.is_err() {
                                            crate::telemetry::record_tool_failure();
                                        }
                                        let native_result = match tool_result {
                                            Ok(output) => crate::provider::NativeToolResult::success(request_id, output.output),
                                            Err(e) => crate::provider::NativeToolResult::error(request_id, e.to_string()),
                                        };
                                        if let Some(sender) = self.provider.native_result_sender() {
                                            let _ = sender.send(native_result).await;
                                        }
                                    }
                                }
                            }
                            Some(Err(e)) => return Err(e),
                            None => break, // Stream ended
                        }
                    }
                }
            }

            // If we interleaved a message, skip post-processing and go straight to new API call
            if interleaved {
                continue;
            }

            // Add assistant message to history
            let mut content_blocks = Vec::new();
            if !text_content.is_empty() {
                content_blocks.push(ContentBlock::Text {
                    text: text_content.clone(),
                    cache_control: None,
                });
            }
            if store_reasoning_content && !reasoning_content.is_empty() {
                content_blocks.push(ContentBlock::Reasoning {
                    text: reasoning_content.clone(),
                });
            }
            for tc in &tool_calls {
                content_blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.input.clone(),
                });
            }

            let assistant_message_id = if !content_blocks.is_empty() {
                crate::telemetry::record_assistant_response();
                let content_clone = content_blocks.clone();
                self.add_provider_message(Message {
                    role: Role::Assistant,
                    content: content_blocks,
                    timestamp: Some(chrono::Utc::now()),
                    tool_duration_ms: None,
                });
                let message_id = self.session.add_message(Role::Assistant, content_clone);
                let _ = self.session.save();
                for tc in &tool_calls {
                    self.tool_result_ids.insert(tc.id.clone());
                }
                Some(message_id)
            } else {
                None
            };

            if let Some((encrypted_content, compacted_count)) = openai_native_compaction.take() {
                self.apply_openai_native_compaction(encrypted_content, compacted_count)?;
            }

            // Add remaining text to display
            let duration = self.processing_started.map(|t| t.elapsed().as_secs_f32());

            // Flush any remaining buffered text
            if let Some(chunk) = self.stream_buffer.flush() {
                self.streaming_text.push_str(&chunk);
            }

            if tool_calls.is_empty() {
                // No tool calls - display full text_content
                if !text_content.is_empty() {
                    self.push_display_message(DisplayMessage {
                        role: "assistant".to_string(),
                        content: text_content.clone(),
                        tool_calls: vec![],
                        duration_secs: duration,
                        title: None,
                        tool_data: None,
                    });
                    self.push_turn_footer(duration);
                }
            } else {
                // Had tool calls - only display text that came AFTER the last tool
                // (text before each tool was already committed in ToolUseEnd handler)
                if !self.streaming_text.is_empty() {
                    self.push_display_message(DisplayMessage {
                        role: "assistant".to_string(),
                        content: self.streaming_text.clone(),
                        tool_calls: vec![],
                        duration_secs: duration,
                        title: None,
                        tool_data: None,
                    });
                    self.push_turn_footer(duration);
                }
            }
            self.clear_streaming_render_state();
            self.stream_buffer.clear();
            self.streaming_tool_calls.clear();

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                break;
            }

            // Execute tools with input handling (non-blocking)
            // SDK may have executed some tools, but custom tools need local execution
            for tc in tool_calls {
                self.status = ProcessingStatus::RunningTool(tc.name.clone());
                self.observe_tool_call(&tc);
                if matches!(tc.name.as_str(), "memory") {
                    crate::memory::set_state(crate::tui::info_widget::MemoryState::Embedding);
                }
                terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;

                let message_id = assistant_message_id
                    .clone()
                    .unwrap_or_else(|| self.session.id.clone());

                // Check if SDK already executed this tool
                if let Some((sdk_content, sdk_is_error)) = sdk_tool_results.remove(&tc.id) {
                    // Use SDK result
                    Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                        session_id: self.session.id.clone(),
                        message_id: message_id.clone(),
                        tool_call_id: tc.id.clone(),
                        tool_name: tc.name.clone(),
                        status: if sdk_is_error {
                            ToolStatus::Error
                        } else {
                            ToolStatus::Completed
                        },
                        title: None,
                    }));

                    // Update the tool's DisplayMessage with the output
                    let display_output = if sdk_is_error
                        && !sdk_content.starts_with("Error:")
                        && !sdk_content.starts_with("error:")
                        && !sdk_content.starts_with("Failed:")
                    {
                        format!("Error: {}", sdk_content)
                    } else {
                        sdk_content.clone()
                    };
                    let _ = self.replace_latest_tool_display_message(&tc.id, None, display_output);

                    self.observe_tool_result(&tc, &sdk_content, sdk_is_error, None);

                    self.add_provider_message(Message {
                        role: Role::User,
                        content: vec![ContentBlock::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: sdk_content,
                            is_error: if sdk_is_error { Some(true) } else { None },
                        }],
                        timestamp: Some(chrono::Utc::now()),
                        tool_duration_ms: None,
                    });
                    self.session.add_message(
                        Role::User,
                        vec![ContentBlock::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: String::new(), // Already added to messages above
                            is_error: if sdk_is_error { Some(true) } else { None },
                        }],
                    );
                    self.session.save()?;
                    continue;
                }

                // Execute locally
                let ctx = ToolContext {
                    session_id: self.session.id.clone(),
                    message_id: message_id.clone(),
                    tool_call_id: tc.id.clone(),
                    working_dir: self.session.working_dir.as_deref().map(PathBuf::from),
                    stdin_request_tx: None,
                    graceful_shutdown_signal: None,
                    execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
                };

                Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                    session_id: self.session.id.clone(),
                    message_id: message_id.clone(),
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                    status: ToolStatus::Running,
                    title: None,
                }));

                // Make tool execution non-blocking - poll in select! so we can handle input
                // Clone registry to avoid borrow issues
                let registry = self.registry.clone();
                let tool_name = tc.name.clone();
                let tool_input = tc.input.clone();
                let tool_start = Instant::now();
                let mut tool_future = std::pin::pin!(registry.execute(&tool_name, tool_input, ctx));

                // Subscribe to bus for subagent status updates
                let mut bus_receiver = Bus::global().subscribe();
                self.subagent_status = None; // Clear previous status
                self.batch_progress = None; // Clear previous batch progress

                let result = loop {
                    tokio::select! {
                        biased;
                        // Handle keyboard input while tool executes
                        event = event_stream.next() => {
                            match event {
                                Some(Ok(Event::Key(key))) => {
                                    self.update_copy_badge_key_event(key);
                                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                                        let scroll_only = super::input::is_scroll_only_key(self, key.code, key.modifiers);
                                        let _ = self.handle_key_press_event(key);
                                        if self.cancel_requested {
                                            self.cancel_requested = false;
                                            self.interleave_message = None;
                                            self.pending_soft_interrupts.clear();
                                            // Partial text+tool_calls were already saved
                                            // to the session before tool execution started.
                                            // Just preserve the visual streaming content.
                                            if let Some(chunk) = self.stream_buffer.flush() {
                                                self.streaming_text.push_str(&chunk);
                                            }
                                            if !self.streaming_text.is_empty() {
                                                let content = self.take_streaming_text();
                                                self.push_display_message(DisplayMessage {
                                                    role: "assistant".to_string(),
                                                    content,
                                                    tool_calls: Vec::new(),
                                                    duration_secs: self.processing_started.map(|t| t.elapsed().as_secs_f32()),
                                                    title: None,
                                                    tool_data: None,
                                                });
                                            }
                                            self.clear_streaming_render_state();
                                            self.stream_buffer.clear();
                                            self.streaming_tool_calls.clear();
                                            self.batch_progress = None;
                                            self.schedule_queued_dispatch_after_interrupt();
                                            self.push_display_message(DisplayMessage::system("Interrupted"));
                                            return Ok(());
                                        }

                                        if !scroll_only {
                                            self.redraw_now(terminal)?;
                                        }
                                    }
                                }
                                Some(Ok(Event::Paste(text))) => {
                                    self.handle_paste(text);
                                    self.redraw_now(terminal)?;
                                }
                                Some(Ok(Event::Mouse(mouse))) => {
                                    let scroll_only = self.handle_mouse_event(mouse);
                                    if !scroll_only {
                                        self.redraw_now(terminal)?;
                                    }
                                }
                                Some(Ok(Event::Resize(_, _))) => {
                                    if self.should_redraw_after_resize() {
                                        self.redraw_now(terminal)?;
                                    }
                                }
                                _ => {}
                            }
                        }
                        // Listen for subagent/batch status updates
                        bus_event = bus_receiver.recv() => {
                            match bus_event {
                                Ok(BusEvent::SubagentStatus(status)) => {
                                    if status.session_id == self.session.id {
                                        let display = if let Some(model) = &status.model {
                                            format!("{} · {}", status.status, model)
                                        } else {
                                            status.status
                                        };
                                        self.subagent_status = Some(display);
                                    }
                                }
                                Ok(BusEvent::BatchProgress(progress)) => {
                                    if progress.session_id == self.session.id {
                                        self.batch_progress = Some(progress);
                                    }
                                }
                                Ok(BusEvent::SidePanelUpdated(update)) => {
                                    if update.session_id == self.session.id {
                                        self.set_side_panel_snapshot(update.snapshot);
                                    }
                                }
                                _ => {}
                            }
                        }
                        // Redraw periodically
                        _ = redraw_interval.tick() => {
                            terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;
                        }
                        // Poll tool execution
                        result = &mut tool_future => {
                            break result;
                        }
                    }
                };

                self.subagent_status = None; // Clear status after tool completes
                self.batch_progress = None; // Clear batch progress after tool completes
                let tool_duration_ms = tool_start.elapsed().as_millis() as u64;
                let (output, is_error, tool_title) = match result {
                    Ok(o) => {
                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: ToolStatus::Completed,
                            title: o.title.clone(),
                        }));
                        (o.output, false, o.title)
                    }
                    Err(e) => {
                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: ToolStatus::Error,
                            title: None,
                        }));
                        (format!("Error: {}", e), true, None)
                    }
                };

                // Update the tool's DisplayMessage with the output
                let _ = self.replace_latest_tool_display_message(
                    &tc.id,
                    tool_title.clone(),
                    output.clone(),
                );

                self.add_provider_message(Message::tool_result_with_duration(
                    &tc.id,
                    &output,
                    is_error,
                    Some(tool_duration_ms),
                ));
                self.session.add_message_with_duration(
                    Role::User,
                    vec![ContentBlock::ToolResult {
                        tool_use_id: tc.id.clone(),
                        content: output.clone(),
                        is_error: if is_error { Some(true) } else { None },
                    }],
                    Some(tool_duration_ms),
                );
                self.observe_tool_result(&tc, &output, is_error, tool_title.as_deref());
                let _ = self.session.save();
            }
        }

        super::commands::maybe_trigger_autoreview_local(self);
        super::commands::maybe_trigger_autojudge_local(self);
        Ok(())
    }

    fn build_system_prompt(&mut self, memory_prompt: Option<&str>) -> String {
        let split = self.build_system_prompt_split(memory_prompt);
        if split.dynamic_part.is_empty() {
            split.static_part
        } else if split.static_part.is_empty() {
            split.dynamic_part
        } else {
            format!("{}\n\n{}", split.static_part, split.dynamic_part)
        }
    }

    /// Build split system prompt for better caching
    pub(super) fn build_system_prompt_split(
        &mut self,
        memory_prompt: Option<&str>,
    ) -> crate::prompt::SplitSystemPrompt {
        // Ambient mode: use the full override prompt directly
        if let Some(ref prompt) = self.ambient_system_prompt {
            return crate::prompt::SplitSystemPrompt {
                static_part: prompt.clone(),
                dynamic_part: String::new(),
            };
        }

        let skill_prompt = self
            .active_skill
            .as_ref()
            .and_then(|name| self.skills.get(name).map(|s| s.get_prompt().to_string()));
        let available_skills: Vec<crate::prompt::SkillInfo> = self
            .skills
            .list()
            .iter()
            .map(|s| crate::prompt::SkillInfo {
                name: s.name.clone(),
                description: s.description.clone(),
            })
            .collect();
        let (mut split, context_info) = crate::prompt::build_system_prompt_split(
            skill_prompt.as_deref(),
            &available_skills,
            self.session.is_canary,
            memory_prompt,
            None,
        );
        self.append_current_turn_system_reminder(&mut split);
        self.context_info = context_info;
        split
    }

    fn show_injected_memory_context(
        &mut self,
        prompt: &str,
        display_prompt: Option<&str>,
        count: usize,
        age_ms: u64,
        memory_ids: Vec<String>,
    ) {
        let count = count.max(1);
        let plural = if count == 1 { "memory" } else { "memories" };
        let display_prompt = if let Some(display_prompt) = display_prompt {
            display_prompt.to_string()
        } else if prompt.trim().is_empty() {
            "# Memory\n\n## Notes\n1. (empty injection payload)".to_string()
        } else {
            prompt.to_string()
        };
        if !self.should_inject_memory_context(prompt) {
            return;
        }
        crate::memory::record_injected_prompt(prompt, count, age_ms);
        let summary = if count == 1 {
            "🧠 auto-recalled 1 memory".to_string()
        } else {
            format!("🧠 auto-recalled {} memories", count)
        };
        // Record to session for replay visualization
        self.session.record_memory_injection(
            summary.clone(),
            display_prompt.clone(),
            count as u32,
            age_ms,
            memory_ids,
        );
        if let Err(err) = self.session.save() {
            crate::logging::warn(&format!(
                "Failed to persist memory injection for session {}: {}",
                self.session.id, err
            ));
        }
        self.push_display_message(DisplayMessage::memory(summary, display_prompt));
        self.set_status_notice(format!("🧠 {} {} injected", count, plural));
    }

    /// Get memory prompt using async non-blocking approach
    /// Takes any pending memory from background check and sends context to memory agent for next turn
    fn build_memory_prompt_nonblocking(
        &self,
        messages: &[Message],
    ) -> Option<crate::memory::PendingMemory> {
        if self.is_remote || !self.memory_enabled {
            return None;
        }

        // Take pending memory if available (computed in background during last turn)
        let pending = crate::memory::take_pending_memory(&self.session.id);

        // Send context to memory agent for the NEXT turn (doesn't block current send)
        let shared_messages: std::sync::Arc<[crate::message::Message]> = messages.to_vec().into();
        crate::memory_agent::update_context_sync_with_dir(
            &self.session.id,
            shared_messages,
            self.session.working_dir.clone(),
        );

        // Return pending memory from previous turn
        pending
    }

    /// Legacy blocking memory prompt - kept for fallback but not used in normal flow
    #[allow(dead_code)]
    async fn build_memory_prompt(&self, messages: &[Message]) -> Option<String> {
        if self.is_remote {
            return None;
        }

        let manager = self
            .session
            .working_dir
            .as_deref()
            .map(|dir| {
                crate::memory::MemoryManager::new()
                    .with_project_dir(dir)
                    .with_skills(self.active_skill.is_none())
            })
            .unwrap_or_else(|| {
                crate::memory::MemoryManager::new().with_skills(self.active_skill.is_none())
            });
        match manager.relevant_prompt_for_messages(messages).await {
            Ok(prompt) => prompt,
            Err(e) => {
                crate::logging::info(&format!("Memory relevance skipped: {}", e));
                None
            }
        }
    }

    /// Extract and store memories from the session transcript at end of session
    pub(super) async fn extract_session_memories(&self) {
        // Skip if remote mode or not enough messages
        if self.is_remote || !self.memory_enabled || self.messages.len() < 4 {
            return;
        }

        crate::logging::info(&format!(
            "Extracting memories from {} messages",
            self.messages.len()
        ));

        // Build transcript from messages
        let mut transcript = String::new();
        for msg in &self.messages {
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
                        // Truncate long results
                        let preview = if content.len() > 200 {
                            format!("{}...", crate::util::truncate_str(content, 200))
                        } else {
                            content.clone()
                        };
                        transcript.push_str(&format!("[Result: {}]\n", preview));
                    }
                    ContentBlock::Reasoning { .. } => {}
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

        if !crate::memory::memory_sidecar_enabled() {
            crate::logging::info("Memory extraction skipped: memory sidecar disabled");
            return;
        }

        // Extract memories using sidecar (with existing context for dedup)
        let manager = self
            .session
            .working_dir
            .as_deref()
            .map(|dir| {
                crate::memory::MemoryManager::new()
                    .with_project_dir(dir)
                    .with_skills(self.active_skill.is_none())
            })
            .unwrap_or_else(|| {
                crate::memory::MemoryManager::new().with_skills(self.active_skill.is_none())
            });
        let existing: Vec<String> = manager
            .list_all()
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.active)
            .map(|e| e.content)
            .collect();
        let sidecar = crate::sidecar::Sidecar::new();
        match sidecar
            .extract_memories_with_existing(&transcript, &existing)
            .await
        {
            Ok(extracted) if !extracted.is_empty() => {
                let manager = self
                    .session
                    .working_dir
                    .as_deref()
                    .map(|dir| crate::memory::MemoryManager::new().with_project_dir(dir))
                    .unwrap_or_else(crate::memory::MemoryManager::new);
                let mut stored_count = 0;

                for memory in extracted {
                    let category = crate::memory::MemoryCategory::from_extracted(&memory.category);

                    // Map trust string to enum
                    let trust = match memory.trust.as_str() {
                        "high" => crate::memory::TrustLevel::High,
                        "low" => crate::memory::TrustLevel::Low,
                        _ => crate::memory::TrustLevel::Medium,
                    };

                    // Create memory entry
                    let entry = crate::memory::MemoryEntry {
                        id: format!("auto_{}", chrono::Utc::now().timestamp_millis()),
                        category,
                        content: memory.content,
                        tags: Vec::new(),
                        search_text: String::new(),
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                        access_count: 0,
                        trust,
                        active: true,
                        superseded_by: None,
                        strength: 1,
                        source: Some(self.session.id.clone()),
                        reinforcements: Vec::new(),
                        embedding: None, // Will be generated when stored
                        confidence: 1.0,
                    };

                    // Store memory
                    if manager.remember_project(entry).is_ok() {
                        stored_count += 1;
                    }
                }

                if stored_count > 0 {
                    crate::logging::info(&format!(
                        "Extracted {} memories from session",
                        stored_count
                    ));
                }
            }
            Ok(_) => {
                // No memories extracted, that's fine
            }
            Err(e) => {
                crate::logging::info(&format!("Memory extraction skipped: {}", e));
            }
        }
    }
}
