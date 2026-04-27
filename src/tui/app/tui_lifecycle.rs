use super::state_ui::RestoredReloadInput;
use super::*;
use crate::tui::{backend, keybind};

impl App {
    pub(super) fn apply_restored_reload_input(&mut self, restored: RestoredReloadInput) {
        self.input = restored.input;
        self.cursor_pos = restored.cursor;
        self.pending_images = restored.pending_images;
        self.submit_input_on_startup = restored.submit_on_restore
            && (!self.input.is_empty() || !self.pending_images.is_empty());
        self.hidden_queued_system_messages = restored.hidden_queued_system_messages;
        if let Some(status_notice) = restored.startup_status_notice {
            self.set_status_notice(status_notice);
        } else if self.submit_input_on_startup {
            self.set_status_notice("Startup prompt queued");
        }
        if let Some((title, message)) = restored.startup_display_message {
            self.push_display_message(DisplayMessage::system(message).with_title(title));
        }
        self.interleave_message = None;
        self.rate_limit_pending_message = restored.rate_limit_pending_message;
        self.rate_limit_reset = restored.rate_limit_reset;
        self.observe_page_markdown = restored.observe_page_markdown;
        self.observe_page_updated_at_ms = restored.observe_page_updated_at_ms;
        self.set_observe_mode_enabled(restored.observe_mode_enabled, restored.observe_mode_enabled);
        self.set_split_view_enabled(restored.split_view_enabled, restored.split_view_enabled);
        self.set_todos_view_enabled(restored.todos_view_enabled, restored.todos_view_enabled);

        let mut queued_messages = restored.queued_messages;
        let mut recovered_followups = Vec::new();
        if let Some(interleave_message) = restored.interleave_message
            && !interleave_message.trim().is_empty()
        {
            recovered_followups.push(interleave_message);
        }
        let recovered_interrupts = restored
            .pending_soft_interrupt_resend
            .unwrap_or(restored.pending_soft_interrupts);
        if !recovered_interrupts.is_empty() {
            crate::logging::info(&format!(
                "Recovered {} pending soft interrupt(s) after reload; re-queueing them as normal follow-ups",
                recovered_interrupts.len()
            ));
            recovered_followups.extend(recovered_interrupts);
        }
        if !recovered_followups.is_empty() {
            let mut recovered_queue = recovered_followups;
            recovered_queue.append(&mut queued_messages);
            queued_messages = recovered_queue;
            self.set_status_notice("Recovered pending prompts after reload");
        }

        self.queued_messages = queued_messages;
        if self.has_queued_followups() {
            self.is_processing = true;
            self.status = ProcessingStatus::Sending;
            if self.processing_started.is_none() {
                self.processing_started = Some(Instant::now());
            }
            if self.is_remote {
                self.pending_queued_dispatch = true;
            } else {
                self.pending_turn = true;
            }
        }
    }

    pub(super) async fn begin_remote_send(
        &mut self,
        remote: &mut backend::RemoteConnection,
        content: String,
        images: Vec<(String, String)>,
        is_system: bool,
    ) -> Result<u64> {
        remote::begin_remote_send(self, remote, content, images, is_system, None, false, 0).await
    }

    pub(super) fn schedule_pending_remote_retry(&mut self, reason: &str) -> bool {
        let Some(pending) = self.rate_limit_pending_message.as_mut() else {
            return false;
        };
        if !pending.auto_retry {
            return false;
        }
        let outcome = {
            let current_attempts = pending.retry_attempts;
            if current_attempts >= Self::AUTO_RETRY_MAX_ATTEMPTS {
                Err(current_attempts)
            } else {
                pending.retry_attempts += 1;
                let retry_attempts = pending.retry_attempts;
                let backoff_secs = Self::AUTO_RETRY_BASE_DELAY_SECS * u64::from(retry_attempts);
                let retry_at = Instant::now() + Duration::from_secs(backoff_secs);
                pending.retry_at = Some(retry_at);
                Ok((retry_attempts, backoff_secs, retry_at))
            }
        };

        match outcome {
            Err(current_attempts) => {
                self.rate_limit_pending_message = None;
                self.rate_limit_reset = None;
                self.push_display_message(DisplayMessage::error(format!(
                    "{} Auto-retry limit reached after {} attempt{}. Use `/poke` again to retry manually.",
                    reason,
                    current_attempts,
                    if current_attempts == 1 { "" } else { "s" }
                )));
                false
            }
            Ok((retry_attempts, backoff_secs, retry_at)) => {
                self.rate_limit_reset = Some(retry_at);
                self.push_display_message(DisplayMessage::system(format!(
                    "{} Auto-retrying in {} second{} (attempt {}/{}).",
                    reason,
                    backoff_secs,
                    if backoff_secs == 1 { "" } else { "s" },
                    retry_attempts,
                    Self::AUTO_RETRY_MAX_ATTEMPTS
                )));
                true
            }
        }
    }

    pub(super) fn clear_pending_remote_retry(&mut self) {
        self.rate_limit_pending_message = None;
        self.rate_limit_reset = None;
    }

    pub(super) fn new_minimal_with_session(
        provider: Arc<dyn Provider>,
        registry: Registry,
        mut session: Session,
    ) -> Self {
        let skills = Arc::new(SkillRegistry::default());
        let mcp_manager = Arc::new(RwLock::new(McpManager::new()));
        if session.model.is_none() {
            session.model = Some(provider.model());
        }
        if session.provider_key.is_none() {
            session.provider_key = crate::session::derive_session_provider_key(provider.name());
        }
        let display = config().display.clone();
        let features = config().features.clone();
        let autoreview_enabled = session
            .autoreview_enabled
            .unwrap_or(config().autoreview.enabled);
        let autojudge_enabled = session
            .autojudge_enabled
            .unwrap_or(config().autojudge.enabled);
        let context_limit = provider.context_window() as u64;
        let mut runtime_memory_log = if crate::runtime_memory_log::client_logging_enabled() {
            Some(crate::runtime_memory_log::RuntimeMemoryLogController::new(
                crate::runtime_memory_log::client_logging_config(),
            ))
        } else {
            None
        };
        if let Some(controller) = runtime_memory_log.as_mut() {
            controller.defer_event(
                crate::runtime_memory_log::RuntimeMemoryLogEvent::new("startup", "client_started")
                    .with_session_id(session.id.clone())
                    .force_attribution(),
            );
        }
        let improve_mode = session.improve_mode.map(|mode| match mode {
            crate::session::SessionImproveMode::ImproveRun => ImproveMode::ImproveRun,
            crate::session::SessionImproveMode::ImprovePlan => ImproveMode::ImprovePlan,
            crate::session::SessionImproveMode::RefactorRun => ImproveMode::RefactorRun,
            crate::session::SessionImproveMode::RefactorPlan => ImproveMode::RefactorPlan,
        });

        crate::logging::info("App::new_minimal_with_session: skipping skill/prompt bootstrap");
        crate::telemetry::begin_session(provider.name(), &provider.model());

        let mut app = Self {
            provider,
            registry,
            skills,
            mcp_manager,
            messages: Vec::new(),
            session,
            display_messages: Vec::new(),
            display_messages_version: 0,
            display_user_message_count: 0,
            display_edit_tool_message_count: 0,
            input: String::new(),
            cursor_pos: 0,
            scroll_offset: 0,
            auto_scroll_paused: false,
            active_skill: None,
            is_processing: false,
            streaming_text: String::new(),
            should_quit: false,
            queued_messages: Vec::new(),
            hidden_queued_system_messages: Vec::new(),
            current_turn_system_reminder: None,
            streaming_input_tokens: 0,
            streaming_output_tokens: 0,
            streaming_cache_read_tokens: None,
            streaming_cache_creation_tokens: None,
            upstream_provider: None,
            connection_type: None,
            status_detail: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost: 0.0,
            cached_prompt_price: None,
            cached_completion_price: None,
            context_limit,
            context_warning_shown: false,
            context_info: crate::prompt::ContextInfo::default(),
            last_stream_activity: None,
            stream_message_ended: false,
            remote_resume_activity: None,
            pending_reload_reconnect_status: None,
            streaming_tps_start: None,
            streaming_tps_elapsed: Duration::ZERO,
            streaming_tps_collect_output: false,
            streaming_total_output_tokens: 0,
            streaming_tps_observed_output_tokens: 0,
            streaming_tps_observed_elapsed: Duration::ZERO,
            status: ProcessingStatus::default(),
            subagent_status: None,
            batch_progress: None,
            processing_started: None,
            visible_turn_started: None,
            last_api_completed: None,
            last_turn_input_tokens: None,
            pending_turn: false,
            auto_poke_incomplete_todos: true,
            pending_provider_failover: None,
            session_save_pending: false,
            streaming_tool_calls: Vec::new(),
            provider_session_id: None,
            cancel_requested: false,
            quit_pending: None,
            last_resize_redraw: None,
            mcp_server_names: Vec::new(),
            stream_buffer: StreamBuffer::new(),
            thinking_start: None,
            thought_line_inserted: false,
            thinking_buffer: String::new(),
            thinking_prefix_emitted: false,
            reload_requested: None,
            rebuild_requested: None,
            update_requested: None,
            background_client_action: None,
            pending_background_client_reload: None,
            restart_requested: None,
            pasted_contents: Vec::new(),
            pending_images: Vec::new(),
            route_next_prompt_to_new_session: false,
            submit_input_on_startup: false,
            copy_badge_ui: CopyBadgeUiState::default(),
            copy_selection_mode: false,
            copy_selection_anchor: None,
            copy_selection_cursor: None,
            copy_selection_pending_anchor: None,
            copy_selection_dragging: false,
            copy_selection_goal_column: None,
            debug_tx: None,
            remote_client_instance_id: crate::id::new_id("client"),
            remote_provider_name: None,
            remote_provider_model: None,
            remote_startup_phase: None,
            remote_startup_phase_started: None,
            remote_reasoning_effort: None,
            remote_service_tier: None,
            remote_transport: None,
            remote_compaction_mode: None,
            remote_available_entries: Vec::new(),
            remote_model_options: Vec::new(),
            pending_remote_model_refresh_snapshot: None,
            remote_mcp_servers: Vec::new(),
            remote_skills: Vec::new(),
            remote_total_tokens: None,
            remote_is_canary: None,
            remote_server_version: None,
            remote_server_has_update: None,
            pending_server_reload: false,
            remote_server_short_name: None,
            remote_server_icon: None,
            current_message_id: None,
            is_remote: false,
            server_spawning: false,
            is_replay: false,
            suppress_terminal_title_updates: false,
            replay_elapsed_override: None,
            replay_processing_started_ms: None,
            tool_call_ids: HashSet::new(),
            tool_result_ids: HashSet::new(),
            tool_output_scan_index: 0,
            remote_session_id: None,
            remote_sessions: Vec::new(),
            remote_side_pane_images: Vec::new(),
            remote_swarm_members: Vec::new(),
            swarm_plan_items: Vec::new(),
            swarm_plan_version: None,
            swarm_plan_swarm_id: None,
            known_stable_version: crate::build::read_stable_version().ok().flatten(),
            last_version_check: Some(Instant::now()),
            pending_migration: None,
            remote_client_count: None,
            resume_session_id: None,
            requested_exit_code: None,
            memory_enabled: features.memory,
            autoreview_enabled,
            autojudge_enabled,
            improve_mode,
            last_injected_memory_signature: None,
            swarm_enabled: features.swarm,
            diff_mode: display.diff_mode,
            centered: display.centered,
            diagram_mode: display.diagram_mode,
            diagram_focus: false,
            diagram_index: 0,
            diagram_scroll_x: 0,
            diagram_scroll_y: 0,
            diagram_pane_ratio: 40,
            diagram_pane_ratio_from: 40,
            diagram_pane_ratio_target: 40,
            diagram_pane_anim_start: None,
            diagram_pane_enabled: true,
            diagram_pane_position: crate::config::DiagramPanePosition::default(),
            diagram_zoom: 100,
            last_visible_diagram_hash: None,
            diagram_pane_dragging: false,
            diff_pane_scroll: 0,
            diff_pane_scroll_x: 0,
            diff_pane_focus: false,
            diff_pane_auto_scroll: true,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
            observe_mode_enabled: false,
            observe_page_markdown: String::new(),
            observe_page_updated_at_ms: 0,
            split_view_enabled: false,
            split_view_markdown: String::new(),
            split_view_updated_at_ms: 0,
            split_view_rendered_display_version: 0,
            split_view_rendered_streaming_hash: 0,
            todos_view_enabled: false,
            todos_view_markdown: String::new(),
            todos_view_updated_at_ms: 0,
            todos_view_rendered_hash: 0,
            last_side_panel_refresh: None,
            last_client_focus_recorded_at: None,
            last_client_focus_session_id: None,
            last_side_panel_focus_id: None,
            pin_images: display.pin_images,
            chat_native_scrollbar: display.native_scrollbars.chat,
            side_panel_native_scrollbar: display.native_scrollbars.side_panel,
            inline_view_state: None,
            inline_interactive_state: None,
            model_picker_cache: None,
            model_picker_catalog_revision: 0,
            pending_model_picker_load: None,
            model_picker_load_request_id: 0,
            pending_model_switch: None,
            pending_account_picker_action: None,
            model_switch_keys: keybind::load_model_switch_keys(),
            effort_switch_keys: keybind::load_effort_switch_keys(),
            centered_toggle_keys: keybind::load_centered_toggle_key(),
            workspace_navigation_keys: keybind::load_workspace_navigation_keys(),
            dictation_key: keybind::load_dictation_key(),
            scroll_keys: keybind::load_scroll_keys(),
            dictation_session: None,
            dictation_in_flight: false,
            dictation_request_id: None,
            dictation_target_session_id: None,
            scroll_bookmark: None,
            typing_scroll_lock: false,
            stashed_input: None,
            input_undo_stack: Vec::new(),
            status_notice: None,
            interleave_message: None,
            pending_soft_interrupts: Vec::new(),
            pending_soft_interrupt_requests: Vec::new(),
            autoreview_after_current_turn: false,
            autojudge_after_current_turn: false,
            pending_split_startup_message: None,
            pending_split_parent_session_id: None,
            pending_split_prompt: None,
            pending_split_model_override: None,
            pending_split_provider_key_override: None,
            pending_split_label: None,
            pending_split_started_at: None,
            pending_split_request: false,
            pending_transfer_request: false,
            pending_local_transfer: None,
            queue_mode: display.queue_mode,
            auto_server_reload: display.auto_server_reload,
            pending_queued_dispatch: false,
            tab_completion_state: None,
            app_started: Instant::now(),
            runtime_memory_log,
            client_binary_mtime: std::env::current_exe()
                .ok()
                .and_then(|p| std::fs::metadata(&p).ok())
                .and_then(|m| m.modified().ok()),
            rate_limit_reset: None,
            rate_limit_pending_message: None,
            last_stream_error: None,
            reload_info: Vec::new(),
            debug_trace: DebugTrace::new(),
            streaming_md_renderer: RefCell::new(IncrementalMarkdownRenderer::new(None)),
            ambient_system_prompt: None,
            pending_login: None,
            pending_account_input: None,
            force_full_redraw: false,
            last_mouse_scroll: None,
            mouse_scroll_target: None,
            mouse_scroll_queue: 0,
            changelog_scroll: None,
            help_scroll: None,
            session_picker_overlay: None,
            session_picker_mode: SessionPickerMode::Resume,
            pending_session_picker_load: None,
            catchup_return_stack: Vec::new(),
            pending_catchup_resume: None,
            in_flight_catchup_resume: None,
            login_picker_overlay: None,
            account_picker_overlay: None,
            usage_overlay: None,
            usage_report_refreshing: false,
        };

        for notice in app.provider.drain_startup_notices() {
            app.status_notice = Some((notice, Instant::now()));
        }

        app
    }

    pub fn new(provider: Arc<dyn Provider>, registry: Registry) -> Self {
        let t0 = std::time::Instant::now();
        let skills = SkillRegistry::shared_snapshot();
        let t_skills = t0.elapsed();
        let mcp_manager = Arc::new(RwLock::new(McpManager::new()));
        let mut session = Session::create(None, None);
        session.mark_active();
        session.model = Some(provider.model());
        session.provider_key = crate::session::derive_session_provider_key(provider.name());
        let display = config().display.clone();
        let features = config().features.clone();
        let autoreview_enabled = session
            .autoreview_enabled
            .unwrap_or(config().autoreview.enabled);
        let autojudge_enabled = session
            .autojudge_enabled
            .unwrap_or(config().autojudge.enabled);
        let context_limit = provider.context_window() as u64;
        let mut runtime_memory_log = if crate::runtime_memory_log::client_logging_enabled() {
            Some(crate::runtime_memory_log::RuntimeMemoryLogController::new(
                crate::runtime_memory_log::client_logging_config(),
            ))
        } else {
            None
        };
        if let Some(controller) = runtime_memory_log.as_mut() {
            controller.defer_event(
                crate::runtime_memory_log::RuntimeMemoryLogEvent::new("startup", "client_started")
                    .with_session_id(session.id.clone())
                    .force_attribution(),
            );
        }
        let improve_mode = session.improve_mode.map(|mode| match mode {
            crate::session::SessionImproveMode::ImproveRun => ImproveMode::ImproveRun,
            crate::session::SessionImproveMode::ImprovePlan => ImproveMode::ImprovePlan,
            crate::session::SessionImproveMode::RefactorRun => ImproveMode::RefactorRun,
            crate::session::SessionImproveMode::RefactorPlan => ImproveMode::RefactorPlan,
        });
        let t_session = t0.elapsed();

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let provider_clone = Arc::clone(&provider);
            handle.spawn(async move {
                let _ = provider_clone.prefetch_models().await;
            });
        }

        // Pre-compute context info so it shows on startup
        let available_skills: Vec<crate::prompt::SkillInfo> = skills
            .list()
            .iter()
            .map(|s| crate::prompt::SkillInfo {
                name: s.name.clone(),
                description: s.description.clone(),
            })
            .collect();
        let (_, context_info) = crate::prompt::build_system_prompt_with_context(
            None,
            &available_skills,
            session.is_canary,
        );
        let t_prompt = t0.elapsed();
        crate::logging::info(&format!(
            "App::new timings: skills={:.1}ms session={:.1}ms prompt={:.1}ms total={:.1}ms",
            t_skills.as_secs_f64() * 1000.0,
            (t_session - t_skills).as_secs_f64() * 1000.0,
            (t_prompt - t_session).as_secs_f64() * 1000.0,
            t_prompt.as_secs_f64() * 1000.0,
        ));

        crate::telemetry::begin_session(provider.name(), &provider.model());

        let mut app = Self {
            provider,
            registry,
            skills,
            mcp_manager,
            messages: Vec::new(),
            session,
            display_messages: Vec::new(),
            display_messages_version: 0,
            display_user_message_count: 0,
            display_edit_tool_message_count: 0,
            input: String::new(),
            cursor_pos: 0,
            scroll_offset: 0,
            auto_scroll_paused: false,
            active_skill: None,
            is_processing: false,
            streaming_text: String::new(),
            should_quit: false,
            queued_messages: Vec::new(),
            hidden_queued_system_messages: Vec::new(),
            current_turn_system_reminder: None,
            streaming_input_tokens: 0,
            streaming_output_tokens: 0,
            streaming_cache_read_tokens: None,
            streaming_cache_creation_tokens: None,
            upstream_provider: None,
            connection_type: None,
            status_detail: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost: 0.0,
            cached_prompt_price: None,
            cached_completion_price: None,
            context_limit,
            context_warning_shown: false,
            context_info,
            last_stream_activity: None,
            stream_message_ended: false,
            remote_resume_activity: None,
            pending_reload_reconnect_status: None,
            streaming_tps_start: None,
            streaming_tps_elapsed: Duration::ZERO,
            streaming_tps_collect_output: false,
            streaming_total_output_tokens: 0,
            streaming_tps_observed_output_tokens: 0,
            streaming_tps_observed_elapsed: Duration::ZERO,
            status: ProcessingStatus::default(),
            subagent_status: None,
            batch_progress: None,
            processing_started: None,
            visible_turn_started: None,
            last_api_completed: None,
            last_turn_input_tokens: None,
            pending_turn: false,
            auto_poke_incomplete_todos: true,
            pending_provider_failover: None,
            session_save_pending: false,
            streaming_tool_calls: Vec::new(),
            provider_session_id: None,
            cancel_requested: false,
            quit_pending: None,
            last_resize_redraw: None,
            mcp_server_names: Vec::new(), // Vec<(name, tool_count)>
            stream_buffer: StreamBuffer::new(),
            thinking_start: None,
            thought_line_inserted: false,
            thinking_buffer: String::new(),
            thinking_prefix_emitted: false,
            reload_requested: None,
            rebuild_requested: None,
            update_requested: None,
            background_client_action: None,
            pending_background_client_reload: None,
            restart_requested: None,
            pasted_contents: Vec::new(),
            pending_images: Vec::new(),
            route_next_prompt_to_new_session: false,
            submit_input_on_startup: false,
            copy_badge_ui: CopyBadgeUiState::default(),
            copy_selection_mode: false,
            copy_selection_anchor: None,
            copy_selection_cursor: None,
            copy_selection_pending_anchor: None,
            copy_selection_dragging: false,
            copy_selection_goal_column: None,
            debug_tx: None,
            remote_client_instance_id: crate::id::new_id("client"),
            remote_provider_name: None,
            remote_provider_model: None,
            remote_startup_phase: None,
            remote_startup_phase_started: None,
            remote_reasoning_effort: None,
            remote_service_tier: None,
            remote_transport: None,
            remote_compaction_mode: None,
            remote_available_entries: Vec::new(),
            remote_model_options: Vec::new(),
            pending_remote_model_refresh_snapshot: None,
            remote_mcp_servers: Vec::new(),
            remote_skills: Vec::new(),
            remote_total_tokens: None,
            remote_is_canary: None,
            remote_server_version: None,
            remote_server_has_update: None,
            pending_server_reload: false,
            remote_server_short_name: None,
            remote_server_icon: None,
            current_message_id: None,
            is_remote: false,
            server_spawning: false,
            is_replay: false,
            suppress_terminal_title_updates: false,
            replay_elapsed_override: None,
            replay_processing_started_ms: None,
            tool_call_ids: HashSet::new(),
            tool_result_ids: HashSet::new(),
            tool_output_scan_index: 0,
            remote_session_id: None,
            remote_sessions: Vec::new(),
            remote_side_pane_images: Vec::new(),
            remote_swarm_members: Vec::new(),
            swarm_plan_items: Vec::new(),
            swarm_plan_version: None,
            swarm_plan_swarm_id: None,
            known_stable_version: crate::build::read_stable_version().ok().flatten(),
            last_version_check: Some(Instant::now()),
            pending_migration: None,
            remote_client_count: None,
            resume_session_id: None,
            requested_exit_code: None,
            memory_enabled: features.memory,
            autoreview_enabled,
            autojudge_enabled,
            improve_mode,
            last_injected_memory_signature: None,
            swarm_enabled: features.swarm,
            diff_mode: display.diff_mode,
            centered: display.centered,
            diagram_mode: display.diagram_mode,
            diagram_focus: false,
            diagram_index: 0,
            diagram_scroll_x: 0,
            diagram_scroll_y: 0,
            diagram_pane_ratio: 40,
            diagram_pane_ratio_from: 40,
            diagram_pane_ratio_target: 40,
            diagram_pane_anim_start: None,
            diagram_pane_enabled: true,
            diagram_pane_position: crate::config::DiagramPanePosition::default(),
            diagram_zoom: 100,
            last_visible_diagram_hash: None,
            diagram_pane_dragging: false,
            diff_pane_scroll: 0,
            diff_pane_scroll_x: 0,
            diff_pane_focus: false,
            diff_pane_auto_scroll: true,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
            observe_mode_enabled: false,
            observe_page_markdown: String::new(),
            observe_page_updated_at_ms: 0,
            split_view_enabled: false,
            split_view_markdown: String::new(),
            split_view_updated_at_ms: 0,
            split_view_rendered_display_version: 0,
            split_view_rendered_streaming_hash: 0,
            todos_view_enabled: false,
            todos_view_markdown: String::new(),
            todos_view_updated_at_ms: 0,
            todos_view_rendered_hash: 0,
            last_side_panel_refresh: None,
            last_client_focus_recorded_at: None,
            last_client_focus_session_id: None,
            last_side_panel_focus_id: None,
            pin_images: display.pin_images,
            chat_native_scrollbar: display.native_scrollbars.chat,
            side_panel_native_scrollbar: display.native_scrollbars.side_panel,
            inline_view_state: None,
            inline_interactive_state: None,
            model_picker_cache: None,
            model_picker_catalog_revision: 0,
            pending_model_picker_load: None,
            model_picker_load_request_id: 0,
            pending_model_switch: None,
            pending_account_picker_action: None,
            model_switch_keys: keybind::load_model_switch_keys(),
            effort_switch_keys: keybind::load_effort_switch_keys(),
            centered_toggle_keys: keybind::load_centered_toggle_key(),
            workspace_navigation_keys: keybind::load_workspace_navigation_keys(),
            dictation_key: keybind::load_dictation_key(),
            scroll_keys: keybind::load_scroll_keys(),
            dictation_session: None,
            dictation_in_flight: false,
            dictation_request_id: None,
            dictation_target_session_id: None,
            scroll_bookmark: None,
            typing_scroll_lock: false,
            stashed_input: None,
            input_undo_stack: Vec::new(),
            status_notice: None,
            interleave_message: None,
            pending_soft_interrupts: Vec::new(),
            pending_soft_interrupt_requests: Vec::new(),
            autoreview_after_current_turn: false,
            autojudge_after_current_turn: false,
            pending_split_startup_message: None,
            pending_split_parent_session_id: None,
            pending_split_prompt: None,
            pending_split_model_override: None,
            pending_split_provider_key_override: None,
            pending_split_label: None,
            pending_split_started_at: None,
            pending_split_request: false,
            pending_transfer_request: false,
            pending_local_transfer: None,
            queue_mode: display.queue_mode,
            auto_server_reload: display.auto_server_reload,
            pending_queued_dispatch: false,
            tab_completion_state: None,
            app_started: Instant::now(),
            runtime_memory_log,
            client_binary_mtime: std::env::current_exe()
                .ok()
                .and_then(|p| std::fs::metadata(&p).ok())
                .and_then(|m| m.modified().ok()),
            rate_limit_reset: None,
            rate_limit_pending_message: None,
            last_stream_error: None,
            reload_info: Vec::new(),
            debug_trace: DebugTrace::new(),
            streaming_md_renderer: RefCell::new(IncrementalMarkdownRenderer::new(None)),
            ambient_system_prompt: None,
            pending_login: None,
            pending_account_input: None,
            force_full_redraw: false,
            last_mouse_scroll: None,
            mouse_scroll_target: None,
            mouse_scroll_queue: 0,
            changelog_scroll: None,
            help_scroll: None,
            session_picker_overlay: None,
            session_picker_mode: SessionPickerMode::Resume,
            pending_session_picker_load: None,
            catchup_return_stack: Vec::new(),
            pending_catchup_resume: None,
            in_flight_catchup_resume: None,
            login_picker_overlay: None,
            account_picker_overlay: None,
            usage_overlay: None,
            usage_report_refreshing: false,
        };

        for notice in app.provider.drain_startup_notices() {
            app.status_notice = Some((notice, Instant::now()));
        }

        app
    }

    /// Configure ambient mode: override system prompt and queue an initial message.
    pub fn set_ambient_mode(&mut self, system_prompt: String, initial_message: String) {
        self.ambient_system_prompt = Some(system_prompt);
        crate::tool::ambient::register_ambient_session(self.session.id.clone());
        self.queued_messages.push(initial_message);
        self.is_processing = true;
        self.status = ProcessingStatus::Sending;
        self.processing_started = Some(Instant::now());
        self.pending_turn = true;
    }

    /// Queue a startup message that should be auto-sent when the TUI starts.
    pub fn queue_startup_message(&mut self, message: String) {
        if message.trim().is_empty() {
            return;
        }
        self.queued_messages.push(message);
        self.is_processing = true;
        self.status = ProcessingStatus::Sending;
        self.processing_started = Some(Instant::now());
        self.pending_turn = true;
    }

    fn restore_remote_startup_history(&mut self, session_id: &str) {
        let load_start = Instant::now();
        let Ok(mut session) = Session::load_for_remote_startup(session_id) else {
            return;
        };

        let render_start = Instant::now();
        let (rendered_messages, rendered_images) =
            crate::session::render_messages_and_images(&session);
        let display_messages = rendered_messages
            .into_iter()
            .map(|item| DisplayMessage {
                role: item.role,
                content: item.content,
                tool_calls: item.tool_calls,
                duration_secs: None,
                title: None,
                tool_data: item.tool_data,
            })
            .collect();
        self.replace_display_messages(display_messages);
        let render_ms = render_start.elapsed().as_millis();

        self.remote_side_pane_images = rendered_images;
        let image_ms = 0;
        self.set_side_panel_snapshot(
            crate::side_panel::snapshot_for_session(session_id).unwrap_or_default(),
        );
        self.remote_session_id = Some(session_id.to_string());
        session.strip_transcript_for_remote_client();
        self.session = session;
        self.autoreview_enabled = self
            .session
            .autoreview_enabled
            .unwrap_or(crate::config::config().autoreview.enabled);
        self.autojudge_enabled = self
            .session
            .autojudge_enabled
            .unwrap_or(crate::config::config().autojudge.enabled);
        if let Some(model) = self.session.model.clone() {
            self.update_context_limit_for_model(&model);
        }
        self.follow_chat_bottom();
        crate::logging::info(&format!(
            "Remote startup fast restore: session={}, display_messages={}, images={}, load={}ms, render={}ms, images_render={}ms, total={}ms",
            session_id,
            self.display_messages.len(),
            self.remote_side_pane_images.len(),
            load_start
                .elapsed()
                .as_millis()
                .saturating_sub(render_ms + image_ms),
            render_ms,
            image_ms,
            load_start.elapsed().as_millis()
        ));
    }

    /// Create an App instance for remote mode (connecting to server)
    pub fn new_for_remote(resume_session: Option<String>) -> Self {
        Self::new_for_remote_with_options(resume_session, false)
    }

    pub fn new_for_remote_with_options(resume_session: Option<String>, fresh_spawn: bool) -> Self {
        let provider: Arc<dyn Provider> = Arc::new(NullProvider);
        let registry = Registry::empty();
        let session = resume_session
            .as_ref()
            .and_then(|session_id| Session::load_startup_stub(session_id).ok())
            .unwrap_or_else(|| Session::create(None, None));
        let mut app = Self::new_minimal_with_session(provider, registry, session);
        app.is_remote = true;
        app.remote_startup_phase = Some(super::RemoteStartupPhase::Connecting);
        app.remote_startup_phase_started = Some(Instant::now());

        // Load session to get canary status (for "client self-dev" badge)
        if let Some(ref session_id) = resume_session {
            if !fresh_spawn {
                app.restore_remote_startup_history(session_id);
            } else {
                crate::logging::info(&format!(
                    "Remote startup fresh-spawn path: skipping local transcript restore for {}",
                    session_id
                ));
            }
            if let Some(restored) = Self::restore_input_for_reload(session_id) {
                app.apply_restored_reload_input(restored);
                if app.has_queued_followups() {
                    app.pending_queued_dispatch = true;
                    if app.processing_started.is_none() {
                        app.processing_started = Some(Instant::now());
                    }
                }
            }
        }

        app.resume_session_id = resume_session;
        app
    }

    /// Mark that a server was just spawned - run_remote will retry initial connection
    /// instead of failing fatally, allowing the TUI to show while the server starts.
    pub fn set_server_spawning(&mut self) {
        self.server_spawning = true;
        self.remote_startup_phase = Some(super::RemoteStartupPhase::StartingServer);
        self.remote_startup_phase_started = Some(Instant::now());
    }
}
