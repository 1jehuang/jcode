// Executed evidence for the UNCOMMITTED `ServerEvent::ModelChanged`
// announcement gate in crates/jcode-tui/src/tui/app/remote/server_events.rs.
//
// The gate captures `user_initiated_switch = app.remote_model_switch_in_flight`
// and then only announces ("✓ Switched to model: ..." + a "Model -> ..."
// status notice) when that flag was true. `remote_model_switch_in_flight` is
// false on TWO server paths, not one:
//
//   * the brand-new subscribe path  (client_session.rs:911, added by 48de3e706)
//   * the resume path               (client_session.rs:1754, pre-dating it)
//
// `handle_resume_session` sent `ModelChanged` BEFORE 48de3e706, so a resume was
// already announcing the route before the gate landed. These tests measure what
// a resume actually leaves the user with, whether the two frames can both fire
// for one resume, and whether the error branch survived the gate.
//
// Everything here is driven through `App::handle_server_event`, i.e. the same
// entry point the socket reader uses, so the assertions are about client state
// the user can observe (transcript, status notice, /cache context_limit,
// /model picker, deferred prompt) rather than about the handler's source text.

const GATE_MODEL: &str = "stealth/space-bunny-alpha@Stealth";
const GATE_PROVIDER: &str = "Stealth";
const GATE_WINDOW: u64 = 1_000_000;

fn gate_compaction_budget(app: &App) -> usize {
    app.registry
        .compaction()
        .try_read()
        .expect("compaction budget should be readable right after the event")
        .token_budget()
}

fn gate_transcript(app: &App) -> Vec<String> {
    app.display_messages()
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
}

fn gate_model_changed(id: u64, model: &str, context_window: Option<u64>) -> crate::protocol::ServerEvent {
    crate::protocol::ServerEvent::ModelChanged {
        id,
        model: model.to_string(),
        provider_name: Some(GATE_PROVIDER.to_string()),
        context_window,
        error: None,
        resolved_credential: None,
        reasoning_effort: Some("high".to_string()),
    }
}

fn gate_history_event(id: u64, session_id: &str) -> crate::protocol::ServerEvent {
    crate::protocol::ServerEvent::History {
        id,
        session_id: session_id.to_string(),
        messages: vec![],
        images: vec![],
        provider_name: Some(GATE_PROVIDER.to_string()),
        provider_model: Some(GATE_MODEL.to_string()),
        subagent_model: None,
        autoreview_enabled: None,
        autojudge_enabled: None,
        available_models: vec![],
        available_model_routes: vec![],
        mcp_servers: vec![],
        skills: vec![],
        total_tokens: None,
        token_usage_totals: None,
        all_sessions: vec![],
        client_count: None,
        is_canary: None,
        reload_recovery: None,
        server_version: None,
        server_name: None,
        server_icon: None,
        server_has_update: None,
        was_interrupted: None,
        connection_type: None,
        status_detail: None,
        upstream_provider: None,
        resolved_credential: None,
        reasoning_effort: Some("high".to_string()),
        service_tier: None,
        compaction_mode: crate::config::CompactionMode::Reactive,
        activity: None,
        side_panel: crate::side_panel::SidePanelSnapshot::default(),
        applets: Default::default(),
    }
}

/// A resume: `App::new_for_remote(Some(id))` followed by the frame order
/// `handle_resume_session` actually emits (History -> Done -> ModelChanged),
/// with the target-aware Subscribe's own ModelChanged folded in.
fn gate_resumed_app(session_id: &str) -> App {
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("resume gate probe".to_string()),
    );
    session.save().expect("save resume target session");
    App::new_for_remote(Some(session_id.to_string()))
}

// ---------------------------------------------------------------------------
// CASE 1 - after a RESUME, is the model still surfaced anywhere?
// ---------------------------------------------------------------------------

/// The gate silences the announcement on a resume. This asserts the model is
/// still reachable to the user through the three surfaces the gate does NOT
/// touch: the status-line provider, the `/model` picker, and the `/cache`
/// context limit (+ the compaction budget derived from it).
#[test]
fn resume_keeps_the_model_visible_after_the_notice_is_suppressed() {
    with_temp_jcode_home(|| {
        let session_id = "ses_gate_resume_01";
        let mut app = gate_resumed_app(session_id);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        // Precondition: a resume is NOT a user-initiated switch.
        assert!(
            !app.remote_model_switch_in_flight,
            "a resumed session must not open with a switch in flight"
        );
        assert!(
            !app.auth_catalog_refresh_pending,
            "a plain resume is not the post-login catalog refresh path"
        );
        assert_eq!(
            app.provider.context_window(),
            200_000,
            "precondition: the remote client's own provider is inert and reports the 200K default"
        );

        app.handle_server_event(
            crate::protocol::ServerEvent::SessionId {
                session_id: session_id.to_string(),
            },
            &mut remote,
        );
        app.handle_server_event(gate_history_event(1, session_id), &mut remote);
        app.handle_server_event(
            gate_model_changed(1, GATE_MODEL, Some(GATE_WINDOW)),
            &mut remote,
        );
        app.handle_server_event(crate::protocol::ServerEvent::Done { id: 1 }, &mut remote);

        // --- what the gate removed ---
        let announced = gate_transcript(&app);
        assert!(
            !announced.iter().any(|c| c.contains("Switched to model")),
            "gate: resume must not claim a switch. transcript: {announced:?}"
        );
        let notice = app.status_notice();
        assert!(
            !notice
                .as_deref()
                .is_some_and(|text| text.starts_with("Model \u{2192}")),
            "gate: resume must not set a 'Model -> ...' status notice. got {notice:?}"
        );

        // --- what survives the gate: is the model still visible? ---
        assert_eq!(
            app.remote_provider_model.as_deref(),
            Some(GATE_MODEL),
            "the route must still be applied to remote state"
        );
        assert_eq!(app.remote_provider_name.as_deref(), Some(GATE_PROVIDER));
        assert_eq!(app.remote_reasoning_effort.as_deref(), Some("high"));
        assert_eq!(
            app.context_limit, GATE_WINDOW,
            "the resumed session's real window must reach /cache; the 200K fallback is the bug \
             48de3e706 fixed"
        );
        assert_eq!(
            gate_compaction_budget(&app),
            GATE_WINDOW as usize,
            "the compaction trigger must use the same window"
        );
        assert_eq!(
            crate::tui::TuiState::provider_model(&app),
            GATE_MODEL,
            "the status line's provider model must still name the active route after a resume"
        );

        // /model must still open on the active route.
        app.open_model_picker();
        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("model picker should open after a resume");
        assert!(
            picker
                .entries
                .iter()
                .any(|entry| entry.name == GATE_MODEL),
            "/model must still list the active route. entries: {:?}",
            picker
                .entries
                .iter()
                .map(|e| e.name.clone())
                .collect::<Vec<_>>()
        );
    });
}

// ---------------------------------------------------------------------------
// CASE 2 - does a resume get TWO ModelChanged frames, and what do they do?
// ---------------------------------------------------------------------------

/// The double-frame shape: subscribe's ModelChanged (:911) plus resume's
/// ModelChanged (:1754) back to back, with NO user-initiated switch. Both
/// frames must apply route state, but neither may announce.
#[test]
fn double_model_changed_frames_both_apply_route_state_and_neither_announces() {
    with_temp_jcode_home(|| {
        let session_id = "ses_gate_resume_02";
        let mut app = gate_resumed_app(session_id);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_server_event(
            crate::protocol::ServerEvent::SessionId {
                session_id: session_id.to_string(),
            },
            &mut remote,
        );
        app.handle_server_event(gate_history_event(1, session_id), &mut remote);

        let revision_before = app.model_picker_catalog_revision;

        // Frame 1: subscribe's route report.
        app.handle_server_event(
            gate_model_changed(1, GATE_MODEL, Some(GATE_WINDOW)),
            &mut remote,
        );
        // Frame 2: the resume handler's route report, same route.
        app.handle_server_event(
            gate_model_changed(1, GATE_MODEL, Some(GATE_WINDOW)),
            &mut remote,
        );
        app.handle_server_event(crate::protocol::ServerEvent::Done { id: 1 }, &mut remote);

        assert_eq!(
            app.model_picker_catalog_revision,
            revision_before + 2,
            "both frames ran `invalidate_model_picker_cache()`; the gate suppresses the \
             announcement, not the state application"
        );
        assert_eq!(
            app.context_limit, GATE_WINDOW,
            "the second frame must not downgrade the window"
        );
        assert_eq!(gate_compaction_budget(&app), GATE_WINDOW as usize);

        let announced = gate_transcript(&app);
        assert!(
            !announced.iter().any(|c| c.contains("Switched to model")),
            "neither resume frame may announce a switch. transcript: {announced:?}"
        );
        let notice = app.status_notice();
        assert!(
            !notice
                .as_deref()
                .is_some_and(|text| text.starts_with("Model \u{2192}")),
            "neither resume frame may set a 'Model -> ...' notice. got {notice:?}"
        );
    });
}

/// The masked-side-effect probe for the second frame: `context_warning_shown`
/// is cleared by `update_context_limit_for_model` (:662). Set it before the
/// SECOND frame and it must be cleared by it, proving the second frame really
/// reaches the same state code rather than being short-circuited.
#[test]
fn second_resume_model_changed_frame_still_runs_update_context_limit() {
    with_temp_jcode_home(|| {
        let session_id = "ses_gate_resume_03";
        let mut app = gate_resumed_app(session_id);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_server_event(
            gate_model_changed(1, GATE_MODEL, Some(GATE_WINDOW)),
            &mut remote,
        );
        assert_eq!(app.context_limit, GATE_WINDOW);

        app.context_warning_shown = true;
        let revision_before = app.model_picker_catalog_revision;

        app.handle_server_event(
            gate_model_changed(1, GATE_MODEL, Some(GATE_WINDOW)),
            &mut remote,
        );

        assert!(
            !app.context_warning_shown,
            "the second frame still runs update_context_limit_for_model, which clears \
             context_warning_shown; the gate does not short-circuit it"
        );
        assert_eq!(
            app.model_picker_catalog_revision,
            revision_before + 1,
            "the second frame still invalidates the picker cache"
        );
    });
}

/// The interaction the gate could plausibly have broken: a real user switch
/// followed by the resume handler's duplicate route report. The first frame's
/// notice must SURVIVE the second frame, not be cleared or overwritten by it.
#[test]
fn user_switch_notice_survives_a_following_non_initiated_frame() {
    with_temp_jcode_home(|| {
        let session_id = "ses_gate_resume_04";
        let mut app = gate_resumed_app(session_id);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        // A switch the user asked for.
        app.remote_model_switch_in_flight = true;
        app.handle_server_event(
            gate_model_changed(7, "gpt-5.6-luna", Some(400_000)),
            &mut remote,
        );
        assert_eq!(
            app.status_notice(),
            Some("Model \u{2192} gpt-5.6-luna".to_string()),
            "a real switch still announces"
        );

        // The duplicate route report that follows it (in_flight is now false).
        app.handle_server_event(
            gate_model_changed(7, "gpt-5.6-luna", Some(400_000)),
            &mut remote,
        );

        assert_eq!(
            app.status_notice(),
            Some("Model \u{2192} gpt-5.6-luna".to_string()),
            "the second frame must not clear or overwrite the first frame's notice"
        );
        let announced = gate_transcript(&app);
        assert_eq!(
            announced
                .iter()
                .filter(|c| c.contains("Switched to model"))
                .count(),
            1,
            "exactly one announcement for one real switch. transcript: {announced:?}"
        );
    });
}

/// The pre-gate delta, measured rather than asserted.
///
/// The uncommitted diff changes exactly one thing in the non-error arm: the two
/// announcements become conditional on `user_initiated_switch`, which is the
/// captured value of `app.remote_model_switch_in_flight`. Every other line of
/// the arm (the route state above them) is untouched. So running this test's
/// frame sequence with `remote_model_switch_in_flight == true` executes the
/// exact branch the pre-gate code always took, and therefore reproduces what a
/// resume used to show the user.
///
/// A startup/reload resume delivers the route TWICE (measured in
/// `runner_modelchanged_resume_tests.rs`: client_lifecycle.rs runs
/// handle_resume_session and then handle_subscribe), so the pre-gate user saw
/// two "Switched to model" lines per resume. That is the bug the gate fixes,
/// not information the gate removes.
#[test]
fn ungated_announcement_block_announced_twice_per_startup_resume() {
    with_temp_jcode_home(|| {
        let session_id = "ses_gate_baseline_01";
        let mut app = gate_resumed_app(session_id);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_server_event(
            crate::protocol::ServerEvent::SessionId {
                session_id: session_id.to_string(),
            },
            &mut remote,
        );
        app.handle_server_event(gate_history_event(1, session_id), &mut remote);

        // Force the branch the pre-gate code always took for both frames.
        app.remote_model_switch_in_flight = true;
        app.handle_server_event(
            gate_model_changed(1, GATE_MODEL, Some(GATE_WINDOW)),
            &mut remote,
        );

        let after_first = gate_transcript(&app);
        assert_eq!(
            after_first
                .iter()
                .filter(|c| c.contains("Switched to model"))
                .count(),
            1,
            "PRE-GATE BASELINE: the ungated arm announced on the first frame. transcript: \
             {after_first:?}"
        );
        assert!(
            !after_first[after_first.len() - 1].ends_with("2]"),
            "one push so far, so there is no repeat badge yet: {after_first:?}"
        );
        assert_eq!(
            app.status_notice(),
            Some(format!("Model \u{2192} {}", GATE_MODEL)),
            "PRE-GATE BASELINE: the ungated arm set the notice unconditionally"
        );

        // The flag is cleared by the first frame, exactly as before the change,
        // so the second frame would also have announced pre-gate.
        app.remote_model_switch_in_flight = true;
        app.handle_server_event(
            gate_model_changed(1, GATE_MODEL, Some(GATE_WINDOW)),
            &mut remote,
        );
        app.handle_server_event(crate::protocol::ServerEvent::Done { id: 1 }, &mut remote);

        // The transcript renders an identical repeated system message as one
        // entry with a "[..2]" repeat badge, so the second announcement shows up
        // as the badge rather than a second line. That badge is the executed
        // evidence that BOTH frames announced pre-gate.
        let after_second = gate_transcript(&app);
        assert_eq!(
            after_second
                .iter()
                .filter(|c| c.contains("Switched to model"))
                .count(),
            1,
            "the repeat is badge-collapsed, not a second line. transcript: {after_second:?}"
        );
        let announcement = after_second
            .iter()
            .find(|c| c.contains("Switched to model"))
            .expect("announcement entry");
        assert!(
            announcement.ends_with("2]"),
            "PRE-GATE BASELINE: the second frame announced too, so the entry must carry a \
             2-repeat badge. got {announcement:?}"
        );
        assert_eq!(
            app.status_notice(),
            Some(format!("Model \u{2192} {}", GATE_MODEL)),
            "PRE-GATE BASELINE: the notice was set again by the second frame"
        );
    });
}

/// The post-login path, which is the one place the pre-gate code ALREADY
/// suppressed the transcript line (`auth_catalog_refresh_pending`) but still set
/// the "Model -> ..." status notice unconditionally. The gate now suppresses
/// both. Assert the route is still applied and still reachable, so the lost
/// notice is cosmetic rather than informational.
#[test]
fn post_login_catalog_refresh_model_changed_still_applies_the_route() {
    with_temp_jcode_home(|| {
        let mut app = App::new_for_remote(None);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.auth_catalog_refresh_pending = true;
        assert!(!app.remote_model_switch_in_flight);

        app.handle_server_event(
            gate_model_changed(3, "claude-opus-4.6", Some(200_000)),
            &mut remote,
        );

        let announced = gate_transcript(&app);
        assert!(
            !announced.iter().any(|c| c.contains("Switched to model")),
            "post-login refresh never announced in the transcript. transcript: {announced:?}"
        );
        let notice = app.status_notice();
        assert!(
            !notice
                .as_deref()
                .is_some_and(|text| text.starts_with("Model \u{2192}")),
            "DELTA: pre-gate this branch set 'Model -> ...' unconditionally even while the \
             catalog refresh was pending; the gate drops that too. got {notice:?}"
        );

        // Route state is untouched by the gate.
        assert_eq!(app.remote_provider_model.as_deref(), Some("claude-opus-4.6"));
        assert_eq!(app.context_limit, 200_000);
        assert_eq!(gate_compaction_budget(&app), 200_000);
        assert_eq!(
            crate::tui::TuiState::provider_model(&app),
            "claude-opus-4.6",
            "the status line still names the post-login route"
        );
    });
}

// ---------------------------------------------------------------------------
// CASE 3 - does the error branch still behave?
// ---------------------------------------------------------------------------

/// A FAILED user-initiated switch. `remote_model_switch_in_flight` is cleared at
/// the top of the arm unconditionally, and `user_initiated_switch` was already
/// captured. The error branch is NOT gated, so guidance and prompt restore must
/// both still happen.
#[test]
fn failed_user_switch_still_shows_guidance_and_restores_deferred_prompt() {
    with_temp_jcode_home(|| {
        let session_id = "ses_gate_error_01";
        let mut app = gate_resumed_app(session_id);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.remote_model_switch_in_flight = true;
        app.pending_prompt_after_model_switch = Some(crate::tui::app::input::PreparedInput {
            raw_input: "please use the selected model".to_string(),
            expanded: "please use the selected model".to_string(),
            images: vec![("image/jpeg".to_string(), "def456".to_string())],
        });

        app.handle_server_event(
            crate::protocol::ServerEvent::ModelChanged {
                context_window: None,
                id: 8,
                model: "Qwen/Qwen3-32B-TEE".to_string(),
                provider_name: Some("Chutes".to_string()),
                error: Some("credentials expired".to_string()),
                resolved_credential: None,
                reasoning_effort: None,
            },
            &mut remote,
        );

        assert_eq!(
            app.status_notice(),
            Some("Model switch failed".to_string()),
            "the error branch must still report the failure"
        );
        let announced = gate_transcript(&app);
        assert!(
            !announced.iter().any(|c| c.contains("Switched to model")),
            "a failed switch must never claim success. transcript: {announced:?}"
        );

        let last = app
            .display_messages
            .last()
            .expect("error message should be pushed");
        assert_eq!(last.role, "error");
        assert!(last.content.contains("credentials expired"));
        assert!(last.content.contains("/model"));
        assert!(last.content.contains("/login"));
        assert!(last.content.contains("reconnect"));

        assert!(
            app.pending_prompt_after_model_switch.is_none(),
            "the deferred prompt must be consumed"
        );
        assert_eq!(app.input, "please use the selected model");
        assert_eq!(app.cursor_pos, app.input.len());
        assert_eq!(app.pending_images.len(), 1);
        assert!(
            !app.remote_model_switch_in_flight,
            "the in-flight flag must be cleared even on failure"
        );
    });
}

/// A resume-time error, i.e. the frame arrives with
/// `remote_model_switch_in_flight == false`. The error branch is reached the
/// same way, so the guidance must still be shown and the stale route must NOT
/// be silently adopted.
#[test]
fn resume_time_model_changed_error_shows_guidance_without_a_switch_in_flight() {
    with_temp_jcode_home(|| {
        let session_id = "ses_gate_error_02";
        let mut app = gate_resumed_app(session_id);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        assert!(!app.remote_model_switch_in_flight);

        app.handle_server_event(
            crate::protocol::ServerEvent::ModelChanged {
                context_window: Some(GATE_WINDOW),
                id: 9,
                model: "claude-opus-4.6".to_string(),
                provider_name: Some("Anthropic".to_string()),
                error: Some("auth required for provider".to_string()),
                resolved_credential: None,
                reasoning_effort: None,
            },
            &mut remote,
        );

        assert_eq!(
            app.status_notice(),
            Some("Model switch failed".to_string()),
            "an errored resume frame must still surface guidance"
        );
        let announced = gate_transcript(&app);
        assert!(
            !announced.iter().any(|c| c.contains("Switched to model")),
            "an errored frame must never announce. transcript: {announced:?}"
        );
        let last = app
            .display_messages
            .last()
            .expect("error message should be pushed");
        assert_eq!(last.role, "error");
        assert!(last.content.contains("auth required for provider"));
        assert!(
            app.remote_provider_model.as_deref() != Some("claude-opus-4.6"),
            "the error branch must not adopt the failed route"
        );
        assert!(
            !app.remote_model_switch_in_flight,
            "the in-flight flag stays cleared"
        );
    });
}
