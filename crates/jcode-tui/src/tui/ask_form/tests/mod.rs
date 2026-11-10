//! Tests for the `ask_user` form module.
//!
//! Every test resets the thread-local state first. Rust's test harness runs
//! each test on its own thread, so `thread_local!` state is naturally isolated,
//! but the reset also keeps tests independent when run in a shared thread.

mod app;
mod keys;
mod render;

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::text::Line;
use serde_json::{Value, json};
use unicode_width::UnicodeWidthStr;

use super::keys as form_keys;
use super::render as form_render;
use super::state::{self, EditorKind, Form, MAX_TEXT_CODE_POINTS, SubmitState};
use super::{FocusProbe, KeyOutcome, ViewContext, paste, reconcile};
use crate::protocol::ServerEvent;
use crate::protocol::elicitation::{ElicitationResolution, FormElicitation, FormSchema};

pub(super) const SESSION: &str = "session-a";

pub(super) fn reset() {
    super::reset_for_test();
}

pub(super) fn schema_json() -> Value {
    json!({
        "_meta": {"jcode.dev/askUser": {"version": 1}},
        "type": "object",
        "required": ["q1", "q2"],
        "properties": {
            "q1": {
                "type": "string",
                "title": "Pick",
                "description": "Which option do you want?",
                "_meta": {"jcode.dev/askUser": {
                    "header": "Pick",
                    "otherField": "q1_other",
                    "notesField": "q1_notes"
                }},
                "oneOf": [
                    {"const": "a", "title": "Alpha", "description": "First option", "_meta": {"preview": "Alpha preview\nline two"}},
                    {"const": "b", "title": "Beta", "description": "Second option"},
                    {"const": "__other__", "title": "Other"}
                ]
            },
            "q1_other": {"type": "string", "maxLength": 8192},
            "q1_notes": {"type": "string", "maxLength": 8192},
            "q2": {
                "type": "array",
                "title": "Many",
                "description": "Pick several.",
                "_meta": {"jcode.dev/askUser": {
                    "header": "Many",
                    "notesField": "q2_notes"
                }},
                "items": {"anyOf": [
                    {"const": "x", "title": "Xray", "description": "Ex"},
                    {"const": "y", "title": "Yankee"}
                ]}
            },
            "q2_notes": {"type": "string", "maxLength": 8192}
        }
    })
}

pub(super) fn single_schema() -> Value {
    json!({
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
                    {"const": "a", "title": "Alpha"},
                    {"const": "b", "title": "Beta"},
                    {"const": "__other__", "title": "Other"}
                ]
            },
            "q1_other": {"type": "string"}
        }
    })
}

pub(super) fn form(id: &str, session: &str, schema: Value) -> FormElicitation {
    FormElicitation {
        elicitation_id: id.to_string(),
        session_id: session.to_string(),
        tool_call_id: None,
        message: "The agent has questions.".to_string(),
        requested_schema: FormSchema(schema),
    }
}

pub(super) fn context(session: &str) -> ViewContext {
    ViewContext {
        session: Some(session.to_string()),
        replay: false,
        blocked: None,
    }
}

pub(super) fn install(session: &str, form: FormElicitation) -> ViewContext {
    let context = context(session);
    state::with(|state| {
        state.loop_live = true;
        state.generation = 1;
        state.attached = true;
        state.session = Some(session.to_string());
        let event = ServerEvent::ElicitationRequest(form);
        let result = reconcile::handle_event(state, &context, &event);
        assert!(result.consumed, "request should be consumed");
    });
    context
}

pub(super) fn install_default() -> ViewContext {
    install(SESSION, form("e1", SESSION, schema_json()))
}

pub(super) fn key_with(
    context: &ViewContext,
    code: KeyCode,
    modifiers: KeyModifiers,
    text: Option<&str>,
) -> KeyOutcome {
    state::with(|state| form_keys::handle_key(state, context, code, modifiers, text).outcome)
}

pub(super) fn key(context: &ViewContext, code: KeyCode) -> KeyOutcome {
    key_with(context, code, KeyModifiers::NONE, None)
}

pub(super) fn paste_text(context: &ViewContext, text: &str) -> bool {
    state::with(|state| {
        let result = paste::handle_paste(state, context, text);
        if let Some(notice) = result.notice
            && let Some(form) = state.active_form_mut()
        {
            form.notice = Some(notice);
        }
        result.consumed
    })
}

pub(super) fn send_event(context: &ViewContext, event: &ServerEvent) -> (bool, Option<String>) {
    state::with(|state| {
        let result = reconcile::handle_event(state, context, event);
        (result.consumed, result.notice)
    })
}

pub(super) fn active<R>(f: impl FnOnce(&Form) -> R) -> R {
    state::snapshot(|state| f(state.active_form().expect("active form")))
}

pub(super) fn active_mut<R>(f: impl FnOnce(&mut Form) -> R) -> R {
    state::with(|state| f(state.active_form_mut().expect("active form")))
}

pub(super) fn form_count() -> usize {
    state::snapshot(|state| state.forms.len())
}

pub(super) fn response_ids() -> Vec<u64> {
    state::snapshot(|state| state.responses.keys().copied().collect())
}

pub(super) fn answer_all(context: &ViewContext) {
    // q1 single: choose Alpha; Enter advances.
    assert_eq!(key(context, KeyCode::Char('1')), KeyOutcome::Consumed);
    assert_eq!(key(context, KeyCode::Enter), KeyOutcome::Consumed);
    // q2 multi: choose Xray and Yankee; Enter advances to Submit.
    assert_eq!(key(context, KeyCode::Char('1')), KeyOutcome::Consumed);
    assert_eq!(key(context, KeyCode::Char('2')), KeyOutcome::Consumed);
    assert_eq!(key(context, KeyCode::Enter), KeyOutcome::Consumed);
}

pub(super) fn line_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

pub(super) fn row_text(buffer: &Buffer, row: u16) -> String {
    if row >= buffer.area.height {
        return String::new();
    }
    let mut text = String::new();
    for x in 0..buffer.area.width {
        if let Some(cell) = buffer.cell((x, row)) {
            text.push_str(cell.symbol());
        }
    }
    text.trim_end().to_string()
}

pub(super) fn first_nonblank_row(buffer: &Buffer) -> Option<u16> {
    (0..buffer.area.height).find(|row| !row_text(buffer, *row).is_empty())
}

// ---------------------------------------------------------------------------
// Key sequences and selection semantics
// ---------------------------------------------------------------------------

#[test]
fn key_sequences_navigate_confirm_and_preserve_answers() {
    reset();
    let context = install_default();

    // Initial highlight is not a selection.
    active(|form| {
        assert_eq!(form.tab, 0);
        assert_eq!(form.questions[0].highlight, 0);
        assert!(form.questions[0].confirmed.is_none());
    });

    // Down moves the highlight without confirming.
    key(&context, KeyCode::Down);
    active(|form| {
        assert_eq!(form.questions[0].highlight, 1);
        assert!(form.questions[0].confirmed.is_none());
    });

    // Space confirms the highlighted choice.
    key(&context, KeyCode::Char(' '));
    active(|form| assert_eq!(form.questions[0].confirmed.as_deref(), Some("b")));

    // Right advances to the next tab without changing the answer.
    key(&context, KeyCode::Right);
    active(|form| assert_eq!(form.tab, 1));
    key(&context, KeyCode::Left);
    active(|form| {
        assert_eq!(form.tab, 0);
        assert_eq!(form.questions[0].confirmed.as_deref(), Some("b"));
        assert_eq!(form.questions[0].highlight, 1);
    });

    // Out-of-range digits do nothing.
    key(&context, KeyCode::Char('9'));
    active(|form| assert_eq!(form.questions[0].confirmed.as_deref(), Some("b")));
}

#[test]
fn multi_select_enter_advances_without_changing_selection() {
    reset();
    let context = install_default();
    key(&context, KeyCode::Right);
    active(|form| assert_eq!(form.tab, 1));

    key(&context, KeyCode::Char('1'));
    key(&context, KeyCode::Char('2'));
    active(|form| {
        let selected = &form.questions[1].selected;
        assert!(selected.contains("x"));
        assert!(selected.contains("y"));
    });

    // Space toggles the highlighted row.
    key(&context, KeyCode::Up);
    key(&context, KeyCode::Char(' '));
    active(|form| assert!(!form.questions[1].selected.contains("x")));

    // Enter advances without changing the selection.
    key(&context, KeyCode::Enter);
    active(|form| {
        assert!(form.is_submit_tab());
        assert!(!form.questions[1].selected.contains("x"));
        assert!(form.questions[1].selected.contains("y"));
    });
}

#[test]
fn shift_tab_and_revisit_preserve_editor_answers() {
    reset();
    let context = install_default();

    // Answer q1, move to q2, then Shift+Tab back.
    key(&context, KeyCode::Char('1'));
    key(&context, KeyCode::Enter);
    key_with(&context, KeyCode::BackTab, KeyModifiers::SHIFT, None);
    active(|form| {
        assert_eq!(form.tab, 0);
        assert_eq!(form.questions[0].confirmed.as_deref(), Some("a"));
    });

    // Revisit q2: the multi selection survives.
    key(&context, KeyCode::Tab);
    active(|form| assert_eq!(form.tab, 1));
}

#[test]
fn digits_select_other_and_open_its_editor() {
    reset();
    let context = install_default();
    key(&context, KeyCode::Char('3'));
    active(|form| {
        assert_eq!(form.questions[0].confirmed.as_deref(), Some("__other__"));
        assert_eq!(
            form.editor,
            Some(super::state::EditorFocus {
                question: 0,
                kind: EditorKind::Other
            })
        );
    });
}

#[test]
fn escape_declines_only_outside_an_editor() {
    reset();
    let context = install_default();
    key(&context, KeyCode::Char('n'));
    active(|form| assert!(form.editor.is_some()));

    // Esc leaves the editor without declining.
    assert_eq!(key(&context, KeyCode::Esc), KeyOutcome::Consumed);
    active(|form| {
        assert!(form.editor.is_none());
        assert_eq!(form.submit, SubmitState::Idle);
    });

    match key(&context, KeyCode::Esc) {
        KeyOutcome::Send { response, .. } => {
            assert!(matches!(
                response,
                crate::protocol::elicitation::ElicitationResponse::Decline
            ));
        }
        other => panic!("expected decline, got {other:?}"),
    }
}

#[test]
fn submit_is_blocked_until_every_question_is_answered() {
    reset();
    let context = install_default();
    // Jump to Submit without answering and focus the Submit entry.
    active_mut(|form| form.focus_submit_tab());
    key(&context, KeyCode::Down);
    key(&context, KeyCode::Down);
    assert_eq!(key(&context, KeyCode::Enter), KeyOutcome::Consumed);
    active(|form| assert_eq!(form.submit, SubmitState::Idle));
    assert!(form_count() == 1);

    // Answer q1 and q2, then submit.
    active_mut(|form| form.tab = 0);
    answer_all(&context);
    active(|form| {
        assert!(form.is_submit_tab());
        assert!(form.all_answered());
    });
    match key(&context, KeyCode::Enter) {
        KeyOutcome::Send {
            session_id,
            elicitation_id,
            response,
        } => {
            assert_eq!(session_id, SESSION);
            assert_eq!(elicitation_id, "e1");
            match response {
                crate::protocol::elicitation::ElicitationResponse::Accept { content } => {
                    let content = content.expect("content");
                    assert_eq!(content.get("q1"), Some(&json!("a")));
                    assert_eq!(content.get("q2"), Some(&json!(["x", "y"])));
                }
                other => panic!("expected accept, got {other:?}"),
            }
        }
        other => panic!("expected send, got {other:?}"),
    }
}

#[test]
fn empty_other_counts_as_unanswered_and_blocks_submit() {
    reset();
    let context = install_default();
    // Select Other but type nothing.
    key(&context, KeyCode::Char('3'));
    // Leave the editor; q1 is still unanswered.
    key(&context, KeyCode::Esc);
    active_mut(|form| form.focus_submit_tab());
    key(&context, KeyCode::Down);
    key(&context, KeyCode::Down);
    assert_eq!(key(&context, KeyCode::Enter), KeyOutcome::Consumed);
    active(|form| {
        assert!(!form.questions[0].answered());
        assert_eq!(form.submit, SubmitState::Idle);
        assert!(form.notice.is_some());
    });
}

#[test]
fn repeat_submit_is_dropped_after_send() {
    reset();
    let context = install_default();
    answer_all(&context);
    assert!(matches!(
        key(&context, KeyCode::Enter),
        KeyOutcome::Send { .. }
    ));
    // A second Enter while the response is outstanding must not send again.
    assert_eq!(key(&context, KeyCode::Enter), KeyOutcome::Consumed);
    assert_eq!(key(&context, KeyCode::Char('1')), KeyOutcome::Consumed);
}

#[test]
fn ctrl_modified_keys_are_never_consumed() {
    reset();
    let context = install_default();
    assert_eq!(
        key_with(&context, KeyCode::Char('c'), KeyModifiers::CONTROL, None),
        KeyOutcome::Ignored
    );
    // The form editor did not receive the text.
    key(&context, KeyCode::Char('n'));
    assert_eq!(
        key_with(&context, KeyCode::Char('c'), KeyModifiers::CONTROL, None),
        KeyOutcome::Ignored
    );
    active(|form| assert!(form.questions[0].notes.is_empty()));
}

#[test]
fn notes_editor_reports_over_limit_edit_without_truncating() {
    reset();
    let context = install_default();
    key(&context, KeyCode::Char('n'));
    active_mut(|form| form.questions[0].notes = "x".repeat(MAX_TEXT_CODE_POINTS));
    assert_eq!(
        key_with(&context, KeyCode::Char('y'), KeyModifiers::NONE, Some("y")),
        KeyOutcome::Consumed
    );
    active(|form| {
        assert_eq!(
            form.questions[0].notes.chars().count(),
            MAX_TEXT_CODE_POINTS
        );
        assert!(
            form.notice
                .as_deref()
                .is_some_and(|notice| notice.contains("Text limit"))
        );
    });
}

#[test]
fn summary_navigation_returns_to_a_question_and_submit_sends() {
    reset();
    let context = install_default();
    answer_all(&context);
    active(|form| {
        assert!(form.is_submit_tab());
        assert_eq!(form.summary_index, form.submit_tab());
    });

    // Up selects the last question entry; Enter returns to it.
    key(&context, KeyCode::Up);
    active(|form| assert_eq!(form.summary_index, 1));
    key(&context, KeyCode::Enter);
    active(|form| assert_eq!(form.tab, 1));

    // Revisit Submit and send from the Submit entry.
    active_mut(|form| form.focus_submit_tab());
    assert!(matches!(
        key(&context, KeyCode::Enter),
        KeyOutcome::Send { .. }
    ));
}

// ---------------------------------------------------------------------------
// Events, reconnect and reconciliation
// ---------------------------------------------------------------------------

#[test]
fn untagged_forms_are_ignored_without_a_response() {
    reset();
    let context = context(SESSION);
    let mut untagged = form(
        "applet",
        SESSION,
        json!({
            "type": "object",
            "properties": {"name": {"type": "string", "title": "Name"}}
        }),
    );
    untagged.message = "Applet".into();
    state::with(|state| {
        state.loop_live = true;
        state.generation = 1;
        state.attached = true;
        state.session = Some(SESSION.to_string());
    });
    let (consumed, _) = send_event(&context, &ServerEvent::ElicitationRequest(untagged));
    assert!(consumed);
    assert_eq!(form_count(), 0);
    assert!(state::snapshot(|state| state.last_sent.is_none()));
}

#[test]
fn events_for_another_session_are_ignored() {
    reset();
    install_default();
    let context = context("session-b");
    let (consumed, _) = send_event(
        &context,
        &ServerEvent::ElicitationRequest(form("e2", "session-c", schema_json())),
    );
    assert!(!consumed);
    assert_eq!(form_count(), 1);
    assert_eq!(
        state::snapshot(|state| state.session.clone()),
        Some(SESSION.to_string())
    );
}

#[test]
fn history_preserves_forms_drafts_and_response_ids() {
    reset();
    let context = install_default();
    key(&context, KeyCode::Char('n'));
    key_with(&context, KeyCode::Char('h'), KeyModifiers::NONE, Some("h"));
    key(&context, KeyCode::Esc);
    answer_all(&context);
    assert!(matches!(
        key(&context, KeyCode::Enter),
        KeyOutcome::Send { .. }
    ));
    state::with(|state| reconcile::record_response_id(state, 7));

    // History is not replay completion.
    let history: ServerEvent = serde_json::from_value(json!({
        "type": "history",
        "id": 1,
        "session_id": SESSION,
        "messages": []
    }))
    .expect("history event");
    let (consumed, _) = send_event(&context, &history);
    assert!(!consumed);
    assert_eq!(form_count(), 1);
    assert_eq!(response_ids(), vec![7]);
    active(|form| assert_eq!(form.questions[0].notes, "h"));
}

#[test]
fn resolution_removes_the_form_and_late_acknowledgements_do_not_resurrect_it() {
    reset();
    let context = install_default();
    answer_all(&context);
    assert!(matches!(
        key(&context, KeyCode::Enter),
        KeyOutcome::Send { .. }
    ));
    state::with(|state| reconcile::record_response_id(state, 11));

    let (consumed, notice) = send_event(
        &context,
        &ServerEvent::ElicitationResolved {
            session_id: SESSION.into(),
            elicitation_id: "e1".into(),
            outcome: ElicitationResolution::Accepted,
        },
    );
    assert!(consumed);
    assert_eq!(notice.as_deref(), Some("Answer submitted."));
    assert_eq!(form_count(), 0);

    // Claimed but stale: consumed, nothing resurrected.
    assert!(
        send_event(
            &context,
            &ServerEvent::Error {
                id: 11,
                message: "late".into(),
                retry_after_secs: None
            }
        )
        .0
    );
    assert_eq!(form_count(), 0);
}

#[test]
fn claimed_error_reopens_a_pending_form_with_a_notice() {
    reset();
    let context = install_default();
    answer_all(&context);
    assert!(matches!(
        key(&context, KeyCode::Enter),
        KeyOutcome::Send { .. }
    ));
    state::with(|state| reconcile::record_response_id(state, 21));

    let (consumed, notice) = send_event(
        &context,
        &ServerEvent::Error {
            id: 21,
            message: "nope".into(),
            retry_after_secs: None,
        },
    );
    assert!(consumed);
    assert!(
        notice
            .as_deref()
            .is_some_and(|notice| notice.contains("rejected"))
    );
    assert_eq!(form_count(), 1);
    active(|form| {
        assert!(matches!(form.submit, SubmitState::Rejected(_)));
        assert_eq!(form.notice.as_deref(), Some("nope"));
    });
}

#[test]
fn claimed_done_is_consumed_and_forgets_the_response_id() {
    reset();
    let context = install_default();
    answer_all(&context);
    assert!(matches!(
        key(&context, KeyCode::Enter),
        KeyOutcome::Send { .. }
    ));
    state::with(|state| reconcile::record_response_id(state, 31));
    assert!(send_event(&context, &ServerEvent::Done { id: 31 }).0);
    assert!(response_ids().is_empty());
    assert_eq!(form_count(), 1);
}

#[test]
fn send_failure_keeps_the_draft_and_rejected_resolution_wins_later() {
    reset();
    let context = install_default();
    answer_all(&context);
    assert!(matches!(
        key(&context, KeyCode::Enter),
        KeyOutcome::Send { .. }
    ));
    let applied = state::with(|state| reconcile::send_failed(state, &context, "offline"));
    assert!(applied);
    active(|form| {
        assert!(matches!(form.submit, SubmitState::Rejected(_)));
        assert!(form.all_answered());
    });

    // Resolution removes the form; a later failure is ignored.
    assert!(
        send_event(
            &context,
            &ServerEvent::ElicitationResolved {
                session_id: SESSION.into(),
                elicitation_id: "e1".into(),
                outcome: ElicitationResolution::Cancelled,
            }
        )
        .0
    );
    assert_eq!(form_count(), 0);
    assert!(!state::with(|state| reconcile::send_failed(
        state, &context, "late"
    )));
}

#[test]
fn connection_replacement_freezes_drafts_and_replay_restores_them() {
    reset();
    let context = install_default();
    key(&context, KeyCode::Char('n'));
    key_with(&context, KeyCode::Char('d'), KeyModifiers::NONE, Some("d"));
    key(&context, KeyCode::Esc);
    key(&context, KeyCode::Char('2'));

    state::with(|state| reconcile::connection_replaced(state, Some(SESSION)));
    assert_eq!(form_count(), 1);
    assert!(state::snapshot(|state| !state.attached));
    active(|form| assert!(form.seen_generation.is_none()));
    // Hidden forms do not capture keys.
    assert_eq!(key(&context, KeyCode::Enter), KeyOutcome::Ignored);

    // Identical replay restores selection, notes and tab.
    let mut replayed = form("e1", SESSION, schema_json());
    replayed.message = "Again".into();
    let (consumed, _) = send_event(&context, &ServerEvent::ElicitationRequest(replayed));
    assert!(consumed);
    assert_eq!(form_count(), 1);
    active(|form| {
        assert!(state_seen(form));
        assert_eq!(form.questions[0].notes, "d");
        assert_eq!(form.questions[0].confirmed.as_deref(), Some("b"));
    });

    let notice = state::with(|state| reconcile::on_replay_complete(state, &context, SESSION));
    assert!(notice.is_none());
    assert!(state::snapshot(|state| state.attached));
    assert_eq!(form_count(), 1);
}

fn state_seen(form: &Form) -> bool {
    form.seen_generation.is_some()
}

#[test]
fn replay_complete_discards_unreplayed_forms() {
    reset();
    let context = install_default();
    state::with(|state| {
        state.forms.push(
            Form::new(&form("e2", SESSION, schema_json()), state.generation).expect("second form"),
        );
    });
    assert_eq!(form_count(), 2);

    state::with(|state| reconcile::connection_replaced(state, Some(SESSION)));
    // Replay only the first form.
    let (consumed, _) = send_event(
        &context,
        &ServerEvent::ElicitationRequest(form("e1", SESSION, schema_json())),
    );
    assert!(consumed);
    let notice = state::with(|state| reconcile::on_replay_complete(state, &context, SESSION));
    assert_eq!(form_count(), 1);
    assert!(
        notice
            .as_deref()
            .is_some_and(|notice| notice.contains("lost while reconnecting"))
    );
}

#[test]
fn resolved_while_disconnected_removes_the_draft() {
    reset();
    let context = install_default();
    key(&context, KeyCode::Char('n'));
    key_with(&context, KeyCode::Char('x'), KeyModifiers::NONE, Some("x"));
    state::with(|state| reconcile::connection_replaced(state, Some(SESSION)));
    assert_eq!(form_count(), 1);

    let (consumed, notice) = send_event(
        &context,
        &ServerEvent::ElicitationResolved {
            session_id: SESSION.into(),
            elicitation_id: "e1".into(),
            outcome: ElicitationResolution::Declined,
        },
    );
    assert!(consumed);
    assert_eq!(notice.as_deref(), Some("Question form declined."));
    assert_eq!(form_count(), 0);

    // Replay completion with nothing replayed keeps it gone.
    state::with(|state| reconcile::on_replay_complete(state, &context, SESSION));
    assert_eq!(form_count(), 0);
}

#[test]
fn session_change_clears_old_drafts_immediately() {
    reset();
    install_default();
    state::with(|state| reconcile::connection_replaced(state, Some("session-b")));
    assert_eq!(form_count(), 0);
    assert_eq!(
        state::snapshot(|state| state.session.clone()),
        Some("session-b".to_string())
    );
}

#[test]
fn replay_mode_never_answers_requests() {
    reset();
    let context = ViewContext {
        session: Some(SESSION.into()),
        replay: true,
        blocked: None,
    };
    let (consumed, _) = send_event(
        &context,
        &ServerEvent::ElicitationRequest(form("e1", SESSION, schema_json())),
    );
    assert!(!consumed);
    assert_eq!(form_count(), 0);
}

// ---------------------------------------------------------------------------
// Focus precedence
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FakeProbe {
    session: Option<String>,
    replay: bool,
    history: bool,
    onboarding: bool,
    copy_selection: bool,
    commit_log: bool,
    changelog: bool,
    help: bool,
    model_status: bool,
    panel_image: bool,
    session_picker: bool,
    login_picker: bool,
    account_picker: bool,
    usage_overlay: bool,
    inline_interactive: bool,
    pending_login: bool,
    swarm_page: bool,
    side_panel: bool,
}

impl FocusProbe for FakeProbe {
    fn probe_session(&self) -> Option<String> {
        self.session.clone()
    }
    fn probe_replay(&self) -> bool {
        self.replay
    }
    fn probe_prompt_history(&self) -> bool {
        self.history
    }
    fn probe_onboarding(&self) -> bool {
        self.onboarding
    }
    fn probe_copy_selection(&self) -> bool {
        self.copy_selection
    }
    fn probe_commit_log(&self) -> bool {
        self.commit_log
    }
    fn probe_changelog(&self) -> bool {
        self.changelog
    }
    fn probe_help(&self) -> bool {
        self.help
    }
    fn probe_model_status(&self) -> bool {
        self.model_status
    }
    fn probe_panel_image(&self) -> bool {
        self.panel_image
    }
    fn probe_session_picker(&self) -> bool {
        self.session_picker
    }
    fn probe_login_picker(&self) -> bool {
        self.login_picker
    }
    fn probe_account_picker(&self) -> bool {
        self.account_picker
    }
    fn probe_usage_overlay(&self) -> bool {
        self.usage_overlay
    }
    fn probe_inline_interactive(&self) -> bool {
        self.inline_interactive
    }
    fn probe_pending_login(&self) -> bool {
        self.pending_login
    }
    fn probe_swarm_page(&self) -> bool {
        self.swarm_page
    }
    fn probe_side_panel_fullscreen(&self) -> bool {
        self.side_panel
    }
}

fn blocked_probe(reason: &str) -> FakeProbe {
    let mut probe = FakeProbe {
        session: Some(SESSION.into()),
        ..FakeProbe::default()
    };
    match reason {
        "history" => probe.history = true,
        "onboarding" => probe.onboarding = true,
        "copy" => probe.copy_selection = true,
        "commit" => probe.commit_log = true,
        "changelog" => probe.changelog = true,
        "help" => probe.help = true,
        "model" => probe.model_status = true,
        "image" => probe.panel_image = true,
        "session" => probe.session_picker = true,
        "login-picker" => probe.login_picker = true,
        "account" => probe.account_picker = true,
        "usage" => probe.usage_overlay = true,
        "inline" => probe.inline_interactive = true,
        "login" => probe.pending_login = true,
        "swarm" => probe.swarm_page = true,
        "side" => probe.side_panel = true,
        other => panic!("unknown probe {other}"),
    }
    probe
}

#[test]
fn every_higher_priority_ui_blocks_render_keys_and_paste() {
    reset();
    install_default();
    for reason in [
        "history",
        "onboarding",
        "copy",
        "commit",
        "changelog",
        "help",
        "model",
        "image",
        "session",
        "login-picker",
        "account",
        "usage",
        "inline",
        "login",
        "swarm",
        "side",
    ] {
        let probe = blocked_probe(reason);
        let context = ViewContext::from_probe(&probe);
        assert!(
            context.blocked.is_some(),
            "{reason} should own focus, context {context:?}"
        );
        assert_eq!(
            key(&context, KeyCode::Enter),
            KeyOutcome::Ignored,
            "{reason}"
        );
        assert!(!paste_text(&context, "hello"), "{reason}");
    }
}

#[test]
fn picker_opened_during_note_editing_suspends_and_resumes_the_draft() {
    reset();
    let context = install_default();
    key(&context, KeyCode::Char('n'));
    key_with(&context, KeyCode::Char('d'), KeyModifiers::NONE, Some("d"));

    let blocked = ViewContext::from_probe(&blocked_probe("inline"));
    assert_eq!(key(&blocked, KeyCode::Char('x')), KeyOutcome::Ignored);
    assert!(!paste_text(&blocked, "hidden"));
    active(|form| assert_eq!(form.questions[0].notes, "d"));

    // Closing the picker resumes the same editor.
    key_with(&context, KeyCode::Char('y'), KeyModifiers::NONE, Some("y"));
    active(|form| {
        assert_eq!(form.questions[0].notes, "dy");
        assert_eq!(
            form.editor,
            Some(super::state::EditorFocus {
                question: 0,
                kind: EditorKind::Notes
            })
        );
    });
}

#[test]
fn hidden_forms_do_not_render_or_take_paste() {
    reset();
    install_default();
    let blocked = ViewContext::from_probe(&blocked_probe("history"));
    let buffer = render_buffer(&blocked, 80, 24);
    assert!(first_nonblank_row(&buffer).is_none());
    assert!(!paste_text(&blocked, "text"));
}

// ---------------------------------------------------------------------------
// Flash helpers used by the render tests
// ---------------------------------------------------------------------------

pub(super) fn render_buffer(context: &ViewContext, width: u16, height: u16) -> Buffer {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            state::with(|state| {
                form_render::render_view(frame, state, context, frame.area());
            });
        })
        .expect("draw");
    terminal.backend().buffer().clone()
}
