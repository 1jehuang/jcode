//! Deterministic queued-Enter tests. The channel holds the search open without
//! filesystem delays or sleeping for the one-second query window.
use super::super::{OverlayAction, PickerResult, SessionPicker, SessionSource, SessionStatus};
use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

fn session(id: &str) -> SessionInfo {
    let now = chrono::Utc::now();
    SessionInfo {
        id: id.into(),
        parent_id: None,
        short_name: id.into(),
        icon: "x".into(),
        title: "Metadata title".into(),
        message_count: 1,
        user_message_count: 1,
        assistant_message_count: 0,
        created_at: now,
        last_message_time: now,
        last_active_at: None,
        working_dir: None,
        model: None,
        provider_key: None,
        is_canary: false,
        is_debug: false,
        saved: false,
        save_label: None,
        status: SessionStatus::Closed,
        needs_catchup: false,
        estimated_tokens: 1,
        first_user_prompt: None,
        messages_preview: Vec::new(),
        search_index: if id == "stale" { "needle" } else { "metadata" }.into(),
        server_name: None,
        server_icon: None,
        source: SessionSource::Codex,
        resume_target: ResumeTarget::CodexSession {
            session_id: id.into(),
            session_path: "unused".into(),
        },
        external_path: Some("unused".into()),
        todo_statuses: Vec::new(),
    }
}

fn pending_picker() -> (SessionPicker, mpsc::Sender<(String, SearchRank)>) {
    let mut picker = SessionPicker::new(vec![session("stale"), session("best")]);
    picker.search_active = true;
    for c in "needle".chars() {
        picker
            .handle_overlay_key(KeyCode::Char(c), KeyModifiers::NONE)
            .unwrap();
    }
    picker.rebuild_items();
    picker.transcript_search.ranks.insert(
        "stale".into(),
        literal_metadata_rank("needle", &["needle".into()]),
    );
    picker.rebuild_items();
    assert_eq!(picker.selected_session().unwrap().id, "stale");
    let (sender, receiver) = mpsc::channel();
    picker.transcript_search.started_at = None;
    picker.transcript_search.receiver = Some(receiver);
    (picker, sender)
}

fn better_result(sender: &mpsc::Sender<(String, SearchRank)>) {
    sender
        .send((
            "best".into(),
            SearchRank {
                opening_prompt: 10,
                literal: true,
                ..SearchRank::default()
            },
        ))
        .unwrap();
}

fn enter(picker: &mut SessionPicker) -> OverlayAction {
    picker
        .handle_overlay_key(KeyCode::Enter, KeyModifiers::NONE)
        .unwrap()
}

fn selected_id(action: Option<OverlayAction>) -> String {
    let Some(OverlayAction::Selected(result)) = action else {
        panic!("expected selected action");
    };
    let (PickerResult::Selected(targets)
    | PickerResult::SelectedInCurrentTerminal(targets)
    | PickerResult::SelectedInNewTerminal(targets)) = result
    else {
        panic!("expected resume targets");
    };
    let ResumeTarget::CodexSession { session_id, .. } = &targets[0] else {
        panic!("expected Codex target");
    };
    session_id.clone()
}

#[test]
fn immediate_type_enter_waits_without_blocking_then_opens_best_on_completion() {
    let (mut picker, sender) = pending_picker();
    assert!(matches!(enter(&mut picker), OverlayAction::Continue));
    assert!(picker.search_submission_pending());
    assert!(picker.poll_search_submission().is_none());
    assert_eq!(picker.selected_session().unwrap().id, "stale");
    better_result(&sender);
    drop(sender);
    assert_eq!(selected_id(picker.poll_search_submission()), "best");
    assert!(!picker.search_submission_pending());
    assert!(picker.poll_search_submission().is_none());
}

#[test]
fn deadline_opens_best_available_even_if_worker_is_still_pending() {
    let (mut picker, sender) = pending_picker();
    enter(&mut picker);
    better_result(&sender);
    picker.search_selection.expire_for_test();
    assert_eq!(selected_id(picker.poll_search_submission()), "best");
    assert!(picker.transcript_search.pending());
}

#[test]
fn repeated_enter_cannot_extend_query_deadline_or_open_stale_selection() {
    let (mut picker, sender) = pending_picker();
    enter(&mut picker);
    assert!(matches!(enter(&mut picker), OverlayAction::Continue));
    better_result(&sender);
    picker.search_selection.expire_for_test();
    assert_eq!(selected_id(Some(enter(&mut picker))), "best");
}

#[test]
fn editing_or_manual_navigation_cancels_queued_enter() {
    for code in [
        KeyCode::Down,
        KeyCode::Up,
        KeyCode::Backspace,
        KeyCode::Char('x'),
        KeyCode::Esc,
    ] {
        let (mut picker, sender) = pending_picker();
        enter(&mut picker);
        picker.handle_overlay_key(code, KeyModifiers::NONE).unwrap();
        assert!(!picker.search_submission_pending(), "{code:?}");
        // Editing may replace the receiver and cancel the old worker.
        let _ = sender.send(("best".into(), SearchRank::default()));
        drop(sender);
        assert!(picker.poll_search_submission().is_none(), "{code:?}");
    }
}

#[test]
fn direct_navigation_and_preview_scroll_cancel_queued_enter() {
    for mode in 0..4 {
        let (mut picker, _sender) = pending_picker();
        enter(&mut picker);
        match mode {
            0 => picker.next(),
            1 => picker.previous(),
            2 => {
                picker.scroll_preview_up(3);
            }
            _ => {
                picker.scroll_preview_down(3);
            }
        }
        assert!(!picker.search_submission_pending());
        picker.search_selection.expire_for_test();
        assert!(picker.poll_search_submission().is_none());
    }
}

#[test]
fn manual_selection_submits_immediately_without_following_background_results() {
    let (mut picker, _sender) = pending_picker();
    picker.next();
    assert_eq!(selected_id(Some(enter(&mut picker))), "stale");
    assert!(!picker.search_submission_pending());
}

#[test]
fn no_matches_at_deadline_keeps_query_and_does_not_submit_later() {
    let (mut picker, sender) = pending_picker();
    picker.all_sessions[0].search_index = "unrelated".into();
    picker.transcript_search.ranks.clear();
    picker.rebuild_items();
    assert!(picker.visible_sessions.is_empty());
    enter(&mut picker);
    assert!(picker.search_submission_pending());
    picker.search_selection.expire_for_test();
    assert!(picker.poll_search_submission().is_none());
    assert_eq!(picker.search_query, "needle");
    assert!(!picker.search_submission_pending());
    better_result(&sender);
    drop(sender);
    assert!(picker.poll_search_submission().is_none());
}

#[test]
fn loading_picker_never_queues_enter() {
    let mut picker = SessionPicker::loading();
    picker.search_active = true;
    picker
        .handle_overlay_key(KeyCode::Char('n'), KeyModifiers::NONE)
        .unwrap();
    assert!(matches!(enter(&mut picker), OverlayAction::Continue));
    assert!(!picker.search_submission_pending());
}
