use super::*;

/// Copy the highlighted session without changing selection and report failures.
#[test]
fn copy_session_identifier_reports_success_and_failure() {
    let session = make_session("session_alpha", "alpha", false, SessionStatus::Closed);
    let mut picker = SessionPicker::new(vec![session]);
    let mut copied = String::new();
    picker.copy_selected_session_with(|id| {
        copied = id.to_owned();
        true
    });
    assert_eq!(copied, "session_alpha");
    assert_eq!(picker.copy_notice, Some("Session ID copied"));
    assert_eq!(picker.selected_session().unwrap().id, "session_alpha");
    picker.copy_selected_session_with(|_| false);
    assert_eq!(picker.copy_notice, Some("Could not copy to clipboard"));
}

/// Copy the actual external identifier and label Pi's path accurately.
#[test]
fn copy_external_session_uses_resume_target() {
    let mut session = make_session("internal-id", "external", false, SessionStatus::Closed);
    session.resume_target = ResumeTarget::PiSession {
        session_path: "pi/session.jsonl".into(),
    };
    let mut picker = SessionPicker::new(vec![session]);
    picker.copy_selected_session_with(|id| {
        assert_eq!(id, "pi/session.jsonl");
        true
    });
    assert_eq!(picker.copy_notice, Some("Session path copied"));
}

/// Search still accepts y as text and the copy action handles an empty list.
#[test]
fn copy_shortcut_does_not_interfere_with_search_or_empty_picker() {
    let mut picker = SessionPicker::new(Vec::new());
    picker
        .handle_overlay_key(KeyCode::Char('y'), KeyModifiers::empty())
        .unwrap();
    assert_eq!(picker.copy_notice, Some("No session selected"));
    picker
        .handle_overlay_key(KeyCode::Char('/'), KeyModifiers::empty())
        .unwrap();
    picker
        .handle_overlay_key(KeyCode::Char('y'), KeyModifiers::empty())
        .unwrap();
    assert_eq!(picker.search_query, "y");
    assert_eq!(picker.copy_notice, None);
}

/// The picker shortcut invokes the shared clipboard implementation.
#[test]
fn copy_shortcut_copies_highlighted_session() {
    let _lock = crate::tui::ui::render_state_test_lock();
    let mut picker = SessionPicker::new(vec![make_session(
        "session_alpha",
        "alpha",
        false,
        SessionStatus::Closed,
    )]);
    picker
        .handle_overlay_key(KeyCode::Char('y'), KeyModifiers::empty())
        .unwrap();
    assert_eq!(picker.copy_notice, Some("Session ID copied"));
}

/// The picker advertises the shortcut and shows the result in its footer.
#[test]
fn copy_shortcut_and_result_are_visible() {
    let _lock = crate::tui::ui::render_state_test_lock();
    let mut picker = SessionPicker::new(vec![make_session(
        "session_alpha",
        "alpha",
        false,
        SessionStatus::Closed,
    )]);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    terminal.draw(|frame| picker.render(frame)).unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("y copy ID"));
    picker
        .handle_overlay_key(KeyCode::Char('y'), KeyModifiers::empty())
        .unwrap();
    terminal.draw(|frame| picker.render(frame)).unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("Session ID copied"));
}

#[test]
fn cloud_filter_supports_flat_sessions_and_debug_visibility() {
    let local = make_session("local", "local", false, SessionStatus::Closed);
    let mut cloud = make_session("cloud", "cloud", false, SessionStatus::Closed);
    cloud.server_icon = Some("☁".into());
    let mut debug_cloud = cloud.clone();
    debug_cloud.id = "debug_cloud".into();
    debug_cloud.is_debug = true;
    let mut picker = SessionPicker::new(vec![local, cloud, debug_cloud]);
    picker.filter_mode = SessionFilterMode::Cloud;
    picker.rebuild_items();
    assert_eq!(picker.visible_sessions.len(), 1);
    assert_eq!(picker.selected_session().unwrap().id, "cloud");
    assert_eq!(picker.hidden_test_count, 1);
    picker.toggle_test_sessions();
    assert_eq!(picker.visible_sessions.len(), 2);
    assert_eq!(picker.hidden_test_count, 0);
    picker.cycle_filter_mode_backwards();
    assert_eq!(picker.filter_mode, SessionFilterMode::All);
    picker.cycle_filter_mode();
    assert_eq!(picker.filter_mode, SessionFilterMode::Cloud);
}

#[test]
fn cloud_history_mixes_with_local_and_preserves_real_servers_and_saved_boost() {
    let now = Utc::now();
    let mut local = make_session("local", "local", false, SessionStatus::Closed);
    local.last_message_time = now;
    let mut cloud = make_session("cloud", "cloud", false, SessionStatus::Closed);
    cloud.server_name = Some("cloud: host".into());
    cloud.server_icon = Some("☁".into());
    cloud.last_message_time = now - ChronoDuration::minutes(2);
    let mut old_local = local.clone();
    old_local.id = "old_local".into();
    old_local.last_message_time = now - ChronoDuration::minutes(3);
    let mut saved_cloud = cloud.clone();
    saved_cloud.id = "saved_cloud".into();
    saved_cloud.saved = true;
    let cloud_group = ServerGroup {
        name: "cloud: host".into(),
        icon: "☁".into(),
        version: String::new(),
        git_hash: String::new(),
        is_running: false,
        sessions: vec![cloud, saved_cloud],
    };
    let mut picker = SessionPicker::new_grouped(vec![cloud_group], vec![old_local, local]);
    let ids = |picker: &SessionPicker| {
        picker
            .visible_session_iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&picker), ["saved_cloud", "local", "cloud", "old_local"]);
    assert!(matches!(
        picker.items[0],
        PickerItem::SavedHeader { session_count: 1 }
    ));
    assert!(!picker.items.iter().any(|item| matches!(
        item,
        PickerItem::ServerHeader { .. } | PickerItem::OrphanHeader { .. }
    )));

    let real = make_session("real", "real", false, SessionStatus::Closed);
    picker.all_server_groups.push(ServerGroup {
        name: "real server".into(),
        icon: "🛰".into(),
        version: "1".into(),
        git_hash: String::new(),
        is_running: true,
        sessions: vec![real],
    });
    picker.rebuild_items();
    assert_eq!(
        ids(&picker),
        ["saved_cloud", "real", "local", "cloud", "old_local"]
    );
    assert!(picker.items.iter().any(|item| matches!(item, PickerItem::ServerHeader { name, session_count: 1, .. } if name == "real server")));
    picker.filter_mode = SessionFilterMode::Cloud;
    picker.rebuild_items();
    assert_eq!(ids(&picker), ["cloud", "saved_cloud"]);
    assert!(
        picker
            .items
            .iter()
            .all(|item| matches!(item, PickerItem::Session))
    );
    picker.search_query = "local".into();
    picker.rebuild_items();
    wait_for_picker_search(&mut picker);
    assert!(picker.visible_sessions.is_empty());
}
