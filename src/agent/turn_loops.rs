use super::*;
use crate::agent::turn_strategy::{TurnStrategy, StandardTurnStrategy};

impl Agent {
    /// Run turns until no more tool calls
    /// Maximum number of context-limit compaction retries before giving up.
    pub(super) const MAX_CONTEXT_LIMIT_RETRIES: u32 = 5;
    pub(super) const MAX_INCOMPLETE_CONTINUATION_ATTEMPTS: u32 = 3;

    pub(super) async fn run_turn(&mut self, print_output: bool) -> Result<String> {
        self.run_turn_with_strategy(&StandardTurnStrategy::new(), print_output).await
    }

    /// Run a turn with a pluggable strategy. Each phase of the turn loop
    /// delegates to the strategy, enabling different behaviors (swarm, batch, etc.)
    /// without duplicating the stream handling and tool execution logic.
    pub(crate) async fn run_turn_with_strategy<S: TurnStrategy>(
        &mut self,
        strategy: &S,
        print_output: bool,
    ) -> Result<String> {
        self.set_log_context();
        let mut final_text = String::new();
        let trace = trace_enabled();
        let mut context_limit_retries = 0u32;
        let mut incomplete_continuations = 0u32;

        loop {
            // Phase 1: Repair missing tool outputs
            let repaired = strategy.repair(self);
            if repaired > 0 {
                logging::warn(&format!(
                    "Recovered {} missing tool output(s) before API call",
                    repaired
                ));
            }

            // Phase 2-3: Prepare messages + handle compaction
            let (messages, compaction_event) = strategy.prepare_messages(self);
            if let Some(event) = compaction_event {
                strategy.handle_compaction(self, &event, print_output);
            }

            // Phase 4-8: Tools + memory + prompt + cache + microcompact
            let tools = strategy.tool_defs(self).await;
            let messages: std::sync::Arc<[Message]> = messages.into();
            let memory_pending = strategy.build_memory(self, std::sync::Arc::clone(&messages));
            let split_prompt = strategy.build_prompt(self);
            self.log_prompt_prefix_accounting(&split_prompt, &tools);
            strategy.record_cache(self, &messages);
            let mut messages_with_memory: Vec<Message> = messages.to_vec();
            strategy.microcompact(&mut messages_with_memory, print_output);

            // Phase 9: Inject memory
            if let Some(ref memory) = memory_pending {
                strategy.inject_memory(&mut messages_with_memory, memory);
                self.record_memory_injection_in_session(memory);
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
            // ===== 智能错误恢复：分类错误 + 指数退避重试 =====
            // 借鉴 Claude Code 的错误分类与恢复策略
            use crate::error_recovery::ErrorClassifier;

            let mut stream = loop {
                match self
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
                    Ok(stream) => break stream,
                    Err(e) => {
                        let error_str = e.to_string();

                        // 原有：上下文压缩重试
                        if self.try_auto_compact_after_context_limit(&error_str) {
                            context_limit_retries += 1;
                            if context_limit_retries > Self::MAX_CONTEXT_LIMIT_RETRIES {
                                logging::warn("Context-limit compaction retry limit reached; giving up");
                                return Err(anyhow::anyhow!(
                                    "Context limit exceeded after {} compaction retries",
                                    Self::MAX_CONTEXT_LIMIT_RETRIES
                                ));
                            }
                            continue;
                        }

                        // 新增：智能错误分类 + 选择性重试
                        let classified = ErrorClassifier::classify(&error_str);
                        match classified.severity {
                            crate::error_recovery::ErrorSeverity::Transient
                            | crate::error_recovery::ErrorSeverity::Retryable
                            | crate::error_recovery::ErrorSeverity::Degradable => {
                                // 对可恢复错误执行指数退避重试
                                // 已在上下文的 retry 循环中，这里只做 1 次额外重试
                                logging::warn(&format!(
                                    "LLM 调用错误 ({}): {} — 尝试重试",
                                    error_str,
                                    classified.degradation.unwrap_or_else(|| "无降级方案".to_string())
                                ));
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                                // 重新进入外层 loop 重试
                                continue;
                            }
                            crate::error_recovery::ErrorSeverity::Fatal => {
                                let msg = if let Some(d) = classified.degradation {
                                    format!("{} — {}", error_str, d)
                                } else {
                                    error_str
                                };
                                return Err(anyhow::anyhow!(msg));
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
            let store_reasoning_content = matches!(
                self.provider.name(),
                "openrouter" | "bedrock" | "gemini" | "claude"
            );
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

            // If no tool calls, check token budget auto-continue before stopping
            if tool_calls.is_empty() {
                // First: handle incomplete/truncated responses (max_tokens hit)
                if self.maybe_continue_incomplete_response(
                    stop_reason.as_deref(),
                    &mut incomplete_continuations,
                )? {
                    continue;
                }

                // Second: check token budget for auto-continue on long tasks
                let global_turn_tokens = usage_input.unwrap_or(0) + usage_output.unwrap_or(0);
                match self.token_budget_tracker.check(global_turn_tokens) {
                    crate::token_budget::BudgetDecision::Continue {
                        nudge_message,
                        continuation_count,
                        pct,
                        turn_tokens,
                        budget,
                    } => {
                        logging::info(&format!(
                            "Token budget auto-continue #{continuation_count}: {pct}% ({turn_tokens}/{budget} tokens)"
                        ));
                        if print_output {
                            println!(
                                "\n[auto-continue] Budget at {pct}% — continuing task (#{continuation_count})\n"
                            );
                        }
                        // Inject nudge as a user message so the model continues
                        self.add_message(Role::User, vec![ContentBlock::Text {
                            text: nudge_message,
                            cache_control: None,
                        }]);
                        self.session.save()?;
                        continue;
                    }
                    crate::token_budget::BudgetDecision::Stop { completion_event } => {
                        if let Some(ev) = completion_event {
                            logging::info(&format!(
                                "Token budget complete: {} continuations in {}ms (diminished={})",
                                ev.continuation_count, ev.duration_ms, ev.diminishing_returns
                            ));
                            if print_output {
                                println!(
                                    "\n[auto-continue] Task complete after {} turn(s)\n",
                                    ev.continuation_count
                                );
                            }
                            // Record telemetry
                            crate::telemetry::record_auto_continue_completed(
                                ev.continuation_count,
                                ev.duration_ms,
                                ev.diminishing_returns,
                            );
                        }
                        // Reset budget tracker for next user request
                        self.token_budget_tracker.reset();
                    }
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

            // 执行工具并收集结果 — 三段式: 验证收集 → 并发执行 → 结果处理
            let mut tool_results_dirty = false;

            // — Phase 1: 验证 + SDK检查 + 收集本地工具 —
            let mut phase2_tools: Vec<PendingNativeTool> = Vec::new();

            for tc in tool_calls {
                let message_id = assistant_message_id
                    .clone()
                    .unwrap_or_else(|| self.session.id.clone());

                // 验证错误 → 立即返回结果，不进入 Phase 2
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
                    if print_output { println!("\n  -> {}", error_msg); }
                    self.add_message(Role::User, vec![ContentBlock::ToolResult {
                        tool_use_id: tc.id, content: error_msg, is_error: Some(true),
                    }]);
                    tool_results_dirty = true;
                    continue;
                }

                self.validate_tool_allowed(&tc.name)?;
                let is_native = JCODE_NATIVE_TOOLS.contains(&tc.name.as_str());

                // SDK 已执行 → 直接使用结果，不进入 Phase 2
                if let Some((sdk_content, sdk_is_error)) = sdk_tool_results.remove(&tc.id) {
                    if is_native && sdk_is_error {
                        // 原生工具 SDK 报错 → 本地重试，放入 Phase 2
                        if trace { eprintln!("[trace] sdk_error_for_native tool={} id={}, fallback to local", tc.name, tc.id); }
                    } else {
                        if trace { eprintln!("[trace] using_sdk_result tool={} id={} err={}", tc.name, tc.id, sdk_is_error); }
                        if print_output {
                            let preview = if sdk_content.len() > 200 {
                                format!("{}...", crate::util::truncate_str(&sdk_content, 200))
                            } else { sdk_content.clone() };
                            println!("{}", preview.lines().next().unwrap_or("(done via SDK)"));
                        }
                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(), message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(), tool_name: tc.name.clone(),
                            status: if sdk_is_error { ToolStatus::Error } else { ToolStatus::Completed }, title: None,
                        }));
                        self.add_message(Role::User, vec![ContentBlock::ToolResult {
                            tool_use_id: tc.id, content: sdk_content, is_error: if sdk_is_error { Some(true) } else { None },
                        }]);
                        tool_results_dirty = true;
                        continue;
                    }
                }

                // 收集到 Phase 2 — 记录执行前快照
                if tc.name == "edit" || tc.name == "multiedit" || tc.name == "batch_edit" || tc.name == "write" {
                    if let Some(file_path) = tc.input.get("file_path").and_then(|v| v.as_str()) {
                        self.snapshot_file(std::path::Path::new(file_path)).await;
                    }
                    if tc.name == "batch_edit" {
                        if let Some(files) = tc.input.get("files").and_then(|v| v.as_array()) {
                            for f in files {
                                if let Some(path) = f.as_str() { self.snapshot_file(std::path::Path::new(path)).await; }
                            }
                        }
                    }
                }

                if print_output { print!("\n  -> "); io::stdout().flush()?; }
                Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                    session_id: self.session.id.clone(), message_id: message_id.clone(),
                    tool_call_id: tc.id.clone(), tool_name: tc.name.clone(),
                    status: ToolStatus::Running, title: None,
                }));

                phase2_tools.push(PendingNativeTool {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.input.clone(),
                    message_id: message_id.clone(),
                    ctx: ToolContext {
                        session_id: self.session.id.clone(),
                        message_id: message_id.clone(),
                        tool_call_id: tc.id.clone(),
                        working_dir: self.working_dir().map(PathBuf::from),
                        stdin_request_tx: self.stdin_request_tx.clone(),
                        graceful_shutdown_signal: Some(self.graceful_shutdown.clone()),
                        execution_mode: ToolExecutionMode::AgentTurn,
                    },
                });
            }

            // — Phase 2: 并发执行所有本地工具 —
            let phase3_results: Vec<NativeToolOutcome> = if !phase2_tools.is_empty() && true {
                // 通知
                for t in &phase2_tools {
                    logging::info(&format!("Concurrent tool starting: {}", t.name));
                    Bus::global().publish(BusEvent::SubagentStatus(SubagentStatus {
                        session_id: self.session.id.clone(),
                        status: format!("running {}", t.name),
                        model: Some(self.provider.model()),
                    }));
                }
                // 并发执行
                let registry = self.registry.clone();
                let tools: Vec<_> = phase2_tools.iter().map(|t| {
                    (t.id.clone(), t.name.clone(), t.input.clone(), t.ctx.clone())
                }).collect();
                let mut outcomes = Vec::new();
                let handles: Vec<_> = tools.into_iter().map(|(id, name, input, ctx)| {
                    let reg = registry.clone();
                    tokio::spawn(async move {
                        let start = Instant::now();
                        let result = reg.execute(&name, input, ctx).await;
                        (id, name, start.elapsed(), result)
                    })
                }).collect();
                for handle in handles {
                    if let Ok((id, name, elapsed, result)) = handle.await {
                        crate::telemetry::record_tool_call();
                        outcomes.push(NativeToolOutcome { id, name, elapsed, result });
                    }
                }
                outcomes
            } else {
                // 回退: 顺序执行（向后兼容）
                let mut outcomes = Vec::new();
                for t in &phase2_tools {
                    logging::info(&format!("Tool starting: {}", t.name));
                    Bus::global().publish(BusEvent::SubagentStatus(SubagentStatus {
                        session_id: self.session.id.clone(),
                        status: format!("running {}", t.name),
                        model: Some(self.provider.model()),
                    }));
                    let start = Instant::now();
                    let result = self.registry.execute(&t.name, t.input.clone(), t.ctx.clone()).await;
                    crate::telemetry::record_tool_call();
                    outcomes.push(NativeToolOutcome {
                        id: t.id.clone(), name: t.name.clone(), elapsed: start.elapsed(), result,
                    });
                }
                outcomes
            };

            // — Phase 3: 处理所有执行结果 —
            for outcome in phase3_results {
                let tc_id = &outcome.id;
                let tc_name = &outcome.name;
                let tool_elapsed = outcome.elapsed;
                let message_id = phase2_tools.iter()
                    .find(|t| t.id == *tc_id)
                    .map(|t| t.message_id.clone())
                    .unwrap_or_else(|| self.session.id.clone());

                self.unlock_tools_if_needed(tc_name);
                logging::info(&format!("Tool finished: {} in {:.2}s", tc_name, tool_elapsed.as_secs_f64()));

                match outcome.result {
                    Ok(output) => {
                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(), message_id: message_id.clone(),
                            tool_call_id: tc_id.clone(), tool_name: tc_name.clone(),
                            status: ToolStatus::Completed, title: output.title.clone(),
                        }));
                        if trace { eprintln!("[trace] tool_exec_done name={} id={}\n{}", tc_name, tc_id, output.output); }
                        if print_output {
                            let preview = if output.output.len() > 200 {
                                format!("{}...", crate::util::truncate_str(&output.output, 200))
                            } else { output.output.clone() };
                            println!("{}", preview.lines().next().unwrap_or("(done)"));
                        }
                        let blocks = tool_output_to_content_blocks(tc_id.clone(), output);
                        self.add_message_with_duration(Role::User, blocks, Some(tool_elapsed.as_millis() as u64));
                        tool_results_dirty = true;
                    }
                    Err(e) => {
                        crate::telemetry::record_tool_failure();
                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(), message_id: message_id.clone(),
                            tool_call_id: tc_id.clone(), tool_name: tc_name.clone(),
                            status: ToolStatus::Error, title: None,
                        }));
                        let error_msg = format!("Error: {}", e);
                        if trace { eprintln!("[trace] tool_exec_error name={} id={} {}", tc_name, tc_id, error_msg); }
                        if print_output { println!("{}", error_msg); }
                        self.add_message_with_duration(Role::User, vec![ContentBlock::ToolResult {
                            tool_use_id: tc_id.clone(), content: error_msg, is_error: Some(true),
                        }], Some(tool_elapsed.as_millis() as u64));
                        tool_results_dirty = true;
                    }
                }

                // MCP 工具调用检测
                if tc_name.starts_with("mcp__") {
                    let parts: Vec<&str> = tc_name.split("__").collect();
                    if parts.len() >= 3 && !self.mcp_tool_names.contains(&parts[1].to_string()) {
                        self.mcp_tool_names.push(parts[1].to_string());
                    }
                }
                // 编辑工具 → 跨文件修复队列
                if tc_name == "edit" || tc_name == "multiedit" || tc_name == "batch_edit" {
                    if let Some(file_path) = phase2_tools.iter()
                        .find(|t| t.id == *tc_id)
                        .and_then(|t| t.input.get("file_path"))
                        .and_then(|v| v.as_str())
                    {
                        if !self.recent_edit_files.contains(&file_path.to_string()) {
                            self.recent_edit_files.push(file_path.to_string());
                        }
                    }
                }
            }

            if tool_results_dirty {
                self.session.save()?;
            }

            // ===== [自主规划] Phase 10: 跨文件依赖分析与规划注入 =====
            if !self.recent_edit_files.is_empty() {
                let edited_count = self.recent_edit_files.len();
                let files_list = self.recent_edit_files.join(", ");

                if edited_count > 1 {
                    logging::info(&format!(
                        "Phase 10: {} files edited ({}) — generating dependency plan",
                        edited_count, files_list
                    ));

                    let plan_msg = format!(
                        "<system-reminder>\n\
                        [Cross-File Dependency Plan]\n\
                        You just edited {} files: {}\n\n\
                        Please ensure consistency across these files:\n\
                        - Verify that all modified files have compatible interfaces\n\
                        - Check that imports and exports are synchronized\n\
                        - Confirm that type definitions match across files\n\
                        - Run compile check if possible to validate changes\n\
                        - Update any related documentation if needed\n\
                        </system-reminder>",
                        edited_count, files_list
                    );
                    self.add_message(Role::User, vec![ContentBlock::Text {
                        text: plan_msg,
                        cache_control: None,
                    }]);

                    logging::info("Dependency plan injected into agent context");
                } else {
                    logging::info(&format!(
                        "Phase 10: single file edited ({}), no cross-file plan needed",
                        files_list
                    ));
                }

                let workspace = self.session.working_dir.as_ref()
                    .map(|d| std::path::Path::new(d))
                    .or_else(|| Some(std::path::Path::new(".")))
                    .unwrap();
                let cargo_toml = workspace.join("Cargo.toml");
                if cargo_toml.exists() {
                    logging::info("Phase 11: Running auto-verify (cargo check)...");
                    match tokio::process::Command::new("cargo")
                        .args(["check", "--color=never", "--message-format=short"])
                        .current_dir(workspace)
                        .output()
                        .await
                    {
                        Ok(output) => {
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            let has_errors = stderr.contains("error[");
                            if has_errors {
                                let error_summary: Vec<&str> = stderr.lines()
                                    .filter(|l| l.contains("error[") || l.contains("error:"))
                                    .take(5)
                                    .collect();
                                let summary = if error_summary.is_empty() {
                                    stderr.lines().take(10).collect::<Vec<_>>().join("\n")
                                } else {
                                    error_summary.join("\n")
                                };

                                let verify_msg = format!(
                                    "<system-reminder>\n\
                                    [Auto-Verify Failed]\n\
                                    Compile check found errors after your edits:\n\
                                    ```\n{}\n```\n\n\
                                    Please fix these errors. You can run `cargo check` to verify.\n\
                                    </system-reminder>",
                                    summary
                                );
                                self.add_message(Role::User, vec![ContentBlock::Text {
                                    text: verify_msg,
                                    cache_control: None,
                                }]);
                                logging::warn(&format!(
                                    "Auto-verify: {} errors found after edit, injected into context",
                                    error_summary.len()
                                ));
                            } else {
                                logging::info("Auto-verify: cargo check passed");
                            }
                        }
                        Err(e) => {
                            logging::info(&format!("Auto-verify: cargo not available ({})", e));
                        }
                    }
                } else {
                    logging::info("Phase 11: Not a Rust project, skipping auto-verify");
                }

                self.recent_edit_files.clear();
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
