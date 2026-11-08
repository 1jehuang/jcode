use super::*;
use ratatui::{Terminal, backend::TestBackend};

fn fixture() -> SessionInfo {
    let now = chrono::Utc::now();
    SessionInfo {
        id: "faithful_refresh".into(),
        parent_id: None,
        short_name: "faithful".into(),
        icon: "🦊".into(),
        title: "Faithful preview".into(),
        message_count: 2,
        user_message_count: 1,
        assistant_message_count: 1,
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
        estimated_tokens: 42,
        first_user_prompt: None,
        messages_preview: vec![],
        search_index: "faithful preview".into(),
        server_name: None,
        server_icon: None,
        source: SessionSource::Jcode,
        resume_target: ResumeTarget::JcodeSession {
            session_id: "faithful_refresh".into(),
        },
        external_path: None,
        todo_statuses: Vec::new(),
    }
}

fn message(role: &str, content: &str) -> PreviewMessage {
    PreviewMessage {
        role: role.into(),
        content: content.into(),
        tool_calls: vec![],
        tool_data: None,
        duration_secs: None,
        title: None,
        timestamp: None,
    }
}

#[test]
fn populated_transcript_still_loads_missing_preview_metadata() {
    let mut snapshot = fixture();
    snapshot
        .messages_preview
        .push(message("assistant", "Already cached"));
    let mut picker = SessionPicker::new(vec![snapshot]);
    picker.ensure_selected_preview_loading();
    assert!(picker.pending_preview_load.is_some());
}

#[test]
fn preview_metadata_refreshes_after_freshness_window() {
    let snapshot = fixture();
    let mut picker = SessionPicker::new(vec![snapshot.clone()]);
    picker.preview_extras.insert(
        snapshot.id.clone(),
        PreviewExtras {
            loaded_at: Some(std::time::Instant::now()),
            ..Default::default()
        },
    );
    picker.ensure_selected_preview_loading();
    assert!(picker.pending_preview_load.is_none());
    picker
        .preview_extras
        .get_mut(&snapshot.id)
        .unwrap()
        .loaded_at = Some(std::time::Instant::now() - Duration::from_secs(16));
    picker.ensure_selected_preview_loading();
    assert!(picker.pending_preview_load.is_some());
}

#[test]
fn metadata_failure_does_not_invalidate_a_populated_transcript() {
    let mut snapshot = fixture();
    snapshot
        .messages_preview
        .push(message("assistant", "Cached conversation"));
    let mut picker = SessionPicker::new(vec![snapshot.clone()]);
    let mode = crate::config::config().display.diff_mode;
    let before = picker.preview_content_hash(&snapshot, mode);
    picker.preview_load_failures.insert(snapshot.id.clone());
    assert_eq!(before, picker.preview_content_hash(&snapshot, mode));
}

#[test]
fn workspace_change_keeps_transcript_but_rejects_old_metadata_and_pending_result() {
    let snapshot = fixture();
    let mut picker = SessionPicker::new(vec![snapshot.clone()]);
    deliver(
        &mut picker,
        vec![message("assistant", "Retained conversation")],
    );
    let (sender, receiver) = std::sync::mpsc::channel();
    picker.pending_preview_load = Some(PendingSessionPreviewLoad {
        session_id: snapshot.id.clone(),
        receiver,
    });
    let mut changed = snapshot;
    changed.working_dir = Some("/different/workspace".into());
    picker.reseed_grouped(vec![], vec![changed]);
    assert!(!picker.preview_extras.contains_key("faithful_refresh"));
    assert!(picker.pending_preview_load.is_none());
    assert!(
        sender.send(None).is_err(),
        "obsolete loader receiver must be dropped"
    );
    assert_eq!(
        picker.all_sessions[0].messages_preview[0].content,
        "Retained conversation"
    );
}

#[test]
fn refresh_equality_includes_duration_and_title() {
    let mut old = fixture();
    let mut row = message("tool", "completed");
    row.duration_secs = Some(1.25);
    row.title = Some("Build".into());
    old.messages_preview.push(row);
    assert!(same_transcript(&old, &old));
    let mut changed = old.clone();
    changed.messages_preview[0].duration_secs = Some(2.5);
    assert!(!same_transcript(&old, &changed));
    changed = old.clone();
    changed.messages_preview[0].title = Some("Test".into());
    assert!(!same_transcript(&old, &changed));
    changed.messages_preview.clear();
    assert!(same_transcript(&old, &changed));
}

fn deliver(picker: &mut SessionPicker, messages: Vec<PreviewMessage>) {
    let (sender, receiver) = std::sync::mpsc::channel();
    picker.pending_preview_load = Some(PendingSessionPreviewLoad {
        session_id: "faithful_refresh".into(),
        receiver,
    });
    sender
        .send(Some(LoadedSessionPreview {
            messages,
            extras: PreviewExtras {
                hidden_prompts: 7,
                ..Default::default()
            },
        }))
        .unwrap();
    assert!(picker.poll_preview_load());
}

fn render(picker: &mut SessionPicker) -> String {
    let mut terminal = Terminal::new(TestBackend::new(160, 32)).unwrap();
    terminal.draw(|frame| picker.render(frame)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn loaded_conversation_survives_metadata_only_refresh_and_group_change() {
    let snapshot = fixture();
    let mut picker = SessionPicker::new(vec![snapshot.clone()]);
    deliver(
        &mut picker,
        vec![
            message("user", "Faithful user prompt"),
            message("assistant", "Faithful assistant response"),
        ],
    );
    assert!(render(&mut picker).contains("Faithful assistant response"));

    // Reproduce the screenshot: the preview finishes before the full list scan,
    // whose summaries intentionally contain no messages_preview.
    picker.reseed_grouped(
        vec![ServerGroup {
            name: "live-server".into(),
            icon: "s".into(),
            version: String::new(),
            git_hash: String::new(),
            is_running: true,
            sessions: vec![snapshot.clone()],
        }],
        vec![],
    );
    let frame = render(&mut picker);
    assert!(frame.contains("Faithful user prompt"), "{frame}");
    assert!(frame.contains("Faithful assistant response"), "{frame}");
    assert!(!frame.contains("(empty session)"));
    assert!(picker.pending_preview_load.is_none());
    assert_eq!(picker.preview_extras["faithful_refresh"].hidden_prompts, 7);

    picker.reseed_grouped(vec![], vec![snapshot]);
    let frame = render(&mut picker);
    assert!(frame.contains("Faithful assistant response"));
    assert!(!frame.contains("(empty session)"));
}

#[test]
fn changed_transcript_invalidates_loaded_state_and_reloads_new_messages() {
    let snapshot = fixture();
    let mut picker = SessionPicker::new(vec![snapshot.clone()]);
    deliver(
        &mut picker,
        vec![message("assistant", "Old assistant content")],
    );
    let mut changed = snapshot;
    changed.last_message_time += chrono::Duration::seconds(1);
    picker.reseed_grouped(vec![], vec![changed]);
    assert!(!picker.preview_extras.contains_key("faithful_refresh"));
    assert!(
        picker
            .selected_session()
            .unwrap()
            .messages_preview
            .is_empty()
    );
    picker.ensure_selected_preview_loading();
    assert!(
        picker.pending_preview_load.is_some(),
        "changed transcript must not remain marked already loaded"
    );
    deliver(
        &mut picker,
        vec![message("assistant", "New assistant content")],
    );
    let frame = render(&mut picker);
    assert!(frame.contains("New assistant content"));
    assert!(!frame.contains("Old assistant content"));
}

#[test]
fn refresh_discards_stale_inflight_load_but_preserves_unchanged_one() {
    let snapshot = fixture();
    let mut picker = SessionPicker::new(vec![snapshot.clone()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    picker.pending_preview_load = Some(PendingSessionPreviewLoad {
        session_id: snapshot.id.clone(),
        receiver,
    });
    picker.reseed_grouped(vec![], vec![snapshot.clone()]);
    assert!(picker.pending_preview_load.is_some());
    let mut changed = snapshot;
    changed.message_count += 1;
    picker.reseed_grouped(vec![], vec![changed]);
    assert!(picker.pending_preview_load.is_none());
    assert!(
        sender.send(None).is_err(),
        "stale worker must no longer publish"
    );
}

#[test]
fn genuinely_empty_completed_preview_remains_loaded_and_removed_sessions_release_extras() {
    let mut snapshot = fixture();
    snapshot.message_count = 0;
    snapshot.user_message_count = 0;
    snapshot.assistant_message_count = 0;
    let mut picker = SessionPicker::new(vec![snapshot.clone()]);
    deliver(&mut picker, vec![]);
    picker.reseed_grouped(vec![], vec![snapshot]);
    picker.ensure_selected_preview_loading();
    assert!(picker.pending_preview_load.is_none());
    assert!(render(&mut picker).contains("(empty session)"));
    picker.reseed_grouped(vec![], vec![]);
    assert!(picker.preview_extras.is_empty());
}

#[test]
fn resume_time_modes_pick_expected_timestamp() {
    use super::super::time_labels::session_resume_time_for;
    use crate::config::ResumeTimeMode;
    use chrono::Duration;

    let now = chrono::Utc::now();
    let mut session = fixture();
    session.created_at = now - Duration::days(3);
    session.last_message_time = now - Duration::hours(2);
    session.last_active_at = Some(now - Duration::minutes(5));

    let at = |s: &SessionInfo, mode| session_resume_time_for(s, mode);
    // Opening without working must not refresh the default label.
    assert_eq!(
        at(&session, ResumeTimeMode::Message),
        now - Duration::hours(2)
    );
    assert_eq!(
        at(&session, ResumeTimeMode::Activity),
        now - Duration::minutes(5)
    );
    assert_eq!(
        at(&session, ResumeTimeMode::Opened),
        now - Duration::minutes(5)
    );
    assert_eq!(
        at(&session, ResumeTimeMode::Created),
        now - Duration::days(3)
    );

    // Long run after opening: message and activity follow the newest message.
    session.last_message_time = now - Duration::minutes(1);
    assert_eq!(
        at(&session, ResumeTimeMode::Message),
        now - Duration::minutes(1)
    );
    assert_eq!(
        at(&session, ResumeTimeMode::Activity),
        now - Duration::minutes(1)
    );
    assert_eq!(
        at(&session, ResumeTimeMode::Opened),
        now - Duration::minutes(5)
    );
}

#[test]
fn running_session_shows_spinner_state_instead_of_age() {
    let session = fixture();
    let mut picker = SessionPicker::new(vec![session.clone()]);
    let text = |picker: &SessionPicker| {
        picker
            .render_session_item_lines_at_frame(&session, false, 0)
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
            .collect::<String>()
    };
    assert!(text(&picker).contains("ago"));

    picker.live_presence.insert(
        session.id.clone(),
        crate::session::SessionPresence {
            session_id: session.id.clone(),
            pid: 1,
            streaming: true,
            streaming_since: None,
            internal: false,
        },
    );
    let running = text(&picker);
    assert!(running.contains("running"), "row: {running}");
    assert!(!running.contains("ago"), "row: {running}");
}
