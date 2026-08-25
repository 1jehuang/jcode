use super::*;
use crate::tui::connection_type_icon;

impl App {
    /// Create an App instance for replay mode (playing back a saved session)
    pub fn new_for_replay(session: crate::session::Session) -> Self {
        Self::new_for_replay_with_title(session, true)
    }

    pub(crate) fn new_for_replay_silent(session: crate::session::Session) -> Self {
        Self::new_for_replay_with_title(session, false)
    }

    fn new_for_replay_with_title(session: crate::session::Session, set_title: bool) -> Self {
        let provider: Arc<dyn Provider> =
            Arc::new(InertRuntimeProvider::new(AppRuntimeMode::Replay));
        let registry = Registry::empty();
        let mut app = Self::new_minimal_with_session(provider, registry, session);
        app.is_remote = false;
        app.is_replay = true;
        app.runtime_mode = AppRuntimeMode::Replay;
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
        let provider_name = match crate::provider::provider_for_model(&effective_model) {
            Some("claude") => "anthropic",
            Some("openai") => "openai",
            Some("openrouter") => "openrouter",
            Some("bedrock") => "bedrock",
            Some("gemini") => "gemini",
            Some("cursor") => "cursor",
            Some("antigravity") => "antigravity",
            Some(other) => other,
            None => "claude",
        };
        app.remote_provider_name = Some(provider_name.to_string());

        app.suppress_terminal_title_updates = !set_title;
        if set_title && !session_name.is_empty() {
            app.update_terminal_title();
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
        // Keep the live terminal title aligned with /resume: an explicit rename
        // wins, then the model's current todo/goal title, then the generated title.
        let todo_title = self
            .session
            .custom_title
            .is_none()
            .then(|| crate::todo::load_session_title(session_id))
            .flatten();
        let display_title = self
            .session
            .custom_title
            .as_deref()
            .or(todo_title.as_deref())
            .or(self.session.title.as_deref());
        let is_canary = if self.is_remote {
            self.remote_is_canary.unwrap_or(self.session.is_canary)
        } else {
            self.session.is_canary
        };
        let server_name = self.remote_server_short_name.as_deref().unwrap_or("jcode");
        let icon = connection_type_icon(self.connection_type.as_deref()).unwrap_or(session_icon);
        let session_label = crate::process_title::terminal_session_label(&session_name, None);
        let fallback_label = if server_name.eq_ignore_ascii_case("jcode") {
            format!("jcode {session_label}")
        } else {
            format!("jcode/{} {session_label}", server_name.to_lowercase())
        };
        if server_name.eq_ignore_ascii_case("jcode") {
            crate::process_title::set_client_display_title(&session_name, is_canary);
        } else {
            crate::process_title::set_client_remote_display_title(
                server_name,
                &session_name,
                is_canary,
            );
        }
        let window_title = crate::process_title::terminal_window_title(
            icon,
            display_title,
            Some(&fallback_label),
            is_canary,
        );
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::SetTitle(window_title)
        );
    }

    pub(super) fn reconnect_target_session_id(&self) -> Option<String> {
        self.remote_session_id
            .clone()
            .or_else(|| self.resume_session_id.clone())
    }

    /// Resolve the session id to resume across a client reload re-exec.
    ///
    /// Prefers the live `remote_session_id`, then the id captured from a
    /// History payload that was deferred for a version mismatch
    /// (`pending_reload_session_id`), then the resume target the client was
    /// launched with. Only when none of those is known do we fabricate a fresh
    /// `ses_*` id. Fabricating eagerly is what caused issue #328: the re-exec
    /// would `jcode --resume <bogus-id>` and crash with "No session found
    /// matching ..." after an auto-update, because the version-mismatch defer
    /// path returns before `remote_session_id` is ever assigned.
    pub(super) fn reload_handoff_session_id(&self) -> String {
        self.remote_session_id
            .clone()
            .or_else(|| self.pending_reload_session_id.clone())
            .or_else(|| self.resume_session_id.clone())
            .unwrap_or_else(|| {
                let fabricated = crate::id::new_id("ses");
                crate::logging::warn(&format!(
                    "Reload handoff has no known session id (remote_session_id, pending reload id, and resume target all empty); fabricating {} for re-exec",
                    fabricated
                ));
                fabricated
            })
    }

    pub fn runtime_mode(&self) -> AppRuntimeMode {
        self.runtime_mode
    }

    pub fn is_remote_client(&self) -> bool {
        self.runtime_mode == AppRuntimeMode::RemoteClient
    }

    pub fn is_replay_runtime(&self) -> bool {
        self.runtime_mode == AppRuntimeMode::Replay
    }

    pub(crate) fn uses_server_or_replay_metadata(&self) -> bool {
        matches!(
            self.runtime_mode,
            AppRuntimeMode::RemoteClient | AppRuntimeMode::Replay
        )
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

        let Some((candidate, _label)) =
            crate::build::preferred_reload_candidate(is_selfdev_session)
        else {
            return false;
        };

        // The candidate may be a channel symlink to a release wrapper script;
        // compare the payload that actually runs (`client_binary_mtime` is the
        // running payload's mtime). Comparing the wrapper's mtime reported a
        // phantom "newer client" forever after release installs whose wrapper
        // was written after the payload, re-execing the client in a loop.
        std::fs::metadata(crate::build::resolve_binary_payload(&candidate))
            .ok()
            .and_then(|m| m.modified().ok())
            .is_some_and(|mtime| mtime > startup_mtime)
    }

    /// After an in-process server reload (e.g. `self-dev build-reload`), the
    /// server PID is unchanged and connected clients never disconnect, so they
    /// keep running their old binary and client-side changes never take effect.
    /// When this is a self-dev session, a newer client binary is on disk, and the
    /// client is idle, re-exec onto the new binary so TUI-side changes apply too.
    /// Returns true when a client reload was requested.
    pub(super) fn maybe_self_reload_after_server_reload(&mut self) -> bool {
        if !self.is_remote {
            return false;
        }
        let is_selfdev_session = self.remote_is_canary.unwrap_or(self.session.is_canary);
        if !is_selfdev_session {
            return false;
        }
        // Never interrupt an in-flight turn; the reconnect path will catch it later.
        if self.is_processing {
            return false;
        }
        if !self.has_newer_binary() {
            return false;
        }
        let session_id = self.reload_handoff_session_id();
        self.append_reload_message(
            "Server reloaded onto a newer build; reloading client binary to match...",
        );
        self.save_input_for_reload(&session_id);
        self.reload_requested = Some(session_id);
        self.should_quit = true;
        true
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
                self.set_status_notice(format!("mcp: {} connected", successes));
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

            for item in jcode_tui_messages::display_messages_from_rendered_messages(
                crate::session::render_messages(&session),
            ) {
                if item.role == "user" {
                    user_turns += 1;
                } else if item.role == "assistant" {
                    assistant_turns += 1;
                }
                total_chars += item.content.len();

                self.push_display_message(item);
            }

            // Don't restore provider_session_id - Claude sessions don't persist across
            // process restarts. The messages are restored, so Claude will get full context.
            self.provider_session_id = None;
            self.session = session;
            crate::memory::sync_injected_memories(
                &self.session.id,
                &self.session.injected_memory_ids(),
            );
            // Clear the saved provider_session_id since it's no longer valid
            self.session.provider_session_id = None;
            let mut restored_model = false;
            if let Some(model) = self.session.model.clone() {
                let model_request =
                    crate::provider::MultiProvider::model_switch_request_for_session_route(
                        &model,
                        self.session.provider_key.as_deref(),
                        self.session.route_api_method.as_deref(),
                    );
                if let Err(e) = crate::provider::set_model_with_auth_refresh(
                    self.provider.as_ref(),
                    &model_request,
                ) {
                    self.push_display_message(DisplayMessage {
                        role: "system".to_string(),
                        content: format!(
                            "⚠ Failed to restore model '{}' via '{}': {}",
                            model, model_request, e
                        ),
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
            self.set_side_panel_snapshot(
                crate::side_panel::snapshot_for_session(session_id).unwrap_or_default(),
            );
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

            let has_reload_ctx = ReloadContext::peek_for_session(session_id)
                .ok()
                .flatten()
                .is_some();
            let message = format!("Reload complete - continuing.{}", stats);

            // Add success message with stats (only if there's actual content or a reload happened)
            if total_turns > 0 || has_reload_ctx {
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
            let background_task_note = super::reload_persisted_background_tasks_note(session_id);
            let directive = ReloadContext::recovery_directive(
                reload_ctx.as_ref(),
                self.was_interrupted_by_reload(),
                &background_task_note,
                Some(total_turns),
            );
            if let Some(directive) = directive {
                let detail = if reload_ctx.is_some() {
                    "restored initiating session after reload"
                } else {
                    "restored interrupted non-initiator session after reload"
                };

                crate::logging::info(&format!(
                    "Queuing reload continuation message ({} chars)",
                    directive.continuation_message.len()
                ));
                ReloadContext::log_recovery_outcome("local_restore", session_id, "resumed", detail);
                self.hidden_queued_system_messages
                    .push(directive.continuation_message);
                // Trigger processing so the queued message gets sent to the LLM.
                // Without this, the local event loop waits for user input since
                // process_queued_messages only runs inside process_turn_with_input.
                self.is_processing = true;
                self.status = ProcessingStatus::Sending;
                self.processing_started = Some(Instant::now());
                self.pending_turn = true;
            } else {
                ReloadContext::log_recovery_outcome(
                    "local_restore",
                    session_id,
                    "skipped",
                    "restored session did not require reload continuation",
                );
            }
        } else {
            crate::logging::error(&format!("Failed to restore session: {}", session_id));

            // Check if this was a reload that failed - inject failure message if so
            if let Ok(Some(ctx)) = ReloadContext::load_for_session(session_id) {
                ReloadContext::log_recovery_outcome(
                    "local_restore",
                    session_id,
                    "failed",
                    "reload context existed but session restore failed",
                );
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
    /// 1. Last message is a User ToolResult containing reload interruption text,
    ///    including the non-error self-dev reload handoff marker
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
                    content == "Reload initiated. Process restarting..."
                        || (is_error.unwrap_or(false)
                            && (content.contains("interrupted by server reload")
                                || content.contains("Skipped - server reloading")))
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
    if trimmed == "/update-sim" || trimmed.starts_with("/update-sim ") {
        let stage = trimmed.strip_prefix("/update-sim").unwrap_or("").trim();
        let simulated_version = "v99.0.0-simulated";
        let status = match stage {
            "" => {
                app.restart_update_simulator();
                app.push_display_message(DisplayMessage::system(
                    "Update simulator started. It will automatically play the complete receive → download → install → restart experience. Press Alt+_ anytime to replay. Nothing real will be changed."
                        .to_string(),
                ));
                return true;
            }
            "available" => crate::bus::UpdateStatus::Available {
                current: jcode_build_meta::version().to_string(),
                latest: simulated_version.to_string(),
            },
            "download" | "downloading" => crate::bus::UpdateStatus::Downloading {
                version: simulated_version.to_string(),
                downloaded: 37 * 1_048_576,
                total: Some(100 * 1_048_576),
            },
            "install" | "installing" => crate::bus::UpdateStatus::Installing {
                version: simulated_version.to_string(),
            },
            "done" | "installed" => crate::bus::UpdateStatus::Installed {
                version: simulated_version.to_string(),
            },
            "error" => crate::bus::UpdateStatus::Error(
                "Simulated network interruption (no real update was attempted)".to_string(),
            ),
            "reset" | "off" => crate::bus::UpdateStatus::UpToDate,
            _ => {
                app.push_display_message(DisplayMessage::system(
                    "Usage: `/update-sim [available|download|install|done|error|reset]`. This preview never downloads, installs, or restarts anything."
                        .to_string(),
                ));
                return true;
            }
        };
        app.handle_update_status(status);
        app.push_display_message(DisplayMessage::system(format!(
            "Update simulator: **{}**. Try `/update-sim download`, then `install`, `done`, or `error`. Use `reset` to clear it. Nothing real was changed.",
            if stage.is_empty() { "available" } else { stage }
        )));
        return true;
    }

    if trimmed == "/onboarding-sim"
        || trimmed == "/onboarding-sim on"
        || trimmed == "/onboarding-sim off"
        || trimmed == "/onboarding-sim status"
    {
        let mode = trimmed.strip_prefix("/onboarding-sim").unwrap_or("").trim();
        match mode {
            "status" => {
                let status = if app.onboarding_sim_active() {
                    "on"
                } else {
                    "off"
                };
                app.push_display_message(DisplayMessage::system(format!(
                    "Onboarding simulator is {status}. Alt+5 resets and opens it; Cmd+5 toggles it; `/onboarding-sim on` / `off` also work. While active: Tab/→ next screen, Shift+Tab/← previous, h/l preview the highlight, Esc exits."
                )));
            }
            "" | "on" => {
                app.start_onboarding_simulator();
                app.push_display_message(DisplayMessage::system(
                    "Onboarding simulator started. Step screens with Tab/→, press Alt+5 to reset to the first screen, or Cmd+5 to toggle. On the import screen Up/Down move the checkbox cursor; h/l preview the highlight; Esc exits. Nothing real is logged in or imported.".to_string(),
                ));
            }
            "off" => {
                app.stop_onboarding_simulator();
                app.push_display_message(DisplayMessage::system(
                    "Onboarding simulator stopped.".to_string(),
                ));
            }
            _ => unreachable!("guarded by command matcher"),
        }
        return true;
    }

    if trimmed.starts_with("/onboarding-sim ") {
        app.push_display_message(DisplayMessage::system(
            "Usage: `/onboarding-sim`, `/onboarding-sim on`, `/onboarding-sim off`, or `/onboarding-sim status`. (Alt+5 resets and opens it; Cmd+5 toggles it.)".to_string(),
        ));
        return true;
    }

    if trimmed == "/onboarding-preview"
        || trimmed == "/onboarding-preview on"
        || trimmed == "/onboarding-preview off"
        || trimmed == "/onboarding-preview status"
    {
        let mode = trimmed
            .strip_prefix("/onboarding-preview")
            .unwrap_or("")
            .trim();
        if mode == "status" {
            let status = if app.onboarding_preview_mode {
                "on"
            } else {
                "off"
            };
            app.push_display_message(DisplayMessage::system(format!(
                "Onboarding preview is {}. Use /onboarding-preview on to mimic the first-run empty screen, or /onboarding-preview off to return to this session.",
                status
            )));
            return true;
        }

        if app.is_processing {
            app.push_display_message(DisplayMessage::system(
                "Onboarding preview is only available while idle.".to_string(),
            ));
            app.set_status_notice("Onboarding preview unavailable while busy");
            return true;
        }

        let enable = match mode {
            "" | "on" => true,
            "off" => false,
            _ => unreachable!("guarded by command matcher"),
        };
        app.onboarding_preview_mode = enable;
        if enable {
            app.input.clear();
            app.cursor_pos = 0;
            app.clear_input_undo_history();
            app.follow_chat_bottom();
            app.force_full_redraw = true;
            app.set_status_notice("Onboarding preview: on");
        } else {
            app.force_full_redraw = true;
            app.set_status_notice("Onboarding preview: off");
        }
        return true;
    }

    if trimmed.starts_with("/onboarding-preview ") {
        app.push_display_message(DisplayMessage::system(
            "Usage: `/onboarding-preview`, `/onboarding-preview on`, `/onboarding-preview off`, or `/onboarding-preview status`.".to_string(),
        ));
        return true;
    }

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
        app.start_background_client_rebuild(app.session.id.clone());
        return true;
    }

    if trimmed == "/update" {
        app.start_background_client_update(app.session.id.clone());
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
                "Premium mode: {}\nEnv JCODE_COPILOT_PREMIUM: {}",
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
            app.set_status_notice(format!("Premium: {}", label));
            app.push_display_message(DisplayMessage::system(format!(
                "Premium mode: {}. Toggle off with /z. (saved to config)",
                label,
            )));
        }
        return true;
    }

    false
}
