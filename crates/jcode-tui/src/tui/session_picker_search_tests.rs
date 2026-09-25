//! Tests for the search-first `/resume` picker: always-visible search box,
//! type-to-search, Tab to the list shortcuts, Esc clear/close.

use super::*;
use chrono::{Duration as ChronoDuration, Utc};

fn type_to_search_picker() -> SessionPicker {
    let mut zebra = make_session("session_zebra", "zebra", false, SessionStatus::Closed);
    let mut otter = make_session("session_otter", "otter", false, SessionStatus::Closed);
    let mut build = make_session("session_build7", "build7", false, SessionStatus::Closed);
    zebra.last_message_time = Utc::now();
    otter.last_message_time = Utc::now() - ChronoDuration::minutes(1);
    build.last_message_time = Utc::now() - ChronoDuration::minutes(2);
    SessionPicker::new(vec![zebra, otter, build])
}

fn visible_ids(picker: &SessionPicker) -> Vec<String> {
    picker
        .visible_sessions
        .iter()
        .filter_map(|r| picker.session_by_ref(*r).map(|s| s.id.clone()))
        .collect()
}

#[test]
fn test_type_to_search_non_shortcut_letter_starts_search() {
    let mut picker = type_to_search_picker();
    assert!(!picker.search_active);
    let action = picker
        .handle_overlay_key(KeyCode::Char('z'), KeyModifiers::empty())
        .unwrap();
    assert!(matches!(action, OverlayAction::Continue));
    assert!(picker.search_active);
    assert_eq!(picker.search_query, "z");
    assert_eq!(visible_ids(&picker), vec!["session_zebra".to_string()]);
}

#[test]
fn test_type_to_search_digit_starts_search() {
    let mut picker = type_to_search_picker();
    picker
        .handle_overlay_key(KeyCode::Char('7'), KeyModifiers::empty())
        .unwrap();
    assert!(picker.search_active);
    assert_eq!(picker.search_query, "7");
    assert_eq!(visible_ids(&picker), vec!["session_build7".to_string()]);
}

#[test]
fn test_type_to_search_shortcuts_still_work_with_empty_query() {
    let mut picker = type_to_search_picker();
    let first = picker.selected_session().map(|s| s.id.clone());

    // j / k navigate instead of typing.
    picker
        .handle_overlay_key(KeyCode::Char('j'), KeyModifiers::empty())
        .unwrap();
    assert!(!picker.search_active);
    assert!(picker.search_query.is_empty());
    assert_ne!(picker.selected_session().map(|s| s.id.clone()), first);
    picker
        .handle_overlay_key(KeyCode::Char('k'), KeyModifiers::empty())
        .unwrap();
    assert_eq!(picker.selected_session().map(|s| s.id.clone()), first);

    // d toggles debug sessions.
    let show_before = picker.show_test_sessions;
    picker
        .handle_overlay_key(KeyCode::Char('d'), KeyModifiers::empty())
        .unwrap();
    assert_ne!(picker.show_test_sessions, show_before);
    assert!(picker.search_query.is_empty());

    // s cycles the filter mode.
    let mode_before = picker.filter_mode;
    picker
        .handle_overlay_key(KeyCode::Char('s'), KeyModifiers::empty())
        .unwrap();
    assert_ne!(picker.filter_mode, mode_before);
    assert!(!picker.search_active);
    assert!(picker.search_query.is_empty());

    // `/` enters search mode without inserting.
    picker
        .handle_overlay_key(KeyCode::Char('/'), KeyModifiers::empty())
        .unwrap();
    assert!(picker.search_active);
    assert!(picker.search_query.is_empty());

    // q closes from normal mode.
    let mut picker = type_to_search_picker();
    let action = picker
        .handle_overlay_key(KeyCode::Char('q'), KeyModifiers::empty())
        .unwrap();
    assert!(matches!(action, OverlayAction::Close));
}

#[test]
fn test_type_to_search_shortcut_letters_are_text_once_searching() {
    let mut picker = type_to_search_picker();
    picker
        .handle_overlay_key(KeyCode::Char('o'), KeyModifiers::empty())
        .unwrap();
    let show_before = picker.show_test_sessions;
    let mode_before = picker.filter_mode;
    for c in ['q', 'j', 'd', 's', 'k'] {
        let action = picker
            .handle_overlay_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
        assert!(
            matches!(action, OverlayAction::Continue),
            "'{c}' must not close/select while searching"
        );
    }
    assert_eq!(picker.search_query, "oqjdsk");
    assert!(picker.search_active);
    assert_eq!(picker.show_test_sessions, show_before);
    assert_eq!(picker.filter_mode, mode_before);
}

#[test]
fn test_search_backspace_on_empty_query_is_noop_and_stays_in_search() {
    let mut picker = type_to_search_picker();
    picker
        .handle_overlay_key(KeyCode::Char('z'), KeyModifiers::empty())
        .unwrap();
    picker
        .handle_overlay_key(KeyCode::Backspace, KeyModifiers::empty())
        .unwrap();
    assert!(picker.search_query.is_empty());
    assert!(
        picker.search_active,
        "first Backspace only deletes the char"
    );
    assert_eq!(visible_ids(&picker).len(), 3);

    // Holding Backspace must not drop into list mode.
    for _ in 0..3 {
        picker
            .handle_overlay_key(KeyCode::Backspace, KeyModifiers::empty())
            .unwrap();
    }
    assert!(picker.search_active, "Backspace on empty query is a no-op");

    // So a following 'q' is text, not close.
    let action = picker
        .handle_overlay_key(KeyCode::Char('q'), KeyModifiers::empty())
        .unwrap();
    assert!(matches!(action, OverlayAction::Continue));
    assert_eq!(picker.search_query, "q");
}

#[test]
fn test_focus_search_input_types_quota_without_closing() {
    let mut picker = type_to_search_picker();
    picker.focus_search_input();
    assert!(picker.search_input_focused());
    assert!(picker.search_query.is_empty(), "focus must not touch query");
    for c in "quota".chars() {
        let action = picker
            .handle_overlay_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
        assert!(matches!(action, OverlayAction::Continue));
    }
    assert_eq!(picker.search_query, "quota");
    assert!(picker.search_active);
}

#[test]
fn test_search_esc_on_empty_query_closes() {
    let mut picker = type_to_search_picker();
    picker.focus_search_input();
    let action = picker
        .handle_overlay_key(KeyCode::Esc, KeyModifiers::empty())
        .unwrap();
    assert!(matches!(action, OverlayAction::Close));
}

#[test]
fn test_search_esc_with_query_clears_and_stays_in_search() {
    let mut picker = type_to_search_picker();
    picker.focus_search_input();
    for c in "otter".chars() {
        picker
            .handle_overlay_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    assert_eq!(visible_ids(&picker), vec!["session_otter".to_string()]);
    let action = picker
        .handle_overlay_key(KeyCode::Esc, KeyModifiers::empty())
        .unwrap();
    assert!(matches!(action, OverlayAction::Continue));
    assert!(picker.search_query.is_empty());
    assert!(picker.search_active);
    assert_eq!(visible_ids(&picker).len(), 3);

    // A second Esc on the now-empty query closes.
    let action = picker
        .handle_overlay_key(KeyCode::Esc, KeyModifiers::empty())
        .unwrap();
    assert!(matches!(action, OverlayAction::Close));
}

#[test]
fn test_search_tab_leaves_search_keeping_filter_then_q_closes() {
    let mut picker = type_to_search_picker();
    picker.focus_search_input();
    for c in "otter".chars() {
        picker
            .handle_overlay_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    picker
        .handle_overlay_key(KeyCode::Tab, KeyModifiers::empty())
        .unwrap();
    assert!(!picker.search_active, "Tab leaves search mode");
    assert_eq!(picker.search_query, "otter", "query stays as the filter");
    assert_eq!(visible_ids(&picker), vec!["session_otter".to_string()]);

    // Tab in list mode keeps toggling list <-> preview focus.
    let focus_before = picker.focus;
    picker
        .handle_overlay_key(KeyCode::Tab, KeyModifiers::empty())
        .unwrap();
    assert_ne!(picker.focus, focus_before);
    assert!(!picker.search_active);

    let action = picker
        .handle_overlay_key(KeyCode::Char('q'), KeyModifiers::empty())
        .unwrap();
    assert!(matches!(action, OverlayAction::Close));
}

#[test]
fn test_search_page_down_moves_selection_without_leaving_search() {
    let mut picker = type_to_search_picker();
    picker.focus_search_input();
    let first = picker.selected_session().map(|s| s.id.clone());
    picker
        .handle_overlay_key(KeyCode::PageDown, KeyModifiers::empty())
        .unwrap();
    assert!(
        picker.search_active,
        "PageDown keeps the search box focused"
    );
    assert!(picker.search_query.is_empty(), "PageDown is not text");
    assert_ne!(picker.selected_session().map(|s| s.id.clone()), first);
    picker
        .handle_overlay_key(KeyCode::PageUp, KeyModifiers::empty())
        .unwrap();
    assert_eq!(picker.selected_session().map(|s| s.id.clone()), first);
}

fn crashed_picker() -> SessionPicker {
    let now = Utc::now();
    let mut crashed = make_session(
        "session_crashed_recent",
        "crashed-recent",
        false,
        SessionStatus::Crashed {
            message: Some("recent crash".to_string()),
        },
    );
    crashed.last_message_time = now - ChronoDuration::minutes(1);
    crashed.last_active_at = Some(now - ChronoDuration::seconds(10));
    let picker = SessionPicker::new_grouped(
        vec![ServerGroup {
            name: "main".to_string(),
            icon: "🛰".to_string(),
            version: "v0.1.0".to_string(),
            git_hash: "abc1234".to_string(),
            is_running: true,
            sessions: vec![crashed],
        }],
        Vec::new(),
    );
    assert!(picker.crashed_sessions.is_some(), "crash banner expected");
    picker
}

#[test]
fn test_search_uppercase_r_restores_crashes_only_with_empty_query() {
    let mut picker = crashed_picker();
    picker.focus_search_input();
    let action = picker
        .handle_overlay_key(KeyCode::Char('R'), KeyModifiers::SHIFT)
        .unwrap();
    assert!(matches!(
        action,
        OverlayAction::Selected(PickerResult::RestoreCrashedGroup(_))
    ));

    let mut picker = crashed_picker();
    picker.focus_search_input();
    let action = picker
        .handle_overlay_key(KeyCode::Char('B'), KeyModifiers::SHIFT)
        .unwrap();
    assert!(matches!(
        action,
        OverlayAction::Selected(PickerResult::RestoreCrashedGroup(_))
    ));

    // Lowercase b is text even with an empty query.
    let mut picker = crashed_picker();
    picker.focus_search_input();
    let action = picker
        .handle_overlay_key(KeyCode::Char('b'), KeyModifiers::empty())
        .unwrap();
    assert!(matches!(action, OverlayAction::Continue));
    assert_eq!(picker.search_query, "b");

    // With a non-empty query, R is text.
    let mut picker = crashed_picker();
    picker.focus_search_input();
    picker
        .handle_overlay_key(KeyCode::Char('c'), KeyModifiers::empty())
        .unwrap();
    let action = picker
        .handle_overlay_key(KeyCode::Char('R'), KeyModifiers::SHIFT)
        .unwrap();
    assert!(matches!(action, OverlayAction::Continue));
    assert_eq!(picker.search_query, "cR");
}

#[test]
fn test_search_uppercase_r_is_text_without_crash_banner() {
    let mut picker = type_to_search_picker();
    picker.focus_search_input();
    let action = picker
        .handle_overlay_key(KeyCode::Char('R'), KeyModifiers::SHIFT)
        .unwrap();
    assert!(matches!(action, OverlayAction::Continue));
    assert_eq!(picker.search_query, "R");
}

#[test]
fn test_search_typing_while_loading_survives_reseed_and_filters() {
    let mut picker = SessionPicker::loading();
    picker.focus_search_input();
    for c in "quota".chars() {
        let action = picker
            .handle_overlay_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
        assert!(
            matches!(action, OverlayAction::Continue),
            "'{c}' must not close the loading picker"
        );
    }
    picker
        .handle_overlay_key(KeyCode::Backspace, KeyModifiers::empty())
        .unwrap();
    picker
        .handle_overlay_key(KeyCode::Char('a'), KeyModifiers::empty())
        .unwrap();
    assert_eq!(picker.search_query, "quota");

    let quota = make_session("session_quota", "quota", false, SessionStatus::Closed);
    let other = make_session("session_other", "other", false, SessionStatus::Closed);
    picker.reseed_grouped(Vec::new(), vec![quota, other]);

    assert!(picker.loading_message.is_none());
    assert_eq!(picker.search_query, "quota");
    assert!(picker.search_active);
    assert_eq!(visible_ids(&picker), vec!["session_quota".to_string()]);
}

#[test]
fn test_loading_picker_esc_clears_query_then_closes() {
    let mut picker = SessionPicker::loading();
    picker.focus_search_input();
    picker
        .handle_overlay_key(KeyCode::Char('x'), KeyModifiers::empty())
        .unwrap();
    // Same as the loaded picker: a non-empty query is cleared first.
    let action = picker
        .handle_overlay_key(KeyCode::Esc, KeyModifiers::empty())
        .unwrap();
    assert!(matches!(action, OverlayAction::Continue));
    assert!(picker.search_query.is_empty());
    assert!(
        picker.search_active,
        "search box keeps focus after clearing"
    );
    let action = picker
        .handle_overlay_key(KeyCode::Esc, KeyModifiers::empty())
        .unwrap();
    assert!(matches!(action, OverlayAction::Close));
    let mut picker = SessionPicker::loading();
    picker.focus_search_input();
    picker
        .handle_overlay_key(KeyCode::Char('x'), KeyModifiers::empty())
        .unwrap();
    let action = picker
        .handle_overlay_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(
        matches!(action, OverlayAction::Close),
        "Ctrl+C always closes"
    );
}

#[test]
fn test_search_bar_always_rendered_with_placeholder() {
    let mut picker = type_to_search_picker();
    let backend = ratatui::backend::TestBackend::new(120, 30);
    let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| picker.render(frame))
        .expect("render picker");
    let buffer = terminal.backend().buffer();
    let first_row: String = (0..buffer.area.width)
        .map(|x| buffer[(x, 0)].symbol().to_string())
        .collect();
    assert!(
        first_row.contains("Type or / to search"),
        "list-mode placeholder should render on the first row: {first_row:?}"
    );

    picker.focus_search_input();
    terminal
        .draw(|frame| picker.render(frame))
        .expect("render picker");
    let buffer = terminal.backend().buffer();
    let first_row: String = (0..buffer.area.width)
        .map(|x| buffer[(x, 0)].symbol().to_string())
        .collect();
    assert!(
        first_row.contains("Type to search sessions") && first_row.contains("Tab for shortcuts"),
        "focused placeholder should render on the first row: {first_row:?}"
    );

    picker
        .handle_overlay_key(KeyCode::Char('z'), KeyModifiers::empty())
        .unwrap();
    terminal
        .draw(|frame| picker.render(frame))
        .expect("render picker");
    let buffer = terminal.backend().buffer();
    let first_row: String = (0..buffer.area.width)
        .map(|x| buffer[(x, 0)].symbol().to_string())
        .collect();
    assert!(
        first_row.contains('z'),
        "query should render: {first_row:?}"
    );
    assert!(!first_row.contains("Type to search sessions"));
}

fn rendered_text(picker: &mut SessionPicker) -> String {
    let backend = ratatui::backend::TestBackend::new(200, 30);
    let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| picker.render(frame))
        .expect("render picker");
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
        .map(|(x, y)| buffer[(x, y)].symbol().to_string())
        .collect()
}

#[test]
fn test_search_live_claude_t_is_text_and_hint_points_at_tab() {
    let session = make_claude_session("claude-search-id");
    let mut picker = SessionPicker::new(vec![session]);
    picker.set_live_presence_for_test(vec![live_presence("claude:claude-search-id", false)]);
    picker.focus_search_input();

    assert!(
        rendered_text(&mut picker).contains("Tab, T take over live Claude"),
        "search mode keeps the takeover hint, pointing at Tab first"
    );

    picker
        .handle_overlay_key(KeyCode::Char('T'), KeyModifiers::empty())
        .unwrap();
    assert!(!picker.claude_takeover_confirmation_active_for_test());
    assert_eq!(
        picker.search_query, "T",
        "T is query text in the search box"
    );

    picker
        .handle_overlay_key(KeyCode::Backspace, KeyModifiers::empty())
        .unwrap();
    picker
        .handle_overlay_key(KeyCode::Tab, KeyModifiers::empty())
        .unwrap();
    assert!(rendered_text(&mut picker).contains(" T take over live Claude"));
    picker
        .handle_overlay_key(KeyCode::Char('T'), KeyModifiers::empty())
        .unwrap();
    assert!(
        picker.claude_takeover_confirmation_active_for_test(),
        "after Tab, T starts the explicit takeover confirmation"
    );
}
