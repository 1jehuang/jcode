use super::*;

impl Agent {
    pub(super) async fn run_turn_streaming_mpsc(
        &mut self,
        event_tx: mpsc::UnboundedSender<ServerEvent>,
    ) -> Result<()> {
        self.set_log_context();
        let trace = trace_enabled();
        let mut context_limit_retries = 0u32;
        let mut incomplete_continuations = 0u32;

        loop {
            let repaired = self.repair_missing_tool_outputs();
            if repaired > 0 {
                logging::warn(&format!(
                    "Recovered {} missing tool output(s) before API call",
                    repaired
                ));
            }
            let (messages, compaction_event) = self.messages_for_provider();
            if let Some(event) = compaction_event {
                // Reset cache tracker and tool lock on compaction since the message history changes
                self.cache_tracker.reset();
                self.locked_tools = None;
                logging::info(&format!(
                    "Context compacted ({}{})",
                    event.trigger,
                    event
                        .pre_tokens
                        .map(|t| format!(" {} tokens", t))
                        .unwrap_or_default()
                ));
                let _ = event_tx.send(ServerEvent::Compaction {
                    trigger: event.trigger.clone(),
                    pre_tokens: event.pre_tokens,
                    post_tokens: event.post_tokens,
                    tokens_saved: event.tokens_saved,
                    duration_ms: event.duration_ms,
                    messages_dropped: None,
                    messages_compacted: event.messages_compacted,
                    summary_chars: event.summary_chars,
                    active_messages: event.active_messages,
                });
            }

            let tools = self.tool_definitions().await;
            let messages: std::sync::Arc<[Message]> = messages.into();
            // Non-blocking memory: uses pending result from last turn, spawns check for next turn
            let memory_pending = self.build_memory_prompt_nonblocking_shared(
                std::sync::Arc::clone(&messages),
                Some(std::sync::Arc::new({
                    let event_tx = event_tx.clone();
                    move |event| {
                        let _ = event_tx.send(event);
                    }
                })),
            );
            // Use split prompt for better caching - static content cached, dynamic not
            let split_prompt = self.build_system_prompt_split(None);
            self.log_prompt_prefix_accounting(&split_prompt, &tools);

            // Check for client-side cache violations before memory injection.
            // Memory is an ephemeral suffix that changes each turn; tracking it would cause
            // false-positive violations every turn (prior turn's memory ≠ current history prefix).
            self.record_client_cache_request(&messages);

            // Inject memory as a user message at the end (preserves cache prefix)
            let mut messages_with_memory: Vec<Message> = messages.iter().cloned().collect();
            if let Some(memory) = memory_pending.as_ref() {
                let memory_count = memory.count.max(1);
                let computed_age_ms = memory.computed_at.elapsed().as_millis() as u64;
                crate::memory::record_injected_prompt(
                    &memory.prompt,
                    memory_count,
                    computed_age_ms,
                );
                self.record_memory_injection_in_session(memory);
                let _ = event_tx.send(ServerEvent::MemoryInjected {
                    count: memory_count,
                    prompt: memory.prompt.clone(),
                    display_prompt: memory.display_prompt.clone(),
                    prompt_chars: memory.prompt.chars().count(),
                    computed_age_ms,
                });
                let memory_msg =
                    format!("<system-reminder>\n{}\n</system-reminder>", memory.prompt);
                messages_with_memory.push(Message::user(&memory_msg));
            }

            logging::info(&format!(
                "API call starting: {} messages, {} tools",
                messages_with_memory.len(),
                tools.len()
            ));
            let api_start = Instant::now();

            let stamped;
            let send_messages: &[Message] = if crate::config::config().features.message_timestamps {
                stamped = Message::with_timestamps(&messages_with_memory);
                &stamped
            } else {
                &messages_with_memory
            };
            let provider = Arc::clone(&self.provider);
            let resume_session_id = self.provider_session_id.clone();
            self.last_status_detail = None;
            let mut keepalive = stream_keepalive_ticker();
            let mut stream = {
                let mut complete_future = std::pin::pin!(provider.complete_split(
                    send_messages,
                    &tools,
                    &split_prompt.static_part,
                    &split_prompt.dynamic_part,
                    resume_session_id.as_deref(),
                ));
                loop {
                    tokio::select! {
                        _ = keepalive.tick() => {
                            send_stream_keepalive_mpsc(&event_tx);
                        }
                        result = &mut complete_future => {
                            match result {
                                Ok(stream) => break stream,
                                Err(e) => {
                                    if self.try_auto_compact_after_context_limit(&e.to_string()) {
                                        context_limit_retries += 1;
                                        if context_limit_retries > Self::MAX_CONTEXT_LIMIT_RETRIES {
                                            logging::warn(
                                                "Context-limit compaction retry limit reached; giving up",
                                            );
                                            return Err(anyhow::anyhow!(
                                                "Context limit exceeded after {} compaction retries",
                                                Self::MAX_CONTEXT_LIMIT_RETRIES
                                            ));
                                        }
                                        let _ = event_tx.send(ServerEvent::Compaction {
                                            trigger: "auto_recovery".to_string(),
                                            pre_tokens: None,
                                            post_tokens: None,
                                            tokens_saved: None,
                                            duration_ms: None,
                                            messages_dropped: None,
                                            messages_compacted: None,
                                            summary_chars: None,
                                            active_messages: None,
                                        });
                                        continue;
                                    }
                                    return Err(e);
                                }
                            }
                        }
                    }
                }
            };

            // Successful API call - reset retry counter
            context_limit_retries = 0;

            logging::info(&format!(
                "API stream opened in {:.2}s",
                api_start.elapsed().as_secs_f64()
            ));

            let mut text_content = String::new();
            let mut text_wrapped_detected = false;
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut generated_image_contexts: Vec<Vec<ContentBlock>> = Vec::new();
            let mut usage_input: Option<u64> = None;
            let mut usage_output: Option<u64> = None;
            let mut usage_cache_read: Option<u64> = None;
            let mut usage_cache_creation: Option<u64> = None;
            let mut stop_reason: Option<String> = None;
            let mut sdk_tool_results: std::collections::HashMap<String, (String, bool)> =
                std::collections::HashMap::new();
            let store_reasoning_content = self.provider.name() == "openrouter";
            let mut reasoning_content = String::new();
            let mut openai_native_compaction: Option<(String, usize)> = None;
            let mut tool_id_to_name: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();

            let mut retry_after_compaction = false;
            let mut keepalive = stream_keepalive_ticker();
            loop {
                let next_event = std::pin::pin!(stream.next());
                let event = tokio::select! {
                    _ = keepalive.tick() => {
                        send_stream_keepalive_mpsc(&event_tx);
                        continue;
                    }
                    event = next_event => event,
                };
                let Some(event) = event else {
                    break;
                };
                let event = match event {
                    Ok(event) => event,
                    Err(e) => {
                        let err_str = e.to_string();
                        if self.try_auto_compact_after_context_limit(&err_str) {
                            context_limit_retries += 1;
                            if context_limit_retries > Self::MAX_CONTEXT_LIMIT_RETRIES {
                                logging::warn(
                                    "Context-limit compaction retry limit reached; giving up",
                                );
                                return Err(anyhow::anyhow!(
                                    "Context limit exceeded after {} compaction retries",
                                    Self::MAX_CONTEXT_LIMIT_RETRIES
                                ));
                            }
                            retry_after_compaction = true;
                            let _ = event_tx.send(ServerEvent::Compaction {
                                trigger: "auto_recovery".to_string(),
                                pre_tokens: None,
                                post_tokens: None,
                                tokens_saved: None,
                                duration_ms: None,
                                messages_dropped: None,
                                messages_compacted: None,
                                summary_chars: None,
                                active_messages: None,
                            });
                            break;
                        }
                        return Err(e);
                    }
                };

                match event {
                    StreamEvent::ThinkingStart | StreamEvent::ThinkingEnd => {}
                    StreamEvent::ThinkingDelta(thinking_text) => {
                        // Only send thinking content if enabled in config
                        if crate::config::config().display.show_thinking {
                            let _ = event_tx.send(ServerEvent::TextDelta {
                                text: format!("💭 {}\n", thinking_text),
                            });
                        }
                        if store_reasoning_content {
                            reasoning_content.push_str(&thinking_text);
                        }
                    }
                    StreamEvent::ThinkingDone { duration_secs } => {
                        let _ = event_tx.send(ServerEvent::TextDelta {
                            text: format!("Thought for {:.1}s\n", duration_secs),
                        });
                    }
                    StreamEvent::TextDelta(text) => {
                        text_content.push_str(&text);
                        if !text_wrapped_detected {
                            if let Some(marker_idx) = text_content
                                .find("to=functions.")
                                .or_else(|| text_content.find("+#+#"))
                            {
                                text_wrapped_detected = true;
                                let clean_prefix =
                                    text_content[..marker_idx].trim_end().to_string();
                                let _ =
                                    event_tx.send(ServerEvent::TextReplace { text: clean_prefix });
                            } else {
                                let _ =
                                    event_tx.send(ServerEvent::TextDelta { text: text.clone() });
                            }
                        }
                        if self.is_graceful_shutdown() {
                            logging::info(
                                "Graceful shutdown during streaming - checkpointing partial response",
                            );
                            let _ = event_tx.send(ServerEvent::TextDelta {
                                text: "\n\n[generation interrupted - server reloading]".to_string(),
                            });
                            text_content
                                .push_str("\n\n[generation interrupted - server reloading]");
                            break;
                        }
                    }
                    StreamEvent::ToolUseStart { id, name } => {
                        let _ = event_tx.send(ServerEvent::ToolStart {
                            id: id.clone(),
                            name: name.clone(),
                        });
                        tool_id_to_name.insert(id.clone(), name.clone());
                        current_tool = Some(ToolCall {
                            id,
                            name,
                            input: serde_json::Value::Null,
                            intent: None,
                        });
                        current_tool_input.clear();
                    }
                    StreamEvent::ToolInputDelta(delta) => {
                        let _ = event_tx.send(ServerEvent::ToolInput {
                            delta: delta.clone(),
                        });
                        current_tool_input.push_str(&delta);
                    }
                    StreamEvent::ToolUseEnd => {
                        if let Some(mut tool) = current_tool.take() {
                            let tool_input =
                                serde_json::from_str::<serde_json::Value>(&current_tool_input)
                                    .unwrap_or(serde_json::Value::Null);
                            tool.input = tool_input;
                            tool.refresh_intent_from_input();

                            let _ = event_tx.send(ServerEvent::ToolExec {
                                id: tool.id.clone(),
                                name: tool.name.clone(),
                            });

                            tool_calls.push(tool);
                            current_tool_input.clear();
                        }
                    }
                    StreamEvent::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let tool_name = tool_id_to_name
                            .get(&tool_use_id)
                            .cloned()
                            .unwrap_or_default();
                        let _ = event_tx.send(ServerEvent::ToolDone {
                            id: tool_use_id.clone(),
                            name: tool_name,
                            output: content.clone(),
                            error: if is_error {
                                Some("Tool error".to_string())
                            } else {
                                None
                            },
                        });
                        sdk_tool_results.insert(tool_use_id, (content, is_error));
                    }
                    StreamEvent::GeneratedImage {
                        id,
                        path,
                        metadata_path,
                        output_format,
                        revised_prompt,
                    } => {
                        if let Some(snapshot) = self.update_generated_image_side_panel(
                            &id,
                            &path,
                            metadata_path.as_deref(),
                            &output_format,
                            revised_prompt.as_deref(),
                        ) {
                            let _ = event_tx.send(ServerEvent::SidePanelState { snapshot });
                        }
                        if self.provider.supports_image_input() {
                            if let Some(blocks) =
                                crate::message::generated_image_visual_context_blocks(
                                    &path,
                                    metadata_path.as_deref(),
                                    &output_format,
                                    revised_prompt.as_deref(),
                                )
                            {
                                generated_image_contexts.push(blocks);
                            } else {
                                crate::logging::warn(&format!(
                                    "Generated image was not attached as visual context: {}",
                                    path
                                ));
                            }
                        }
                        let _ = event_tx.send(ServerEvent::GeneratedImage {
                            id,
                            path,
                            metadata_path,
                            output_format,
                            revised_prompt,
                        });
                    }
                    StreamEvent::TokenUsage {
                        input_tokens,
                        output_tokens,
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    } => {
                        if let Some(input) = input_tokens {
                            usage_input = Some(input);
                        }
                        if let Some(output) = output_tokens {
                            usage_output = Some(output);
                        }
                        if cache_read_input_tokens.is_some() {
                            usage_cache_read = cache_read_input_tokens;
                        }
                        if cache_creation_input_tokens.is_some() {
                            usage_cache_creation = cache_creation_input_tokens;
                        }
                        if let Some(input) = usage_input {
                            self.update_compaction_usage_from_stream(
                                input,
                                usage_cache_read,
                                usage_cache_creation,
                            );
                        }
                    }
                    StreamEvent::ConnectionType { connection } => {
                        crate::telemetry::record_connection_type(&connection);
                        self.last_connection_type = Some(connection.clone());
                        let _ = event_tx.send(ServerEvent::ConnectionType { connection });
                    }
                    StreamEvent::ConnectionPhase { phase } => {
                        let _ = event_tx.send(ServerEvent::ConnectionPhase {
                            phase: phase.to_string(),
                        });
                    }
                    StreamEvent::StatusDetail { detail } => {
                        self.last_status_detail = Some(detail.clone());
                        let _ = event_tx.send(ServerEvent::StatusDetail { detail });
                    }
                    StreamEvent::MessageEnd {
                        stop_reason: reason,
                    } => {
                        if reason.is_some() {
                            stop_reason = reason;
                        }
                        let _ = event_tx.send(ServerEvent::MessageEnd);
                    }
                    StreamEvent::SessionId(sid) => {
                        self.provider_session_id = Some(sid.clone());
                        self.session.provider_session_id = Some(sid.clone());
                        let _ = event_tx.send(ServerEvent::SessionId { session_id: sid });
                    }
                    StreamEvent::Compaction {
                        openai_encrypted_content,
                        ..
                    } => {
                        if let Some(encrypted_content) = openai_encrypted_content {
                            openai_native_compaction
                                .get_or_insert((encrypted_content, self.session.messages.len()));
                        }
                    }
                    StreamEvent::NativeToolCall {
                        request_id,
                        tool_name,
                        input,
                    } => {
                        // Execute native tool and send result back to SDK bridge
                        let ctx = ToolContext {
                            session_id: self.session.id.clone(),
                            message_id: self.session.id.clone(),
                            tool_call_id: request_id.clone(),
                            working_dir: self.working_dir().map(PathBuf::from),
                            stdin_request_tx: self.stdin_request_tx.clone(),
                            graceful_shutdown_signal: Some(self.graceful_shutdown.clone()),
                            execution_mode: ToolExecutionMode::AgentTurn,
                        };
                        crate::telemetry::record_tool_call();
                        let tool_result = self.registry.execute(&tool_name, input, ctx).await;
                        if tool_result.is_err() {
                            crate::telemetry::record_tool_failure();
                        }
                        let native_result = match tool_result {
                            Ok(output) => NativeToolResult::success(request_id, output.output),
                            Err(e) => NativeToolResult::error(request_id, e.to_string()),
                        };
                        if let Some(sender) = self.provider.native_result_sender() {
                            let _ = sender.send(native_result).await;
                        }
                    }
                    StreamEvent::UpstreamProvider { provider } => {
                        self.last_upstream_provider = Some(provider.clone());
                        let _ = event_tx.send(ServerEvent::UpstreamProvider { provider });
                    }
                    StreamEvent::Error {
                        message,
                        retry_after_secs,
                    } => {
                        if self.try_auto_compact_after_context_limit(&message) {
                            context_limit_retries += 1;
                            if context_limit_retries > Self::MAX_CONTEXT_LIMIT_RETRIES {
                                logging::warn(
                                    "Context-limit compaction retry limit reached; giving up",
                                );
                                return Err(anyhow::anyhow!(
                                    "Context limit exceeded after {} compaction retries",
                                    Self::MAX_CONTEXT_LIMIT_RETRIES
                                ));
                            }
                            retry_after_compaction = true;
                            let _ = event_tx.send(ServerEvent::Compaction {
                                trigger: "auto_recovery".to_string(),
                                pre_tokens: None,
                                post_tokens: None,
                                tokens_saved: None,
                                duration_ms: None,
                                messages_dropped: None,
                                messages_compacted: None,
                                summary_chars: None,
                                active_messages: None,
                            });
                            break;
                        }
                        return Err(StreamError::new(message, retry_after_secs).into());
                    }
                }
            }

            if retry_after_compaction {
                continue;
            }

            let api_elapsed = api_start.elapsed();
            logging::info(&format!(
                "API call complete in {:.2}s (input={} output={} cache_read={} cache_write={})",
                api_elapsed.as_secs_f64(),
                usage_input.unwrap_or(0),
                usage_output.unwrap_or(0),
                usage_cache_read.unwrap_or(0),
                usage_cache_creation.unwrap_or(0),
            ));

            if usage_input.is_some()
                || usage_output.is_some()
                || usage_cache_read.is_some()
                || usage_cache_creation.is_some()
            {
                crate::telemetry::record_token_usage(
                    usage_input.unwrap_or(0),
                    usage_output.unwrap_or(0),
                    usage_cache_read,
                    usage_cache_creation,
                );
            }

            if usage_input.is_some()
                || usage_output.is_some()
                || usage_cache_read.is_some()
                || usage_cache_creation.is_some()
            {
                let _ = event_tx.send(ServerEvent::TokenUsage {
                    input: usage_input.unwrap_or(0),
                    output: usage_output.unwrap_or(0),
                    cache_read_input: usage_cache_read,
                    cache_creation_input: usage_cache_creation,
                });
            }

            // Store usage for debug queries
            self.last_usage = TokenUsage {
                input_tokens: usage_input.unwrap_or(0),
                output_tokens: usage_output.unwrap_or(0),
                cache_read_input_tokens: usage_cache_read,
                cache_creation_input_tokens: usage_cache_creation,
            };

            let had_tool_calls_before = !tool_calls.is_empty();
            self.recover_text_wrapped_tool_call(&mut text_content, &mut tool_calls);

            if !had_tool_calls_before
                && !tool_calls.is_empty()
                && let Some(tc) = tool_calls.last()
                && tc.id.starts_with("fallback_text_call_")
            {
                let _ = event_tx.send(ServerEvent::TextReplace {
                    text: text_content.clone(),
                });
                let _ = event_tx.send(ServerEvent::ToolStart {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                });
                tool_id_to_name.insert(tc.id.clone(), tc.name.clone());
                let _ = event_tx.send(ServerEvent::ToolInput {
                    delta: tc.input.to_string(),
                });
                let _ = event_tx.send(ServerEvent::ToolExec {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                });
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
                let token_usage = Some(crate::session::StoredTokenUsage {
                    input_tokens: self.last_usage.input_tokens,
                    output_tokens: self.last_usage.output_tokens,
                    cache_read_input_tokens: self.last_usage.cache_read_input_tokens,
                    cache_creation_input_tokens: self.last_usage.cache_creation_input_tokens,
                });
                let message_id =
                    self.add_message_ext(Role::Assistant, content_blocks, None, token_usage);
                self.push_embedding_snapshot_if_semantic(&text_content);
                self.session.save()?;
                Some(message_id)
            } else {
                None
            };

            if let Some((encrypted_content, compacted_count)) = openai_native_compaction.take() {
                self.apply_openai_native_compaction(encrypted_content, compacted_count)?;
            }

            // If stop_reason indicates truncation (e.g. max_tokens), discard tool calls
            // with null/empty inputs since they were likely truncated mid-generation.
            self.filter_truncated_tool_calls(
                stop_reason.as_deref(),
                &mut tool_calls,
                assistant_message_id.as_ref(),
            );

            if tool_calls.is_empty() && !generated_image_contexts.is_empty() {
                for blocks in generated_image_contexts.drain(..) {
                    self.add_message(Role::User, blocks);
                }
                self.session.save()?;
                logging::info(
                    "Continuing turn so model can inspect generated image visual context",
                );
                continue;
            }

            // If no tool calls, check for soft interrupt or exit
            // NOTE: We only inject here (Point B) when there are no tools.
            // Injecting before tool_results would break the API requirement that
            // tool_use must be immediately followed by tool_result.
            if tool_calls.is_empty() {
                match self.handle_streaming_no_tool_calls(
                    stop_reason.as_deref(),
                    &mut incomplete_continuations,
                )? {
                    NoToolCallOutcome::Break => break,
                    NoToolCallOutcome::ContinueWithoutEvent => continue,
                    NoToolCallOutcome::ContinueWithSoftInterrupt { injected, point } => {
                        for event in Self::build_soft_interrupt_events(injected, point, None) {
                            let _ = event_tx.send(event);
                        }
                        continue;
                    }
                }
            }

            // If graceful shutdown was signaled during streaming and we have tool calls,
            // we need to provide tool results for them (API requires tool_use -> tool_result)
            // then exit cleanly
            if self.is_graceful_shutdown() {
                logging::info(&format!(
                    "Graceful shutdown - skipping {} tool call(s)",
                    tool_calls.len()
                ));
                for tc in &tool_calls {
                    self.add_message(
                        Role::User,
                        vec![ContentBlock::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: "[Skipped - server reloading]".to_string(),
                            is_error: Some(true),
                        }],
                    );
                }
                self.session.save()?;
                break;
            }

            logging::info(&format!(
                "Turn has {} tool calls to execute",
                tool_calls.len()
            ));

            if self.provider.handles_tools_internally() {
                tool_calls.retain(|tc| JCODE_NATIVE_TOOLS.contains(&tc.name.as_str()));
                if tool_calls.is_empty() {
                    // === INJECTION POINT D: After provider-handled tools, before next API call ===
                    let injected = self.inject_soft_interrupts();
                    if !injected.is_empty() {
                        for event in Self::build_soft_interrupt_events(injected, "D", None) {
                            let _ = event_tx.send(event);
                        }
                        // Don't break - continue loop to process injected message
                        continue;
                    }
                    break;
                }
            }

            // Execute tools and add results
            let tool_count = tool_calls.len();
            let mut tool_results_dirty = false;
            for tool_index in 0..tool_count {
                // === INJECTION POINT C (before): Check for urgent abort before each tool (except first) ===
                if tool_index > 0 && self.has_urgent_interrupt() {
                    crate::telemetry::record_user_cancelled();
                    // Add tool_results for all remaining skipped tools to maintain valid history
                    for skipped_tc in &tool_calls[tool_index..] {
                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: skipped_tc.id.clone(),
                                content: "[Skipped: user interrupted]".to_string(),
                                is_error: Some(true),
                            }],
                        );
                    }
                    let tools_remaining = tool_count - tool_index;
                    let injected = self.inject_soft_interrupts();
                    if !injected.is_empty() {
                        for event in
                            Self::build_soft_interrupt_events(injected, "C", Some(tools_remaining))
                        {
                            let _ = event_tx.send(event);
                        }
                        // Add note about skipped tools for the AI
                        self.add_message(
                            Role::User,
                            vec![ContentBlock::Text {
                                text: format!(
                                    "[User interrupted: {} remaining tool(s) skipped]",
                                    tools_remaining
                                ),
                                cache_control: None,
                            }],
                        );
                    }
                    self.persist_session_best_effort("streamed tool output");
                    break; // Skip remaining tools
                }
                let tc = &tool_calls[tool_index];

                self.validate_tool_allowed(&tc.name)?;

                let message_id = assistant_message_id
                    .clone()
                    .unwrap_or_else(|| self.session.id.clone());

                let is_native_tool = JCODE_NATIVE_TOOLS.contains(&tc.name.as_str());

                if let Some((sdk_content, sdk_is_error)) = sdk_tool_results.remove(&tc.id) {
                    // For native tools, ignore SDK errors and execute locally
                    if !(is_native_tool && sdk_is_error) {
                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: tc.id.clone(),
                                content: sdk_content,
                                is_error: if sdk_is_error { Some(true) } else { None },
                            }],
                        );
                        tool_results_dirty = true;

                        // NOTE: No injection here - wait for Point D after all tools

                        continue;
                    }
                    // Fall through to local execution for native tools with SDK errors
                }

                let ctx = ToolContext {
                    session_id: self.session.id.clone(),
                    message_id: message_id.clone(),
                    tool_call_id: tc.id.clone(),
                    working_dir: self.working_dir().map(PathBuf::from),
                    stdin_request_tx: self.stdin_request_tx.clone(),
                    graceful_shutdown_signal: Some(self.graceful_shutdown.clone()),
                    execution_mode: ToolExecutionMode::AgentTurn,
                };

                if trace {
                    eprintln!("[trace] tool_exec_start name={} id={}", tc.name, tc.id);
                }

                logging::info(&format!("Tool starting: {}", tc.name));
                let tool_start = Instant::now();

                // Spawn tool in its own task so we can detach it to background on Alt+B
                let registry_clone = self.registry.clone();
                let tool_name_for_spawn = tc.name.clone();
                let tool_input_for_spawn = tc.input.clone();
                let tool_handle = tokio::spawn(async move {
                    registry_clone
                        .execute(&tool_name_for_spawn, tool_input_for_spawn, ctx)
                        .await
                });

                // Reset background signal before waiting
                self.background_tool_signal.reset();

                // Wait for tool completion OR background signal from user (Alt+B)
                // OR graceful shutdown signal from server reload
                let bg_signal = self.background_tool_signal.clone();
                let shutdown_signal = self.graceful_shutdown.clone();
                let allow_reload_handoff = tc.name == "bash";
                let tool_result;
                let mut tool_handle = tool_handle;
                tokio::select! {
                    biased;
                    res = &mut tool_handle => {
                        tool_result = Some(match res {
                            Ok(r) => r,
                            Err(e) => Err(anyhow::anyhow!("Tool task panicked: {}", e)),
                        });
                    }
                    _ = async {
                        tokio::select! {
                            _ = bg_signal.notified() => {}
                            _ = shutdown_signal.notified() => {}
                        }
                    } => {
                        if self.is_graceful_shutdown() && allow_reload_handoff {
                            tool_result = match tokio::time::timeout(
                                Duration::from_millis(750),
                                &mut tool_handle,
                            )
                            .await
                            {
                                Ok(res) => Some(match res {
                                    Ok(r) => r,
                                    Err(e) => Err(anyhow::anyhow!("Tool task panicked: {}", e)),
                                }),
                                Err(_) => None,
                            };
                        } else {
                            tool_result = None;
                        }
                    }
                };

                self.unlock_tools_if_needed(&tc.name);
                let tool_elapsed = tool_start.elapsed();

                if let Some(result) = tool_result {
                    // Normal tool completion
                    logging::info(&format!(
                        "Tool finished: {} in {:.2}s",
                        tc.name,
                        tool_elapsed.as_secs_f64()
                    ));

                    match result {
                        Ok(output) => {
                            let _ = event_tx.send(ServerEvent::ToolDone {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                output: output.output.clone(),
                                error: None,
                            });

                            let blocks = tool_output_to_content_blocks(tc.id.clone(), output);
                            self.add_message_with_duration(
                                Role::User,
                                blocks,
                                Some(tool_elapsed.as_millis() as u64),
                            );
                            tool_results_dirty = true;
                        }
                        Err(e) => {
                            let error_msg = format!("Error: {}", e);
                            let _ = event_tx.send(ServerEvent::ToolDone {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                output: error_msg.clone(),
                                error: Some(error_msg.clone()),
                            });

                            self.add_message_with_duration(
                                Role::User,
                                vec![ContentBlock::ToolResult {
                                    tool_use_id: tc.id.clone(),
                                    content: error_msg,
                                    is_error: Some(true),
                                }],
                                Some(tool_elapsed.as_millis() as u64),
                            );
                            tool_results_dirty = true;
                        }
                    }
                } else if self.is_graceful_shutdown() {
                    // Server reload - abort tool and save interrupted result
                    logging::info(&format!(
                        "Tool '{}' interrupted by server reload after {:.1}s",
                        tc.name,
                        tool_elapsed.as_secs_f64()
                    ));
                    tool_handle.abort();

                    // For selfdev reload, the interruption is intentional -
                    // the tool triggered the reload and blocked waiting for shutdown.
                    // Use a non-error message so the conversation history is clean.
                    let is_selfdev_reload = tc.name == "selfdev";
                    let interrupted_msg = if is_selfdev_reload {
                        "Reload initiated. Process restarting...".to_string()
                    } else {
                        format!(
                            "[Tool '{}' interrupted by server reload after {:.1}s]",
                            tc.name,
                            tool_elapsed.as_secs_f64()
                        )
                    };

                    let _ = event_tx.send(ServerEvent::ToolDone {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        output: interrupted_msg.clone(),
                        error: if is_selfdev_reload {
                            None
                        } else {
                            Some("interrupted by reload".to_string())
                        },
                    });

                    self.add_message_with_duration(
                        Role::User,
                        vec![ContentBlock::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: interrupted_msg,
                            is_error: Some(!is_selfdev_reload),
                        }],
                        Some(tool_elapsed.as_millis() as u64),
                    );
                    self.session.save()?;

                    // Add results for any remaining tools too
                    for remaining_tc in &tool_calls[(tool_index + 1)..] {
                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: remaining_tc.id.clone(),
                                content: "[Skipped - server reloading]".to_string(),
                                is_error: Some(true),
                            }],
                        );
                    }
                    self.session.save()?;
                    return Ok(());
                } else {
                    // User pressed Alt+B — move tool to background
                    logging::info(&format!(
                        "Tool '{}' moved to background after {:.1}s",
                        tc.name,
                        tool_elapsed.as_secs_f64()
                    ));

                    let bg_info = crate::background::global()
                        .adopt(&tc.name, &self.session.id, tool_handle)
                        .await;

                    let bg_msg = format!(
                        "Tool '{}' was moved to background by the user (task_id: {}). \
                         Use the `bg` tool with action 'wait' to wait for completion/checkpoints, \
                         or action 'status'/'output' to inspect it.",
                        tc.name, bg_info.task_id
                    );

                    let _ = event_tx.send(ServerEvent::ToolDone {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        output: bg_msg.clone(),
                        error: None,
                    });

                    self.add_message_with_duration(
                        Role::User,
                        vec![ContentBlock::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: bg_msg,
                            is_error: None,
                        }],
                        Some(tool_elapsed.as_millis() as u64),
                    );
                    self.session.save()?;

                    self.background_tool_signal.reset();
                }

                // NOTE: We do NOT inject between tools (non-urgent) because that would
                // place user text between tool_results, which may violate API constraints.
                // All non-urgent injection happens at Point D after all tools are done.
            }

            if tool_results_dirty {
                self.session.save()?;
            }

            if !generated_image_contexts.is_empty() {
                for blocks in generated_image_contexts.drain(..) {
                    self.add_message(Role::User, blocks);
                }
                self.session.save()?;
            }

            // === INJECTION POINT D: All tools done, before next API call ===
            // This is the safest point for non-urgent injection since all tool_results
            // have been added and the conversation is in a valid state.
            if let PostToolInterruptOutcome::SoftInterrupt { injected, point } =
                self.take_post_tool_soft_interrupt()
            {
                for event in Self::build_soft_interrupt_events(injected, point, None) {
                    let _ = event_tx.send(event);
                }
            }
        }

        Ok(())
    }
}
