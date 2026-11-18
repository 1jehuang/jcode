//! Remote startup and retry scheduling.

use super::*;

impl App {
    pub fn new_for_remote_with_options(resume_session: Option<String>, fresh_spawn: bool) -> Self {
        let new_start = Instant::now();
        let mut lap = Instant::now();
        let mut laps: Vec<(&'static str, u128)> = Vec::new();
        let mut mark = |name: &'static str, laps: &mut Vec<(&'static str, u128)>| {
            laps.push((name, lap.elapsed().as_millis()));
            lap = Instant::now();
        };
        // Kick the background git probe first so its ~20ms of subprocess work
        // overlaps with the rest of client setup. `run_remote` gives it a short
        // bounded wait before the first paint.
        let _ = super::super::helpers::gather_git_info();
        mark("git_kick", &mut laps);
        let provider: Arc<dyn Provider> =
            Arc::new(InertRuntimeProvider::new(AppRuntimeMode::RemoteClient));
        let registry = Registry::empty();
        let session = resume_session
            .as_ref()
            .filter(|_| !crate::tui::is_ssh_remote())
            .and_then(|session_id| Session::load_startup_stub(session_id).ok())
            .unwrap_or_else(|| Session::create(None, None));
        mark("stub", &mut laps);
        let mut app = Self::new_minimal_with_session(provider, registry, session);
        mark("minimal", &mut laps);
        app.is_remote = true;
        app.runtime_mode = AppRuntimeMode::RemoteClient;
        app.remote_startup_phase = Some(super::super::RemoteStartupPhase::Connecting);
        app.remote_startup_phase_started = Some(Instant::now());

        let handoff_preview_session = std::env::var("JCODE_CLOUD_HANDOFF_PREVIEW_SESSION").ok();
        crate::env::remove_var("JCODE_CLOUD_HANDOFF_PREVIEW_SESSION");
        if let Some(host) = crate::tui::ssh_remote_host() {
            // An explicit /cloud handoff has a known local source transcript.
            // Render it before the first paint, while still waiting for authoritative
            // server History. Never infer this from a remote session ID alone.
            if let Some(session_id) = resume_session.as_deref()
                && handoff_preview_session.as_deref() == Some(session_id)
            {
                app.restore_remote_startup_history(session_id);
                app.remote_provider_model = app.session.model.clone();
            }
            // The server supplies history, credentials, models and project state.
            // Local reload files can belong to an unrelated session with the same id.
            app.onboarding_startup_checked = true;
            app.auto_server_reload = false;
            app.session.working_dir = None;
            app.resume_session_id = resume_session;
            app.set_status_notice(format!("SSH: {host} (remote server)"));
            // `/cloud` hands a mid-task session over and asks the new runtime
            // to keep going without the user retyping anything. One-shot.
            if let Ok(message) = std::env::var("JCODE_CLOUD_CONTINUE_MESSAGE") {
                crate::env::remove_var("JCODE_CLOUD_CONTINUE_MESSAGE");
                if !message.trim().is_empty() {
                    app.hidden_queued_system_messages.push(message);
                    app.set_status_notice(format!("Continuing on {host}"));
                }
            }
            return app;
        }

        // Minimal local clients start with empty skill registries. Load global
        // metadata once so autocomplete works before the first History event.
        // SSH clients above must use only the remote server's skill metadata.
        app.refresh_skills_snapshot();
        mark("skills", &mut laps);

        let reload_fast_start = std::env::var("JCODE_RELOAD_FAST_START")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        // One-shot handoff flag. A later ordinary resume in the same process
        // must retain the existing eager local-history behavior.
        crate::env::remove_var("JCODE_RELOAD_FAST_START");

        // Load session to get canary status (for "client self-dev" badge)
        if let Some(ref session_id) = resume_session {
            if reload_fast_start {
                crate::logging::info(&format!(
                    "Remote reload fast start: rendering persisted transcript for {} in background",
                    session_id
                ));
                // The server can take seconds to answer History while dozens
                // of clients reconnect after a reload. Parse and render the
                // local copy off the UI thread and show it as soon as it is
                // ready; server History still replaces it when it arrives.
                let (tx, rx) = std::sync::mpsc::channel();
                let (base_tx, base_rx) = std::sync::mpsc::channel();
                let id = session_id.clone();
                let spawned = std::thread::Builder::new()
                    .name("jcode-startup-transcript".to_string())
                    .spawn(move || {
                        let start = Instant::now();
                        let Ok(session) = Session::load_for_remote_startup(&id) else {
                            return;
                        };
                        let (rendered, images) =
                            crate::session::render_messages_and_images(&session);
                        drop(session);
                        // Same shape as server History messages, so the
                        // server can verify this prefix and send only the
                        // tail (delta History).
                        let mut fingerprinter = crate::protocol::HistoryFingerprinter::default();
                        for message in &rendered {
                            fingerprinter.push(crate::protocol::HistoryMessageRef {
                                role: &message.role,
                                content: &message.content,
                                tool_calls: &message.tool_calls,
                                tool_data: message.tool_data.as_ref(),
                            });
                        }
                        if fingerprinter.count() > 0 {
                            let mut base = fingerprinter.base();
                            base.images_fingerprint =
                                Some(crate::protocol::history_images_fingerprint(&images));
                            let _ = base_tx.send((id.clone(), base));
                        }
                        drop(base_tx);
                        let display =
                            jcode_tui_messages::display_messages_from_rendered_messages(rendered);
                        crate::logging::info(&format!(
                            "[TIMING] startup_transcript_background: session={}, messages={}, total={}ms",
                            id,
                            display.len(),
                            start.elapsed().as_millis()
                        ));
                        let _ = tx.send((id, display, images));
                    });
                if spawned.is_ok() {
                    app.pending_startup_transcript = Some(rx);
                    if !crate::tui::is_ssh_remote() {
                        app.pending_history_base = Some(base_rx);
                    }
                }
                mark("transcript_spawn", &mut laps);
            } else {
                app.restore_remote_startup_history(session_id);
                mark("transcript_sync", &mut laps);
            }
            if fresh_spawn && !reload_fast_start {
                crate::logging::info(&format!(
                    "Remote startup fresh-spawn path: restored persisted transcript for {} while awaiting server history",
                    session_id
                ));
            }
            let restored_input = Self::restore_input_for_reload(session_id);
            mark("input_read", &mut laps);
            if let Some(restored) = restored_input {
                app.apply_restored_reload_input(restored);
            }
            mark("input_apply", &mut laps);
        }

        app.resume_session_id = resume_session;
        // Paint persisted todos with the first frame, not a throttled tick after
        // the server has finished loading the resumed conversation.
        app.refresh_pinned_todos_now();
        // After `resume_session_id`: the hint needs to know whether this is a
        // new session (configured effort applies) or a resumed one.
        app.apply_remote_header_hint();
        mark("header_hint", &mut laps);
        crate::logging::info(&format!(
            "[TIMING] app_new_for_remote: total={}ms, {}",
            new_start.elapsed().as_millis(),
            laps.iter()
                .map(|(name, ms)| format!("{name}={ms}ms"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
        app
    }
}

impl App {
    pub(in super::super) fn schedule_pending_remote_retry_with_limit(
        &mut self,
        reason: &str,
        max_attempts: u8,
    ) -> bool {
        let Some(pending) = self.rate_limit_pending_message.as_mut() else {
            self.log_recovery_event(
                crate::logging::LogLevel::Warn,
                "RECOVERY_RETRY_SKIPPED",
                "no_pending_turn",
            );
            return false;
        };
        if !pending.auto_retry {
            self.log_recovery_event(
                crate::logging::LogLevel::Info,
                "RECOVERY_RETRY_SKIPPED",
                "auto_retry_disabled",
            );
            return false;
        }
        let outcome = {
            let current_attempts = pending.retry_attempts;
            if current_attempts >= max_attempts {
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
                self.log_recovery_event(
                    crate::logging::LogLevel::Warn,
                    "RECOVERY_RETRY_STOPPED",
                    "retry_budget_exhausted",
                );
                self.restore_terminal_remote_retry_input();
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
                self.log_recovery_event(
                    crate::logging::LogLevel::Info,
                    "RECOVERY_RETRY_ARMED",
                    "transient_failure",
                );
                let content = format!(
                    "⚡ Connection lost - retrying (attempt {}/{}, in {}s) - {}",
                    retry_attempts,
                    max_attempts,
                    backoff_secs,
                    reason
                        .trim()
                        .trim_start_matches("⚡ ")
                        .trim_start_matches("Connection lost")
                        .trim_start_matches('(')
                        .trim_end_matches('.')
                        .trim()
                );
                if let Some(idx) = self.display_messages.iter().rposition(|message| {
                    message.role == "system"
                        && (message.title.as_deref() == Some("Connection")
                            || message
                                .content
                                .starts_with("⚡ Server reload in progress - waiting for handoff")
                            || message.content.starts_with("⚡ Connection lost"))
                }) {
                    self.replace_display_message_title_and_content(
                        idx,
                        Some("Connection".to_string()),
                        content,
                    );
                } else {
                    self.push_display_message(DisplayMessage {
                        role: "system".to_string(),
                        content,
                        tool_calls: Vec::new(),
                        duration_secs: None,
                        title: Some("Connection".to_string()),
                        tool_data: None,
                    });
                }
                true
            }
        }
    }
}
