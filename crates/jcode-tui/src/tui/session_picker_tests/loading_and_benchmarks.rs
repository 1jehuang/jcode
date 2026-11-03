use super::*;

#[test]
fn standalone_resume_renders_before_list_load_finishes() {
    let mut picker = SessionPicker::loading();
    let (_sender, receiver) = std::sync::mpsc::channel::<SessionListUpdate>();
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).expect("test terminal");
    terminal
        .draw(|frame| picker.render(frame))
        .expect("loading frame");
    assert!(matches!(
        receiver.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert!(picker.loading_message.is_some());
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("Loading sessions"));
}

#[test]
fn shortcut_search_accepts_typing_before_sessions_arrive() {
    let mut picker = SessionPicker::loading();
    picker.activate_search();
    picker
        .handle_overlay_key(KeyCode::Char('b'), KeyModifiers::empty())
        .expect("search input");
    assert!(picker.search_active);
    assert_eq!(picker.search_query, "b");
    picker.reseed_grouped(Vec::new(), Vec::new());
    assert!(picker.search_active);
    assert_eq!(picker.search_query, "b");
}

#[test]
fn loading_picker_accepts_and_renders_search_before_sessions_arrive() {
    let mut picker = SessionPicker::loading();
    let (_sender, receiver) = std::sync::mpsc::channel::<SessionListUpdate>();
    for code in
        std::iter::once(KeyCode::Char('/')).chain("query-before-data".chars().map(KeyCode::Char))
    {
        assert!(matches!(
            picker
                .handle_overlay_key(code, KeyModifiers::empty())
                .expect("loading search input"),
            OverlayAction::Continue
        ));
    }
    assert!(matches!(
        receiver.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert!(picker.loading_message.is_some());
    assert!(picker.search_active);
    assert_eq!(picker.search_query, "query-before-data");

    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).expect("test terminal");
    terminal
        .draw(|frame| picker.render(frame))
        .expect("loading search frame");
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("Loading sessions"));
    assert!(text.contains("query-before-data"));
}

#[test]
fn loading_picker_search_is_editable_and_escape_clears_without_closing() {
    let mut picker = SessionPicker::loading();
    for code in [
        KeyCode::Char('/'),
        KeyCode::Char('q'),
        KeyCode::Char('x'),
        KeyCode::Backspace,
        KeyCode::Char('z'),
    ] {
        assert!(matches!(
            picker
                .handle_overlay_key(code, KeyModifiers::empty())
                .expect("edit loading search"),
            OverlayAction::Continue
        ));
    }
    assert_eq!(picker.search_query, "qz");
    assert!(picker.search_active);
    assert!(picker.loading_message.is_some());
    assert!(matches!(
        picker
            .handle_overlay_key(KeyCode::Esc, KeyModifiers::empty())
            .expect("leave search"),
        OverlayAction::Continue
    ));
    assert!(!picker.search_active);
    assert!(picker.search_query.is_empty());
    assert!(picker.loading_message.is_some());
    assert!(matches!(
        picker
            .handle_overlay_key(KeyCode::Esc, KeyModifiers::empty())
            .expect("close after leaving search"),
        OverlayAction::Close
    ));
}

#[test]
fn loading_picker_without_search_keeps_escape_and_ctrl_c_cancellation() {
    for (code, modifiers) in [
        (KeyCode::Esc, KeyModifiers::empty()),
        (KeyCode::Char('c'), KeyModifiers::CONTROL),
    ] {
        let mut picker = SessionPicker::loading();
        assert!(!picker.search_active);
        assert!(matches!(
            picker
                .handle_overlay_key(code, modifiers)
                .expect("cancel loading picker"),
            OverlayAction::Close
        ));
    }
}

#[test]
fn loading_picker_search_survives_cached_and_fresh_updates_and_filters_both() {
    let mut picker = SessionPicker::loading();
    for code in std::iter::once(KeyCode::Char('/')).chain("needle".chars().map(KeyCode::Char)) {
        assert!(matches!(
            picker
                .handle_overlay_key(code, KeyModifiers::empty())
                .expect("search before list arrives"),
            OverlayAction::Continue
        ));
    }
    let matching = make_session("matching", "needle", false, SessionStatus::Closed);
    let other = make_session("other", "unrelated", false, SessionStatus::Closed);
    assert!(
        !picker
            .apply_session_list_update(SessionListUpdate::Cached((
                Vec::new(),
                vec![other.clone(), matching.clone()],
            )))
            .expect("cached list")
    );
    assert_eq!(picker.search_query, "needle");
    assert!(picker.search_active);
    assert_eq!(picker.visible_sessions.len(), 1);
    assert_eq!(
        picker.selected_session().map(|s| s.id.as_str()),
        Some("matching")
    );

    assert!(
        picker
            .apply_session_list_update(SessionListUpdate::Fresh(Ok((
                Vec::new(),
                vec![matching, other],
            ))))
            .expect("fresh list")
    );
    assert_eq!(picker.search_query, "needle");
    assert!(picker.search_active);
    assert_eq!(picker.visible_sessions.len(), 1);
    assert_eq!(
        picker.selected_session().map(|s| s.id.as_str()),
        Some("matching")
    );
}

#[test]
fn loading_picker_enter_preserves_search_without_queuing_resume() {
    let mut picker = SessionPicker::loading();
    for code in [KeyCode::Char('/'), KeyCode::Char('q'), KeyCode::Enter] {
        assert!(matches!(
            picker
                .handle_overlay_key(code, KeyModifiers::empty())
                .unwrap(),
            OverlayAction::Continue
        ));
    }
    assert_eq!(picker.search_query, "q");
    assert!(picker.search_active);
    picker
        .apply_session_list_update(SessionListUpdate::Cached((
            Vec::new(),
            vec![make_session("matching", "q", false, SessionStatus::Closed)],
        )))
        .unwrap();
    assert_eq!(picker.search_query, "q");
    assert!(picker.search_active);
    assert_eq!(
        picker.selected_session().map(|s| s.id.as_str()),
        Some("matching")
    );
}

#[test]
fn loading_picker_filter_choice_survives_refresh() {
    let mut picker = SessionPicker::loading();
    picker
        .handle_overlay_key(KeyCode::Char('s'), KeyModifiers::empty())
        .unwrap();
    let chosen = picker.filter_mode;
    assert!(picker.filter_mode_user_chosen);
    assert_eq!(chosen, SessionFilterMode::CurrentDir);
    picker
        .apply_session_list_update(SessionListUpdate::Cached((
            Vec::new(),
            vec![make_session("first", "first", false, SessionStatus::Closed)],
        )))
        .unwrap();
    assert_eq!(picker.filter_mode, chosen);
    assert!(picker.filter_mode_user_chosen);
    picker
        .apply_session_list_update(SessionListUpdate::Fresh(Ok((Vec::new(), Vec::new()))))
        .unwrap();
    assert_eq!(picker.filter_mode, chosen);
    assert!(picker.filter_mode_user_chosen);
}

#[test]
fn standalone_resume_refresh_preserves_cached_selection_and_search() {
    let mut picker = SessionPicker::loading();
    let first = make_session("first", "first", false, SessionStatus::Closed);
    let second = make_session("second", "second", false, SessionStatus::Closed);
    assert!(
        !picker
            .apply_session_list_update(SessionListUpdate::Cached((
                Vec::new(),
                vec![first.clone(), second.clone()],
            )))
            .expect("cached list")
    );
    assert!(picker.loading_message.is_none());
    let index = picker
        .find_item_index_for_session_id("second")
        .expect("second item");
    picker.list_state.select(Some(index));
    picker.search_query = "test".to_string();
    picker.search_active = true;
    picker.filter_mode_user_chosen = true;
    assert!(
        picker
            .apply_session_list_update(SessionListUpdate::Fresh(Ok((
                Vec::new(),
                vec![second, first],
            ))))
            .expect("fresh list")
    );
    assert_eq!(
        picker.selected_session().map(|s| s.id.as_str()),
        Some("second")
    );
    assert_eq!(picker.search_query, "test");
    assert!(picker.search_active);
    assert!(picker.filter_mode_user_chosen);
}

#[test]
fn standalone_resume_load_error_preserves_existing_list() {
    let mut picker = SessionPicker::loading();
    picker
        .apply_session_list_update(SessionListUpdate::Cached((
            Vec::new(),
            vec![make_session(
                "cached",
                "cached",
                false,
                SessionStatus::Closed,
            )],
        )))
        .expect("cached list");
    let err = picker
        .apply_session_list_update(SessionListUpdate::Fresh(Err(anyhow::anyhow!(
            "scan failed"
        ))))
        .expect_err("report loader failure");
    assert_eq!(err.to_string(), "scan failed");
    assert_eq!(
        picker.selected_session().map(|s| s.id.as_str()),
        Some("cached")
    );
}

#[test]
fn standalone_resume_empty_refresh_finishes_loading() {
    let mut picker = SessionPicker::loading();
    assert!(
        picker
            .apply_session_list_update(SessionListUpdate::Fresh(Ok((Vec::new(), Vec::new(),))))
            .expect("empty list")
    );
    assert!(picker.loading_message.is_none());
    assert!(picker.selected_session().is_none());
}

#[test]
#[ignore = "developer benchmark: profiles real /resume through first rendered picker frame"]
fn benchmark_real_resume_first_render_reports_timings() {
    invalidate_session_list_cache();

    let total_start = std::time::Instant::now();

    let loading_render_start = std::time::Instant::now();
    let mut loading_picker = SessionPicker::loading();
    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| loading_picker.render(frame))
        .expect("render loading picker");
    let loading_render_elapsed = loading_render_start.elapsed();

    let load_start = std::time::Instant::now();
    let (server_groups, orphan_sessions) = load_sessions_grouped().expect("load sessions grouped");
    let load_elapsed = load_start.elapsed();
    let loaded_count: usize = server_groups
        .iter()
        .map(|group| group.sessions.len())
        .sum::<usize>()
        + orphan_sessions.len();

    let construct_start = std::time::Instant::now();
    let mut picker = SessionPicker::new_grouped(server_groups, orphan_sessions);
    let selected_before_render = picker.selected_session().map(|session| {
        (
            session.id.clone(),
            session.title.clone(),
            session.external_path.clone(),
            session.messages_preview.len(),
        )
    });
    let construct_elapsed = construct_start.elapsed();

    let first_render_start = std::time::Instant::now();
    terminal
        .draw(|frame| picker.render(frame))
        .expect("render loaded picker");
    let first_render_elapsed = first_render_start.elapsed();
    let selected_after_first_render = picker
        .selected_session()
        .map(|session| (session.id.clone(), session.messages_preview.len()));

    let second_render_start = std::time::Instant::now();
    terminal
        .draw(|frame| picker.render(frame))
        .expect("render loaded picker again");
    let second_render_elapsed = second_render_start.elapsed();

    eprintln!(
        "real resume first render: total={}ms loading_render={}ms load_grouped={}ms/{} construct={}ms first_render={}ms second_render={}ms selected_before={:?} selected_after={:?}",
        total_start.elapsed().as_millis(),
        loading_render_elapsed.as_millis(),
        load_elapsed.as_millis(),
        loaded_count,
        construct_elapsed.as_millis(),
        first_render_elapsed.as_millis(),
        second_render_elapsed.as_millis(),
        selected_before_render,
        selected_after_first_render,
    );
}

#[test]
#[ignore = "developer benchmark: profiles cached /resume first render latency"]
fn benchmark_real_resume_cached_first_render_reports_timings() {
    invalidate_session_list_cache();

    let refresh_start = std::time::Instant::now();
    let (_fresh_groups, _fresh_orphans) =
        load_sessions_grouped().expect("refresh sessions grouped");
    let refresh_elapsed = refresh_start.elapsed();

    let total_start = std::time::Instant::now();
    let cache_start = std::time::Instant::now();
    let (server_groups, orphan_sessions) =
        load_cached_sessions_grouped().expect("load cached sessions grouped");
    let cache_elapsed = cache_start.elapsed();
    let cached_count: usize = server_groups
        .iter()
        .map(|group| group.sessions.len())
        .sum::<usize>()
        + orphan_sessions.len();

    let construct_start = std::time::Instant::now();
    let mut picker = SessionPicker::new_grouped(server_groups, orphan_sessions);
    let construct_elapsed = construct_start.elapsed();

    let render_start = std::time::Instant::now();
    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| picker.render(frame))
        .expect("render cached picker");
    let render_elapsed = render_start.elapsed();

    eprintln!(
        "real resume cached first render: total={}ms cache_read={}ms/{} construct={}ms first_render={}ms cache_refresh={}ms",
        total_start.elapsed().as_millis(),
        cache_elapsed.as_millis(),
        cached_count,
        construct_elapsed.as_millis(),
        render_elapsed.as_millis(),
        refresh_elapsed.as_millis(),
    );
}
