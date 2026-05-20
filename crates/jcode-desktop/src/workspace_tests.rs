use super::*;
use std::collections::HashSet;

#[test]
fn h_and_l_focus_neighboring_columns_in_current_workspace() {
    let mut workspace = Workspace::fake();
    assert_eq!(workspace.focused_id, 1);
    assert_eq!(
        workspace.handle_key(KeyInput::Character("l".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.focused_id, 2);
    assert_eq!(
        workspace.handle_key(KeyInput::Character("h".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.focused_id, 1);
}

#[test]
fn j_and_k_focus_workspace_below_and_above() {
    let mut workspace = Workspace::fake();
    assert_eq!(workspace.current_workspace(), 0);
    assert_eq!(
        workspace.handle_key(KeyInput::Character("j".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.current_workspace(), 1);
    assert_eq!(
        workspace.handle_key(KeyInput::Character("k".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.current_workspace(), 0);
    assert_eq!(
        workspace.handle_key(KeyInput::Character("k".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.current_workspace(), -1);
}

#[test]
fn moving_to_missing_workspace_creates_placeholder_surface() {
    let mut workspace = Workspace::fake();
    workspace.handle_key(KeyInput::Character("j".to_string()));
    workspace.handle_key(KeyInput::Character("j".to_string()));
    assert_eq!(workspace.current_workspace(), 2);
    assert!(
        workspace.surfaces.iter().any(|surface| {
            surface.lane == 2 && surface.kind == SurfaceKind::WorkspacePlaceholder
        })
    );
    assert_unique_positions(&workspace);
}

#[test]
fn placeholder_titles_do_not_define_surface_kind() {
    let mut workspace = Workspace::fake();
    workspace.handle_key(KeyInput::Character("n".to_string()));
    let focused = workspace.focused_id;
    let surface = workspace
        .surfaces
        .iter_mut()
        .find(|surface| surface.id == focused)
        .unwrap();
    surface.title = format!("workspace {}", surface.lane);

    assert_eq!(surface.kind, SurfaceKind::Scratch);
    assert_eq!(workspace.occupied_lane_bounds(), (-1, 1));
}

#[test]
fn workspace_navigation_stops_two_empty_lanes_beyond_occupied_lanes() {
    let mut workspace = Workspace::fake();
    assert_eq!(workspace.occupied_lane_bounds(), (-1, 1));

    for expected_lane in [1, 2, 3] {
        assert_eq!(
            workspace.handle_key(KeyInput::Character("j".to_string())),
            KeyOutcome::Redraw
        );
        assert_eq!(workspace.current_workspace(), expected_lane);
    }
    assert_eq!(
        workspace.handle_key(KeyInput::Character("j".to_string())),
        KeyOutcome::None
    );
    assert_eq!(workspace.current_workspace(), 3);
    assert!(!workspace.surfaces.iter().any(|surface| surface.lane == 4));

    for expected_lane in [2, 1, 0, -1, -2, -3] {
        assert_eq!(
            workspace.handle_key(KeyInput::Character("k".to_string())),
            KeyOutcome::Redraw
        );
        assert_eq!(workspace.current_workspace(), expected_lane);
    }
    assert_eq!(
        workspace.handle_key(KeyInput::Character("k".to_string())),
        KeyOutcome::None
    );
    assert_eq!(workspace.current_workspace(), -3);
    assert!(!workspace.surfaces.iter().any(|surface| surface.lane == -4));
}

#[test]
fn uppercase_h_and_l_swap_focused_surface_with_neighbor() {
    let mut workspace = Workspace::fake();
    workspace.handle_key(KeyInput::Character("L".to_string()));
    assert_eq!(
        workspace
            .focused_surface()
            .map(|surface| (surface.lane, surface.column)),
        Some((0, 1))
    );
    assert_unique_positions(&workspace);
}

#[test]
fn uppercase_j_and_k_move_surface_between_workspaces() {
    let mut workspace = Workspace::fake();
    workspace.handle_key(KeyInput::Character("J".to_string()));
    assert_eq!(
        workspace.focused_surface().map(|surface| surface.lane),
        Some(1)
    );
    workspace.handle_key(KeyInput::Character("K".to_string()));
    assert_eq!(
        workspace.focused_surface().map(|surface| surface.lane),
        Some(0)
    );
}

#[test]
fn insert_mode_captures_text_and_escape_returns_to_navigation() {
    let mut workspace = Workspace::fake();
    assert_eq!(
        workspace.handle_key(KeyInput::Character("i".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.mode, InputMode::Insert);
    workspace.handle_key(KeyInput::Character("hello".to_string()));
    assert_eq!(workspace.draft, "hello");
    workspace.handle_key(KeyInput::Escape);
    assert_eq!(workspace.mode, InputMode::Navigation);
}

#[test]
fn insert_mode_supports_cursor_editing_and_undo() {
    let mut workspace = Workspace::fake();
    workspace.handle_key(KeyInput::Character("i".to_string()));
    workspace.handle_key(KeyInput::Character("helo".to_string()));
    workspace.handle_key(KeyInput::MoveCursorLeft);
    workspace.handle_key(KeyInput::Character("l".to_string()));

    assert_eq!(workspace.draft, "hello");
    assert_eq!(workspace.draft_cursor, 4);

    workspace.handle_key(KeyInput::DeleteNextChar);
    assert_eq!(workspace.draft, "hell");

    workspace.handle_key(KeyInput::UndoInput);
    assert_eq!(workspace.draft, "hello");
    assert_eq!(workspace.draft_cursor, 4);
}

#[test]
fn insert_mode_autocompletes_workspace_slash_command() {
    let mut workspace = Workspace::fake();
    workspace.handle_key(KeyInput::Character("i".to_string()));
    workspace.handle_key(KeyInput::Character("/mod".to_string()));

    assert_eq!(
        workspace.handle_key(KeyInput::Autocomplete),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.draft, "/model");
    assert_eq!(workspace.draft_cursor, "/model".len());
}

#[test]
fn insert_mode_cuts_input_line_to_clipboard_and_undo_restores() {
    let mut workspace = Workspace::fake();
    workspace.handle_key(KeyInput::Character("i".to_string()));
    workspace.handle_key(KeyInput::Character("copy me".to_string()));

    assert_eq!(
        workspace.handle_key(KeyInput::CutInputLine),
        KeyOutcome::CutDraftToClipboard("copy me".to_string())
    );
    assert!(workspace.draft.is_empty());
    assert_eq!(workspace.draft_cursor, 0);

    workspace.handle_key(KeyInput::UndoInput);
    assert_eq!(workspace.draft, "copy me");
    assert_eq!(workspace.draft_cursor, "copy me".len());
}

#[test]
fn toggle_input_mode_switches_between_navigation_and_insert() {
    let mut workspace = Workspace::fake();
    assert_eq!(workspace.mode, InputMode::Navigation);
    assert_eq!(
        workspace.handle_key(KeyInput::ToggleInputMode),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.mode, InputMode::Insert);
    assert_eq!(
        workspace.handle_key(KeyInput::ToggleInputMode),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.mode, InputMode::Navigation);
}

#[test]
fn navigation_escape_exits() {
    let mut workspace = Workspace::fake();
    assert_eq!(workspace.handle_key(KeyInput::Escape), KeyOutcome::Exit);
}

#[test]
fn new_and_close_surface_update_focus_without_overlapping() {
    let mut workspace = Workspace::fake();
    workspace.handle_key(KeyInput::Character("n".to_string()));
    assert_eq!(workspace.focused_id, 8);
    assert_eq!(workspace.surfaces.len(), 8);
    assert_eq!(
        workspace.focused_surface().map(|surface| surface.lane),
        Some(0)
    );
    assert_unique_positions(&workspace);
    workspace.handle_key(KeyInput::Character("x".to_string()));
    assert_eq!(workspace.surfaces.len(), 7);
    assert_ne!(workspace.focused_id, 8);
}

#[test]
fn spawn_panel_shortcut_adds_surface_in_current_workspace() {
    let mut workspace = Workspace::fake();
    assert_eq!(
        workspace.handle_key(KeyInput::SpawnPanel),
        KeyOutcome::SpawnSession
    );
    assert_eq!(workspace.focused_id, 1);
    assert_unique_positions(&workspace);
}

#[test]
fn hotkey_help_shortcut_opens_single_help_surface() {
    let mut workspace = Workspace::fake();
    assert_eq!(
        workspace.handle_key(KeyInput::HotkeyHelp),
        KeyOutcome::Redraw
    );
    assert_eq!(
        workspace
            .focused_surface()
            .map(|surface| surface.title.as_str()),
        Some("hotkey help")
    );
    let help_id = workspace.focused_id;
    assert_eq!(
        workspace.handle_key(KeyInput::HotkeyHelp),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.focused_id, help_id);
    assert_eq!(
        workspace
            .surfaces
            .iter()
            .filter(|surface| surface.title == "hotkey help")
            .count(),
        1
    );
    assert!(workspace.focused_surface().is_some_and(|surface| {
        surface
            .body_lines
            .contains(&"enter insert mode".to_string())
    }));
}

#[test]
fn hotkey_help_mentions_opening_when_focused_on_real_session() {
    let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);

    assert_eq!(
        workspace.handle_key(KeyInput::HotkeyHelp),
        KeyOutcome::Redraw
    );

    assert!(workspace.focused_surface().is_some_and(|surface| {
        surface
            .body_lines
            .contains(&"o or enter open session".to_string())
    }));
}

#[test]
fn panel_size_presets_update_preferred_screen_fraction() {
    let mut workspace = Workspace::fake();
    assert_eq!(workspace.preferred_panel_screen_fraction(), 0.25);
    assert_eq!(
        workspace.handle_key(KeyInput::SetPanelSize(PanelSizePreset::Half)),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.preferred_panel_screen_fraction(), 0.50);
    assert_eq!(
        workspace.handle_key(KeyInput::SetPanelSize(PanelSizePreset::ThreeQuarter)),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.preferred_panel_screen_fraction(), 0.75);
    assert_eq!(
        workspace.handle_key(KeyInput::SetPanelSize(PanelSizePreset::Full)),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.preferred_panel_screen_fraction(), 1.00);
}

#[test]
fn session_cards_create_real_session_surfaces() {
    let workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);

    assert_eq!(workspace.surfaces.len(), 1);
    assert_eq!(workspace.surfaces[0].kind, SurfaceKind::Session);
    assert_eq!(workspace.surfaces[0].title, "alpha");
    assert_eq!(workspace.surfaces[0].session_id.as_deref(), Some("a"));
    assert_eq!(workspace.surfaces[0].body_lines.len(), 4);
    assert!(
        workspace.surfaces[0]
            .body_lines
            .contains(&"recent transcript".to_string())
    );
    assert!(
        workspace.surfaces[0]
            .detail_lines
            .contains(&"expanded transcript".to_string())
    );
    assert!(
        workspace.surfaces[0]
            .detail_lines
            .contains(&"user expanded hello".to_string())
    );
}

#[test]
fn loading_workspace_renders_before_session_cards_arrive() {
    let workspace = Workspace::loading_sessions();

    assert_eq!(workspace.surfaces.len(), 1);
    assert_eq!(workspace.surfaces[0].kind, SurfaceKind::Loading);
    assert!(workspace.status_title().contains("loading jcode sessions"));
}

#[test]
fn applying_preferences_does_not_create_missing_workspace_placeholder() {
    let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);
    let original_surface_count = workspace.surfaces.len();
    let original_focus = workspace.focused_id;

    workspace.apply_preferences(DesktopPreferences {
        panel_size: PanelSizePreset::ThreeQuarter,
        focused_session_id: Some("missing-session".to_string()),
        workspace_lane: 2,
        space_hold_toggle_ms: 300,
    });

    assert_eq!(workspace.surfaces.len(), original_surface_count);
    assert_eq!(workspace.focused_id, original_focus);
    assert_eq!(workspace.preferred_panel_screen_fraction(), 0.75);
    assert_eq!(workspace.space_hold_toggle_duration().as_millis(), 300);
    assert!(
        !workspace
            .surfaces
            .iter()
            .any(|surface| surface.kind == SurfaceKind::WorkspacePlaceholder && surface.lane == 2)
    );
}

#[test]
fn applying_preferences_focuses_existing_lane_without_placeholder_creation() {
    let mut workspace = Workspace::fake();
    let original_surface_count = workspace.surfaces.len();

    workspace.apply_preferences(DesktopPreferences {
        panel_size: PanelSizePreset::Half,
        focused_session_id: None,
        workspace_lane: 1,
        space_hold_toggle_ms: DEFAULT_SPACE_HOLD_TOGGLE_MS,
    });

    assert_eq!(workspace.surfaces.len(), original_surface_count);
    assert_eq!(workspace.current_workspace(), 1);
    assert_eq!(workspace.preferred_panel_screen_fraction(), 0.50);
}

#[test]
fn replacing_session_cards_preserves_focus_when_possible() {
    let mut workspace =
        Workspace::from_session_cards(vec![session_card("a", "alpha"), session_card("b", "bravo")]);
    workspace.focused_id = 2;
    workspace.handle_key(KeyInput::SetPanelSize(PanelSizePreset::Half));
    workspace.handle_key(KeyInput::Character("i".to_string()));
    workspace.handle_key(KeyInput::Character("draft".to_string()));
    workspace.attach_image("image/png".to_string(), "abc123".to_string());

    workspace.replace_session_cards(vec![session_card("b", "bravo refreshed")]);

    assert_eq!(
        workspace
            .focused_surface()
            .map(|surface| surface.title.as_str()),
        Some("bravo refreshed")
    );
    assert_eq!(workspace.preferred_panel_screen_fraction(), 0.50);
    assert_eq!(workspace.draft, "draft");
    assert_eq!(workspace.pending_images.len(), 1);
}

#[test]
fn replacing_session_cards_reconciles_without_destroying_manual_layout() {
    let mut workspace =
        Workspace::from_session_cards(vec![session_card("a", "alpha"), session_card("b", "bravo")]);
    workspace.focused_id = 2;
    workspace.handle_key(KeyInput::Character("J".to_string()));
    let focused_before = workspace.focused_id;
    let focused_position_before = workspace
        .focused_surface()
        .map(|surface| (surface.lane, surface.column))
        .unwrap();
    workspace.handle_key(KeyInput::Character("n".to_string()));
    let scratch_id = workspace.focused_id;
    workspace.focused_id = focused_before;
    workspace.handle_key(KeyInput::HotkeyHelp);
    let help_id = workspace.focused_id;
    workspace.focused_id = focused_before;

    workspace.replace_session_cards(vec![
        session_card("b", "bravo refreshed"),
        session_card("c", "charlie"),
    ]);

    assert!(
        workspace
            .surfaces
            .iter()
            .all(|surface| { surface.session_id.as_deref() != Some("a") })
    );
    let refreshed = workspace
        .surfaces
        .iter()
        .find(|surface| surface.session_id.as_deref() == Some("b"))
        .unwrap();
    assert_eq!(refreshed.id, focused_before);
    assert_eq!(refreshed.title, "bravo refreshed");
    assert_eq!((refreshed.lane, refreshed.column), focused_position_before);
    assert_eq!(workspace.focused_id, focused_before);
    assert!(
        workspace
            .surfaces
            .iter()
            .any(|surface| { surface.id == scratch_id && surface.kind == SurfaceKind::Scratch })
    );
    assert!(
        workspace
            .surfaces
            .iter()
            .any(|surface| { surface.id == help_id && surface.kind == SurfaceKind::HotkeyHelp })
    );
    assert!(workspace.surfaces.iter().any(|surface| {
        surface.session_id.as_deref() == Some("c") && surface.kind == SurfaceKind::Session
    }));
    assert_unique_positions(&workspace);
}

#[test]
fn o_opens_focused_session_surface() {
    let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);

    assert_eq!(
        workspace.handle_key(KeyInput::Character("o".to_string())),
        KeyOutcome::OpenSession {
            session_id: "a".to_string(),
            title: "alpha".to_string()
        }
    );
}

#[test]
fn enter_opens_real_session_but_still_inserts_for_placeholder() {
    let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);
    assert_eq!(
        workspace.handle_key(KeyInput::Enter),
        KeyOutcome::OpenSession {
            session_id: "a".to_string(),
            title: "alpha".to_string()
        }
    );

    let mut placeholder_workspace = Workspace::fake();
    assert_eq!(
        placeholder_workspace.handle_key(KeyInput::Enter),
        KeyOutcome::Redraw
    );
    assert_eq!(placeholder_workspace.mode, InputMode::Insert);
}

#[test]
fn ctrl_enter_submits_insert_draft_to_focused_session() {
    let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);
    workspace.handle_key(KeyInput::Character("i".to_string()));
    workspace.handle_key(KeyInput::Character(" hello ".to_string()));

    assert_eq!(
        workspace.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::SendDraft {
            session_id: "a".to_string(),
            title: "alpha".to_string(),
            message: "hello".to_string(),
            images: Vec::new()
        }
    );
    assert_eq!(workspace.mode, InputMode::Navigation);
    assert!(workspace.draft.is_empty());
}

#[test]
fn submit_draft_opens_focused_session_in_navigation_mode() {
    let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);

    assert_eq!(
        workspace.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::OpenSession {
            session_id: "a".to_string(),
            title: "alpha".to_string()
        }
    );
}

#[test]
fn paste_text_appends_to_workspace_insert_draft() {
    let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);
    workspace.handle_key(KeyInput::Character("i".to_string()));

    assert_eq!(
        workspace.handle_key(KeyInput::PasteText),
        KeyOutcome::PasteText
    );
    assert!(workspace.paste_text("hello  paste"));
    assert_eq!(workspace.draft, "hello  paste");
}

#[test]
fn attach_image_adds_to_workspace_insert_draft() {
    let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);
    assert!(!workspace.attach_image("image/png".to_string(), "ignored".to_string()));

    workspace.handle_key(KeyInput::Character("i".to_string()));
    assert_eq!(
        workspace.handle_key(KeyInput::AttachClipboardImage),
        KeyOutcome::AttachClipboardImage
    );
    assert!(workspace.attach_image("image/png".to_string(), "abc123".to_string()));
    assert_eq!(workspace.pending_images.len(), 1);
    assert!(workspace.status_title().contains("1 image"));
}

#[test]
fn clear_attached_images_shortcut_clears_workspace_images() {
    let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);
    workspace.handle_key(KeyInput::Character("i".to_string()));
    workspace.attach_image("image/png".to_string(), "abc123".to_string());

    assert_eq!(
        workspace.handle_key(KeyInput::ClearAttachedImages),
        KeyOutcome::Redraw
    );
    assert!(workspace.pending_images.is_empty());
    assert_eq!(
        workspace.handle_key(KeyInput::ClearAttachedImages),
        KeyOutcome::None
    );
}

#[test]
fn workspace_image_draft_submits_images_and_clears_pending_images() {
    let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);
    workspace.handle_key(KeyInput::Character("i".to_string()));
    workspace.attach_image("image/png".to_string(), "abc123".to_string());

    assert_eq!(
        workspace.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::SendDraft {
            session_id: "a".to_string(),
            title: "alpha".to_string(),
            message: String::new(),
            images: vec![("image/png".to_string(), "abc123".to_string())]
        }
    );
    assert_eq!(workspace.mode, InputMode::Navigation);
    assert!(workspace.pending_images.is_empty());
}

#[test]
fn workspace_placeholder_preserves_image_draft_when_submit_has_no_target() {
    let mut workspace = Workspace::fake();
    workspace.handle_key(KeyInput::Character("i".to_string()));
    workspace.handle_key(KeyInput::Character("hello".to_string()));
    workspace.attach_image("image/png".to_string(), "abc123".to_string());

    assert_eq!(
        workspace.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::None
    );
    assert_eq!(workspace.draft, "hello");
    assert_eq!(workspace.pending_images.len(), 1);
}

#[test]
fn empty_or_placeholder_draft_does_not_submit() {
    let mut workspace = Workspace::fake();
    workspace.handle_key(KeyInput::Character("i".to_string()));
    workspace.handle_key(KeyInput::Character("hello".to_string()));

    assert_eq!(
        workspace.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::None
    );
    assert_eq!(workspace.draft, "hello");
}

#[test]
fn zoomed_j_and_k_scroll_detail_instead_of_switching_workspace() {
    let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);
    workspace.surfaces[0].detail_lines = vec![
        "line 0".to_string(),
        "line 1".to_string(),
        "line 2".to_string(),
        "line 3".to_string(),
    ];
    workspace.handle_key(KeyInput::Character("z".to_string()));

    assert_eq!(workspace.current_workspace(), 0);
    assert_eq!(
        workspace.handle_key(KeyInput::Character("j".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.detail_scroll, 1);
    assert_eq!(workspace.current_workspace(), 0);
    assert_eq!(
        workspace.handle_key(KeyInput::Character("k".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.detail_scroll, 0);
}

#[test]
fn zoomed_g_and_shift_g_jump_detail_scroll() {
    let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);
    workspace.surfaces[0].detail_lines = (0..5).map(|index| format!("line {index}")).collect();
    workspace.handle_key(KeyInput::Character("z".to_string()));

    assert_eq!(
        workspace.handle_key(KeyInput::Character("G".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.detail_scroll, 4);
    assert_eq!(
        workspace.handle_key(KeyInput::Character("g".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.detail_scroll, 0);
}

#[test]
fn zoomed_global_scroll_shortcuts_scroll_detail() {
    let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);
    workspace.surfaces[0].detail_lines = (0..20).map(|index| format!("line {index}")).collect();
    workspace.handle_key(KeyInput::Character("z".to_string()));

    assert_eq!(
        workspace.handle_key(KeyInput::ScrollBodyToBottom),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.detail_scroll, 19);

    assert_eq!(
        workspace.handle_key(KeyInput::ScrollBodyLines(1)),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.detail_scroll, 18);

    assert_eq!(
        workspace.handle_key(KeyInput::ScrollBodyPages(1)),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.detail_scroll, 6);

    assert_eq!(
        workspace.handle_key(KeyInput::ScrollBodyToTop),
        KeyOutcome::Redraw
    );
    assert_eq!(workspace.detail_scroll, 0);
}

#[test]
fn workspace_exit_shortcut_requests_exit() {
    let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);
    assert_eq!(workspace.handle_key(KeyInput::ExitApp), KeyOutcome::Exit);

    workspace.handle_key(KeyInput::Character("i".to_string()));
    assert_eq!(workspace.handle_key(KeyInput::ExitApp), KeyOutcome::Exit);
}

fn assert_unique_positions(workspace: &Workspace) {
    let positions: HashSet<(i32, i32)> = workspace
        .surfaces
        .iter()
        .map(|surface| (surface.lane, surface.column))
        .collect();
    assert_eq!(positions.len(), workspace.surfaces.len());
}

fn session_card(id: &str, title: &str) -> SessionCard {
    SessionCard {
        session_id: id.to_string(),
        title: title.to_string(),
        subtitle: "active · model".to_string(),
        detail: "1 msgs · workspace".to_string(),
        preview_lines: vec!["user hello".to_string()],
        detail_lines: vec!["user expanded hello".to_string()],
    }
}
