use super::wait_for_picker_search;
use super::*;

#[test]
fn start_search_on_loading_picker_survives_reseed_with_results() {
    // The real flow for `/sessions <query>` with no cached session list:
    // the picker is created in the loading state, the query is set, and the
    // background load later reseeds the picker with the actual sessions.
    // The query must still be active and now actually filter the reseeded list.
    let sessions: Vec<SessionInfo> = vec![
        make_session("session_load_a", "load-a", false, SessionStatus::Closed),
        make_session("session_load_b", "load-b", false, SessionStatus::Closed),
    ];
    let mut picker = SessionPicker::loading();

    picker.start_search("load-b");

    // Still in the loading state: the query is recorded and search mode is on.
    assert_eq!(picker.search_query, "load-b");
    assert!(picker.search_active);

    // The background load lands (same path as apply_loaded_session_picker).
    picker.reseed_grouped(Vec::new(), sessions);
    wait_for_picker_search(&mut picker);

    // Query survived the reseed and now filters the just-loaded list down to
    // the one matching session.
    assert_eq!(picker.search_query, "load-b");
    assert!(picker.search_active);
    let visible: Vec<&str> = picker
        .visible_session_iter_for_test()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible, vec!["session_load_b"]);
}

#[test]
fn search_edits_work_while_picker_is_loading() {
    // `/sessions <query>` with no cached session list opens the picker in the
    // loading state with the search bar active and a cursor rendered. Editing
    // keys must reach the search handler even while loading: before the fix,
    // `handle_overlay_key` checked `loading_message` first, so typing `q`
    // CLOSED the picker instead of adding a letter, and Backspace did nothing.
    // Enter stays blocked until results arrive (nothing can be selected from
    // an empty list), and Esc / q / Ctrl+C keep closing while NOT in search
    // mode (inside search mode they edit, as on the loaded picker).
    let mut picker = SessionPicker::loading();
    picker.start_search("loa");

    // Typing extends the query instead of being treated as picker shortcuts.
    let _ = picker.handle_overlay_key(KeyCode::Char('q'), KeyModifiers::empty());
    assert_eq!(picker.search_query, "loaq");

    // Backspace edits the query too.
    let _ = picker.handle_overlay_key(KeyCode::Backspace, KeyModifiers::empty());
    assert_eq!(picker.search_query, "loa");

    // Enter does not select anything while loading; the picker stays open.
    let action = picker
        .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("enter while loading");
    assert_eq!(action, OverlayAction::Continue);

    // Ctrl+C still closes the loading picker from search mode.
    let action = picker
        .handle_overlay_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
        .expect("ctrl+c closes");
    assert_eq!(action, OverlayAction::Close);
}

#[test]
fn loading_picker_without_search_still_blocks_letters() {
    // Plain loading picker (no `/sessions <query>`): every key stays inert as
    // before, only Esc / q / Ctrl+C close. The search-mode exception above
    // must not leak into the plain loading state.
    let mut picker = SessionPicker::loading();

    let action = picker
        .handle_overlay_key(KeyCode::Char('x'), KeyModifiers::empty())
        .expect("letter while loading");
    assert_eq!(action, OverlayAction::Continue);
    assert!(!picker.search_active);

    let action = picker
        .handle_overlay_key(KeyCode::Esc, KeyModifiers::empty())
        .expect("esc closes");
    assert_eq!(action, OverlayAction::Close);
}

#[test]
fn start_search_filters_immediately_on_loaded_picker() {
    let sessions: Vec<SessionInfo> = vec![
        make_session("session_pick_a", "pick-a", false, SessionStatus::Closed),
        make_session("session_pick_b", "pick-b", false, SessionStatus::Closed),
    ];
    let mut picker = SessionPicker::new(sessions);

    picker.start_search("pick-b");
    wait_for_picker_search(&mut picker);

    assert!(picker.search_active);
    let visible: Vec<&str> = picker
        .visible_session_iter_for_test()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible, vec!["session_pick_b"]);

    // Clearing the query restores the full list.
    picker.search_query.clear();
    picker.search_active = false;
    picker.rebuild_items();
    assert_eq!(picker.visible_session_count(), 2);
}

#[test]
fn start_search_with_no_match_leaves_empty_but_recoverable_list() {
    let sessions: Vec<SessionInfo> = vec![make_session(
        "session_pick_c",
        "pick-c",
        false,
        SessionStatus::Closed,
    )];
    let mut picker = SessionPicker::new(sessions);

    picker.start_search("no-such-session-needle");
    wait_for_picker_search(&mut picker);

    // Nothing matches: the visible list is empty and the search stays active
    // so the user can keep editing the query (Enter in this state clears it).
    assert!(picker.search_active);
    assert_eq!(picker.visible_session_count(), 0);

    // Simulating Enter-on-empty-result: the query clears and the list is back.
    picker.search_query.clear();
    picker.search_active = false;
    picker.rebuild_items();
    assert_eq!(picker.visible_session_count(), 1);
}

#[test]
fn start_search_accepts_unicode_query() {
    let sessions: Vec<SessionInfo> = vec![
        make_session("session_utf8_1", "utf8-one", false, SessionStatus::Closed),
        make_session("session_utf8_2", "utf8-two", false, SessionStatus::Closed),
    ];
    let mut picker = SessionPicker::new(sessions);

    // The token-based matcher lowercases the query; non-ASCII must round-trip
    // through start_search without truncating or panicking on char boundaries.
    picker.start_search("UTF8-ünïcode ö query");
    wait_for_picker_search(&mut picker);

    assert_eq!(picker.search_query, "UTF8-ünïcode ö query");
    assert!(picker.search_active);
    // Multi-token AND matching: both tokens must appear in the index, so this
    // query (utf8-ünïcode AND ö) matches nothing in the test data, which is
    // the expected behavior rather than an error.
    assert_eq!(picker.visible_session_count(), 0);

    // A single matching unicode token does filter correctly.
    picker.start_search("utf8-one");
    wait_for_picker_search(&mut picker);
    let visible: Vec<&str> = picker
        .visible_session_iter_for_test()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(visible, vec!["session_utf8_1"]);
}

#[test]
fn test_filter_matches_recent_message_content() {
    let mut picker = SessionPicker::new(vec![make_session(
        "session_content",
        "content",
        false,
        SessionStatus::Closed,
    )]);

    picker.search_query = "world".to_string();
    picker.rebuild_items();
    assert_eq!(picker.visible_sessions.len(), 1);

    picker.search_query = "not-in-preview".to_string();
    picker.rebuild_items();
    wait_for_picker_search(&mut picker);
    assert!(picker.visible_sessions.is_empty());
}

#[test]
fn resume_search_recovers_matches_as_typo_query_grows() {
    let mut session = make_session("typo_target", "target", false, SessionStatus::Closed);
    session.search_index = "config deploy".into();
    let mut picker = SessionPicker::new(vec![session]);
    for query in ["c", "co", "coo", "coon", "coonf", "coonfig"] {
        picker.search_query = query.into();
        picker.rebuild_items();
    }
    wait_for_picker_search(&mut picker);
    assert_eq!(picker.visible_sessions.len(), 1);
    picker.search_query = "deply conifg".into();
    picker.rebuild_items();
    wait_for_picker_search(&mut picker);
    assert_eq!(picker.visible_sessions.len(), 1);
}

#[test]
fn resume_search_ranks_literal_matches_before_newer_typo_matches() {
    let mut exact = make_session("literal", "literal", false, SessionStatus::Closed);
    exact.search_index = "config deploy".into();
    let mut typo = make_session("typo", "typo", false, SessionStatus::Closed);
    typo.search_index = "conifg deploy".into();
    typo.last_message_time = exact.last_message_time + chrono::Duration::seconds(10);
    let mut picker = SessionPicker::new(vec![typo, exact]);
    picker.search_query = "config".into();
    picker.rebuild_items();
    wait_for_picker_search(&mut picker);
    assert_eq!(picker.visible_sessions.len(), 2);
    assert_eq!(
        picker
            .session_by_ref(picker.visible_sessions[0])
            .unwrap()
            .id,
        "literal"
    );
}
