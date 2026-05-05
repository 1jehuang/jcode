use super::{
    App, DisplayMessage, OvernightAutoPokeFingerprint, OvernightAutoPokeState, ProcessingStatus,
};
use crate::message::{ContentBlock, Message, Role};
use crate::overnight::{OvernightCommand, OvernightRunStatus, OvernightStartOptions};
use crate::provider::Provider;
use chrono::Utc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const OVERNIGHT_STALL_LIMIT: u8 = 3;
const OVERNIGHT_ERROR_LIMIT: u8 = 2;
const OVERNIGHT_MAX_POKES: u16 = 48;

pub(super) fn handle_overnight_command(app: &mut App, trimmed: &str) -> bool {
    let Some(command) = crate::overnight::parse_overnight_command(trimmed) else {
        return false;
    };

    match command {
        Ok(OvernightCommand::Help) => show_overnight_help(app),
        Ok(OvernightCommand::Status) => show_overnight_status(app),
        Ok(OvernightCommand::Log) => show_overnight_log(app),
        Ok(OvernightCommand::Review) => open_overnight_review(app),
        Ok(OvernightCommand::Cancel) => cancel_overnight(app),
        Ok(OvernightCommand::Start { duration, mission }) => {
            let working_dir = app
                .session
                .working_dir
                .as_deref()
                .map(std::path::PathBuf::from)
                .filter(|path| path.is_dir())
                .or_else(|| std::env::current_dir().ok());
            let provider = overnight_provider_for_app(app);
            let visible_provider = provider.clone();
            let options = OvernightStartOptions {
                duration,
                mission,
                parent_session: app.session.clone(),
                provider,
                registry: app.registry.clone(),
                working_dir,
                use_current_session: true,
            };
            match crate::overnight::start_overnight_run(options) {
                Ok(launch) => {
                    let manifest = launch.manifest;
                    app.enable_overnight_auto_poke(&manifest);
                    app.upsert_overnight_display_card(&manifest);
                    if let Some(prompt) = launch.initial_prompt {
                        if !app.is_remote {
                            app.provider = visible_provider;
                        }
                        start_visible_overnight_turn(app, prompt);
                        app.set_status_notice("Overnight started in current session");
                    } else {
                        app.set_status_notice("Overnight started");
                    }
                }
                Err(error) => app.push_display_message(DisplayMessage::error(format!(
                    "Failed to start overnight run: {}",
                    crate::util::format_error_chain(&error)
                ))),
            }
        }
        Err(error) => app.push_display_message(DisplayMessage::error(error)),
    }

    true
}

fn start_visible_overnight_turn(app: &mut App, content: String) {
    if app.is_remote {
        app.commit_pending_streaming_assistant_message();
        app.queued_messages.push(content);
        app.set_status_notice("Overnight queued in current remote session");
        return;
    }

    app.commit_pending_streaming_assistant_message();
    app.add_provider_message(Message::user(&content));
    app.session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: content,
            cache_control: None,
        }],
    );
    if let Err(e) = app.session.save() {
        crate::logging::warn(&format!("Failed to save session: {}", e));
    }

    app.is_processing = true;
    app.status = ProcessingStatus::Sending;
    app.clear_streaming_render_state();
    app.stream_buffer.clear();
    app.thought_line_inserted = false;
    app.thinking_prefix_emitted = false;
    app.thinking_buffer.clear();
    app.streaming_tool_calls.clear();
    app.batch_progress = None;
    app.streaming_input_tokens = 0;
    app.streaming_output_tokens = 0;
    app.streaming_cache_read_tokens = None;
    app.streaming_cache_creation_tokens = None;
    app.upstream_provider = None;
    app.status_detail = None;
    app.streaming_tps_start = None;
    app.streaming_tps_elapsed = Duration::ZERO;
    app.streaming_tps_collect_output = false;
    app.streaming_total_output_tokens = 0;
    app.streaming_tps_observed_output_tokens = 0;
    app.streaming_tps_observed_elapsed = Duration::ZERO;
    app.processing_started = Some(Instant::now());
    app.visible_turn_started = Some(Instant::now());
    app.pending_turn = true;
}

fn show_overnight_help(app: &mut App) {
    app.push_display_message(DisplayMessage::system(
        "`/overnight <hours>[h|m] [mission]`\nStart one visible overnight coordinator with guarded auto-poke follow-ups until the target wake/wrap time. The coordinator prioritizes verifiable, low-risk work, maintains logs, and updates a review HTML page.\n\n`/overnight status`\nShow the latest overnight run status.\n\n`/overnight log`\nShow recent overnight events.\n\n`/overnight review`\nOpen the generated review page.\n\n`/overnight cancel`\nRequest cancellation after the current coordinator turn and stop overnight auto-poke.".to_string(),
    ));
}

fn overnight_provider_for_app(app: &mut App) -> Arc<dyn Provider> {
    if !app.is_remote {
        return app.provider.fork();
    }

    // Remote-attached TUIs intentionally use NullProvider because normal turns
    // execute in the remote backend process. `/overnight` is supervised by the
    // launching TUI process, so it needs a real local provider instead of the
    // remote placeholder. Restore the displayed session model when possible and
    // otherwise fall back to the local default provider.
    let provider: Arc<dyn Provider> = Arc::new(crate::provider::MultiProvider::new_fast());
    if let Some(model) = app
        .session
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty() && *model != "unknown")
        && let Err(error) = provider.set_model(model)
    {
        app.push_display_message(DisplayMessage::system(format!(
            "Overnight could not restore remote model `{}` locally: {}. Using local default provider `{}` instead.",
            model,
            error,
            provider.name()
        )));
    }
    provider
}

fn show_overnight_status(app: &mut App) {
    match crate::overnight::latest_manifest() {
        Ok(Some(manifest)) => {
            if !app.upsert_overnight_display_card(&manifest) {
                app.push_display_message(DisplayMessage::system(
                    crate::overnight::format_status_markdown(&manifest),
                ));
            }
            app.set_status_notice("Overnight status");
        }
        Ok(None) => app.push_display_message(DisplayMessage::system(
            "No overnight runs found.".to_string(),
        )),
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to read overnight status: {}",
            crate::util::format_error_chain(&error)
        ))),
    }
}

fn show_overnight_log(app: &mut App) {
    match crate::overnight::latest_manifest() {
        Ok(Some(manifest)) => {
            app.push_display_message(DisplayMessage::system(
                crate::overnight::format_log_markdown(&manifest, 30),
            ));
            app.set_status_notice("Overnight log");
        }
        Ok(None) => app.push_display_message(DisplayMessage::system(
            "No overnight runs found.".to_string(),
        )),
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to read overnight log: {}",
            crate::util::format_error_chain(&error)
        ))),
    }
}

fn open_overnight_review(app: &mut App) {
    match crate::overnight::latest_manifest() {
        Ok(Some(manifest)) => {
            if let Err(error) = crate::overnight::render_review_html(&manifest) {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to refresh overnight review page: {}",
                    crate::util::format_error_chain(&error)
                )));
                return;
            }
            match open::that_detached(&manifest.review_path) {
                Ok(()) => {
                    app.push_display_message(DisplayMessage::system(format!(
                        "Opened overnight review page: `{}`",
                        manifest.review_path.display()
                    )));
                    app.set_status_notice("Overnight review opened");
                }
                Err(error) => app.push_display_message(DisplayMessage::error(format!(
                    "Failed to open overnight review page `{}`: {}",
                    manifest.review_path.display(),
                    error
                ))),
            }
        }
        Ok(None) => app.push_display_message(DisplayMessage::system(
            "No overnight runs found.".to_string(),
        )),
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to read overnight review: {}",
            crate::util::format_error_chain(&error)
        ))),
    }
}

fn cancel_overnight(app: &mut App) {
    match crate::overnight::cancel_latest_run() {
        Ok(manifest) => {
            app.overnight_auto_poke = None;
            if !app.upsert_overnight_display_card(&manifest) {
                app.push_display_message(DisplayMessage::system(format!(
                    "Cancellation requested for overnight run `{}`. The coordinator will stop after the current turn reaches a safe boundary.",
                    manifest.run_id,
                )));
            }
            app.set_status_notice("Overnight cancel requested");
        }
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to cancel overnight run: {}",
            crate::util::format_error_chain(&error)
        ))),
    }
}

impl App {
    pub(super) fn cancel_overnight_for_interrupt(&mut self) -> bool {
        if self.overnight_auto_poke.is_none()
            && !self
                .queued_messages
                .iter()
                .any(|message| is_overnight_auto_poke_message(message))
        {
            return false;
        }

        self.overnight_auto_poke = None;
        let before = self.queued_messages.len();
        self.queued_messages
            .retain(|message| !is_overnight_auto_poke_message(message));
        if before != self.queued_messages.len() && !self.has_queued_followups() {
            self.pending_queued_dispatch = false;
        }

        match crate::overnight::cancel_latest_run() {
            Ok(manifest) => {
                let _ = self.upsert_overnight_display_card(&manifest);
                self.push_display_message(DisplayMessage::system(format!(
                    "🌙 Overnight run `{}` cancelled by interrupt.",
                    manifest.run_id
                )));
            }
            Err(error) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Interrupted, but failed to cancel overnight run: {}",
                    crate::util::format_error_chain(&error)
                )));
            }
        }
        true
    }

    pub(super) fn enable_overnight_auto_poke(
        &mut self,
        manifest: &crate::overnight::OvernightManifest,
    ) {
        let fingerprint = overnight_fingerprint_for_app(self, manifest);
        self.overnight_auto_poke = Some(OvernightAutoPokeState {
            run_id: manifest.run_id.clone(),
            last_fingerprint: fingerprint,
            stalled_turns: 0,
            error_turns: 0,
            total_pokes_sent: 0,
            diagnostic_sent: false,
            morning_report_poked: false,
            final_wrap_poked: false,
        });
    }

    pub(super) fn stop_overnight_auto_poke_for_non_retryable_error(&mut self, error: &str) -> bool {
        if self.overnight_auto_poke.is_none()
            || !super::commands::is_non_retryable_auto_poke_error(error)
        {
            return false;
        }
        self.overnight_auto_poke = None;
        self.push_display_message(DisplayMessage::system(
            "🛑 Overnight auto-poke stopped because the last request failed with a non-retryable error. Fix the request/session, then run `/overnight status` and continue manually if appropriate.".to_string(),
        ));
        self.set_status_notice("Overnight poke stopped: non-retryable error");
        true
    }

    pub(super) fn schedule_overnight_poke_followup_if_needed(&mut self) -> bool {
        if self.overnight_auto_poke.is_none()
            || self.pending_queued_dispatch
            || self.pending_turn
            || self.has_queued_followups()
        {
            return false;
        }

        let Some(mut state) = self.overnight_auto_poke.take() else {
            return false;
        };
        let manifest = match crate::overnight::latest_manifest() {
            Ok(Some(manifest)) if manifest.run_id == state.run_id => manifest,
            _ => return false,
        };

        if !matches!(
            manifest.status,
            OvernightRunStatus::Running | OvernightRunStatus::CancelRequested
        ) {
            self.push_display_message(DisplayMessage::system(format!(
                "✅ Overnight auto-poke finished: run `{}` is {}.",
                manifest.run_id,
                overnight_status_label(&manifest.status)
            )));
            self.set_status_notice("Overnight auto-poke finished");
            return false;
        }
        if matches!(manifest.status, OvernightRunStatus::CancelRequested) {
            self.push_display_message(DisplayMessage::system(
                "🛑 Overnight auto-poke stopped: cancellation requested.".to_string(),
            ));
            self.set_status_notice("Overnight auto-poke stopped");
            return false;
        }

        let fingerprint = overnight_fingerprint_for_app(self, &manifest);
        let progressed = fingerprint != state.last_fingerprint;
        if progressed {
            state.stalled_turns = 0;
        } else {
            state.stalled_turns = state.stalled_turns.saturating_add(1);
        }
        state.last_fingerprint = fingerprint;

        if state.stalled_turns >= OVERNIGHT_STALL_LIMIT {
            self.push_display_message(DisplayMessage::system(format!(
                "🛑 Overnight auto-poke stopped after {} consecutive no-progress turns. Review `{}` before continuing manually.",
                state.stalled_turns,
                manifest.review_path.display()
            )));
            self.set_status_notice("Overnight stopped: no progress");
            return false;
        }
        if state.error_turns >= OVERNIGHT_ERROR_LIMIT {
            self.push_display_message(DisplayMessage::system(
                "🛑 Overnight auto-poke stopped after repeated turn errors.".to_string(),
            ));
            self.set_status_notice("Overnight stopped: errors");
            return false;
        }
        if state.total_pokes_sent >= overnight_poke_budget(&manifest) {
            self.push_display_message(DisplayMessage::system(format!(
                "🛑 Overnight auto-poke stopped after reaching its safety budget of {} follow-up turns.",
                state.total_pokes_sent
            )));
            self.set_status_notice("Overnight stopped: poke budget");
            return false;
        }

        let phase = overnight_poke_phase(&manifest, &state);
        if matches!(phase, OvernightPokePhase::FinalDone) {
            self.push_display_message(DisplayMessage::system(format!(
                "✅ Overnight auto-poke finished after final wrap request. Review `{}`.",
                manifest.review_path.display()
            )));
            self.set_status_notice("Overnight auto-poke complete");
            return false;
        }

        if matches!(phase, OvernightPokePhase::MorningReport) {
            state.morning_report_poked = true;
        }
        if matches!(phase, OvernightPokePhase::FinalWrap) {
            state.final_wrap_poked = true;
        }
        if matches!(phase, OvernightPokePhase::Diagnostic) {
            state.diagnostic_sent = true;
        }
        state.total_pokes_sent = state.total_pokes_sent.saturating_add(1);

        let prompt = build_overnight_poke_message(&manifest, phase, state.stalled_turns);
        self.push_display_message(DisplayMessage::system(format!(
            "🌙 Overnight auto-poking: {}. `/overnight cancel` to stop.",
            overnight_phase_label(phase)
        )));
        self.queued_messages.push(prompt);
        self.pending_queued_dispatch = true;
        self.overnight_auto_poke = Some(state);
        true
    }
}

fn is_overnight_auto_poke_message(message: &str) -> bool {
    message.starts_with("Overnight auto-poke for run `")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OvernightPokePhase {
    Continue,
    Diagnostic,
    Handoff,
    MorningReport,
    PostWake,
    FinalWrap,
    FinalDone,
}

fn overnight_poke_phase(
    manifest: &crate::overnight::OvernightManifest,
    state: &OvernightAutoPokeState,
) -> OvernightPokePhase {
    let now = Utc::now();
    if state.stalled_turns > 0 && !state.diagnostic_sent {
        return OvernightPokePhase::Diagnostic;
    }
    if now >= manifest.post_wake_grace_until {
        return if state.final_wrap_poked {
            OvernightPokePhase::FinalDone
        } else {
            OvernightPokePhase::FinalWrap
        };
    }
    if now >= manifest.target_wake_at {
        return if !state.morning_report_poked && manifest.morning_report_posted_at.is_none() {
            OvernightPokePhase::MorningReport
        } else {
            OvernightPokePhase::PostWake
        };
    }
    if now >= manifest.handoff_ready_at {
        return OvernightPokePhase::Handoff;
    }
    OvernightPokePhase::Continue
}

fn overnight_phase_label(phase: OvernightPokePhase) -> &'static str {
    match phase {
        OvernightPokePhase::Continue => "continuation",
        OvernightPokePhase::Diagnostic => "diagnostic no-progress check",
        OvernightPokePhase::Handoff => "handoff-ready reminder",
        OvernightPokePhase::MorningReport => "morning report",
        OvernightPokePhase::PostWake => "post-wake continuation",
        OvernightPokePhase::FinalWrap => "final wrap-up",
        OvernightPokePhase::FinalDone => "complete",
    }
}

fn overnight_status_label(status: &OvernightRunStatus) -> &'static str {
    match status {
        OvernightRunStatus::Running => "running",
        OvernightRunStatus::CancelRequested => "cancel requested",
        OvernightRunStatus::Completed => "completed",
        OvernightRunStatus::Failed => "failed",
    }
}

fn build_overnight_poke_message(
    manifest: &crate::overnight::OvernightManifest,
    phase: OvernightPokePhase,
    stalled_turns: u8,
) -> String {
    let prefix = format!(
        "Overnight auto-poke for run `{}`. First inspect manifest `{}`, review notes `{}`, task cards `{}`, validation `{}`, and git/todo state. Keep artifacts current before stopping. ",
        manifest.run_id,
        manifest.run_dir.join("manifest.json").display(),
        manifest.review_notes_path.display(),
        manifest.task_cards_dir.display(),
        manifest.validation_dir.display(),
    );
    let body = match phase {
        OvernightPokePhase::Diagnostic => format!(
            "The auto-poke guard detected {} no-progress turn(s). Do not continue blindly. Diagnose why progress stalled: blocked task, missing credentials, failing tool, context/model issue, or unclear next step. Either recover with one small verifiable task, or mark the run/task blocked and stop.",
            stalled_turns
        ),
        OvernightPokePhase::Handoff => "Enter handoff-ready mode. Update review notes, task cards, validation evidence, dirty repo state, risks, skipped work, and next steps. Avoid starting large or risky new work.".to_string(),
        OvernightPokePhase::MorningReport => "Target wake time reached. Post the morning report now before starting any new work. Include completed work, current state, validation, files changed, risks, and next steps. Set `morning_report_posted_at` in the manifest when done.".to_string(),
        OvernightPokePhase::PostWake => "Post-wake continuation. Continue only bounded, safe, verifiable work that is in progress or clearly high-value. Do not start broad/risky new changes. Keep artifacts current.".to_string(),
        OvernightPokePhase::FinalWrap | OvernightPokePhase::FinalDone => "Final wrap-up. Stop starting new work. Finish immediate cleanup only, update review notes/task cards/review page with final evidence and risks, then mark the manifest completed.".to_string(),
        OvernightPokePhase::Continue => "Continue the overnight run. If the previous task is done, choose the next highest-confidence bounded task. If blocked, record why and switch to another useful task. Prove/reproduce before fixing, validate after, and update task cards/review notes.".to_string(),
    };
    format!("{}{}", prefix, body)
}

fn overnight_fingerprint_for_app(
    app: &App,
    manifest: &crate::overnight::OvernightManifest,
) -> OvernightAutoPokeFingerprint {
    let task_summary = crate::overnight::summarize_task_cards(manifest);
    OvernightAutoPokeFingerprint {
        run_id: manifest.run_id.clone(),
        status: overnight_status_label(&manifest.status).to_string(),
        last_activity_at: manifest.last_activity_at.to_rfc3339(),
        events_len: crate::overnight::read_events(manifest)
            .map(|events| events.len())
            .unwrap_or(0),
        task_total: task_summary.total,
        task_completed: task_summary.counts.completed,
        task_active: task_summary.counts.active,
        task_blocked: task_summary.counts.blocked,
        task_validated: task_summary.validated,
        session_message_count: app.session.messages.len(),
        review_notes_mtime: file_mtime_secs(&manifest.review_notes_path),
        validation_files: count_files(&manifest.validation_dir),
    }
}

fn overnight_poke_budget(manifest: &crate::overnight::OvernightManifest) -> u16 {
    let duration_hours = manifest
        .target_wake_at
        .signed_duration_since(manifest.started_at)
        .num_minutes()
        .max(1) as f32
        / 60.0;
    ((duration_hours.ceil() as u16).saturating_mul(4)).clamp(4, OVERNIGHT_MAX_POKES)
}

fn file_mtime_secs(path: &std::path::Path) -> Option<u64> {
    path.metadata()
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

fn count_files(path: &std::path::Path) -> usize {
    std::fs::read_dir(path)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;
    use std::path::PathBuf;

    fn test_manifest_with_times(
        started_at: chrono::DateTime<Utc>,
        target_wake_at: chrono::DateTime<Utc>,
        post_wake_grace_until: chrono::DateTime<Utc>,
    ) -> crate::overnight::OvernightManifest {
        crate::overnight::OvernightManifest {
            version: 1,
            run_id: "overnight_test".to_string(),
            parent_session_id: "parent".to_string(),
            coordinator_session_id: "session".to_string(),
            coordinator_session_name: "Session".to_string(),
            started_at,
            target_wake_at,
            handoff_ready_at: target_wake_at - ChronoDuration::minutes(30),
            post_wake_grace_until,
            morning_report_posted_at: None,
            completed_at: None,
            cancel_requested_at: None,
            status: OvernightRunStatus::Running,
            mission: Some("test".to_string()),
            working_dir: None,
            provider_name: "mock".to_string(),
            model: "mock-model".to_string(),
            max_agents_guidance: 2,
            process_id: 1,
            run_dir: PathBuf::from("/tmp/overnight_test"),
            events_path: PathBuf::from("/tmp/overnight_test/events.jsonl"),
            human_log_path: PathBuf::from("/tmp/overnight_test/run.log"),
            review_path: PathBuf::from("/tmp/overnight_test/review.html"),
            review_notes_path: PathBuf::from("/tmp/overnight_test/review-notes.md"),
            preflight_path: PathBuf::from("/tmp/overnight_test/preflight.json"),
            task_cards_dir: PathBuf::from("/tmp/overnight_test/task-cards"),
            issue_drafts_dir: PathBuf::from("/tmp/overnight_test/issue-drafts"),
            validation_dir: PathBuf::from("/tmp/overnight_test/validation"),
            last_activity_at: started_at,
        }
    }

    fn test_state(manifest: &crate::overnight::OvernightManifest) -> OvernightAutoPokeState {
        OvernightAutoPokeState {
            run_id: manifest.run_id.clone(),
            last_fingerprint: OvernightAutoPokeFingerprint {
                run_id: manifest.run_id.clone(),
                status: "running".to_string(),
                last_activity_at: manifest.last_activity_at.to_rfc3339(),
                events_len: 0,
                task_total: 0,
                task_completed: 0,
                task_active: 0,
                task_blocked: 0,
                task_validated: 0,
                session_message_count: 0,
                review_notes_mtime: None,
                validation_files: 0,
            },
            stalled_turns: 0,
            error_turns: 0,
            total_pokes_sent: 0,
            diagnostic_sent: false,
            morning_report_poked: false,
            final_wrap_poked: false,
        }
    }

    #[test]
    fn overnight_poke_phase_requests_morning_report_at_target_once() {
        let now = Utc::now();
        let manifest = test_manifest_with_times(
            now - ChronoDuration::hours(2),
            now - ChronoDuration::minutes(1),
            now + ChronoDuration::hours(1),
        );
        let mut state = test_state(&manifest);
        assert_eq!(
            overnight_poke_phase(&manifest, &state),
            OvernightPokePhase::MorningReport
        );
        state.morning_report_poked = true;
        assert_eq!(
            overnight_poke_phase(&manifest, &state),
            OvernightPokePhase::PostWake
        );
    }

    #[test]
    fn overnight_poke_phase_stops_after_final_wrap_requested() {
        let now = Utc::now();
        let manifest = test_manifest_with_times(
            now - ChronoDuration::hours(4),
            now - ChronoDuration::hours(3),
            now - ChronoDuration::minutes(1),
        );
        let mut state = test_state(&manifest);
        assert_eq!(
            overnight_poke_phase(&manifest, &state),
            OvernightPokePhase::FinalWrap
        );
        state.final_wrap_poked = true;
        assert_eq!(
            overnight_poke_phase(&manifest, &state),
            OvernightPokePhase::FinalDone
        );
    }

    #[test]
    fn overnight_poke_phase_sends_one_diagnostic_on_stall() {
        let now = Utc::now();
        let manifest = test_manifest_with_times(
            now,
            now + ChronoDuration::hours(2),
            now + ChronoDuration::hours(4),
        );
        let mut state = test_state(&manifest);
        state.stalled_turns = 1;
        assert_eq!(
            overnight_poke_phase(&manifest, &state),
            OvernightPokePhase::Diagnostic
        );
        state.diagnostic_sent = true;
        assert_eq!(
            overnight_poke_phase(&manifest, &state),
            OvernightPokePhase::Continue
        );
    }

    #[test]
    fn overnight_poke_budget_is_bounded_by_duration_and_cap() {
        let now = Utc::now();
        let short = test_manifest_with_times(
            now,
            now + ChronoDuration::minutes(30),
            now + ChronoDuration::hours(2),
        );
        assert_eq!(overnight_poke_budget(&short), 4);
        let long = test_manifest_with_times(
            now,
            now + ChronoDuration::hours(72),
            now + ChronoDuration::hours(74),
        );
        assert_eq!(overnight_poke_budget(&long), OVERNIGHT_MAX_POKES);
    }
}
