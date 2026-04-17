use super::*;
use crate::tui::app as app_mod;

pub(in crate::tui::app) fn handle_server_event(
    app: &mut App,
    event: ServerEvent,
    remote: &mut impl RemoteEventState,
) -> bool {
    let eager_stream_redraw = !crate::perf::tui_policy().enable_decorative_animations;
    if app.is_processing {
        app.last_stream_activity = Some(Instant::now());
    }

    if matches!(
        &event,
        ServerEvent::TextDelta { .. }
            | ServerEvent::TextReplace { .. }
            | ServerEvent::ToolStart { .. }
            | ServerEvent::ToolInput { .. }
            | ServerEvent::ToolExec { .. }
            | ServerEvent::ToolDone { .. }
            | ServerEvent::BatchProgress { .. }
            | ServerEvent::TokenUsage { .. }
            | ServerEvent::ConnectionType { .. }
            | ServerEvent::ConnectionPhase { .. }
            | ServerEvent::StatusDetail { .. }
            | ServerEvent::MessageEnd
            | ServerEvent::UpstreamProvider { .. }
            | ServerEvent::Interrupted
            | ServerEvent::Done { .. }
            | ServerEvent::Error { .. }
    ) {
        app.remote_resume_activity = None;
    }

    let call_output_tokens_seen = remote.call_output_tokens_seen();

    match event {
        ServerEvent::TextDelta { text } => {
            if let Some(thought_line) = App::extract_thought_line(&text) {
                if let Some(chunk) = app.stream_buffer.flush() {
                    app.append_streaming_text(&chunk);
                }
                app.insert_thought_line(thought_line);
                return eager_stream_redraw;
            }
            let mut needs_redraw = false;
            if matches!(
                app.status,
                ProcessingStatus::Sending
                    | ProcessingStatus::Connecting(_)
                    | ProcessingStatus::Thinking(_)
            ) || (app.is_processing && matches!(app.status, ProcessingStatus::Idle))
            {
                app.status = ProcessingStatus::Streaming;
                needs_redraw = true;
            }
            app.resume_streaming_tps();
            if let Some(chunk) = app.stream_buffer.push(&text) {
                app.append_streaming_text(&chunk);
                needs_redraw = true;
            }
            app.last_stream_activity = Some(Instant::now());
            eager_stream_redraw && needs_redraw
        }
        ServerEvent::TextReplace { text } => {
            app.stream_buffer.flush();
            app.replace_streaming_text(text);
            app.resume_streaming_tps();
            true
        }
        ServerEvent::ToolStart { id, name } => {
            app.pause_streaming_tps(false);
            remote.handle_tool_start(&id, &name);
            app.commit_pending_streaming_assistant_message();
            if matches!(name.as_str(), "memory") {
                crate::memory::set_state(crate::tui::info_widget::MemoryState::Embedding);
            }
            app.status = ProcessingStatus::RunningTool(name.clone());
            app.streaming_tool_calls.push(ToolCall {
                id,
                name,
                input: serde_json::Value::Null,
                intent: None,
            });
            eager_stream_redraw
        }
        ServerEvent::ToolInput { delta } => {
            remote.handle_tool_input(&delta);
            false
        }
        ServerEvent::ToolExec { id, name } => {
            app.pause_streaming_tps(false);
            let parsed_input = remote.get_current_tool_input();
            let tool_call = ToolCall {
                id: id.clone(),
                name: name.clone(),
                input: parsed_input.clone(),
                intent: None,
            };
            if let Some(tc) = app.streaming_tool_calls.iter_mut().find(|tc| tc.id == id) {
                tc.input = parsed_input;
            }
            remote.handle_tool_exec(&id, &name);
            app.observe_tool_call(&tool_call);
            eager_stream_redraw
                || app.side_panel.focused_page_id.as_deref()
                    == Some(app_mod::observe::OBSERVE_PAGE_ID)
        }
        ServerEvent::ToolDone {
            id,
            name,
            output,
            error,
        } => {
            let display_output = remote.handle_tool_done(&id, &name, &output);
            let display_output = if error.is_some()
                && !display_output.starts_with("Error:")
                && !display_output.starts_with("error:")
                && !display_output.starts_with("Failed:")
            {
                format!("Error: {}", display_output)
            } else {
                display_output
            };
            let tool_input = app
                .streaming_tool_calls
                .iter()
                .find(|tc| tc.id == id)
                .map(|tc| tc.input.clone())
                .unwrap_or(serde_json::Value::Null);
            let tool_call = ToolCall {
                id,
                name,
                input: tool_input,
                intent: None,
            };
            app.commit_pending_streaming_assistant_message();
            crate::tui::mermaid::clear_streaming_preview_diagram();
            let is_batch = tool_call.name == "batch";
            app.observe_tool_result(&tool_call, &output, error.is_some(), None);
            app.push_display_message(DisplayMessage {
                role: "tool".to_string(),
                content: display_output,
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: Some(tool_call),
            });
            if is_batch {
                app.batch_progress = None;
            }
            app.streaming_tool_calls.clear();
            app.status = ProcessingStatus::Streaming;
            true
        }
        ServerEvent::BatchProgress { progress } => {
            app.batch_progress = Some(progress);
            false
        }
        ServerEvent::TokenUsage {
            input,
            output,
            cache_read_input,
            cache_creation_input,
        } => {
            app.accumulate_streaming_output_tokens(output, call_output_tokens_seen);
            app.streaming_input_tokens = input;
            app.streaming_output_tokens = output;
            if cache_read_input.is_some() {
                app.streaming_cache_read_tokens = cache_read_input;
            }
            if cache_creation_input.is_some() {
                app.streaming_cache_creation_tokens = cache_creation_input;
            }
            eager_stream_redraw && matches!(app.status, ProcessingStatus::Streaming)
        }
        ServerEvent::ConnectionType { connection } => {
            app.connection_type = Some(connection);
            app.update_terminal_title();
            false
        }
        ServerEvent::Pong { .. } => false,
        ServerEvent::ConnectionPhase { phase } => {
            let cp = match phase.as_str() {
                "authenticating" => crate::message::ConnectionPhase::Authenticating,
                "connecting" => crate::message::ConnectionPhase::Connecting,
                "waiting for response" => crate::message::ConnectionPhase::WaitingForResponse,
                "streaming" => crate::message::ConnectionPhase::Streaming,
                _ if phase.starts_with("retrying (") && phase.ends_with(')') => {
                    let inner = &phase[10..phase.len() - 1];
                    let (attempt, max) = inner
                        .split_once('/')
                        .and_then(|(a, m)| Some((a.parse::<u32>().ok()?, m.parse::<u32>().ok()?)))
                        .unwrap_or((1, 1));
                    crate::message::ConnectionPhase::Retrying { attempt, max }
                }
                _ => crate::message::ConnectionPhase::Connecting,
            };
            app.status = if matches!(cp, crate::message::ConnectionPhase::Streaming) {
                ProcessingStatus::Streaming
            } else {
                ProcessingStatus::Connecting(cp)
            };
            eager_stream_redraw
        }
        ServerEvent::StatusDetail { detail } => {
            app.status_detail = Some(detail);
            eager_stream_redraw
        }
        ServerEvent::MessageEnd => {
            app.pause_streaming_tps(true);
            app.stream_message_ended = true;
            true
        }
        ServerEvent::UpstreamProvider { provider } => {
            app.upstream_provider = Some(provider);
            false
        }
        ServerEvent::Ack { id } => {
            let _ = app.acknowledge_pending_soft_interrupt(id);
            false
        }
        ServerEvent::Interrupted => {
            let keep_pending_retry = app
                .rate_limit_pending_message
                .as_ref()
                .is_some_and(|pending| pending.auto_retry && app.rate_limit_reset.is_some());
            if !keep_pending_retry {
                app.clear_pending_remote_retry();
            }
            let recovered_local = recover_local_interleave_to_queue(app, "interrupt");
            if let Some(chunk) = app.stream_buffer.flush() {
                app.append_streaming_text(&chunk);
            }
            if !app.streaming_text.is_empty() {
                let content = app.take_streaming_text();
                app.push_display_message(DisplayMessage {
                    role: "assistant".to_string(),
                    content,
                    tool_calls: Vec::new(),
                    duration_secs: app.processing_started.map(|t| t.elapsed().as_secs_f32()),
                    title: None,
                    tool_data: None,
                });
            }
            app.clear_streaming_render_state();
            app.stream_buffer.clear();
            app.streaming_tool_calls.clear();
            app.batch_progress = None;
            app.thought_line_inserted = false;
            app.thinking_prefix_emitted = false;
            app.thinking_buffer.clear();
            if recovered_local || !app.pending_soft_interrupts.is_empty() {
                crate::logging::info(&format!(
                    "Preserving {} pending soft interrupt(s) across interrupt",
                    app.pending_soft_interrupts.len()
                ));
            }
            app.schedule_queued_dispatch_after_interrupt();
            app.push_display_message(DisplayMessage::system("Interrupted"));
            app.is_processing = false;
            app.status = ProcessingStatus::Idle;
            app.stream_message_ended = false;
            app.processing_started = None;
            app.current_message_id = None;
            remote.clear_pending();
            remote.reset_call_output_tokens_seen();
            false
        }
        ServerEvent::Done { id } => {
            crate::logging::info(&format!(
                "Client received Done id={}, current_message_id={:?}",
                id, app.current_message_id
            ));
            if app.current_message_id == Some(id) {
                app.clear_pending_remote_retry();
                if let Some(chunk) = app.stream_buffer.flush() {
                    app.append_streaming_text(&chunk);
                }
                app.pause_streaming_tps(false);
                if !app.streaming_text.is_empty() {
                    let duration = app.processing_started.map(|s| s.elapsed().as_secs_f32());
                    let content = app.take_streaming_text();
                    app.push_display_message(DisplayMessage {
                        role: "assistant".to_string(),
                        content,
                        tool_calls: vec![],
                        duration_secs: duration,
                        title: None,
                        tool_data: None,
                    });
                    app.push_turn_footer(duration);
                }
                crate::tui::mermaid::clear_streaming_preview_diagram();
                app.is_processing = false;
                app.status = ProcessingStatus::Idle;
                app.stream_message_ended = false;
                app.processing_started = None;
                app.replay_processing_started_ms = None;
                app.replay_elapsed_override = None;
                app.batch_progress = None;
                app.streaming_tool_calls.clear();
                app.current_message_id = None;
                app.thought_line_inserted = false;
                app.thinking_prefix_emitted = false;
                app.thinking_buffer.clear();
                remote.clear_pending();
                remote.reset_call_output_tokens_seen();
                app.note_runtime_memory_event_force("turn_completed", "remote_turn_finished");
            } else if app.is_processing {
                let is_stale = app.current_message_id.is_some_and(|mid| id < mid);
                if is_stale {
                    crate::logging::info(&format!(
                        "Ignoring stale Done id={} (current_message_id={:?}), likely from Subscribe/ResumeSession",
                        id, app.current_message_id
                    ));
                } else {
                    crate::logging::info(&format!(
                        "Ignoring unrelated Done id={} while processing current_message_id={:?}; preserving active/queued turn",
                        id, app.current_message_id
                    ));
                }
            }
            false
        }
        ServerEvent::Error {
            message,
            retry_after_secs,
            ..
        } => {
            let reset_duration = retry_after_secs
                .map(Duration::from_secs)
                .or_else(|| parse_rate_limit_error(&message));
            if let Some(reset_duration) = reset_duration {
                app.rate_limit_reset = Some(Instant::now() + reset_duration);
                if let Some(is_system) = app
                    .rate_limit_pending_message
                    .as_ref()
                    .map(|pending| pending.is_system)
                {
                    app.push_display_message(DisplayMessage::system(format!(
                        "⏳ Rate limit hit. Will auto-retry in {} seconds...",
                        reset_duration.as_secs()
                    )));
                    if is_system {
                        app.set_status_notice("Rate limited; queued system retry");
                    } else {
                        app.set_status_notice("Rate limited; queued retry");
                    }
                    app.is_processing = false;
                    app.status = ProcessingStatus::Idle;
                    app.stream_message_ended = false;
                    app.processing_started = None;
                    app.current_message_id = None;
                    remote.clear_pending();
                    remote.reset_call_output_tokens_seen();
                    return false;
                }
            }
            let is_failover_prompt =
                crate::provider::parse_failover_prompt_message(&message).is_some();
            app.push_display_message(DisplayMessage {
                role: "error".to_string(),
                content: message,
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            app.is_processing = false;
            app.status = ProcessingStatus::Idle;
            app.stream_message_ended = false;
            let recovered_local = recover_local_interleave_to_queue(app, "request error");
            crate::tui::mermaid::clear_streaming_preview_diagram();
            app.thought_line_inserted = false;
            app.thinking_prefix_emitted = false;
            app.thinking_buffer.clear();
            if recovered_local || !app.pending_soft_interrupts.is_empty() {
                crate::logging::info(&format!(
                    "Preserving {} pending soft interrupt(s) across remote error",
                    app.pending_soft_interrupts.len()
                ));
            }
            remote.clear_pending();
            remote.reset_call_output_tokens_seen();
            if !is_failover_prompt && !app.schedule_pending_remote_retry("⚠ Remote request failed.")
            {
                app.clear_pending_remote_retry();
            }
            false
        }
        ServerEvent::SessionId { session_id } => {
            remote.set_session_id(session_id.clone());
            app.remote_session_id = Some(session_id.clone());
            crate::set_current_session(&session_id);
            app.note_client_focus(true);
            app.update_terminal_title();
            false
        }
        ServerEvent::Reloading { .. } => {
            app.append_reload_message("🔄 Server reload initiated...");
            false
        }
        ServerEvent::ReloadProgress {
            step,
            message,
            success,
            output,
        } => {
            let mut content = if let Some(ok) = success {
                let status_icon = if ok { "✓" } else { "✗" };
                format!("[{}] {} {}", step, status_icon, message)
            } else {
                format!("[{}] {}", step, message)
            };

            if let Some(out) = output
                && !out.is_empty()
            {
                content.push_str("\n```\n");
                content.push_str(&out);
                content.push_str("\n```");
            }

            app.append_reload_message(&content);

            if step == "verify" || step == "git" {
                app.reload_info.push(message.clone());
            }

            app.status_notice = Some((format!("Reload: {}", message), std::time::Instant::now()));
            false
        }
        ServerEvent::History {
            messages,
            images,
            session_id,
            provider_name,
            provider_model,
            subagent_model,
            autoreview_enabled,
            autojudge_enabled,
            available_models,
            available_model_routes,
            mcp_servers,
            skills,
            all_sessions,
            client_count,
            is_canary,
            server_version,
            server_name,
            server_icon,
            server_has_update,
            was_interrupted,
            connection_type,
            status_detail,
            upstream_provider,
            reasoning_effort,
            service_tier,
            compaction_mode,
            activity,
            side_panel,
            ..
        } => {
            let prev_session_id = app.remote_session_id.clone();
            let history_message_count = messages.len();
            let history_mcp_count = mcp_servers.len();
            let history_model = provider_model.clone();
            remote.set_session_id(session_id.clone());
            app.remote_session_id = Some(session_id.clone());
            crate::set_current_session(&session_id);
            app.note_client_focus(true);
            let session_changed = prev_session_id.as_deref() != Some(session_id.as_str());

            if session_changed {
                app.rate_limit_pending_message = None;
                app.rate_limit_reset = None;
                app.connection_type = None;
                app.status_detail = None;
                app.clear_display_messages();
                app.clear_streaming_render_state();
                app.streaming_tool_calls.clear();
                app.thought_line_inserted = false;
                app.thinking_prefix_emitted = false;
                app.thinking_buffer.clear();
                app.streaming_input_tokens = 0;
                app.streaming_output_tokens = 0;
                app.streaming_cache_read_tokens = None;
                app.streaming_cache_creation_tokens = None;
                app.processing_started = None;
                app.replay_processing_started_ms = None;
                app.replay_elapsed_override = None;
                app.reset_streaming_tps();
                app.last_stream_activity = None;
                app.stream_message_ended = false;
                app.remote_resume_activity = None;
                app.is_processing = false;
                app.status = ProcessingStatus::Idle;
                app.follow_chat_bottom();
                if prev_session_id.is_some() {
                    app.queued_messages.clear();
                    app.interleave_message = None;
                    app.clear_pending_soft_interrupt_tracking();
                }
                app.remote_total_tokens = None;
                app.remote_side_pane_images.clear();
                app.remote_swarm_members.clear();
                app.swarm_plan_items.clear();
                app.swarm_plan_version = None;
                app.swarm_plan_swarm_id = None;
                remote.reset_call_output_tokens_seen();
            }
            if let Some(name) = provider_name {
                app.remote_provider_name = Some(name);
            }
            if let Some(model) = provider_model {
                app.update_context_limit_for_model(&model);
                app.remote_provider_model = Some(model);
            }
            app.clear_remote_startup_phase();
            app.session.subagent_model = subagent_model;
            app.session.autoreview_enabled = autoreview_enabled;
            app.session.autojudge_enabled = autojudge_enabled;
            app.autoreview_enabled =
                autoreview_enabled.unwrap_or(crate::config::config().autoreview.enabled);
            app.autojudge_enabled =
                autojudge_enabled.unwrap_or(crate::config::config().autojudge.enabled);
            if upstream_provider.is_some() {
                app.upstream_provider = upstream_provider;
            }
            if session_changed || connection_type.is_some() {
                app.connection_type = connection_type;
            }
            if session_changed || status_detail.is_some() {
                app.status_detail = status_detail;
            }
            app.remote_reasoning_effort = reasoning_effort;
            app.remote_service_tier = service_tier;
            app.remote_compaction_mode = Some(compaction_mode);
            app.set_side_panel_snapshot(side_panel);
            app.remote_side_pane_images = images;
            app.remote_available_entries = available_models;
            app.remote_model_options = available_model_routes;
            app.remote_skills = skills;
            app.remote_sessions = all_sessions;
            app.remote_client_count = client_count;
            app.remote_is_canary = is_canary;
            app.remote_server_version = server_version;
            app.remote_server_has_update = server_has_update;
            crate::tui::workspace_client::sync_after_history(&session_id, &app.remote_sessions);

            if server_has_update == Some(true) && !app.pending_server_reload {
                app.pending_server_reload = true;
                app.set_status_notice("Server update available");
            }
            app.remote_server_short_name = server_name;
            if let Some(icon) = server_icon {
                app.remote_server_icon = Some(icon);
            }

            app.update_terminal_title();

            if !mcp_servers.is_empty() {
                app.mcp_server_names = mcp_servers
                    .iter()
                    .filter_map(|s| {
                        let (name, count_str) = s.split_once(':')?;
                        let count = count_str.parse::<usize>().unwrap_or(0);
                        Some((name.to_string(), count))
                    })
                    .collect();
            }

            let should_apply_history_payload = session_changed || !remote.has_loaded_history();
            if should_apply_history_payload {
                if let Some(activity) = activity.filter(|activity| activity.is_processing) {
                    let current_tool_name = activity.current_tool_name.clone();
                    app.is_processing = true;
                    if app.processing_started.is_none() {
                        app.processing_started = Some(Instant::now());
                    }
                    if app.last_stream_activity.is_none() {
                        app.last_stream_activity = Some(Instant::now());
                    }
                    app.remote_resume_activity = Some(RemoteResumeActivity {
                        session_id: session_id.clone(),
                        observed_at: Instant::now(),
                        current_tool_name: current_tool_name.clone(),
                    });
                    app.status = match current_tool_name {
                        Some(tool_name) => ProcessingStatus::RunningTool(tool_name),
                        None => ProcessingStatus::Thinking(Instant::now()),
                    };
                } else {
                    app.remote_resume_activity = None;
                }
            }
            if should_apply_history_payload {
                crate::logging::info(&format!(
                    "[TIMING] remote bootstrap: history after {}ms (session={}, resumed={}, messages={}, mcp_servers={}, model={})",
                    app.app_started.elapsed().as_millis(),
                    session_id,
                    app.resume_session_id.is_some(),
                    history_message_count,
                    history_mcp_count,
                    history_model.as_deref().unwrap_or("<none>")
                ));
                remote.mark_history_loaded();
                if messages.is_empty() && !session_changed && !app.display_messages().is_empty() {
                    crate::logging::info(
                        "Preserving locally restored display history for metadata-only History bootstrap",
                    );
                } else {
                    let restored_messages = messages
                        .into_iter()
                        .map(|msg| DisplayMessage {
                            role: msg.role,
                            content: msg.content,
                            tool_calls: msg.tool_calls.unwrap_or_default(),
                            duration_secs: None,
                            title: None,
                            tool_data: msg.tool_data,
                        })
                        .collect();
                    app.replace_display_messages(restored_messages);
                }

                if history_matches_pending_startup_prompt(app) {
                    crate::logging::info(
                        "Reload-restored startup prompt already present in server history; skipping client resubmit",
                    );
                    app.submit_input_on_startup = false;
                    app.input.clear();
                    app.cursor_pos = 0;
                    app.pending_images.clear();
                    app.set_status_notice("Reload complete — prompt preserved");
                }
                app.note_runtime_memory_event_force("history_loaded", "remote_history_applied");
            } else {
                crate::logging::info(
                    "Ignoring duplicate History event for active session after local state was restored",
                );
            }

            app.maybe_show_catchup_after_history(&session_id);

            if was_interrupted == Some(true) && !app.display_messages.is_empty() {
                crate::logging::info(
                    "Session was interrupted mid-generation, queuing continuation",
                );
                app.push_display_message(DisplayMessage::system(
                    "Reload complete — continuing.".to_string(),
                ));
                app.hidden_queued_system_messages.push(
                    "Your session was interrupted by a server reload while you were working. The session has been restored. Any tool that was running was aborted and its results may be incomplete. Continue exactly where you left off and do not ask the user what to do next."
                        .to_string(),
                );
            }

            false
        }
        ServerEvent::SidePanelState { snapshot } => {
            app.set_side_panel_snapshot(snapshot);
            false
        }
        ServerEvent::SwarmStatus { members } => {
            if app.swarm_enabled {
                app.remote_swarm_members = members;
                persist_swarm_status_snapshot(app);
            } else {
                app.remote_swarm_members.clear();
            }
            false
        }
        ServerEvent::SwarmPlan {
            swarm_id,
            version,
            items,
            participants,
            reason,
        } => {
            app.swarm_plan_swarm_id = Some(swarm_id.clone());
            app.swarm_plan_version = Some(version);
            app.swarm_plan_items = items.clone();
            persist_swarm_plan_snapshot(app, swarm_id, version, items, participants, reason);
            app.set_status_notice(format!(
                "Swarm plan synced (v{}, {} items)",
                version,
                app.swarm_plan_items.len()
            ));
            false
        }
        ServerEvent::SwarmPlanProposal {
            swarm_id,
            proposer_session,
            proposer_name,
            summary,
            ..
        } => {
            let proposer =
                proposer_name.unwrap_or_else(|| proposer_session.chars().take(8).collect());
            let message = format!(
                "Plan proposal received in swarm {}\nFrom: {}\nSummary: {}",
                swarm_id, proposer, summary
            );
            app.push_display_message(DisplayMessage::system(message.clone()));
            persist_replay_display_message(app, "system", None, &message);
            app.set_status_notice("Plan proposal received");
            false
        }
        ServerEvent::McpStatus { servers } => {
            app.mcp_server_names = servers
                .iter()
                .filter_map(|s| {
                    let (name, count_str) = s.split_once(':')?;
                    let count = count_str.parse::<usize>().unwrap_or(0);
                    Some((name.to_string(), count))
                })
                .collect();
            false
        }
        ServerEvent::ModelChanged {
            model,
            provider_name,
            error,
            ..
        } => {
            if let Some(err) = error {
                app.push_display_message(DisplayMessage::error(
                    crate::tui::app::model_context::model_switch_failure_message(&err, true),
                ));
                app.set_status_notice("Model switch failed");
            } else {
                app.update_context_limit_for_model(&model);
                app.remote_provider_model = Some(model.clone());
                app.clear_remote_startup_phase();
                if let Some(ref pname) = provider_name {
                    app.remote_provider_name = Some(pname.clone());
                }
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Switched to model: {}",
                    model
                )));
                app.set_status_notice(format!("Model → {}", model));
            }
            false
        }
        ServerEvent::AvailableModelsUpdated {
            available_models,
            available_model_routes,
        } => {
            app.remote_available_entries = available_models;
            app.remote_model_options = available_model_routes;
            false
        }
        ServerEvent::ReasoningEffortChanged { effort, error, .. } => {
            if let Some(err) = error {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set effort: {}",
                    err
                )));
            } else {
                app.remote_reasoning_effort = effort.clone();
                let label = effort
                    .as_deref()
                    .map(app_mod::effort_display_label)
                    .unwrap_or("default");
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Reasoning effort → {}",
                    label
                )));
                app.set_status_notice(format!("Effort: {}", label));
            }
            false
        }
        ServerEvent::ServiceTierChanged {
            service_tier,
            error,
            ..
        } => {
            if let Some(err) = error {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set fast mode: {}",
                    err
                )));
            } else {
                app.remote_service_tier = service_tier.clone();
                let enabled = service_tier.as_deref() == Some("priority");
                let label = service_tier
                    .as_deref()
                    .map(app_mod::service_tier_display_label)
                    .unwrap_or("Standard");
                let applies_next_request = app.is_processing;
                app.push_display_message(DisplayMessage::system(
                    app_mod::fast_mode_success_message(enabled, label, applies_next_request),
                ));
                app.set_status_notice(app_mod::fast_mode_status_notice(
                    enabled,
                    applies_next_request,
                ));
            }
            false
        }
        ServerEvent::TransportChanged {
            transport, error, ..
        } => {
            if let Some(err) = error {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set transport: {}",
                    err
                )));
            } else {
                app.remote_transport = transport.clone();
                let label = transport.as_deref().unwrap_or("unknown");
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Transport → {}",
                    label
                )));
                app.set_status_notice(format!("Transport: {}", label));
            }
            false
        }
        ServerEvent::CompactionModeChanged { mode, error, .. } => {
            if let Some(err) = error {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set compaction mode: {}",
                    err
                )));
            } else {
                let label = mode.as_str();
                app.remote_compaction_mode = Some(mode);
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Compaction mode → {}",
                    label
                )));
                app.set_status_notice(format!("Compaction: {}", label));
            }
            false
        }
        ServerEvent::SoftInterruptInjected {
            content,
            display_role,
            point: _,
            tools_skipped,
        } => {
            if let Some(chunk) = app.stream_buffer.flush() {
                app.append_streaming_text(&chunk);
            }
            if !app.streaming_text.is_empty() {
                let duration = app.processing_started.map(|s| s.elapsed().as_secs_f32());
                let flushed = app.take_streaming_text();
                app.push_display_message(DisplayMessage {
                    role: "assistant".to_string(),
                    content: flushed,
                    tool_calls: vec![],
                    duration_secs: duration,
                    title: None,
                    tool_data: None,
                });
                app.push_turn_footer(duration);
            }
            app.mark_soft_interrupt_injected(&content);
            let role = display_role.unwrap_or_else(|| "user".to_string());
            app.push_display_message(DisplayMessage {
                role,
                content: content.clone(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            if let Some(n) = tools_skipped {
                app.set_status_notice(format!("⚡ {} tool(s) skipped", n));
            }
            false
        }
        ServerEvent::MemoryInjected {
            count,
            prompt,
            display_prompt,
            prompt_chars: _,
            computed_age_ms,
        } => {
            if app.memory_enabled {
                let plural = if count == 1 { "memory" } else { "memories" };
                let display_prompt = if let Some(display_prompt) = display_prompt {
                    display_prompt.clone()
                } else if prompt.trim().is_empty() {
                    "# Memory\n\n## Notes\n1. (content unavailable from server event)".to_string()
                } else {
                    prompt.clone()
                };
                crate::memory::record_injected_prompt(&prompt, count, computed_age_ms);
                let summary = if count == 1 {
                    "🧠 auto-recalled 1 memory".to_string()
                } else {
                    format!("🧠 auto-recalled {} memories", count)
                };
                app.push_display_message(DisplayMessage::memory(summary, display_prompt));
                app.set_status_notice(format!("🧠 {} relevant {} injected", count, plural));
            }
            false
        }
        ServerEvent::MemoryActivity { activity } => {
            if app.memory_enabled {
                crate::memory::apply_remote_activity_snapshot(&activity);
            }
            false
        }
        ServerEvent::Notification {
            from_session,
            from_name,
            notification_type,
            message,
        } => {
            let sender = from_name
                .clone()
                .or_else(|| crate::id::extract_session_name(&from_session).map(str::to_string))
                .unwrap_or_else(|| from_session[..8.min(from_session.len())].to_string());

            let background_task_scope = matches!(
                &notification_type,
                crate::protocol::NotificationType::Message {
                    scope: Some(scope),
                    ..
                } if scope == "background_task"
            );

            if background_task_scope
                && crate::message::parse_background_task_notification_markdown(&message).is_some()
            {
                let presentation =
                    present_swarm_notification(&sender, &notification_type, &message);
                app.push_display_message(DisplayMessage::background_task(message.clone()));
                persist_replay_display_message(app, "background_task", None, &message);
                app.set_status_notice(presentation.status_notice);
                return false;
            }

            let presentation = present_swarm_notification(&sender, &notification_type, &message);
            app.push_display_message(DisplayMessage::swarm(
                presentation.title.clone(),
                presentation.message.clone(),
            ));
            persist_replay_display_message(
                app,
                "swarm",
                Some(presentation.title.clone()),
                &presentation.message,
            );
            app.set_status_notice(presentation.status_notice);
            false
        }
        ServerEvent::Transcript { text, mode } => {
            apply_transcript_event(app, text, mode);
            false
        }
        ServerEvent::InputShellResult { result } => {
            app.push_display_message(DisplayMessage::system(
                crate::message::format_input_shell_result_markdown(&result),
            ));
            app.set_status_notice(crate::message::input_shell_status_notice(&result));
            false
        }
        ServerEvent::Compaction {
            trigger,
            pre_tokens,
            post_tokens,
            tokens_saved,
            duration_ms,
            messages_dropped,
            messages_compacted,
            summary_chars,
            active_messages,
        } => {
            app.handle_compaction_event(crate::compaction::CompactionEvent {
                trigger,
                pre_tokens,
                post_tokens,
                tokens_saved,
                duration_ms,
                messages_dropped,
                messages_compacted,
                summary_chars,
                active_messages,
            });
            false
        }
        ServerEvent::SplitResponse {
            new_session_id,
            new_session_name,
            ..
        } => {
            if crate::tui::workspace_client::handle_split_response(&new_session_id) {
                finish_remote_split_launch(app);
                app.pending_split_request = false;
                app.pending_split_startup_message = None;
                app.pending_split_parent_session_id = None;
                app.pending_split_prompt = None;
                app.pending_split_model_override = None;
                app.pending_split_provider_key_override = None;
                app.pending_split_label = None;
                app.push_display_message(DisplayMessage::system(format!(
                    "Added **{}** to workspace.",
                    new_session_name,
                )));
                app.set_status_notice(format!("Workspace + {}", new_session_name));
                return false;
            }
            finish_remote_split_launch(app);
            app.pending_split_request = false;
            let startup_message = app.pending_split_startup_message.take();
            let parent_session_id_override = app.pending_split_parent_session_id.take();
            let startup_prompt = app.pending_split_prompt.take();
            let model_override = app.pending_split_model_override.take();
            let provider_key_override = app.pending_split_provider_key_override.take();
            let split_label = app.pending_split_label.take();
            if let Some(startup_message) = startup_message {
                app_mod::commands::prepare_review_spawned_session(
                    &new_session_id,
                    startup_message,
                    model_override,
                    provider_key_override,
                    split_label.clone().map(|label| label.to_ascii_lowercase()),
                    parent_session_id_override,
                );
            } else if let Some(startup_prompt) = startup_prompt {
                App::save_startup_submission_for_session(
                    &new_session_id,
                    startup_prompt.content,
                    startup_prompt.images,
                );
            }
            let exe = app_mod::launch_client_executable();
            let cwd = crate::session::Session::load(&new_session_id)
                .ok()
                .and_then(|session| session.working_dir)
                .map(std::path::PathBuf::from)
                .filter(|path| path.is_dir())
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let socket = std::env::var("JCODE_SOCKET").ok();
            match spawn_in_new_terminal(&exe, &new_session_id, &cwd, socket.as_deref()) {
                Ok(true) => {
                    if let Some(label) = split_label.as_deref() {
                        app.push_display_message(DisplayMessage::system(format!(
                            "🔍 {} launched in **{}**.",
                            label, new_session_name,
                        )));
                        app.set_status_notice(format!("{} launched", label));
                    } else {
                        app.push_display_message(DisplayMessage::system(format!(
                            "✂ Split → **{}** (opened in new window)",
                            new_session_name,
                        )));
                        app.set_status_notice(format!("Split → {}", new_session_name));
                    }
                }
                Ok(false) => {
                    if let Some(label) = split_label.as_deref() {
                        app.push_display_message(DisplayMessage::system(format!(
                            "🔍 {} session **{}** created.\n\nNo terminal found. Resume manually:\n```\njcode --resume {}\n```",
                            label, new_session_name, new_session_id,
                        )));
                        app.set_status_notice(format!("{} session created", label));
                    } else {
                        app.push_display_message(DisplayMessage::system(format!(
                            "✂ Split → **{}**\n\nNo terminal found. Resume manually:\n```\njcode --resume {}\n```",
                            new_session_name, new_session_id,
                        )));
                    }
                }
                Err(e) => {
                    if let Some(label) = split_label.as_deref() {
                        app.push_display_message(DisplayMessage::error(format!(
                            "{} session **{}** was created but failed to open a window: {}\n\nResume manually: `jcode --resume {}`",
                            label, new_session_name, e, new_session_id,
                        )));
                        app.set_status_notice(format!("{} open failed", label));
                    } else {
                        app.push_display_message(DisplayMessage::error(format!(
                            "Split created **{}** but failed to open window: {}\n\nResume manually: `jcode --resume {}`",
                            new_session_name, e, new_session_id,
                        )));
                    }
                }
            }
            false
        }
        ServerEvent::CompactResult {
            message, success, ..
        } => {
            if success {
                app.push_display_message(DisplayMessage::system(message));
                app.set_status_notice("Compacting context");
            } else {
                app.push_display_message(DisplayMessage::system(message));
                app.set_status_notice("Compaction failed");
            }
            false
        }
        ServerEvent::StdinRequest { .. } => {
            app.set_status_notice("⌨ Interactive terminal detected (command will timeout)");
            false
        }
        _ => false,
    }
}
