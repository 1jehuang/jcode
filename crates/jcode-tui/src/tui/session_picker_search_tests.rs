//! Tests for the search-first `/resume` picker: always-visible search box,
//! type-to-search, Tab to the list shortcuts, Esc clear/close.

use super::*;
use chrono::{Duration as ChronoDuration, Utc};

#[path = "session_picker_selection_intent_tests.rs"]
mod selection_intent;

struct ProjectScopeRefreshEnv {
    home: Option<std::ffi::OsString>,
    scope: Option<std::ffi::OsString>,
    temp: tempfile::TempDir,
}

impl ProjectScopeRefreshEnv {
    fn new(enabled: bool) -> Self {
        let temp = tempfile::tempdir().expect("test home");
        let guard = Self {
            home: std::env::var_os("JCODE_HOME"),
            scope: std::env::var_os("JCODE_RESUME_PROJECT_SCOPE"),
            temp,
        };
        crate::env::set_var("JCODE_HOME", guard.temp.path());
        crate::env::set_var("JCODE_RESUME_PROJECT_SCOPE", enabled.to_string());
        crate::config::Config::invalidate_cache();
        guard
    }
}

impl Drop for ProjectScopeRefreshEnv {
    fn drop(&mut self) {
        for (name, previous) in [
            ("JCODE_HOME", &self.home),
            ("JCODE_RESUME_PROJECT_SCOPE", &self.scope),
        ] {
            match previous {
                Some(value) => crate::env::set_var(name, value),
                None => crate::env::remove_var(name),
            }
        }
        crate::config::Config::invalidate_cache();
    }
}

fn project_scope_session(id: &str, dir: &str, minutes_ago: i64) -> SessionInfo {
    let mut session = make_session(id, id, false, SessionStatus::Closed);
    session.working_dir = Some(dir.to_string());
    session.last_message_time = Utc::now() - ChronoDuration::minutes(minutes_ago);
    session
}

fn assert_cached_project_scope_highlight_survives(enabled: bool) {
    let _lock = crate::storage::lock_test_env();
    let _env = ProjectScopeRefreshEnv::new(enabled);
    let first = project_scope_session("outside_first", "/work/elsewhere", 1);
    let chosen = project_scope_session("outside_chosen", "/work/elsewhere", 2);
    let here = project_scope_session("fresh_project", "/work/project", 0);
    let mut picker = SessionPicker::loading();
    picker.set_current_dir(Some("/work/project".to_string()));
    picker.focus_search_input();
    picker
        .apply_session_list_update(SessionListUpdate::Cached((
            Vec::new(),
            vec![first.clone(), chosen.clone()],
        )))
        .unwrap();
    assert_eq!(picker.filter_mode, SessionFilterMode::All);
    picker
        .handle_overlay_key(KeyCode::Down, KeyModifiers::empty())
        .unwrap();
    assert_eq!(picker.selected_session().unwrap().id, "outside_chosen");
    assert!(!picker.filter_mode_user_chosen);

    picker
        .apply_session_list_update(SessionListUpdate::Fresh(Ok((
            Vec::new(),
            vec![here, chosen, first],
        ))))
        .unwrap();
    assert_eq!(picker.filter_mode, SessionFilterMode::All);
    assert_eq!(picker.selected_session().unwrap().id, "outside_chosen");
    let action = picker
        .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    assert!(matches!(
        action,
        OverlayAction::Selected(
            PickerResult::SelectedInCurrentTerminal(targets)
                | PickerResult::SelectedInNewTerminal(targets)
        ) if matches!(targets.as_slice(), [ResumeTarget::JcodeSession { session_id }]
                if session_id == "outside_chosen")
    ));
}

#[test]
fn test_project_scope_refresh_keeps_cached_highlight_when_enabled() {
    assert_cached_project_scope_highlight_survives(true);
}

#[test]
fn test_project_scope_refresh_keeps_cached_highlight_when_disabled() {
    assert_cached_project_scope_highlight_survives(false);
}

#[test]
fn test_project_scope_refresh_scopes_first_nonempty_result_after_empty_cache() {
    let _lock = crate::storage::lock_test_env();
    let _env = ProjectScopeRefreshEnv::new(true);
    let mut picker = SessionPicker::loading();
    picker.set_current_dir(Some("/work/project".to_string()));
    picker
        .apply_session_list_update(SessionListUpdate::Cached((Vec::new(), Vec::new())))
        .unwrap();
    assert!(picker.selected_session().is_none());
    picker
        .apply_session_list_update(SessionListUpdate::Fresh(Ok((
            Vec::new(),
            vec![
                project_scope_session("outside", "/work/elsewhere", 0),
                project_scope_session("here", "/work/project", 1),
            ],
        ))))
        .unwrap();
    assert_eq!(picker.filter_mode, SessionFilterMode::CurrentDir);
    assert_eq!(visible_ids(&picker), vec!["here".to_string()]);
}

#[test]
fn test_project_scope_refresh_preserves_explicit_all_choice_after_empty_cache() {
    let _lock = crate::storage::lock_test_env();
    let _env = ProjectScopeRefreshEnv::new(true);
    let mut picker = SessionPicker::loading();
    picker.set_current_dir(Some("/work/project".to_string()));
    picker.focus_search_input();
    for modifiers in [
        KeyModifiers::CONTROL,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ] {
        picker
            .handle_overlay_key(KeyCode::Char('s'), modifiers)
            .unwrap();
    }
    picker
        .apply_session_list_update(SessionListUpdate::Cached((Vec::new(), Vec::new())))
        .unwrap();
    picker
        .apply_session_list_update(SessionListUpdate::Fresh(Ok((
            Vec::new(),
            vec![
                project_scope_session("outside", "/work/elsewhere", 0),
                project_scope_session("here", "/work/project", 1),
            ],
        ))))
        .unwrap();
    assert!(picker.filter_mode_user_chosen);
    assert_eq!(picker.filter_mode, SessionFilterMode::All);
    assert_eq!(visible_ids(&picker).len(), 2);
}

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

/// In the search box plain `s` is text, so the filter shortcut must still be
/// reachable: Ctrl+S cycles forward, Ctrl+Shift+S backward, the query stays
/// untouched, and the hints name the chord.
#[test]
fn test_search_ctrl_s_cycles_filter_without_typing() {
    let mut picker = type_to_search_picker();
    picker.focus_search_input();
    picker
        .handle_overlay_key(KeyCode::Char('o'), KeyModifiers::empty())
        .unwrap();
    let start = picker.filter_mode;

    picker
        .handle_overlay_key(KeyCode::Char('s'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(picker.filter_mode, start.next(), "Ctrl+S cycles forward");
    assert_eq!(picker.search_query, "o", "Ctrl+S is not query text");
    assert!(picker.search_active, "Ctrl+S keeps the search box focused");

    // Terminals report Ctrl+Shift+S either as 'S' or as 's' with SHIFT.
    for (c, mods) in [
        ('S', KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        ('s', KeyModifiers::CONTROL | KeyModifiers::SHIFT),
    ] {
        let before = picker.filter_mode;
        picker.handle_overlay_key(KeyCode::Char(c), mods).unwrap();
        assert_eq!(picker.filter_mode, before.previous(), "{c:?} cycles back");
    }
    assert_eq!(picker.search_query, "o");

    let text = rendered_text(&mut picker);
    assert!(text.contains("(Ctrl+S filter)"), "title names the chord");
    assert!(
        text.contains("Ctrl+S filter · Tab shortcuts"),
        "footer names it"
    );

    // Plain `s` is still text while searching.
    picker
        .handle_overlay_key(KeyCode::Char('s'), KeyModifiers::empty())
        .unwrap();
    assert_eq!(picker.search_query, "os");
}

/// `/resume` opens while the index still loads, so Ctrl+S must work then too
/// and the chosen filter must apply once the sessions arrive.
#[test]
fn test_loading_picker_ctrl_s_filter_survives_reseed() {
    let mut picker = SessionPicker::loading();
    picker.focus_search_input();
    let start = picker.filter_mode;
    // All -> current dir -> catch up -> saved.
    for _ in 0..3 {
        picker
            .handle_overlay_key(KeyCode::Char('s'), KeyModifiers::CONTROL)
            .unwrap();
    }
    assert_eq!(picker.filter_mode, start.next().next().next());
    assert_eq!(picker.filter_mode, SessionFilterMode::Saved);
    assert!(picker.search_query.is_empty(), "Ctrl+S is not query text");

    let mut saved = make_session("session_saved", "saved", false, SessionStatus::Closed);
    saved.saved = true;
    let other = make_session("session_other", "other", false, SessionStatus::Closed);
    picker.reseed_grouped(Vec::new(), vec![saved, other]);
    assert_eq!(picker.filter_mode, SessionFilterMode::Saved);
    assert_eq!(visible_ids(&picker), vec!["session_saved".to_string()]);
}

/// A filter picked with Ctrl+S while the index loads is the user's choice, so
/// the project-scope default applied after the load must not replace it.
#[test]
fn test_loading_picker_ctrl_s_choice_beats_project_scope_default() {
    let mut picker = SessionPicker::loading();
    picker.set_current_dir(Some("/work/project".to_string()));
    picker.focus_search_input();
    // Forward then back again: the user lands on All on purpose.
    for mods in [
        KeyModifiers::CONTROL,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ] {
        picker.handle_overlay_key(KeyCode::Char('s'), mods).unwrap();
    }
    assert_eq!(picker.filter_mode, SessionFilterMode::All);

    let mut here = make_session("session_here", "here", false, SessionStatus::Closed);
    here.working_dir = Some("/work/project".to_string());
    let other = make_session("session_other", "other", false, SessionStatus::Closed);
    picker.reseed_grouped(Vec::new(), vec![here, other]);
    picker.prefer_current_dir_filter();
    assert_eq!(picker.filter_mode, SessionFilterMode::All);
    assert_eq!(visible_ids(&picker).len(), 2);
}

#[test]
fn test_loading_list_typing_restarts_search_before_cached_and_fresh_updates() {
    for use_slash in [false, true] {
        let mut picker = SessionPicker::loading();
        picker.focus_search_input();
        picker
            .handle_overlay_key(KeyCode::Tab, KeyModifiers::empty())
            .unwrap();
        if use_slash {
            picker
                .handle_overlay_key(KeyCode::Char('/'), KeyModifiers::empty())
                .unwrap();
            assert!(picker.search_input_focused());
            assert!(picker.search_query.is_empty());
        }
        picker
            .handle_overlay_key(KeyCode::Char('x'), KeyModifiers::empty())
            .unwrap();
        assert!(picker.search_input_focused());
        assert_eq!(picker.search_query, "x");

        for fresh in [false, true] {
            let matching = make_session("session_x", "x", false, SessionStatus::Closed);
            let other = make_session("session_other", "other", false, SessionStatus::Closed);
            let sessions = (Vec::new(), vec![matching, other]);
            let update = if fresh {
                SessionListUpdate::Fresh(Ok(sessions))
            } else {
                SessionListUpdate::Cached(sessions)
            };
            picker.apply_session_list_update(update).unwrap();
            assert!(picker.search_input_focused());
            assert_eq!(picker.search_query, "x");
            assert_eq!(visible_ids(&picker), vec!["session_x".to_string()]);
        }
    }
}

#[test]
fn test_loading_tab_preserves_list_focus_through_cached_and_fresh_updates() {
    for query in ["", "quota"] {
        let mut picker = SessionPicker::loading();
        picker.focus_search_input();
        for c in query.chars() {
            picker
                .handle_overlay_key(KeyCode::Char(c), KeyModifiers::empty())
                .unwrap();
        }
        picker
            .handle_overlay_key(KeyCode::Tab, KeyModifiers::empty())
            .unwrap();
        assert!(!picker.search_input_focused());
        assert_eq!(picker.search_query, query);

        for fresh in [false, true] {
            let session = make_session("session_quota", "quota", false, SessionStatus::Closed);
            let sessions = (Vec::new(), vec![session]);
            let update = if fresh {
                SessionListUpdate::Fresh(Ok(sessions))
            } else {
                SessionListUpdate::Cached(sessions)
            };
            picker.apply_session_list_update(update).unwrap();
            assert!(!picker.search_input_focused());
            assert_eq!(picker.search_query, query);
            assert!(matches!(
                picker
                    .handle_overlay_key(KeyCode::Char('q'), KeyModifiers::empty())
                    .unwrap(),
                OverlayAction::Close
            ));
        }
    }
}

#[test]
fn test_search_query_and_filter_survive_cached_and_fresh_session_updates() {
    let mut picker = SessionPicker::loading();
    picker.focus_search_input();
    for _ in 0..3 {
        picker
            .handle_overlay_key(KeyCode::Char('s'), KeyModifiers::CONTROL)
            .unwrap();
    }
    for c in "quota".chars() {
        picker
            .handle_overlay_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }

    for fresh in [false, true] {
        let mut saved = make_session("session_quota", "quota", false, SessionStatus::Closed);
        saved.saved = true;
        let other = make_session("session_other", "other", false, SessionStatus::Closed);
        let sessions = (Vec::new(), vec![saved, other]);
        let update = if fresh {
            SessionListUpdate::Fresh(Ok(sessions))
        } else {
            SessionListUpdate::Cached(sessions)
        };

        assert_eq!(picker.apply_session_list_update(update).unwrap(), fresh);
        assert!(picker.search_input_focused());
        assert_eq!(picker.search_query, "quota");
        assert_eq!(picker.filter_mode, SessionFilterMode::Saved);
        assert_eq!(visible_ids(&picker), vec!["session_quota".to_string()]);
    }
}
