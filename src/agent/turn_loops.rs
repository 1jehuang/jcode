use super::*;

impl Agent {
    /// Run turns until no more tool calls
    /// Maximum number of context-limit compaction retries before giving up.
    pub(super) const MAX_CONTEXT_LIMIT_RETRIES: u32 = 5;
    pub(super) const MAX_INCOMPLETE_CONTINUATION_ATTEMPTS: u32 = 3;

    pub(super) async fn run_turn(&mut self, print_output: bool) -> Result<String> {
        self.set_log_context();
        let mut final_text = String::new();
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
                if print_output {
                    let tokens_str = event
                        .pre_tokens
                        .map(|t| format!(" ({} tokens)", t))
                        .unwrap_or_default();
                    println!("📦 Context compacted ({}){}", event.trigger, tokens_str);
                }
            }

            let tools = self.tool_definitions().await;
            let messages: std::sync::Arc<[Message]> = messages.into();
            // Non-blocking memory: uses pending result from last turn, spawns check for next turn
            let memory_pending =
                self.build_memory_prompt_nonblocking_shared(std::sync::Arc::clone(&messages), None);
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
                let age_ms = memory.computed_at.elapsed().as_millis() as u64;
                crate::memory::record_injected_prompt(&memory.prompt, memory_count, age_ms);
                self.record_memory_injection_in_session(memory);
                logging::info(&format!(
                    "Memory injected as message ({} chars)",
                    memory.prompt.len()
                ));
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

            // Publish status for TUI to show during Task execution
            Bus::global().publish(BusEvent::SubagentStatus(SubagentStatus {
                session_id: self.session.id.clone(),
                status: "calling API".to_string(),
                model: Some(self.provider.model()),
            }));

            let stamped;
            let send_messages: &[Message] = if crate::config::config().features.message_timestamps {
                stamped = Message::with_timestamps(&messages_with_memory);
                &stamped
            } else {
                &messages_with_memory
            };
            self.last_status_detail = None;
            let mut stream = match self
                .provider
                .complete_split(
                    send_messages,
                    &tools,
                    &split_prompt.static_part,
                    &split_prompt.dynamic_part,
                    self.provider_session_id.as_deref(),
                )
                .await
            {
                Ok(stream) => stream,
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
                        continue;
                    }
                    return Err(e);
                }
            };

            // Successful API call - reset retry counter
            context_limit_retries = 0;

            logging::info(&format!(
                "API stream opened in {:.2}s",
                api_start.elapsed().as_secs_f64()
            ));

            Bus::global().publish(BusEvent::SubagentStatus(SubagentStatus {
                session_id: self.session.id.clone(),
                status: "streaming".to_string(),
                model: Some(self.provider.model()),
            }));

            let mut text_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut generated_image_contexts: Vec<Vec<ContentBlock>> = Vec::new();
            let mut usage_input: Option<u64> = None;
            let mut usage_output: Option<u64> = None;
            let mut usage_cache_read: Option<u64> = None;
            let mut usage_cache_creation: Option<u64> = None;
            let mut saw_message_end = false;
            let mut stop_reason: Option<String> = None;
            let mut _thinking_start: Option<Instant> = None;
            let store_reasoning_content = self.provider.name() == "openrouter";
            let mut reasoning_content = String::new();
            // Track tool results from provider (already executed by Claude Code CLI)
            let mut sdk_tool_results: std::collections::HashMap<String, (String, bool)> =
                std::collections::HashMap::new();
            let mut openai_native_compaction: Option<(String, usize)> = None;

            let mut retry_after_compaction = false;
            while let Some(event) = stream.next().await {
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
                            break;
                        }
                        return Err(e);
                    }
                };

                match event {
                    StreamEvent::ThinkingStart => {
                        // Track start but don't print - wait for ThinkingDone
                        _thinking_start = Some(Instant::now());
                    }
                    StreamEvent::ThinkingDelta(thinking_text) => {
                        // Display reasoning content only if enabled
                        if print_output && crate::config::config().display.show_thinking {
                            println!("💭 {}", thinking_text);
                        }
                        if store_reasoning_content {
                            reasoning_content.push_str(&thinking_text);
                        }
                    }
                    StreamEvent::ThinkingEnd => {
                        // Don't print here - ThinkingDone has accurate timing
                        _thinking_start = None;
                    }
                    StreamEvent::ThinkingDone { duration_secs } => {
                        // Bridge provides accurate wall-clock timing
                        if print_output {
                            println!("Thought for {:.1}s\n", duration_secs);
                        }
                    }
                    StreamEvent::TextDelta(text) => {
                        if print_output {
                            print!("{}", text);
                            io::stdout().flush()?;
                        }
                        text_content.push_str(&text);
                    }
                    StreamEvent::ToolUseStart { id, name } => {
                        if trace {
                            eprintln!("\n[trace] tool_use_start name={} id={}", name, id);
                        }
                        if print_output {
                            print!("\n[{}] ", name);
                            io::stdout().flush()?;
                        }
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
                        if let Some(mut tool) = current_tool.take() {
                            // Parse the accumulated JSON
                            let tool_input =
                                serde_json::from_str::<serde_json::Value>(&current_tool_input)
                                    .unwrap_or(serde_json::Value::Null);
                            tool.input = tool_input.clone();
                            tool.intent = ToolCall::intent_from_input(&tool_input);

                            if trace {
                                if current_tool_input.trim().is_empty() {
                                    eprintln!("[trace] tool_input {} (empty)", tool.name);
                                } else if tool_input == serde_json::Value::Null {
                                    eprintln!(
                                        "[trace] tool_input {} (raw) {}",
                                        tool.name, current_tool_input
                                    );
                                } else {
                                    let pretty = serde_json::to_string_pretty(&tool_input)
                                        .unwrap_or_else(|_| tool_input.to_string());
                                    eprintln!("[trace] tool_input {} {}", tool.name, pretty);
                                }
                            }

                            if print_output {
                                // Show brief tool info
                                print_tool_summary(&tool);
                            }

                            tool_calls.push(tool);
                            current_tool_input.clear();
                        }
                    }
                    StreamEvent::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        // SDK already executed this tool, store the result
                        if trace {
                            eprintln!(
                                "[trace] sdk_tool_result id={} is_error={} content_len={}",
                                tool_use_id,
                                is_error,
                                content.len()
                            );
                        }
                        sdk_tool_results.insert(tool_use_id, (content, is_error));
                    }
                    StreamEvent::GeneratedImage {
                        id,
                        path,
                        metadata_path,
                        output_format,
                        revised_prompt,
                    } => {
                        if trace {
                            eprintln!(
                                "[trace] generated_image id={} format={} path={} metadata={}",
                                id,
                                output_format,
                                path,
                                metadata_path.as_deref().unwrap_or("none")
                            );
                        }
                        if print_output {
                            let summary = crate::message::generated_image_summary(
                                &path,
                                metadata_path.as_deref(),
                                &output_format,
                                revised_prompt.as_deref(),
                            );
                            eprintln!(
                                "\n[{}] {}",
                                crate::message::GENERATED_IMAGE_TOOL_NAME,
                                summary
                            );
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
                        if trace {
                            eprintln!(
                                "[trace] token_usage input={} output={} cache_read={} cache_write={}",
                                usage_input.unwrap_or(0),
                                usage_output.unwrap_or(0),
                                usage_cache_read.unwrap_or(0),
                                usage_cache_creation.unwrap_or(0)
                            );
                        }
                    }
                    StreamEvent::ConnectionType { connection } => {
                        if trace {
                            eprintln!("[trace] connection_type={}", connection);
                        }
                        crate::telemetry::record_connection_type(&connection);
                        self.last_connection_type = Some(connection);
                    }
                    StreamEvent::ConnectionPhase { phase } => {
                        if trace {
                            eprintln!("[trace] connection_phase={}", phase);
                        }
                    }
                    StreamEvent::StatusDetail { detail } => {
                        if trace {
                            eprintln!("[trace] status_detail={}", detail);
                        }
                        self.last_status_detail = Some(detail);
                    }
                    StreamEvent::MessageEnd {
                        stop_reason: reason,
                    } => {
                        saw_message_end = true;
                        if reason.is_some() {
                            stop_reason = reason;
                        }
                        // Don't break yet - wait for SessionId which comes after MessageEnd
                        // (but stream close will also end the loop for providers without SessionId)
                    }
                    StreamEvent::SessionId(sid) => {
                        if trace {
                            eprintln!("[trace] session_id {}", sid);
                        }
                        self.provider_session_id = Some(sid.clone());
                        self.session.provider_session_id = Some(sid);
                        // We've received session_id, can exit the loop now
                        if saw_message_end {
                            break;
                        }
                    }
                    StreamEvent::UpstreamProvider { provider } => {
                        // Log upstream provider for local trace output
                        if trace {
                            eprintln!("[trace] upstream_provider={}", provider);
                        }
                        self.last_upstream_provider = Some(provider);
                    }
                    StreamEvent::Compaction {
                        trigger,
                        pre_tokens,
                        openai_encrypted_content,
                    } => {
                        if let Some(encrypted_content) = openai_encrypted_content {
                            openai_native_compaction
                                .get_or_insert((encrypted_content, self.session.messages.len()));
                        }
                        if print_output {
                            let tokens_str = pre_tokens
                                .map(|t| format!(" ({} tokens)", t))
                                .unwrap_or_default();
                            println!("📦 Context compacted ({}){}", trigger, tokens_str);
                        }
                    }
                    StreamEvent::NativeToolCall {
                        request_id,
                        tool_name,
                        input,
                    } => {
                        // Execute native tool and send result back to SDK bridge
                        if trace {
                            eprintln!(
                                "[trace] native_tool_call request_id={} tool={}",
                                request_id, tool_name
                            );
                        }
                        let ctx = ToolContext {
                            session_id: self.session.id.clone(),
                            message_id: self.session.id.clone(),
                            tool_call_id: request_id.clone(),
                            working_dir: self.working_dir().map(PathBuf::from),
                            sandbox_root: self.sandbox_root(),
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
                        // Send result back to SDK bridge
                        if let Some(sender) = self.provider.native_result_sender() {
                            let _ = sender.send(native_result).await;
                        }
                    }
                    StreamEvent::Error {
                        message,
                        retry_after_secs,
                    } => {
                        if trace {
                            eprintln!("[trace] stream_error {}", message);
                        }
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

            if print_output
                && (usage_input.is_some()
                    || usage_output.is_some()
                    || usage_cache_read.is_some()
                    || usage_cache_creation.is_some())
            {
                let input = usage_input.unwrap_or(0);
                let output = usage_output.unwrap_or(0);
                let cache_read = usage_cache_read.unwrap_or(0);
                let cache_creation = usage_cache_creation.unwrap_or(0);
                let cache_str = if usage_cache_read.is_some() || usage_cache_creation.is_some() {
                    format!(
                        " cache_read: {} cache_write: {}",
                        cache_read, cache_creation
                    )
                } else {
                    String::new()
                };
                print!(
                    "\n[Tokens] upload: {} download: {}{}\n",
                    input, output, cache_str
                );
                io::stdout().flush()?;
            }

            // Store usage for debug queries
            self.last_usage = TokenUsage {
                input_tokens: usage_input.unwrap_or(0),
                output_tokens: usage_output.unwrap_or(0),
                cache_read_input_tokens: usage_cache_read,
                cache_creation_input_tokens: usage_cache_creation,
            };

            self.recover_text_wrapped_tool_call(&mut text_content, &mut tool_calls);

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
            // This prevents executing broken tool calls and instead requests a continuation.
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

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                if self.maybe_continue_incomplete_response(
                    stop_reason.as_deref(),
                    &mut incomplete_continuations,
                )? {
                    continue;
                }
                logging::info("Turn complete - no tool calls, returning");
                if print_output {
                    println!();
                }
                final_text = text_content;
                break;
            }

            logging::info(&format!(
                "Turn has {} tool calls to execute",
                tool_calls.len()
            ));

            // If provider handles tools internally (like Claude Code CLI), only run native tools locally
            if self.provider.handles_tools_internally() {
                tool_calls.retain(|tc| JCODE_NATIVE_TOOLS.contains(&tc.name.as_str()));
                if tool_calls.is_empty() {
                    if !generated_image_contexts.is_empty() {
                        for blocks in generated_image_contexts.drain(..) {
                            self.add_message(Role::User, blocks);
                        }
                        self.session.save()?;
                        logging::info(
                            "Continuing turn so model can inspect generated image visual context",
                        );
                        continue;
                    }
                    logging::info("Provider handles tools internally - task complete");
                    break;
                }
                logging::info("Provider handles tools internally - executing native tools locally");
            }

            // Execute tools and add results
            let mut tool_results_dirty = false;
            for tc in tool_calls {
                let message_id = assistant_message_id
                    .clone()
                    .unwrap_or_else(|| self.session.id.clone());

                if let Some(error_msg) = tc.validation_error() {
                    logging::warn(&error_msg);
                    Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                        session_id: self.session.id.clone(),
                        message_id: message_id.clone(),
                        tool_call_id: tc.id.clone(),
                        tool_name: tc.name.clone(),
                        status: ToolStatus::Error,
                        title: None,
                    }));
                    if print_output {
                        println!("\n  → {}", error_msg);
                    }
                    self.add_message(
                        Role::User,
                        vec![ContentBlock::ToolResult {
                            tool_use_id: tc.id,
                            content: error_msg,
                            is_error: Some(true),
                        }],
                    );
                    tool_results_dirty = true;
                    continue;
                }

                self.validate_tool_allowed(&tc.name)?;

                let is_native_tool = JCODE_NATIVE_TOOLS.contains(&tc.name.as_str());

                // Check if SDK already executed this tool
                if let Some((sdk_content, sdk_is_error)) = sdk_tool_results.remove(&tc.id) {
                    // For native tools, ignore SDK errors and execute locally
                    if is_native_tool && sdk_is_error {
                        if trace {
                            eprintln!(
                                "[trace] sdk_error_for_native_tool name={} id={}, executing locally",
                                tc.name, tc.id
                            );
                        }
                        // Fall through to local execution below
                    } else {
                        if trace {
                            eprintln!(
                                "[trace] using_sdk_result name={} id={} is_error={}",
                                tc.name, tc.id, sdk_is_error
                            );
                        }
                        if print_output {
                            print!("\n  → ");
                            let preview = if sdk_content.len() > 200 {
                                format!("{}...", crate::util::truncate_str(&sdk_content, 200))
                            } else {
                                sdk_content.clone()
                            };
                            println!("{}", preview.lines().next().unwrap_or("(done via SDK)"));
                        }

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

                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: tc.id,
                                content: sdk_content,
                                is_error: if sdk_is_error { Some(true) } else { None },
                            }],
                        );
                        tool_results_dirty = true;
                        continue;
                    }
                }

                // SDK didn't execute this tool, run it locally
                if print_output {
                    print!("\n  → ");
                    io::stdout().flush()?;
                }

                let ctx = ToolContext {
                    session_id: self.session.id.clone(),
                    message_id: message_id.clone(),
                    tool_call_id: tc.id.clone(),
                    working_dir: self.working_dir().map(PathBuf::from),
                    sandbox_root: self.sandbox_root(),
                    stdin_request_tx: self.stdin_request_tx.clone(),
                    graceful_shutdown_signal: Some(self.graceful_shutdown.clone()),
                    execution_mode: ToolExecutionMode::AgentTurn,
                };

                if trace {
                    eprintln!("[trace] tool_exec_start name={} id={}", tc.name, tc.id);
                }
                Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                    session_id: self.session.id.clone(),
                    message_id: message_id.clone(),
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                    status: ToolStatus::Running,
                    title: None,
                }));

                logging::info(&format!("Tool starting: {}", tc.name));
                let tool_start = Instant::now();

                // Publish status for TUI to show during Task execution
                Bus::global().publish(BusEvent::SubagentStatus(SubagentStatus {
                    session_id: self.session.id.clone(),
                    status: format!("running {}", tc.name),
                    model: Some(self.provider.model()),
                }));

                let result = self.registry.execute(&tc.name, tc.input.clone(), ctx).await;
                crate::telemetry::record_tool_call();
                self.unlock_tools_if_needed(&tc.name);
                let tool_elapsed = tool_start.elapsed();
                logging::info(&format!(
                    "Tool finished: {} in {:.2}s",
                    tc.name,
                    tool_elapsed.as_secs_f64()
                ));

                match result {
                    Ok(output) => {
                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: ToolStatus::Completed,
                            title: output.title.clone(),
                        }));

                        if trace {
                            eprintln!(
                                "[trace] tool_exec_done name={} id={}\n{}",
                                tc.name, tc.id, output.output
                            );
                        }
                        if print_output {
                            let preview = if output.output.len() > 200 {
                                format!("{}...", crate::util::truncate_str(&output.output, 200))
                            } else {
                                output.output.clone()
                            };
                            println!("{}", preview.lines().next().unwrap_or("(done)"));
                        }

                        let blocks = tool_output_to_content_blocks(tc.id, output);
                        self.add_message_with_duration(
                            Role::User,
                            blocks,
                            Some(tool_elapsed.as_millis() as u64),
                        );
                        tool_results_dirty = true;
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

                        let error_msg = format!("Error: {}", e);
                        if trace {
                            eprintln!(
                                "[trace] tool_exec_error name={} id={} {}",
                                tc.name, tc.id, error_msg
                            );
                        }
                        if print_output {
                            println!("{}", error_msg);
                        }
                        self.add_message_with_duration(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: tc.id,
                                content: error_msg,
                                is_error: Some(true),
                            }],
                            Some(tool_elapsed.as_millis() as u64),
                        );
                        tool_results_dirty = true;
                    }
                }
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

            if print_output {
                println!();
            }

            // Check for soft interrupts (e.g. Telegram messages) and inject them for the next turn
            let injected = self.inject_soft_interrupts();
            if !injected.is_empty() {
                let total_chars: usize = injected.iter().map(|item| item.content.len()).sum();
                logging::info(&format!(
                    "Soft interrupt injected into headless turn ({} message(s), {} chars)",
                    injected.len(),
                    total_chars
                ));
            }
        }

        Ok(final_text)
    }
}
