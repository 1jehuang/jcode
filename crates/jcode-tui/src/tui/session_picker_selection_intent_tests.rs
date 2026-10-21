//! Explicit selection intent survives cached-to-fresh resume updates.

use super::*;

fn assert_cached_selection(
    enabled: bool,
    interact: impl FnOnce(&mut SessionPicker),
    expected: &str,
    scoped: bool,
) {
    let _lock = crate::storage::lock_test_env();
    let _env = ProjectScopeRefreshEnv::new(enabled);
    let first = project_scope_session("outside_first", "/work/elsewhere", 1);
    let second = project_scope_session("outside_second", "/work/elsewhere", 2);
    let mut picker = SessionPicker::loading();
    picker.set_current_dir(Some("/work/project".to_string()));
    picker.focus_search_input();
    picker
        .apply_session_list_update(SessionListUpdate::Cached((
            Vec::new(),
            vec![first.clone(), second.clone()],
        )))
        .unwrap();
    assert!(rendered_text(&mut picker).contains("outside_first"));
    assert_eq!(picker.selected_session().unwrap().id, "outside_first");
    assert!(!picker.user_selection_intent);
    interact(&mut picker);
    picker
        .apply_session_list_update(SessionListUpdate::Fresh(Ok((
            Vec::new(),
            vec![
                project_scope_session("fresh_project", "/work/project", 0),
                second,
                first,
            ],
        ))))
        .unwrap();
    assert_eq!(
        picker.filter_mode,
        if scoped {
            SessionFilterMode::CurrentDir
        } else {
            SessionFilterMode::All
        }
    );
    assert_eq!(picker.selected_session().unwrap().id, expected);
    assert!(rendered_text(&mut picker).contains(expected));
    let action = picker
        .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    assert!(matches!(
        action,
        OverlayAction::Selected(
            PickerResult::SelectedInCurrentTerminal(targets)
                | PickerResult::SelectedInNewTerminal(targets)
        ) if matches!(targets.as_slice(), [ResumeTarget::JcodeSession { session_id }]
                if session_id == expected)
    ));
}

#[test]
fn test_project_scope_refresh_scopes_untouched_automatic_cached_highlight() {
    assert_cached_selection(true, |_| {}, "fresh_project", true);
}

#[test]
fn test_project_scope_refresh_keeps_automatic_highlight_when_scope_disabled() {
    assert_cached_selection(false, |_| {}, "outside_first", false);
}

#[test]
fn test_project_scope_refresh_keeps_boundary_and_returned_keyboard_selection() {
    for keys in [vec![KeyCode::Up], vec![KeyCode::Down, KeyCode::Up]] {
        assert_cached_selection(
            true,
            |picker| {
                for key in keys {
                    picker
                        .handle_overlay_key(key, KeyModifiers::empty())
                        .unwrap();
                }
                assert!(picker.user_selection_intent);
            },
            "outside_first",
            false,
        );
    }
}

#[test]
fn test_project_scope_refresh_keeps_direct_navigation_at_both_boundaries() {
    for bottom in [false, true] {
        assert_cached_selection(
            true,
            |picker| {
                if bottom {
                    picker.next();
                    picker.next();
                } else {
                    picker.previous();
                }
                assert!(picker.user_selection_intent);
            },
            if bottom {
                "outside_second"
            } else {
                "outside_first"
            },
            false,
        );
    }
}

#[test]
fn test_project_scope_refresh_keeps_mouse_selection_including_boundary() {
    for (kind, expected) in [
        (MouseEventKind::ScrollUp, "outside_first"),
        (MouseEventKind::ScrollDown, "outside_second"),
    ] {
        assert_cached_selection(
            true,
            |picker| {
                let area = picker.last_list_area.expect("rendered list area");
                picker.handle_overlay_mouse(crossterm::event::MouseEvent {
                    kind,
                    column: area.x + 1,
                    row: area.y + 1,
                    modifiers: KeyModifiers::empty(),
                });
                assert!(picker.user_selection_intent);
            },
            expected,
            false,
        );
    }
}

#[test]
fn test_project_scope_refresh_keeps_marked_and_unmarked_cached_selection() {
    for toggles in [1, 2] {
        assert_cached_selection(
            true,
            |picker| {
                picker
                    .handle_overlay_key(KeyCode::Tab, KeyModifiers::empty())
                    .unwrap();
                for _ in 0..toggles {
                    picker
                        .handle_overlay_key(KeyCode::Char(' '), KeyModifiers::empty())
                        .unwrap();
                }
                assert!(picker.user_selection_intent);
            },
            "outside_first",
            false,
        );
    }
}

#[test]
fn test_project_scope_refresh_keeps_explicit_all_choice_with_nonempty_cache() {
    assert_cached_selection(
        true,
        |picker| {
            for modifiers in [
                KeyModifiers::CONTROL,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ] {
                picker
                    .handle_overlay_key(KeyCode::Char('s'), modifiers)
                    .unwrap();
            }
            assert!(picker.filter_mode_user_chosen);
        },
        "outside_first",
        false,
    );
}

#[test]
fn test_project_scope_refresh_preserves_typed_target_without_arrow_navigation() {
    assert_cached_selection(
        true,
        |picker| {
            for c in "outside_first".chars() {
                picker
                    .handle_overlay_key(KeyCode::Char(c), KeyModifiers::empty())
                    .unwrap();
            }
            assert!(!picker.user_selection_intent);
        },
        "outside_first",
        false,
    );
}

#[test]
fn test_project_scope_refresh_preserves_connected_group_selection() {
    let _lock = crate::storage::lock_test_env();
    let _env = ProjectScopeRefreshEnv::new(true);
    let connected = project_scope_session("connected", "/work/elsewhere", 1);
    let group = ServerGroup {
        name: "connected-server".to_string(),
        icon: String::new(),
        version: "test".to_string(),
        git_hash: "test".to_string(),
        is_running: true,
        sessions: vec![connected],
    };
    let mut picker = SessionPicker::loading();
    picker.set_current_dir(Some("/work/project".to_string()));
    picker.set_current_session_id(Some("connected".to_string()));
    picker
        .apply_session_list_update(SessionListUpdate::Cached((vec![group.clone()], Vec::new())))
        .unwrap();
    picker.previous();
    assert_eq!(picker.selected_session().unwrap().id, "connected");
    picker
        .apply_session_list_update(SessionListUpdate::Fresh(Ok((
            vec![group],
            vec![project_scope_session("fresh_project", "/work/project", 0)],
        ))))
        .unwrap();
    assert_eq!(picker.filter_mode, SessionFilterMode::All);
    assert_eq!(picker.selected_session().unwrap().id, "connected");
    assert!(picker.session_is_current(picker.selected_session().unwrap()));
}
