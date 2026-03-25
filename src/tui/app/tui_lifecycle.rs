use super::state_ui::RestoredReloadInput;
use super::*;
use crate::tui::{backend, connection_type_icon, keybind, ui};

impl App {
    fn apply_restored_reload_input(&mut self, restored: RestoredReloadInput) {
        self.input = restored.input;
        self.cursor_pos = restored.cursor;
        self.hidden_queued_system_messages = restored.hidden_queued_system_messages;
        self.interleave_message = restored.interleave_message;
        self.rate_limit_pending_message = restored.rate_limit_pending_message;
        self.rate_limit_reset = restored.rate_limit_reset;

        let mut queued_messages = restored.queued_messages;
        let mut recovered_interrupts = restored.pending_soft_interrupts;
        if !recovered_interrupts.is_empty() {
            crate::logging::info(&format!(
                "Recovered {} pending soft interrupt(s) after reload; re-queueing them for resend",
                recovered_interrupts.len()
            ));
            if self.interleave_message.is_none() {
                self.interleave_message = Some(recovered_interrupts.remove(0));
            }
            if !recovered_interrupts.is_empty() {
                let mut recovered_queue = recovered_interrupts;
                recovered_queue.append(&mut queued_messages);
                queued_messages = recovered_queue;
            }
            self.set_status_notice("Recovered pending prompts after reload");
        }

        self.queued_messages = queued_messages;
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

    fn new_minimal_with_session(
        provider: Arc<dyn Provider>,
        registry: Registry,
        mut session: Session,
    ) -> Self {
        let skills = SkillRegistry::default();
        let mcp_manager = Arc::new(RwLock::new(McpManager::new()));
        if session.model.is_none() {
            session.model = Some(provider.model());
        }
        if session.provider_key.is_none() {
            session.provider_key = crate::session::derive_session_provider_key(provider.name());
        }
        let display = config().display.clone();
        let features = config().features.clone();
        let context_limit = provider.context_window() as u64;
        let improve_mode = session.improve_mode.map(|mode| match mode {
            crate::session::SessionImproveMode::Run => ImproveMode::Run,
            crate::session::SessionImproveMode::Plan => ImproveMode::Plan,
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
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost: 0.0,
            cached_prompt_price: None,
            cached_completion_price: None,
            context_limit,
            context_warning_shown: false,
            context_info: crate::prompt::ContextInfo::default(),
            last_stream_activity: None,
            streaming_tps_start: None,
            streaming_tps_elapsed: Duration::ZERO,
            streaming_total_output_tokens: 0,
            status: ProcessingStatus::default(),
            subagent_status: None,
            batch_progress: None,
            processing_started: None,
            last_api_completed: None,
            last_turn_input_tokens: None,
            pending_turn: false,
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
            restart_requested: None,
            pasted_contents: Vec::new(),
            pending_images: Vec::new(),
            copy_badge_ui: CopyBadgeUiState::default(),
            copy_selection_mode: false,
            copy_selection_anchor: None,
            copy_selection_cursor: None,
            copy_selection_pending_anchor: None,
            copy_selection_dragging: false,
            copy_selection_goal_column: None,
            debug_tx: None,
            remote_provider_name: None,
            remote_provider_model: None,
            remote_reasoning_effort: None,
            remote_service_tier: None,
            remote_transport: None,
            remote_compaction_mode: None,
            remote_available_models: Vec::new(),
            remote_model_routes: Vec::new(),
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
            tool_result_ids: HashSet::new(),
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
            diagram_pane_dragging: false,
            diff_pane_scroll: 0,
            diff_pane_focus: false,
            diff_pane_auto_scroll: true,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
            pin_images: display.pin_images,
            picker_state: None,
            pending_model_switch: None,
            pending_account_picker_selection: None,
            model_switch_keys: keybind::load_model_switch_keys(),
            effort_switch_keys: keybind::load_effort_switch_keys(),
            centered_toggle_keys: keybind::load_centered_toggle_key(),
            dictation_key: keybind::load_dictation_key(),
            scroll_keys: keybind::load_scroll_keys(),
            dictation_session: None,
            dictation_in_flight: false,
            scroll_bookmark: None,
            typing_scroll_lock: false,
            stashed_input: None,
            input_undo_stack: Vec::new(),
            status_notice: None,
            interleave_message: None,
            pending_soft_interrupts: Vec::new(),
            queue_mode: display.queue_mode,
            pending_queued_dispatch: false,
            tab_completion_state: None,
            app_started: Instant::now(),
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
            last_mouse_scroll: None,
            changelog_scroll: None,
            help_scroll: None,
            session_picker_overlay: None,
            login_picker_overlay: None,
            account_picker_overlay: None,
        };

        for notice in app.provider.drain_startup_notices() {
            app.status_notice = Some((notice, Instant::now()));
        }

        app
    }

    pub fn new(provider: Arc<dyn Provider>, registry: Registry) -> Self {
        let t0 = std::time::Instant::now();
        let skills = SkillRegistry::load().unwrap_or_default();
        let t_skills = t0.elapsed();
        let mcp_manager = Arc::new(RwLock::new(McpManager::new()));
        let mut session = Session::create(None, None);
        session.model = Some(provider.model());
        session.provider_key = crate::session::derive_session_provider_key(provider.name());
        let display = config().display.clone();
        let features = config().features.clone();
        let context_limit = provider.context_window() as u64;
        let improve_mode = session.improve_mode.map(|mode| match mode {
            crate::session::SessionImproveMode::Run => ImproveMode::Run,
            crate::session::SessionImproveMode::Plan => ImproveMode::Plan,
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
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost: 0.0,
            cached_prompt_price: None,
            cached_completion_price: None,
            context_limit,
            context_warning_shown: false,
            context_info,
            last_stream_activity: None,
            streaming_tps_start: None,
            streaming_tps_elapsed: Duration::ZERO,
            streaming_total_output_tokens: 0,
            status: ProcessingStatus::default(),
            subagent_status: None,
            batch_progress: None,
            processing_started: None,
            last_api_completed: None,
            last_turn_input_tokens: None,
            pending_turn: false,
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
            restart_requested: None,
            pasted_contents: Vec::new(),
            pending_images: Vec::new(),
            copy_badge_ui: CopyBadgeUiState::default(),
            copy_selection_mode: false,
            copy_selection_anchor: None,
            copy_selection_cursor: None,
            copy_selection_pending_anchor: None,
            copy_selection_dragging: false,
            copy_selection_goal_column: None,
            debug_tx: None,
            remote_provider_name: None,
            remote_provider_model: None,
            remote_reasoning_effort: None,
            remote_service_tier: None,
            remote_transport: None,
            remote_compaction_mode: None,
            remote_available_models: Vec::new(),
            remote_model_routes: Vec::new(),
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
            tool_result_ids: HashSet::new(),
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
            diagram_pane_dragging: false,
            diff_pane_scroll: 0,
            diff_pane_focus: false,
            diff_pane_auto_scroll: true,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
            pin_images: display.pin_images,
            picker_state: None,
            pending_model_switch: None,
            pending_account_picker_selection: None,
            model_switch_keys: keybind::load_model_switch_keys(),
            effort_switch_keys: keybind::load_effort_switch_keys(),
            centered_toggle_keys: keybind::load_centered_toggle_key(),
            dictation_key: keybind::load_dictation_key(),
            scroll_keys: keybind::load_scroll_keys(),
            dictation_session: None,
            dictation_in_flight: false,
            scroll_bookmark: None,
            typing_scroll_lock: false,
            stashed_input: None,
            input_undo_stack: Vec::new(),
            status_notice: None,
            interleave_message: None,
            pending_soft_interrupts: Vec::new(),
            queue_mode: display.queue_mode,
            pending_queued_dispatch: false,
            tab_completion_state: None,
            app_started: Instant::now(),
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
            last_mouse_scroll: None,
            changelog_scroll: None,
            help_scroll: None,
            session_picker_overlay: None,
            login_picker_overlay: None,
            account_picker_overlay: None,
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

    /// Create an App instance for remote mode (connecting to server)
    pub fn new_for_remote(resume_session: Option<String>) -> Self {
        let provider: Arc<dyn Provider> = Arc::new(NullProvider);
        let registry = Registry::empty();
        let session = resume_session
            .as_ref()
            .and_then(|session_id| Session::load(session_id).ok())
            .unwrap_or_else(|| Session::create(None, None));
        let mut app = Self::new_minimal_with_session(provider, registry, session);
        app.is_remote = true;

        // Load session to get canary status (for "client self-dev" badge)
        if let Some(ref session_id) = resume_session {
            if let Some(restored) = Self::restore_input_for_reload(session_id) {
                app.apply_restored_reload_input(restored);
            }
        }

        app.resume_session_id = resume_session;
        app
    }

    /// Mark that a server was just spawned - run_remote will retry initial connection
    /// instead of failing fatally, allowing the TUI to show while the server starts.
    pub fn set_server_spawning(&mut self) {
        self.server_spawning = true;
    }

    /// Create an App instance for replay mode (playing back a saved session)
    pub fn new_for_replay(session: crate::session::Session) -> Self {
        Self::new_for_replay_with_title(session, true)
    }

    pub(crate) fn new_for_replay_silent(session: crate::session::Session) -> Self {
        Self::new_for_replay_with_title(session, false)
    }

    fn new_for_replay_with_title(session: crate::session::Session, set_title: bool) -> Self {
        let provider: Arc<dyn Provider> = Arc::new(NullProvider);
        let registry = Registry::empty();
        let mut app = Self::new_minimal_with_session(provider, registry, session);
        app.is_remote = false;
        app.is_replay = true;
        let model_name = app.session.model.clone().unwrap_or_default();
        let session_name = app.session.short_name.clone().unwrap_or_default();

        // Set provider/model info so status widgets show correct values
        let effective_model = if model_name.is_empty() {
            // Try to infer model from message content (e.g., usage events)
            // Default to a sensible value for demo purposes
            "claude-sonnet-4-20250514".to_string()
        } else {
            model_name
        };
        app.remote_provider_model = Some(effective_model.clone());
        // Infer provider name from model string
        let provider_name = if effective_model.contains("claude")
            || effective_model.contains("opus")
            || effective_model.contains("sonnet")
            || effective_model.contains("haiku")
        {
            "anthropic"
        } else if effective_model.contains("gpt")
            || effective_model.contains("o1")
            || effective_model.contains("o3")
            || effective_model.contains("o4")
        {
            "openai"
        } else if effective_model.contains('/') {
            "openrouter"
        } else {
            "claude"
        };
        app.remote_provider_name = Some(provider_name.to_string());

        app.suppress_terminal_title_updates = !set_title;
        if set_title && !session_name.is_empty() {
            let icon = crate::id::session_icon(&session_name);
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::terminal::SetTitle(format!("{} replay: {}", icon, session_name))
            );
        }
        app
    }

    /// Get the current session ID
    pub fn session_id(&self) -> &str {
        &self.session.id
    }

    pub(super) fn update_terminal_title(&self) {
        if self.suppress_terminal_title_updates {
            return;
        }
        let session_id = if self.is_remote {
            self.remote_session_id
                .as_deref()
                .unwrap_or(&self.session.id)
        } else {
            &self.session.id
        };
        let session_name = crate::id::extract_session_name(session_id)
            .map(|s| s.to_string())
            .unwrap_or_else(|| session_id.to_string());
        let session_icon = crate::id::session_icon(&session_name);
        let is_canary = if self.is_remote {
            self.remote_is_canary.unwrap_or(self.session.is_canary)
        } else {
            self.session.is_canary
        };
        let suffix = if is_canary { " [self-dev]" } else { "" };
        let server_name = self.remote_server_short_name.as_deref().unwrap_or("jcode");
        let icon = connection_type_icon(self.connection_type.as_deref()).unwrap_or(session_icon);
        let server_label = if server_name.eq_ignore_ascii_case("jcode") {
            "jcode".to_string()
        } else {
            format!("jcode/{}", server_name.to_lowercase())
        };
        crate::process_title::set_client_display_title(&session_name, is_canary);
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::SetTitle(format!(
                "{} {} {}{}",
                icon,
                server_label,
                ui::capitalize(&session_name),
                suffix
            ))
        );
    }

    pub(super) fn reconnect_target_session_id(&self) -> Option<String> {
        self.remote_session_id
            .clone()
            .or_else(|| self.resume_session_id.clone())
    }

    /// Check if the selected reload candidate is newer than startup.
    /// Candidate selection matches `/reload` so the `cli↑` badge and reload target stay aligned.
    pub(super) fn has_newer_binary(&self) -> bool {
        let Some(startup_mtime) = self.client_binary_mtime else {
            return false;
        };

        let is_selfdev_session = if self.is_remote {
            self.remote_is_canary.unwrap_or(self.session.is_canary)
        } else {
            self.session.is_canary
        };

        let Some((candidate, _label)) = crate::build::client_update_candidate(is_selfdev_session)
        else {
            return false;
        };

        std::fs::metadata(&candidate)
            .ok()
            .and_then(|m| m.modified().ok())
            .is_some_and(|mtime| mtime > startup_mtime)
    }

    /// Initialize MCP servers (call after construction)
    pub async fn init_mcp(&mut self) {
        // Always register the MCP management tool so agent can connect servers
        let mcp_tool = crate::tool::mcp::McpManagementTool::new(Arc::clone(&self.mcp_manager))
            .with_registry(self.registry.clone());
        self.registry
            .register("mcp".to_string(), Arc::new(mcp_tool))
            .await;

        let manager = self.mcp_manager.read().await;
        let server_count = manager.config().servers.len();
        if server_count > 0 {
            drop(manager);

            // Log configured servers
            crate::logging::info(&format!("MCP: Found {} server(s) in config", server_count));

            let (successes, failures) = {
                let manager = self.mcp_manager.write().await;
                let result = manager.connect_all().await.unwrap_or((0, Vec::new()));
                // Cache server names with tool counts
                let servers = manager.connected_servers().await;
                let all_tools = manager.all_tools().await;
                self.mcp_server_names = servers
                    .into_iter()
                    .map(|name| {
                        let count = all_tools.iter().filter(|(s, _)| s == &name).count();
                        (name, count)
                    })
                    .collect();
                result
            };

            // Show connection results
            if successes > 0 {
                let msg = format!("MCP: Connected to {} server(s)", successes);
                crate::logging::info(&msg);
                self.set_status_notice(&format!("mcp: {} connected", successes));
            }

            if !failures.is_empty() {
                for (name, error) in &failures {
                    let msg = format!("MCP '{}' failed: {}", name, error);
                    self.push_display_message(DisplayMessage::error(msg));
                }
                if successes == 0 {
                    self.set_status_notice("MCP: all connections failed");
                }
            }

            // Register MCP server tools
            let tools = crate::mcp::create_mcp_tools(Arc::clone(&self.mcp_manager)).await;
            for (name, tool) in tools {
                self.registry.register(name, tool).await;
            }
        }

        // Register self-dev tools if this is a canary session
        if self.session.is_canary {
            self.registry.register_selfdev_tools().await;
        }
    }

    /// Restore a previous session (for hot-reload)
    pub fn restore_session(&mut self, session_id: &str) {
        if let Some(restored) = Self::restore_input_for_reload(session_id) {
            self.apply_restored_reload_input(restored);
        }
        if let Ok(session) = Session::load(session_id) {
            // Count stats before restoring
            let mut user_turns = 0;
            let mut assistant_turns = 0;
            let mut total_chars = 0;

            // Convert session messages to display messages (including tools)
            for item in crate::session::render_messages(&session) {
                if item.role == "user" {
                    user_turns += 1;
                } else if item.role == "assistant" {
                    assistant_turns += 1;
                }
                total_chars += item.content.len();

                self.push_display_message(DisplayMessage {
                    role: item.role,
                    content: item.content,
                    tool_calls: item.tool_calls,
                    duration_secs: None,
                    title: None,
                    tool_data: item.tool_data,
                });
            }

            // Don't restore provider_session_id - Claude sessions don't persist across
            // process restarts. The messages are restored, so Claude will get full context.
            self.provider_session_id = None;
            self.session = session;
            self.replace_provider_messages(self.session.messages_for_provider());
            // Clear the saved provider_session_id since it's no longer valid
            self.session.provider_session_id = None;
            let mut restored_model = false;
            if let Some(model) = self.session.model.clone() {
                if let Err(e) = self.provider.set_model(&model) {
                    self.push_display_message(DisplayMessage {
                        role: "system".to_string(),
                        content: format!("⚠ Failed to restore model '{}': {}", model, e),
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                } else {
                    restored_model = true;
                }
            }

            let active_model = self.provider.model();
            if restored_model || self.session.model.is_none() {
                self.session.model = Some(active_model.clone());
            }
            self.update_context_limit_for_model(&active_model);
            // Mark session as active now that it's being used again
            self.session.mark_active();
            self.side_panel =
                crate::side_panel::snapshot_for_session(session_id).unwrap_or_default();
            crate::telemetry::begin_resumed_session(self.provider.name(), &active_model);
            crate::logging::info(&format!("Restored session: {}", session_id));

            // Build stats message
            let total_turns = user_turns + assistant_turns;
            let estimated_tokens = total_chars / 4; // Rough estimate: ~4 chars per token
            let stats = if total_turns > 0 {
                format!(
                    " ({} turns, ~{}k tokens)",
                    total_turns,
                    estimated_tokens / 1000
                )
            } else {
                String::new()
            };

            // Check for reload info to show what triggered the reload
            let reload_info = if let Ok(jcode_dir) = crate::storage::jcode_dir() {
                let info_path = jcode_dir.join("reload-info");
                if info_path.exists() {
                    let info = std::fs::read_to_string(&info_path).ok();
                    let _ = std::fs::remove_file(&info_path); // Clean up
                    info
                } else {
                    None
                }
            } else {
                None
            };

            // Build the reload message based on what triggered it
            // Extract build hash for the AI notification
            let is_reload = reload_info.is_some();
            let (message, _build_hash) = if let Some(info) = reload_info {
                if let Some(hash) = info.strip_prefix("reload:") {
                    let h = hash.trim().to_string();
                    (format!("Reload complete — continuing.{}", stats), h)
                } else if let Some(hash) = info.strip_prefix("rebuild:") {
                    let h = hash.trim().to_string();
                    (format!("Rebuild complete — continuing.{}", stats), h)
                } else {
                    (
                        format!("Reload complete — continuing.{}", stats),
                        "unknown".to_string(),
                    )
                }
            } else {
                (
                    format!("Reload complete — continuing.{}", stats),
                    "unknown".to_string(),
                )
            };

            // Add success message with stats (only if there's actual content or a reload happened)
            if total_turns > 0 || is_reload {
                self.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: message,
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }

            // Queue an automatic message to notify the AI that reload completed
            let reload_ctx = ReloadContext::load_for_session(session_id).ok().flatten();
            if let Some(ctx) = reload_ctx {
                // This session initiated the reload - send the reload-specific continuation
                let task_info = ctx
                    .task_context
                    .map(|t| format!("\nTask context: {}", t))
                    .unwrap_or_default();
                let background_task_note =
                    super::reload_persisted_background_tasks_note(session_id);

                let continuation_msg = format!(
                    "Reload succeeded ({} → {}).{}{} Session restored with {} turns. Continue immediately from where you left off. Do not ask the user what to do next. Do not summarize the reload.",
                    ctx.version_before,
                    ctx.version_after,
                    task_info,
                    background_task_note,
                    total_turns
                );

                crate::logging::info(&format!(
                    "Queuing reload continuation message ({} chars)",
                    continuation_msg.len()
                ));
                self.hidden_queued_system_messages.push(continuation_msg);
                // Trigger processing so the queued message gets sent to the LLM.
                // Without this, the local event loop waits for user input since
                // process_queued_messages only runs inside process_turn_with_input.
                self.is_processing = true;
                self.status = ProcessingStatus::Sending;
                self.processing_started = Some(Instant::now());
                self.pending_turn = true;
            } else if self.was_interrupted_by_reload() {
                // This session was interrupted by another session's reload.
                // The conversation has incomplete tool results - auto-continue.
                crate::logging::info(
                    "Session was interrupted by reload (not initiator), queuing continuation",
                );
                self.push_display_message(DisplayMessage::system(
                    "Reload complete — continuing.".to_string(),
                ));
                self.hidden_queued_system_messages.push(
                    "Your session was interrupted by a server reload while a tool was running. The tool was aborted and results may be incomplete. Continue exactly where you left off and do not ask the user what to do next.".to_string(),
                );
                self.is_processing = true;
                self.status = ProcessingStatus::Sending;
                self.processing_started = Some(Instant::now());
                self.pending_turn = true;
            }
        } else {
            crate::logging::error(&format!("Failed to restore session: {}", session_id));

            // Check if this was a reload that failed - inject failure message if so
            if let Ok(Some(ctx)) = ReloadContext::load_for_session(session_id) {
                let task_info = ctx
                    .task_context
                    .map(|t| format!(" You were working on: {}", t))
                    .unwrap_or_default();

                self.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: format!(
                        "⚠ Reload failed. Session could not be restored. Previous version: {}, Target version: {}.{}\n\
                         Starting fresh session. You may need to re-examine your changes.",
                        ctx.version_before,
                        ctx.version_after,
                        task_info
                    ),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
        }
    }

    /// Check if the current session was interrupted by a server reload.
    /// Detects two patterns:
    /// 1. Last message is a User ToolResult containing reload interruption text
    /// 2. Last assistant message ends with "[generation interrupted - server reloading]"
    pub(super) fn was_interrupted_by_reload(&self) -> bool {
        use crate::message::{ContentBlock, Role};
        let messages = &self.session.messages;
        if messages.is_empty() {
            return false;
        }
        let last = &messages[messages.len() - 1];
        match last.role {
            Role::User => last.content.iter().any(|block| match block {
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    is_error.unwrap_or(false)
                        && (content.contains("interrupted by server reload")
                            || content.contains("Skipped - server reloading"))
                }
                _ => false,
            }),
            Role::Assistant => last.content.iter().any(|block| match block {
                ContentBlock::Text { text, .. } => {
                    text.ends_with("[generation interrupted - server reloading]")
                }
                _ => false,
            }),
        }
    }
}

pub(super) fn handle_dev_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed == "/reload" {
        if !app.has_newer_binary() {
            app.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: "No newer binary found. Nothing to reload.\nUse /rebuild to build a new version.".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            return true;
        }
        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content: "Reloading with newer binary...".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        app.session.provider_session_id = app.provider_session_id.clone();
        app.session
            .set_status(crate::session::SessionStatus::Reloaded);
        let _ = app.session.save();
        app.save_input_for_reload(&app.session.id.clone());
        app.reload_requested = Some(app.session.id.clone());
        app.should_quit = true;
        return true;
    }

    if trimmed == "/restart" {
        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content: "Restarting jcode (same binary, session preserved)...".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        app.session.provider_session_id = app.provider_session_id.clone();
        app.session
            .set_status(crate::session::SessionStatus::Reloaded);
        let _ = app.session.save();
        app.save_input_for_reload(&app.session.id.clone());
        app.restart_requested = Some(app.session.id.clone());
        app.should_quit = true;
        return true;
    }

    if trimmed == "/rebuild" {
        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content: "Rebuilding jcode (git pull + cargo build + tests)...".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        app.session.provider_session_id = app.provider_session_id.clone();
        app.session
            .set_status(crate::session::SessionStatus::Reloaded);
        let _ = app.session.save();
        app.rebuild_requested = Some(app.session.id.clone());
        app.should_quit = true;
        return true;
    }

    if trimmed == "/update" {
        app.push_display_message(DisplayMessage::system(
            "Checking for updates...".to_string(),
        ));
        app.session.provider_session_id = app.provider_session_id.clone();
        app.session
            .set_status(crate::session::SessionStatus::Reloaded);
        let _ = app.session.save();
        app.update_requested = Some(app.session.id.clone());
        app.should_quit = true;
        return true;
    }

    if trimmed == "/z" || trimmed == "/zz" || trimmed == "/zzz" || trimmed == "/zstatus" {
        use crate::provider::copilot::PremiumMode;
        let current = app.provider.premium_mode();

        if trimmed == "/zstatus" {
            let label = match current {
                PremiumMode::Normal => "normal",
                PremiumMode::OnePerSession => "one premium per session",
                PremiumMode::Zero => "zero premium requests",
            };
            let env = std::env::var("JCODE_COPILOT_PREMIUM").ok();
            let env_label = match env.as_deref() {
                Some("0") => "0 (zero)",
                Some("1") => "1 (one per session)",
                _ => "unset (normal)",
            };
            app.push_display_message(DisplayMessage::system(format!(
                "Premium mode: **{}**\nEnv JCODE_COPILOT_PREMIUM: {}",
                label, env_label,
            )));
            return true;
        }

        if trimmed == "/z" {
            app.provider.set_premium_mode(PremiumMode::Normal);
            let _ = crate::config::Config::set_copilot_premium(None);
            app.set_status_notice("Premium: normal");
            app.push_display_message(DisplayMessage::system(
                "Premium request mode reset to normal. (saved to config)".to_string(),
            ));
            return true;
        }

        let mode = if trimmed == "/zzz" {
            PremiumMode::Zero
        } else {
            PremiumMode::OnePerSession
        };
        if current == mode {
            app.provider.set_premium_mode(PremiumMode::Normal);
            let _ = crate::config::Config::set_copilot_premium(None);
            app.set_status_notice("Premium: normal");
            app.push_display_message(DisplayMessage::system(
                "Premium request mode reset to normal. (saved to config)".to_string(),
            ));
        } else {
            app.provider.set_premium_mode(mode);
            let config_val = match mode {
                PremiumMode::Zero => "zero",
                PremiumMode::OnePerSession => "one",
                PremiumMode::Normal => "normal",
            };
            let _ = crate::config::Config::set_copilot_premium(Some(config_val));
            let label = match mode {
                PremiumMode::OnePerSession => "one premium per session",
                PremiumMode::Zero => "zero premium requests",
                PremiumMode::Normal => "normal",
            };
            app.set_status_notice(&format!("Premium: {}", label));
            app.push_display_message(DisplayMessage::system(format!(
                "Premium mode: **{}**. Toggle off with `/z`. (saved to config)",
                label,
            )));
        }
        return true;
    }

    false
}
