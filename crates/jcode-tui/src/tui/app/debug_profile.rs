use super::*;
use std::borrow::Cow;

impl App {
    pub(in crate::tui::app) fn runtime_memory_profile(&self) -> serde_json::Value {
        self.memory_profile_value(false)
    }

    pub(in crate::tui::app) fn debug_memory_profile(&self) -> serde_json::Value {
        self.memory_profile_value(true)
    }

    fn memory_profile_value(&self, include_history: bool) -> serde_json::Value {
        let process = crate::process_memory::snapshot_with_source("client:memory");
        let markdown = crate::tui::markdown::debug_memory_profile();
        let mermaid = crate::tui::mermaid::debug_memory_profile();
        let visual_debug = crate::tui::visual_debug::debug_memory_profile();
        let ui_render = crate::tui::ui::debug_memory_profile();
        let side_panel_render = crate::tui::ui::debug_side_panel_memory_profile();
        let mcp = self
            .mcp_manager
            .try_read()
            .map(|manager| manager.debug_memory_profile())
            .ok();
        let (provider_view_source, materialized_provider_messages): (&str, Cow<'_, [Message]>) =
            if self.is_remote || !self.messages.is_empty() {
                ("resident_ui", Cow::Borrowed(&self.messages))
            } else {
                (
                    "session_materialized",
                    Cow::Owned(self.session.messages_for_provider_uncached()),
                )
            };
        let transcript_memory = crate::tui::transcript_memory_profile(
            &self.session,
            &self.messages,
            &materialized_provider_messages,
            provider_view_source,
            &self.display_messages,
            &self.side_panel,
        );

        let provider_messages_json_bytes: usize = self
            .messages
            .iter()
            .map(crate::process_memory::estimate_json_bytes)
            .sum();
        let mut provider_message_memory = ProviderMessageMemoryStats::default();
        for message in &self.messages {
            provider_message_memory.record_message(message);
        }
        let display_messages_bytes: usize = self
            .display_messages
            .iter()
            .map(estimate_display_message_bytes)
            .sum();
        let mut display_message_memory = DisplayMessageMemoryStats::default();
        for message in &self.display_messages {
            display_message_memory.record_message(message);
        }
        let streaming_tool_calls_json_bytes: usize = self
            .streaming_tool_calls
            .iter()
            .map(crate::process_memory::estimate_json_bytes)
            .sum();
        let remote_side_pane_images_bytes =
            estimate_rendered_images_bytes(&self.remote_side_pane_images);
        let remote_model_options_json_bytes: usize = self
            .remote_model_options
            .iter()
            .map(crate::process_memory::estimate_json_bytes)
            .sum();
        let remote_total_tokens_json_bytes = self
            .remote_total_tokens
            .as_ref()
            .map(crate::process_memory::estimate_json_bytes)
            .unwrap_or(0);

        let mut payload = serde_json::json!({
            "process": process,
            "session": self.session.debug_memory_profile(),
            "markdown": markdown,
            "mermaid": mermaid,
            "visual_debug": visual_debug,
            "ui_render": ui_render,
            "side_panel_render": side_panel_render,
            "ui": {
                "provider_messages": {
                    "count": self.messages.len(),
                    "json_bytes": provider_messages_json_bytes,
                    "content_blocks": provider_message_memory.content_blocks,
                    "payload_text_bytes": provider_message_memory.payload_text_bytes(),
                    "text_bytes": provider_message_memory.text_bytes,
                    "reasoning_bytes": provider_message_memory.reasoning_bytes,
                    "tool_use_input_json_bytes": provider_message_memory.tool_use_input_json_bytes,
                    "tool_result_bytes": provider_message_memory.tool_result_bytes,
                    "image_data_bytes": provider_message_memory.image_data_bytes,
                    "openai_compaction_bytes": provider_message_memory.openai_compaction_bytes,
                    "large_blob_count": provider_message_memory.large_blob_count,
                    "large_blob_bytes": provider_message_memory.large_blob_bytes,
                    "large_tool_result_count": provider_message_memory.large_tool_result_count,
                    "large_tool_result_bytes": provider_message_memory.large_tool_result_bytes,
                    "max_block_bytes": provider_message_memory.max_block_bytes,
                },
                "display_messages": {
                    "count": self.display_messages.len(),
                    "estimate_bytes": display_messages_bytes,
                    "role_bytes": display_message_memory.role_bytes,
                    "content_bytes": display_message_memory.content_bytes,
                    "tool_call_text_bytes": display_message_memory.tool_call_text_bytes,
                    "title_bytes": display_message_memory.title_bytes,
                    "tool_data_json_bytes": display_message_memory.tool_data_json_bytes,
                    "large_content_count": display_message_memory.large_content_count,
                    "large_content_bytes": display_message_memory.large_content_bytes,
                    "max_content_bytes": display_message_memory.max_content_bytes,
                },
                "transcript_memory": transcript_memory,
                "input": {
                    "text_bytes": self.input.len(),
                    "cursor_pos": self.cursor_pos,
                },
                "streaming": {
                    "streaming_text_bytes": self.streaming.streaming_text.len(),
                    "thinking_buffer_bytes": self.thinking_buffer.len(),
                    "stream_buffer": self.stream_buffer.debug_memory_profile(),
                    "streaming_tool_calls_count": self.streaming_tool_calls.len(),
                    "streaming_tool_calls_json_bytes": streaming_tool_calls_json_bytes,
                },
                "queued_messages": {
                    "visible_count": self.queued_messages.len(),
                    "visible_text_bytes": estimate_string_vec_bytes(&self.queued_messages),
                    "hidden_count": self.hidden_queued_system_messages.len(),
                    "hidden_text_bytes": estimate_string_vec_bytes(&self.hidden_queued_system_messages),
                    "current_turn_system_reminder_bytes": self.current_turn_system_reminder.as_ref().map(|value| value.len()).unwrap_or(0),
                },
                "clipboard_and_input_media": {
                    "pasted_contents_count": self.pasted_contents.len(),
                    "pasted_contents_bytes": estimate_string_vec_bytes(&self.pasted_contents),
                    "pending_images_count": self.pending_images.len(),
                    "pending_images_bytes": estimate_pending_images_bytes(&self.pending_images),
                },
                "images_and_views": {
                    "remote_side_pane_images_count": self.remote_side_pane_images.len(),
                    "remote_side_pane_images_bytes": remote_side_pane_images_bytes,
                },
                "remote_state": {
                    "available_entries_count": self.remote_available_entries.len(),
                    "available_entries_bytes": estimate_string_vec_bytes(&self.remote_available_entries),
                    "model_options_count": self.remote_model_options.len(),
                    "model_options_json_bytes": remote_model_options_json_bytes,
                    "skills_count": self.remote_skills.len(),
                    "skills_bytes": estimate_string_vec_bytes(&self.remote_skills),
                    "mcp_servers_count": self.remote_mcp_servers.len(),
                    "mcp_servers_bytes": estimate_string_vec_bytes(&self.remote_mcp_servers),
                    "mcp_server_names_count": self.mcp_server_names.len(),
                    "mcp_server_names_bytes": estimate_pair_vec_bytes(&self.mcp_server_names),
                    "remote_total_tokens_json_bytes": remote_total_tokens_json_bytes,
                },
                "skills": {
                    "available_count": self.current_skills_snapshot().list().len(),
                },
                "mcp": mcp,
            },
        });

        if include_history {
            payload["app_owned"] = self.debug_app_owned_memory_profile();
            payload["summary"] = build_debug_summary(&payload);
            payload["history"] = serde_json::to_value(crate::process_memory::history(64))
                .unwrap_or_else(|_| serde_json::Value::Array(Vec::new()));
        }

        payload
    }

    fn debug_app_owned_memory_profile(&self) -> serde_json::Value {
        let streaming_markdown_renderer =
            self.streaming_md_renderer.borrow().debug_memory_profile();
        let inline_view = self
            .inline_view_state
            .as_ref()
            .map(|state| state.debug_memory_profile())
            .unwrap_or_else(|| serde_json::json!({"present": false, "total_estimate_bytes": 0}));
        let inline_interactive = self
            .inline_interactive_state
            .as_ref()
            .map(|state| state.debug_memory_profile())
            .unwrap_or_else(|| serde_json::json!({"present": false, "total_estimate_bytes": 0}));
        let pending_remote_message_bytes = self
            .rate_limit_pending_message
            .as_ref()
            .map(estimate_pending_remote_message_bytes)
            .unwrap_or(0);
        let pending_split_prompt_bytes = self
            .pending_split_prompt
            .as_ref()
            .map(estimate_pending_split_prompt_bytes)
            .unwrap_or(0);
        let pending_catchup_resume_bytes = self
            .pending_catchup_resume
            .as_ref()
            .map(estimate_pending_catchup_resume_bytes)
            .unwrap_or(0);
        let in_flight_catchup_resume_bytes = self
            .in_flight_catchup_resume
            .as_ref()
            .map(estimate_pending_catchup_resume_bytes)
            .unwrap_or(0);
        let input_undo_stack_bytes: usize = self
            .input_undo_stack
            .iter()
            .map(|entry| entry.input.capacity())
            .sum();
        let stashed_input_bytes = self
            .stashed_input
            .as_ref()
            .map(|(text, _)| text.capacity())
            .unwrap_or(0);
        let pending_soft_interrupts_bytes: usize = self
            .pending_soft_interrupts
            .iter()
            .map(|value| value.capacity())
            .sum();
        let pending_soft_interrupt_requests_bytes: usize = self
            .pending_soft_interrupt_requests
            .iter()
            .map(|(_, value)| value.capacity())
            .sum();
        let reload_info_bytes: usize = self.reload_info.iter().map(|value| value.capacity()).sum();
        let catchup_return_stack_bytes: usize = self
            .catchup_return_stack
            .iter()
            .map(|value| value.capacity())
            .sum();
        let tool_tracking_bytes: usize = self
            .tool_call_ids
            .iter()
            .map(|value| value.capacity())
            .sum::<usize>()
            + self
                .tool_result_ids
                .iter()
                .map(|value| value.capacity())
                .sum::<usize>();
        let remote_sessions_bytes: usize = self
            .remote_sessions
            .iter()
            .map(|value| value.capacity())
            .sum();
        let remote_swarm_members_json_bytes =
            crate::process_memory::estimate_json_bytes(&self.remote_swarm_members);
        let swarm_plan_items_json_bytes =
            crate::process_memory::estimate_json_bytes(&self.swarm_plan_items);
        let remote_side_pane_images_bytes =
            estimate_rendered_images_bytes(&self.remote_side_pane_images);
        let session_picker = self
            .session_picker_overlay
            .as_ref()
            .map(|overlay| overlay.borrow().debug_memory_profile())
            .unwrap_or_else(|| serde_json::json!({"present": false, "total_estimate_bytes": 0}));
        let login_picker = self
            .login_picker_overlay
            .as_ref()
            .map(|overlay| overlay.borrow().debug_memory_profile())
            .unwrap_or_else(|| serde_json::json!({"present": false, "total_estimate_bytes": 0}));
        let account_picker = self
            .account_picker_overlay
            .as_ref()
            .map(|overlay| overlay.borrow().debug_memory_profile())
            .unwrap_or_else(|| serde_json::json!({"present": false, "total_estimate_bytes": 0}));
        let usage_overlay = self
            .usage_overlay
            .as_ref()
            .map(|overlay| overlay.borrow().debug_memory_profile())
            .unwrap_or_else(|| serde_json::json!({"present": false, "total_estimate_bytes": 0}));
        let debug_trace_events_bytes: usize = self
            .debug_trace
            .events
            .iter()
            .map(|event| event.kind.capacity() + event.detail.capacity())
            .sum();
        let string_state_bytes = self.observe_page_markdown.capacity()
            + self.split_view_markdown.capacity()
            + self
                .status_notice
                .as_ref()
                .map(|(value, _)| value.capacity())
                .unwrap_or(0)
            + self
                .interleave_message
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .pending_split_startup_message
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .pending_split_parent_session_id
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .pending_split_model_override
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .pending_split_provider_key_override
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .pending_split_label
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .rate_limit_pending_message
                .as_ref()
                .and_then(|message| message.system_reminder.as_ref())
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .ambient_system_prompt
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .last_stream_error
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .active_skill
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .provider_session_id
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .upstream_provider
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .connection_type
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .status_detail
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .remote_provider_name
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .remote_provider_model
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .remote_reasoning_effort
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .remote_service_tier
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .remote_transport
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .remote_server_version
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .remote_server_short_name
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .remote_server_icon
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .remote_session_id
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .pending_migration
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
            + self
                .resume_session_id
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0);

        let totals = serde_json::json!({
            "pending_remote_message_bytes": pending_remote_message_bytes,
            "pending_split_prompt_bytes": pending_split_prompt_bytes,
            "pending_catchup_resume_bytes": pending_catchup_resume_bytes,
            "in_flight_catchup_resume_bytes": in_flight_catchup_resume_bytes,
            "input_undo_stack_bytes": input_undo_stack_bytes,
            "stashed_input_bytes": stashed_input_bytes,
            "pending_soft_interrupts_bytes": pending_soft_interrupts_bytes,
            "pending_soft_interrupt_requests_bytes": pending_soft_interrupt_requests_bytes,
            "reload_info_bytes": reload_info_bytes,
            "catchup_return_stack_bytes": catchup_return_stack_bytes,
            "tool_tracking_bytes": tool_tracking_bytes,
            "remote_sessions_bytes": remote_sessions_bytes,
            "remote_swarm_members_json_bytes": remote_swarm_members_json_bytes,
            "swarm_plan_items_json_bytes": swarm_plan_items_json_bytes,
            "remote_side_pane_images_bytes": remote_side_pane_images_bytes,
            "debug_trace_events_bytes": debug_trace_events_bytes,
            "string_state_bytes": string_state_bytes,
            "session_picker_bytes": nested_usize(&session_picker, &["total_estimate_bytes"]),
            "login_picker_bytes": nested_usize(&login_picker, &["total_estimate_bytes"]),
            "account_picker_bytes": nested_usize(&account_picker, &["total_estimate_bytes"]),
            "usage_overlay_bytes": nested_usize(&usage_overlay, &["total_estimate_bytes"]),
            "inline_view_bytes": nested_usize(&inline_view, &["total_estimate_bytes"]),
            "inline_interactive_bytes": nested_usize(&inline_interactive, &["total_estimate_bytes"]),
            "streaming_markdown_renderer_bytes": nested_usize(&streaming_markdown_renderer, &["total_estimate_bytes"]),
        });

        let total_estimate_bytes = totals
            .as_object()
            .map(|map| map.values().filter_map(|value| value.as_u64()).sum::<u64>())
            .unwrap_or(0);

        serde_json::json!({
            "pending_remote_message": {
                "present": self.rate_limit_pending_message.is_some(),
                "estimate_bytes": pending_remote_message_bytes,
            },
            "pending_split_prompt": {
                "present": self.pending_split_prompt.is_some(),
                "estimate_bytes": pending_split_prompt_bytes,
            },
            "catchup": {
                "pending_estimate_bytes": pending_catchup_resume_bytes,
                "in_flight_estimate_bytes": in_flight_catchup_resume_bytes,
                "return_stack_bytes": catchup_return_stack_bytes,
            },
            "pending_interrupts": {
                "soft_interrupts_count": self.pending_soft_interrupts.len(),
                "soft_interrupts_bytes": pending_soft_interrupts_bytes,
                "requests_count": self.pending_soft_interrupt_requests.len(),
                "requests_bytes": pending_soft_interrupt_requests_bytes,
            },
            "input_history": {
                "undo_entries": self.input_undo_stack.len(),
                "undo_stack_bytes": input_undo_stack_bytes,
                "stashed_input_bytes": stashed_input_bytes,
            },
            "remote_state_extra": {
                "remote_sessions_count": self.remote_sessions.len(),
                "remote_sessions_bytes": remote_sessions_bytes,
                "remote_swarm_members_count": self.remote_swarm_members.len(),
                "remote_swarm_members_json_bytes": remote_swarm_members_json_bytes,
                "swarm_plan_items_count": self.swarm_plan_items.len(),
                "swarm_plan_items_json_bytes": swarm_plan_items_json_bytes,
            },
            "images_and_views": {
                "remote_side_pane_images_count": self.remote_side_pane_images.len(),
                "remote_side_pane_images_bytes": remote_side_pane_images_bytes,
                "observe_page_markdown_bytes": self.observe_page_markdown.capacity(),
                "split_view_markdown_bytes": self.split_view_markdown.capacity(),
            },
            "tool_tracking": {
                "tool_call_ids_count": self.tool_call_ids.len(),
                "tool_result_ids_count": self.tool_result_ids.len(),
                "estimate_bytes": tool_tracking_bytes,
            },
            "debug": {
                "trace_enabled": self.debug_trace.enabled,
                "trace_event_count": self.debug_trace.events.len(),
                "trace_event_bytes": debug_trace_events_bytes,
                "reload_info_bytes": reload_info_bytes,
            },
            "string_state": {
                "estimate_bytes": string_state_bytes,
            },
            "overlays": {
                "session_picker": session_picker,
                "login_picker": login_picker,
                "account_picker": account_picker,
                "usage_overlay": usage_overlay,
            },
            "inline": {
                "view": inline_view,
                "interactive": inline_interactive,
            },
            "streaming_markdown_renderer": streaming_markdown_renderer,
            "totals": totals,
            "total_estimate_bytes": total_estimate_bytes,
        })
    }
}

fn build_debug_summary(payload: &serde_json::Value) -> serde_json::Value {
    let process_pss_bytes = nested_usize(payload, &["process", "os", "pss_bytes"]);
    let mut buckets = vec![
        (
            "session_json_bytes".to_string(),
            nested_usize(payload, &["session", "totals", "json_bytes"]),
        ),
        (
            "resident_provider_messages_json_bytes".to_string(),
            nested_usize(payload, &["ui", "provider_messages", "json_bytes"]),
        ),
        (
            "display_messages_estimate_bytes".to_string(),
            nested_usize(payload, &["ui", "display_messages", "estimate_bytes"]),
        ),
        (
            "side_panel_estimate_bytes".to_string(),
            nested_usize(
                payload,
                &["ui", "transcript_memory", "side_panel", "estimate_bytes"],
            ),
        ),
        (
            "streaming_text_bytes".to_string(),
            nested_usize(payload, &["ui", "streaming", "streaming_text_bytes"]),
        ),
        (
            "thinking_buffer_bytes".to_string(),
            nested_usize(payload, &["ui", "streaming", "thinking_buffer_bytes"]),
        ),
        (
            "stream_buffered_text_bytes".to_string(),
            nested_usize(
                payload,
                &["ui", "streaming", "stream_buffer", "buffered_text_bytes"],
            ),
        ),
        (
            "streaming_tool_calls_json_bytes".to_string(),
            nested_usize(
                payload,
                &["ui", "streaming", "streaming_tool_calls_json_bytes"],
            ),
        ),
        (
            "queued_messages_visible_bytes".to_string(),
            nested_usize(payload, &["ui", "queued_messages", "visible_text_bytes"]),
        ),
        (
            "queued_messages_hidden_bytes".to_string(),
            nested_usize(payload, &["ui", "queued_messages", "hidden_text_bytes"]),
        ),
        (
            "current_turn_system_reminder_bytes".to_string(),
            nested_usize(
                payload,
                &[
                    "ui",
                    "queued_messages",
                    "current_turn_system_reminder_bytes",
                ],
            ),
        ),
        (
            "pasted_contents_bytes".to_string(),
            nested_usize(
                payload,
                &["ui", "clipboard_and_input_media", "pasted_contents_bytes"],
            ),
        ),
        (
            "pending_images_bytes".to_string(),
            nested_usize(
                payload,
                &["ui", "clipboard_and_input_media", "pending_images_bytes"],
            ),
        ),
        (
            "remote_state_bytes".to_string(),
            nested_usize(payload, &["ui", "remote_state", "available_entries_bytes"])
                + nested_usize(payload, &["ui", "remote_state", "model_options_json_bytes"])
                + nested_usize(payload, &["ui", "remote_state", "skills_bytes"])
                + nested_usize(payload, &["ui", "remote_state", "mcp_servers_bytes"])
                + nested_usize(payload, &["ui", "remote_state", "mcp_server_names_bytes"])
                + nested_usize(
                    payload,
                    &["ui", "remote_state", "remote_total_tokens_json_bytes"],
                ),
        ),
        (
            "markdown_cache_estimate_bytes".to_string(),
            nested_usize(payload, &["markdown", "highlight_cache_estimate_bytes"]),
        ),
        (
            "mermaid_working_set_estimate_bytes".to_string(),
            nested_usize(payload, &["mermaid", "mermaid_working_set_estimate_bytes"]),
        ),
        (
            "mermaid_render_cache_metadata_estimate_bytes".to_string(),
            nested_usize(
                payload,
                &["mermaid", "render_cache_metadata_estimate_bytes"],
            ),
        ),
        (
            "visual_debug_frame_estimate_bytes".to_string(),
            nested_usize(payload, &["visual_debug", "frame_json_estimate_bytes"]),
        ),
        (
            "mcp_estimate_bytes".to_string(),
            nested_usize(payload, &["ui", "mcp", "configured_json_bytes"])
                + nested_usize(payload, &["ui", "mcp", "tool_schema_estimate_bytes"]),
        ),
        (
            "app_owned_extra_bytes".to_string(),
            nested_usize(payload, &["app_owned", "total_estimate_bytes"]),
        ),
        (
            "ui_render_cache_bytes".to_string(),
            nested_usize(payload, &["ui_render", "total_estimate_bytes"]),
        ),
        (
            "side_panel_render_cache_bytes".to_string(),
            nested_usize(payload, &["side_panel_render", "total_estimate_bytes"]),
        ),
        (
            "streaming_markdown_renderer_bytes".to_string(),
            nested_usize(
                payload,
                &[
                    "app_owned",
                    "streaming_markdown_renderer",
                    "total_estimate_bytes",
                ],
            ),
        ),
        (
            "inline_view_bytes".to_string(),
            nested_usize(
                payload,
                &["app_owned", "inline", "view", "total_estimate_bytes"],
            ),
        ),
        (
            "inline_interactive_bytes".to_string(),
            nested_usize(
                payload,
                &["app_owned", "inline", "interactive", "total_estimate_bytes"],
            ),
        ),
    ];

    buckets.retain(|(_, value)| *value > 0);
    buckets.sort_by(|left, right| right.1.cmp(&left.1));
    let total_app_owned_estimate_bytes: usize = buckets.iter().map(|(_, value)| *value).sum();
    let unattributed_process_pss_bytes =
        process_pss_bytes.saturating_sub(total_app_owned_estimate_bytes);
    let app_owned_coverage_ratio = ratio(total_app_owned_estimate_bytes, process_pss_bytes);

    // Explain the unattributed remainder with allocator-level stats: split it
    // into heap the app still holds (unlabeled live) vs freed-but-retained
    // allocator slack. This does not attribute per-structure, but it tells you
    // whether growth is an app leak or allocator retention.
    let allocator_live_bytes = nested_usize(
        payload,
        &["process", "allocator", "stats", "allocated_bytes"],
    );
    let allocator_retained_bytes = nested_usize(
        payload,
        &["process", "allocator", "stats", "retained_bytes"],
    );
    let allocator_breakdown = if allocator_live_bytes > 0 || allocator_retained_bytes > 0 {
        let unlabeled_live_heap_bytes =
            allocator_live_bytes.saturating_sub(total_app_owned_estimate_bytes);
        serde_json::json!({
            "heap_live_bytes": allocator_live_bytes,
            "heap_retained_bytes": allocator_retained_bytes,
            "unlabeled_live_heap_bytes": unlabeled_live_heap_bytes,
        })
    } else {
        serde_json::Value::Null
    };

    // Process-level buckets that app-owned estimators can never see: freed
    // heap the allocator retains, file-backed mappings (binary text/rodata),
    // thread stacks, and shmem. Together with the app-owned estimates these
    // explain most of PSS, so the coverage ratio measures estimator quality
    // instead of allocator behavior.
    let pss_anon_bytes = nested_opt_u64(payload, &["process", "os", "pss_anon_bytes"])
        .or_else(|| nested_opt_u64(payload, &["process", "os", "rss_anon_bytes"]));
    let allocator_retained_estimate_bytes =
        crate::runtime_memory_log::allocator_retained_resident_estimate(
            nested_opt_u64(
                payload,
                &["process", "allocator", "stats", "retained_bytes"],
            ),
            nested_opt_u64(
                payload,
                &["process", "allocator", "stats", "allocated_bytes"],
            ),
            pss_anon_bytes,
        )
        .unwrap_or(0) as usize;
    let thread_stack_estimate_bytes = crate::runtime_memory_log::thread_stack_estimate(
        nested_opt_u64(payload, &["process", "thread_count"]),
        nested_opt_u64(payload, &["process", "main_stack_bytes"]),
    )
    .unwrap_or(0) as usize;
    let file_backed_pss_bytes = nested_usize(payload, &["process", "os", "pss_file_bytes"]);
    let shmem_pss_bytes = nested_usize(payload, &["process", "os", "pss_shmem_bytes"]);
    let mut process_buckets = vec![
        (
            "allocator_retained_estimate_bytes",
            allocator_retained_estimate_bytes,
        ),
        ("file_backed_pss_bytes", file_backed_pss_bytes),
        ("thread_stack_estimate_bytes", thread_stack_estimate_bytes),
        ("shmem_pss_bytes", shmem_pss_bytes),
    ];
    process_buckets.retain(|(_, value)| *value > 0);
    process_buckets.sort_by(|left, right| right.1.cmp(&left.1));
    let total_process_bucket_bytes: usize = process_buckets.iter().map(|(_, value)| *value).sum();
    let total_explained_bytes = process_pss_bytes
        .min(total_app_owned_estimate_bytes.saturating_add(total_process_bucket_bytes));
    let coverage_ratio = ratio(total_explained_bytes, process_pss_bytes);
    let live_heap_coverage_ratio = ratio(total_app_owned_estimate_bytes, allocator_live_bytes);

    serde_json::json!({
        "process_pss_bytes": process_pss_bytes,
        "total_app_owned_estimate_bytes": total_app_owned_estimate_bytes,
        "unattributed_process_pss_bytes": unattributed_process_pss_bytes,
        "total_explained_bytes": total_explained_bytes,
        "unexplained_process_pss_bytes": process_pss_bytes.saturating_sub(total_explained_bytes),
        "coverage_ratio": coverage_ratio,
        "app_owned_coverage_ratio": app_owned_coverage_ratio,
        "live_heap_coverage_ratio": live_heap_coverage_ratio,
        "allocator_breakdown": allocator_breakdown,
        "process_buckets": process_buckets
            .into_iter()
            .map(|(name, bytes)| serde_json::json!({"name": name, "bytes": bytes}))
            .collect::<Vec<_>>(),
        "top_buckets": buckets
            .into_iter()
            .take(16)
            .map(|(name, bytes)| serde_json::json!({"name": name, "bytes": bytes}))
            .collect::<Vec<_>>(),
    })
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn estimate_pending_remote_message_bytes(value: &PendingRemoteMessage) -> usize {
    value.content.capacity()
        + estimate_pending_images_bytes(&value.images)
        + value
            .system_reminder
            .as_ref()
            .map(|text| text.capacity())
            .unwrap_or(0)
}

fn estimate_pending_split_prompt_bytes(value: &PendingSplitPrompt) -> usize {
    value.content.capacity() + estimate_pending_images_bytes(&value.images)
}

fn estimate_pending_catchup_resume_bytes(value: &PendingCatchupResume) -> usize {
    value.target_session_id.capacity()
        + value
            .source_session_id
            .as_ref()
            .map(|text| text.capacity())
            .unwrap_or(0)
}

fn estimate_rendered_images_bytes(images: &[crate::session::RenderedImage]) -> usize {
    images
        .iter()
        .map(|image| {
            image.media_type.capacity()
                + image.data.capacity()
                + image
                    .label
                    .as_ref()
                    .map(|label| label.capacity())
                    .unwrap_or(0)
        })
        .sum()
}

fn nested_usize(value: &serde_json::Value, path: &[&str]) -> usize {
    let mut cursor = value;
    for key in path {
        let Some(next) = cursor.get(*key) else {
            return 0;
        };
        cursor = next;
    }
    cursor
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0)
}

/// Like [`nested_usize`] but preserves absence, for estimators that need to
/// distinguish "missing" from zero.
fn nested_opt_u64(value: &serde_json::Value, path: &[&str]) -> Option<u64> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    cursor.as_u64()
}

#[cfg(test)]
mod debug_summary_tests {
    use super::build_debug_summary;

    #[test]
    fn summary_explains_pss_with_process_buckets() {
        // Numbers mirror a real client capture: 146MB PSS with 22MB live heap,
        // 110MB allocator retention, 11MB file-backed PSS, 10 threads.
        let mb = 1024 * 1024_u64;
        let payload = serde_json::json!({
            "process": {
                "thread_count": 10,
                "main_stack_bytes": 132 * 1024,
                "os": {
                    "pss_bytes": 146 * mb,
                    "pss_anon_bytes": 127 * mb,
                    "pss_file_bytes": 11 * mb,
                    "pss_shmem_bytes": 0,
                },
                "allocator": {
                    "stats": {
                        "allocated_bytes": 22 * mb,
                        "retained_bytes": 110 * mb,
                    },
                },
            },
            "session": { "totals": { "json_bytes": 5 * mb } },
        });

        let summary = build_debug_summary(&payload);

        assert_eq!(summary["process_pss_bytes"], 146 * mb);
        assert_eq!(summary["total_app_owned_estimate_bytes"], 5 * mb);
        // Retention capped at pss_anon - allocated = 105MB.
        let process_buckets = summary["process_buckets"].as_array().unwrap();
        assert_eq!(
            process_buckets[0]["name"],
            "allocator_retained_estimate_bytes"
        );
        assert_eq!(process_buckets[0]["bytes"], 105 * mb);
        assert_eq!(process_buckets[1]["name"], "file_backed_pss_bytes");
        assert_eq!(process_buckets[1]["bytes"], 11 * mb);
        // Explained = 5 (app) + 105 (retained) + 11 (file) + ~0.7MB stacks.
        let explained = summary["total_explained_bytes"].as_u64().unwrap();
        assert!(explained >= 121 * mb, "explained={explained}");
        let coverage = summary["coverage_ratio"].as_f64().unwrap();
        assert!(coverage > 0.8, "coverage_ratio={coverage}");
        let app_owned = summary["app_owned_coverage_ratio"].as_f64().unwrap();
        assert!(app_owned < 0.05, "app_owned_coverage_ratio={app_owned}");
        // Live-heap coverage: 5MB attributed of 22MB live heap.
        let live = summary["live_heap_coverage_ratio"].as_f64().unwrap();
        assert!(
            (live - 5.0 / 22.0).abs() < 0.01,
            "live_heap_coverage_ratio={live}"
        );
    }

    #[test]
    fn summary_coverage_never_exceeds_one() {
        // Overlapping estimates (app-owned + retention) must clamp at PSS.
        let payload = serde_json::json!({
            "process": {
                "os": { "pss_bytes": 100_u64, "pss_anon_bytes": 100_u64, "pss_file_bytes": 50_u64 },
                "allocator": { "stats": { "allocated_bytes": 10_u64, "retained_bytes": 90_u64 } },
            },
            "session": { "totals": { "json_bytes": 80_u64 } },
        });
        let summary = build_debug_summary(&payload);
        assert_eq!(summary["total_explained_bytes"], 100);
        assert_eq!(summary["coverage_ratio"].as_f64().unwrap(), 1.0);
        assert_eq!(summary["unexplained_process_pss_bytes"], 0);
    }
}
