//! Integration tests for the `ask_user` form hooks in `handle_server_event`.
//!
//! The form module is driven through its exported entry points, exactly as
//! `key_events.rs` and `handle_server_event` drive it, so these tests cover the
//! wiring rather than lane B's renderer.

use super::*;
use crate::protocol::ServerEvent;
use crate::protocol::elicitation::{
    ElicitationResolution, ElicitationResponse, FormElicitation, FormSchema,
};
use crate::tui::ask_form::{self, KeyOutcome};
use crate::tui::backend::RemoteConnection;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use serde_json::json;

const SESSION: &str = "session-a";

fn schema() -> FormSchema {
    FormSchema(json!({
        "_meta": {"jcode.dev/askUser": {"version": 1}},
        "type": "object",
        "required": ["q1"],
        "properties": {
            "q1": {
                "type": "string",
                "title": "Pick",
                "description": "Which one?",
                "_meta": {"jcode.dev/askUser": {"header": "Pick", "otherField": "q1_other"}},
                "oneOf": [
                    {"const": "a", "title": "A"},
                    {"const": "b", "title": "B"},
                    {"const": "__other__", "title": "Other"}
                ]
            },
            "q1_other": {"type": "string", "maxLength": 8192}
        }
    }))
}

fn untagged_schema() -> FormSchema {
    FormSchema(json!({
        "type": "object",
        "required": ["x"],
        "properties": {"x": {"type": "string"}}
    }))
}

fn request(session: &str, elicitation_id: &str) -> ServerEvent {
    ServerEvent::ElicitationRequest(FormElicitation {
        elicitation_id: elicitation_id.into(),
        session_id: session.into(),
        tool_call_id: Some("call-1".into()),
        message: "The agent has a question.".into(),
        requested_schema: schema(),
    })
}

fn history(session: &str) -> ServerEvent {
    ServerEvent::History {
        id: 1,
        session_id: session.to_string(),
        messages: vec![],
        messages_base_count: None,
        images_unchanged: false,
        images: vec![],
        provider_name: None,
        provider_model: None,
        subagent_model: None,
        agent_model_overrides: Default::default(),
        autoreview_enabled: None,
        autojudge_enabled: None,
        available_models: vec![],
        available_model_routes: vec![],
        mcp_servers: vec![],
        skills: vec![],
        total_tokens: None,
        token_usage_totals: None,
        all_sessions: vec![],
        client_count: Some(1),
        is_canary: Some(false),
        reload_recovery: None,
        server_version: None,
        server_name: None,
        server_icon: None,
        server_has_update: Some(false),
        was_interrupted: None,
        connection_type: None,
        status_detail: None,
        upstream_provider: None,
        provider_oauth_email: None,
        provider_oauth_account_label: None,
        resolved_credential: None,
        reasoning_effort: None,
        service_tier: None,
        available_service_tiers: None,
        compaction_mode: crate::config::CompactionMode::Reactive,
        activity: None,
        side_panel: crate::side_panel::SidePanelSnapshot::default(),
        applets: Default::default(),
    }
}

struct Fixture {
    app: crate::tui::app::App,
    remote: RemoteConnection,
    // `RemoteConnection::dummy` opens a tokio Unix socket pair, which needs a
    // reactor for the lifetime of the connection.
    _runtime: tokio::runtime::Runtime,
}

impl Fixture {
    fn new(session: &str) -> Self {
        ask_form::reset_for_test();
        let mut app = crate::tui::app::tests::create_test_app();
        app.is_remote = true;
        app.remote_session_id = Some(session.to_string());
        ask_form::on_remote_loop_start(&mut app);
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let remote = {
            let _guard = runtime.enter();
            RemoteConnection::dummy()
        };
        Self {
            app,
            remote,
            _runtime: runtime,
        }
    }

    fn feed(&mut self, event: ServerEvent) -> bool {
        handle_server_event(&mut self.app, event, &mut self.remote)
    }

    fn key(&mut self, code: KeyCode) -> KeyOutcome {
        ask_form::handle_key(&mut self.app, code, KeyModifiers::NONE, None)
    }

    fn notice(&self) -> Option<String> {
        self.app
            .status_notice
            .as_ref()
            .map(|(text, _)| text.clone())
    }
}

fn expect_consumed(outcome: KeyOutcome) {
    assert_eq!(outcome, KeyOutcome::Consumed);
}

fn expect_ignored(outcome: KeyOutcome) {
    assert_eq!(outcome, KeyOutcome::Ignored);
}

fn expect_send(outcome: KeyOutcome) -> ElicitationResponse {
    match outcome {
        KeyOutcome::Send { response, .. } => response,
        other => panic!("expected a response, got {other:?}"),
    }
}

fn select_first_question_answer(app: &mut crate::tui::app::App, digit: char) {
    expect_consumed(ask_form::handle_key(
        app,
        KeyCode::Char(digit),
        KeyModifiers::NONE,
        None,
    ));
}

fn go_to_submit(app: &mut crate::tui::app::App) {
    expect_consumed(ask_form::handle_key(
        app,
        KeyCode::Right,
        KeyModifiers::NONE,
        None,
    ));
}

/// Answer question one and submit the summary tab, recording `request_id` as
/// the acknowledgement owner exactly like the key hook does.
fn answer_and_submit(app: &mut crate::tui::app::App, request_id: u64) -> ElicitationResponse {
    select_first_question_answer(app, '1');
    go_to_submit(app);
    let response = expect_send(ask_form::handle_key(
        app,
        KeyCode::Enter,
        KeyModifiers::NONE,
        None,
    ));
    ask_form::record_response_id(app, request_id);
    response
}

fn accept_content(response: &ElicitationResponse) -> &serde_json::Map<String, serde_json::Value> {
    match response {
        ElicitationResponse::Accept {
            content: Some(content),
        } => content,
        other => panic!("expected an accepted response, got {other:?}"),
    }
}

#[test]
fn claimed_done_does_not_settle_an_observed_turn() {
    let mut fixture = Fixture::new(SESSION);
    assert!(fixture.feed(request(SESSION, "e1")));
    answer_and_submit(&mut fixture.app, 42);

    // An observed turn: no local request id, an unscoped fence.
    fixture.app.is_processing = true;
    fixture.app.streaming.streaming_text = "observed output".into();
    fixture.app.status = crate::tui::ProcessingStatus::Streaming;

    assert!(fixture.feed(ServerEvent::Done { id: 42 }));
    assert!(fixture.app.is_processing, "claimed Done settled the turn");
    assert!(matches!(
        fixture.app.status,
        crate::tui::ProcessingStatus::Streaming
    ));
    assert_eq!(fixture.app.streaming.streaming_text, "observed output");
}

#[test]
fn claimed_error_reopens_the_form_without_a_transcript_error() {
    let mut fixture = Fixture::new(SESSION);
    assert!(fixture.feed(request(SESSION, "e1")));
    answer_and_submit(&mut fixture.app, 7);
    let transcript = fixture.app.display_messages().len();

    assert!(fixture.feed(ServerEvent::Error {
        id: 7,
        message: "answer rejected".into(),
        retry_after_secs: None,
    }));
    assert_eq!(fixture.app.display_messages().len(), transcript);
    assert_eq!(
        fixture.notice().as_deref(),
        Some("The answer was rejected: answer rejected")
    );

    // Editing resumes: leave the summary tab and submit again.
    expect_consumed(fixture.key(KeyCode::Left));
    go_to_submit(&mut fixture.app);
    let response = expect_send(fixture.key(KeyCode::Enter));
    assert_eq!(accept_content(&response).get("q1"), Some(&json!("a")));
}

#[test]
fn same_session_history_preserves_forms_drafts_and_response_ids() {
    let mut fixture = Fixture::new(SESSION);
    assert!(fixture.feed(request(SESSION, "e1")));
    select_first_question_answer(&mut fixture.app, '1');

    fixture.feed(history(SESSION));
    go_to_submit(&mut fixture.app);
    let response = expect_send(fixture.key(KeyCode::Enter));
    assert_eq!(accept_content(&response).get("q1"), Some(&json!("a")));
    ask_form::record_response_id(&mut fixture.app, 77);

    // A later refresh still keeps the response ownership.
    fixture.feed(history(SESSION));
    fixture.app.is_processing = true;
    fixture.app.streaming.streaming_text = "observed".into();
    fixture.app.status = crate::tui::ProcessingStatus::Streaming;
    assert!(fixture.feed(ServerEvent::Done { id: 77 }));
    assert!(fixture.app.is_processing);

    assert!(fixture.feed(ServerEvent::ElicitationResolved {
        session_id: SESSION.into(),
        elicitation_id: "e1".into(),
        outcome: ElicitationResolution::Accepted,
    }));
    expect_ignored(fixture.key(KeyCode::Char('1')));
}

#[test]
fn connection_replacement_freezes_then_restores_a_replayed_form() {
    let mut fixture = Fixture::new(SESSION);
    assert!(fixture.feed(request(SESSION, "e1")));
    select_first_question_answer(&mut fixture.app, '2');

    ask_form::on_connection_replaced(&mut fixture.app);
    expect_ignored(fixture.key(KeyCode::Char('1')));

    // The broker replays the identical form, then ends replay.
    assert!(fixture.feed(request(SESSION, "e1")));
    assert!(fixture.feed(ServerEvent::ElicitationReplayComplete {
        session_id: SESSION.into(),
    }));

    go_to_submit(&mut fixture.app);
    let response = expect_send(fixture.key(KeyCode::Enter));
    assert_eq!(accept_content(&response).get("q1"), Some(&json!("b")));
}

#[test]
fn empty_replay_after_a_reload_discards_drafts() {
    let mut fixture = Fixture::new(SESSION);
    assert!(fixture.feed(request(SESSION, "e1")));
    select_first_question_answer(&mut fixture.app, '1');

    ask_form::on_connection_replaced(&mut fixture.app);
    assert!(fixture.feed(ServerEvent::ElicitationReplayComplete {
        session_id: SESSION.into(),
    }));
    expect_ignored(fixture.key(KeyCode::Char('1')));
    assert_eq!(
        fixture.notice().as_deref(),
        Some("1 question form(s) were lost while reconnecting.")
    );
}

#[test]
fn replay_before_history_stages_the_new_session() {
    let mut fixture = Fixture::new(SESSION);
    assert!(fixture.feed(request(SESSION, "old")));
    select_first_question_answer(&mut fixture.app, '1');

    // A reconnect is followed by replay for the new attachment before its
    // History has been processed: the app still reports the old session.
    ask_form::on_connection_replaced(&mut fixture.app);
    assert!(fixture.feed(request("session-b", "new")));
    expect_ignored(fixture.key(KeyCode::Char('1')));

    fixture.feed(history("session-b"));
    assert_eq!(fixture.app.remote_session_id.as_deref(), Some("session-b"));
    expect_consumed(fixture.key(KeyCode::Char('1')));
}

#[test]
fn resolved_while_disconnected_removes_the_draft() {
    let mut fixture = Fixture::new(SESSION);
    assert!(fixture.feed(request(SESSION, "e1")));
    select_first_question_answer(&mut fixture.app, '1');

    ask_form::on_connection_replaced(&mut fixture.app);
    assert!(fixture.feed(ServerEvent::ElicitationResolved {
        session_id: SESSION.into(),
        elicitation_id: "e1".into(),
        outcome: ElicitationResolution::Accepted,
    }));
    assert!(fixture.feed(ServerEvent::ElicitationReplayComplete {
        session_id: SESSION.into(),
    }));
    expect_ignored(fixture.key(KeyCode::Char('1')));
}

#[test]
fn loop_teardown_clears_state() {
    let mut fixture = Fixture::new(SESSION);
    assert!(fixture.feed(request(SESSION, "e1")));
    select_first_question_answer(&mut fixture.app, '1');

    ask_form::on_remote_loop_exit();
    expect_ignored(fixture.key(KeyCode::Char('1')));

    // A fresh loop does not inherit the previous loop's form.
    ask_form::on_remote_loop_start(&mut fixture.app);
    expect_ignored(fixture.key(KeyCode::Char('1')));
}

#[test]
fn resolution_before_acknowledgement_stays_resolved() {
    let mut fixture = Fixture::new(SESSION);
    assert!(fixture.feed(request(SESSION, "e1")));
    answer_and_submit(&mut fixture.app, 9);

    assert!(fixture.feed(ServerEvent::ElicitationResolved {
        session_id: SESSION.into(),
        elicitation_id: "e1".into(),
        outcome: ElicitationResolution::Accepted,
    }));
    assert_eq!(fixture.notice().as_deref(), Some("Answer submitted."));
    expect_ignored(fixture.key(KeyCode::Char('1')));

    // A late rejection must not resurrect the closed form.
    assert!(fixture.feed(ServerEvent::Error {
        id: 9,
        message: "too late".into(),
        retry_after_secs: None,
    }));
    expect_ignored(fixture.key(KeyCode::Char('1')));
}

#[test]
fn late_acknowledgement_after_resolution_is_claimed() {
    let mut fixture = Fixture::new(SESSION);
    assert!(fixture.feed(request(SESSION, "e1")));
    answer_and_submit(&mut fixture.app, 13);
    assert!(fixture.feed(ServerEvent::ElicitationResolved {
        session_id: SESSION.into(),
        elicitation_id: "e1".into(),
        outcome: ElicitationResolution::Accepted,
    }));
    let transcript = fixture.app.display_messages().len();
    assert!(fixture.feed(ServerEvent::Done { id: 13 }));
    assert_eq!(fixture.app.display_messages().len(), transcript);
    expect_ignored(fixture.key(KeyCode::Char('1')));
}

#[test]
fn another_client_resolving_during_submit_closes_the_form() {
    let mut fixture = Fixture::new(SESSION);
    assert!(fixture.feed(request(SESSION, "e1")));
    answer_and_submit(&mut fixture.app, 11);

    // Another attachment wins while our answer is outstanding.
    assert!(fixture.feed(ServerEvent::ElicitationResolved {
        session_id: SESSION.into(),
        elicitation_id: "e1".into(),
        outcome: ElicitationResolution::Accepted,
    }));
    assert_eq!(fixture.notice().as_deref(), Some("Answer submitted."));
    expect_ignored(fixture.key(KeyCode::Enter));

    // Our acknowledgement is claimed without resurrecting the closed form.
    assert!(fixture.feed(ServerEvent::Done { id: 11 }));
    expect_ignored(fixture.key(KeyCode::Enter));
}

#[test]
fn transport_send_failure_restores_the_draft_with_a_notice() {
    let mut fixture = Fixture::new(SESSION);
    assert!(fixture.feed(request(SESSION, "e1")));
    select_first_question_answer(&mut fixture.app, '1');
    go_to_submit(&mut fixture.app);
    let _ = expect_send(fixture.key(KeyCode::Enter));

    ask_form::send_failed(&mut fixture.app, "transport closed");
    assert_eq!(
        fixture.notice().as_deref(),
        Some("Could not send the answer: transport closed")
    );

    let response = expect_send(fixture.key(KeyCode::Enter));
    assert_eq!(accept_content(&response).get("q1"), Some(&json!("a")));
}

#[test]
fn duplicate_submit_is_ignored_while_a_response_is_outstanding() {
    let mut fixture = Fixture::new(SESSION);
    assert!(fixture.feed(request(SESSION, "e1")));
    select_first_question_answer(&mut fixture.app, '1');
    go_to_submit(&mut fixture.app);
    let first = expect_send(fixture.key(KeyCode::Enter));

    expect_consumed(fixture.key(KeyCode::Enter));
    expect_consumed(fixture.key(KeyCode::Char('2')));
    ask_form::record_response_id(&mut fixture.app, 21);
    assert_eq!(accept_content(&first).get("q1"), Some(&json!("a")));
}

#[test]
fn untagged_forms_are_ignored_without_a_response() {
    let mut fixture = Fixture::new(SESSION);
    let event = ServerEvent::ElicitationRequest(FormElicitation {
        elicitation_id: "applet".into(),
        session_id: SESSION.into(),
        tool_call_id: None,
        message: "Applet form".into(),
        requested_schema: untagged_schema(),
    });
    assert!(fixture.feed(event));
    expect_ignored(fixture.key(KeyCode::Char('1')));
}

#[test]
fn connection_loss_hides_the_form_and_restores_its_draft() {
    let mut fixture = Fixture::new(SESSION);
    assert!(fixture.feed(request(SESSION, "e1")));
    select_first_question_answer(&mut fixture.app, '1');

    // No transport means no answers: keys and paste are released, not queued.
    ask_form::on_connection_lost();
    expect_ignored(fixture.key(KeyCode::Char('2')));
    assert!(!ask_form::handle_paste(
        &mut fixture.app,
        "typed while away"
    ));

    // The reconnect replays the same form and the confirmed answer survives.
    ask_form::on_connection_replaced(&mut fixture.app);
    assert!(fixture.feed(request(SESSION, "e1")));
    assert!(fixture.feed(ServerEvent::ElicitationReplayComplete {
        session_id: SESSION.into(),
    }));
    go_to_submit(&mut fixture.app);
    let response = expect_send(fixture.key(KeyCode::Enter));
    assert_eq!(accept_content(&response).get("q1"), Some(&json!("a")));
}

#[test]
fn submitted_form_reopens_after_the_connection_changes() {
    let mut fixture = Fixture::new(SESSION);
    assert!(fixture.feed(request(SESSION, "e1")));
    let first = answer_and_submit(&mut fixture.app, 7);
    assert_eq!(accept_content(&first).get("q1"), Some(&json!("a")));

    // Submitting swallows input until the server resolves or rejects it.
    expect_consumed(fixture.key(KeyCode::Enter));

    // The acknowledgement never arrived; the next connection must reopen the
    // form instead of leaving it stuck on `Submitting` forever.
    ask_form::on_connection_lost();
    ask_form::on_connection_replaced(&mut fixture.app);
    assert!(fixture.feed(request(SESSION, "e1")));
    assert!(fixture.feed(ServerEvent::ElicitationReplayComplete {
        session_id: SESSION.into(),
    }));
    assert!(
        fixture
            .notice()
            .is_some_and(|notice| notice.contains("not acknowledged")),
        "reopening should explain why the answer needs review"
    );
    let second = expect_send(fixture.key(KeyCode::Enter));
    assert_eq!(accept_content(&second).get("q1"), Some(&json!("a")));
}
