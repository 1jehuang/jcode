#[test]
fn test_side_diagram_uses_left_splitter_instead_of_rounded_box() {
    let _lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_pane_position = crate::config::DiagramPanePosition::Side;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x444, 900, 450, Some("side".to_string()));

    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    let text = render_and_snap(&app, &mut terminal);

    let diagram_area = crate::tui::ui::last_layout_snapshot()
        .and_then(|layout| layout.diagram_area)
        .expect("expected side diagram area after render");
    let buf = terminal.backend().buffer();

    assert_eq!(buf[(diagram_area.x, diagram_area.y)].symbol(), "│");
    assert_eq!(buf[(diagram_area.x, diagram_area.y + 1)].symbol(), "│");
    assert!(text.contains("pinned 1/1"), "rendered text: {text}");

    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_tool_side_panel_focus_supports_horizontal_pan_keys() {
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "hello".to_string(),
            updated_at_ms: 1,
        }],
    };

    assert!(app.handle_diagram_ctrl_key(KeyCode::Char('l'), false));
    assert!(app.diff_pane_focus);

    app.handle_key(KeyCode::Right, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.diff_pane_scroll_x, 4);
    assert!(app.input.is_empty());

    app.handle_key(KeyCode::Left, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.diff_pane_scroll_x, 0);
}

#[test]
fn test_mouse_horizontal_scroll_over_tool_side_panel_pans_without_focus_change() {
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.diff_pane_scroll_x = 0;
    app.diff_pane_focus = false;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "hello".to_string(),
            updated_at_ms: 1,
        }],
    };

    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 40, 20),
        None,
        Some(Rect::new(40, 0, 20, 20)),
        None,
    );

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollRight,
        column: 45,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(
        !scroll_only,
        "side-panel horizontal pan should request an immediate redraw"
    );
    assert_eq!(app.diff_pane_scroll_x, 3);
    assert!(!app.diff_pane_focus);
}

#[test]
fn test_mouse_scroll_events_are_classified_as_scroll_only() {
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::File;

    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 40, 20),
        None,
        Some(Rect::new(40, 0, 20, 20)),
        None,
    );

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 45,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(
        scroll_only,
        "scroll wheel events should be deferrable during streaming"
    );

    let non_scroll = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 10,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(!non_scroll, "clicks should still redraw immediately");
}

#[test]
fn test_handterm_native_scroll_command_updates_chat_offset() {
    let mut app = create_test_app();
    let (_scroll_app, mut terminal) = create_scroll_test_app(50, 12, 0, 24);
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("draw failed");
    crate::tui::ui::record_layout_snapshot(Rect::new(0, 0, 50, 12), None, None, None);

    app.auto_scroll_paused = true;
    app.scroll_offset = 6;
    app.apply_handterm_native_scroll(super::handterm_native_scroll::HostToApp::Scroll {
        pane: super::handterm_native_scroll::PaneKind::Chat,
        delta: -2,
    });
    assert_eq!(app.scroll_offset, 4);

    app.apply_handterm_native_scroll(super::handterm_native_scroll::HostToApp::Scroll {
        pane: super::handterm_native_scroll::PaneKind::Chat,
        delta: 3,
    });
    assert_eq!(app.scroll_offset, 7);
}

#[cfg(unix)]
#[test]
fn test_handterm_native_scroll_client_roundtrips_over_socket() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;

    let _lock = crate::storage::lock_test_env();
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("handterm-scroll.sock");
    let listener = UnixListener::bind(&socket_path).expect("bind unix listener");
    unsafe {
        std::env::set_var("HANDTERM_NATIVE_SCROLL_SOCKET", &socket_path);
    }

    let mut client = super::handterm_native_scroll::HandtermNativeScrollClient::connect_from_env()
        .expect("native scroll client should connect from env");
    let (mut server, _) = listener.accept().expect("accept client");
    server
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("set read timeout");

    let (mut app, mut terminal) = create_scroll_test_app(50, 12, 0, 24);
    app.auto_scroll_paused = true;
    app.scroll_offset = 6;
    let _ = render_and_snap(&app, &mut terminal);

    client.sync_from_app(&app);

    let mut buf = [0u8; 4096];
    let n = server.read(&mut buf).expect("read pane snapshot");
    let line = std::str::from_utf8(&buf[..n]).expect("utf8 snapshot");
    assert!(line.contains("pane_snapshot"));
    assert!(line.contains("chat"));
    assert!(line.contains("\"position\":6"));

    server
        .write_all(b"{\"type\":\"scroll\",\"pane\":\"chat\",\"delta\":-2}\n")
        .expect("write host scroll command");

    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let command = runtime
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(1), client.recv())
                .await
                .expect("timeout waiting for scroll command")
        })
        .expect("scroll command should arrive");

    app.apply_handterm_native_scroll(command);
    assert_eq!(app.scroll_offset, 4);

    unsafe {
        std::env::remove_var("HANDTERM_NATIVE_SCROLL_SOCKET");
    }
}

#[test]
fn test_mouse_scroll_help_overlay_updates_help_scroll() {
    let mut app = create_test_app();
    app.help_scroll = Some(5);

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(
        scroll_only,
        "help overlay mouse wheel should be scroll-only"
    );
    assert_eq!(app.help_scroll, Some(6));

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(scroll_only);
    assert_eq!(app.help_scroll, Some(5));
}

#[test]
fn test_mouse_scroll_changelog_overlay_updates_changelog_scroll() {
    let mut app = create_test_app();
    app.changelog_scroll = Some(2);

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(
        scroll_only,
        "changelog overlay mouse wheel should be scroll-only"
    );
    assert_eq!(app.changelog_scroll, Some(1));

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(scroll_only);
    assert_eq!(app.changelog_scroll, Some(2));
}

#[test]
fn test_mouse_scroll_over_unfocused_diagram_does_not_resize_pane() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_pane_position = crate::config::DiagramPanePosition::Side;
    app.diagram_pane_ratio = 40;
    app.diagram_pane_ratio_from = 40;
    app.diagram_pane_ratio_target = 40;
    app.diagram_pane_anim_start = None;
    app.diagram_focus = false;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x444, 900, 450, None);
    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 80, 30),
        Some(Rect::new(80, 0, 40, 30)),
        None,
        None,
    );

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 90,
        row: 10,
        modifiers: KeyModifiers::empty(),
    });

    assert!(scroll_only);
    assert_eq!(app.diagram_pane_ratio, 40);
    assert_eq!(app.diagram_pane_ratio_from, 40);
    assert_eq!(app.diagram_pane_ratio_target, 40);
    assert!(app.diagram_pane_anim_start.is_none());

    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_dragging_diagram_border_resizes_immediately_without_animation() {
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_pane_position = crate::config::DiagramPanePosition::Side;
    app.diagram_pane_ratio = 40;
    app.diagram_pane_ratio_from = 40;
    app.diagram_pane_ratio_target = 40;
    app.diagram_pane_anim_start = Some(Instant::now());
    app.diagram_pane_dragging = false;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x445, 900, 450, None);
    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 80, 30),
        Some(Rect::new(80, 0, 40, 30)),
        None,
        None,
    );

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 80,
        row: 10,
        modifiers: KeyModifiers::empty(),
    });
    assert!(app.diagram_pane_dragging);

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 72,
        row: 10,
        modifiers: KeyModifiers::empty(),
    });

    assert_eq!(app.diagram_pane_ratio, 40);
    assert_eq!(app.diagram_pane_ratio_from, 40);
    assert_eq!(app.diagram_pane_ratio_target, 40);
    assert!(app.diagram_pane_anim_start.is_none());

    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_is_scroll_only_key_detects_navigation_inputs() {
    let mut app = create_test_app();

    let (up_code, up_mods) = scroll_up_key(&app);
    assert!(super::input::is_scroll_only_key(&app, up_code, up_mods));

    let (down_code, down_mods) = scroll_down_key(&app);
    assert!(super::input::is_scroll_only_key(&app, down_code, down_mods));

    app.diff_pane_focus = true;
    assert!(super::input::is_scroll_only_key(
        &app,
        KeyCode::Char('j'),
        KeyModifiers::empty()
    ));

    assert!(super::input::is_scroll_only_key(
        &app,
        KeyCode::BackTab,
        KeyModifiers::empty()
    ));

    assert!(!super::input::is_scroll_only_key(
        &app,
        KeyCode::Char('a'),
        KeyModifiers::empty()
    ));
    assert!(!super::input::is_scroll_only_key(
        &app,
        KeyCode::Enter,
        KeyModifiers::empty()
    ));
}

#[test]
fn test_fuzzy_command_suggestions() {
    let app = create_test_app();
    let suggestions = app.get_suggestions_for("/mdl");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/model"));
}

#[test]
fn test_refresh_model_list_command_suggestions() {
    let app = create_test_app();
    let suggestions = app.get_suggestions_for("/refresh");
    assert!(
        suggestions
            .iter()
            .any(|(cmd, _)| cmd == "/refresh-model-list")
    );
    assert!(!suggestions.iter().any(|(cmd, _)| cmd == "/refresh-models"));

    let spaced = app.get_suggestions_for("/refresh ");
    assert!(spaced.is_empty());
}

#[test]
fn test_registered_command_suggestions_include_aliases_and_hide_secret_commands() {
    let app = create_test_app();
    let suggestions = app.get_suggestions_for("/");
    let commands: Vec<&str> = suggestions.iter().map(|(cmd, _)| cmd.as_str()).collect();

    assert!(commands.contains(&"/models"));
    assert!(commands.contains(&"/sessions"));
    assert!(commands.contains(&"/dictation"));
    assert!(commands.contains(&"/feedback"));
    assert!(!commands.contains(&"/z"));
    assert!(!commands.contains(&"/zz"));
    assert!(!commands.contains(&"/zzz"));
}

#[test]
fn test_auth_doctor_command_suggestion_is_not_shadowed_by_provider_suggestions() {
    let app = create_test_app();
    let suggestions = app.get_suggestions_for("/auth d");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/auth doctor"));
}

#[test]
fn test_top_level_command_suggestions_include_config_and_subscription() {
    let app = create_test_app();
    let suggestions = app.get_suggestions_for("/con");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/config"));
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/context"));

    let suggestions = app.get_suggestions_for("/ali");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/alignment"));

    let suggestions = app.get_suggestions_for("/sub");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/subscription"));
}

#[test]
fn test_help_cmds_command_is_registered_and_runs() {
    let mut app = create_test_app();

    // Autocomplete: typing `/help-` should suggest `/help-cmds`.
    let suggestions = app.get_suggestions_for("/help-");
    assert!(
        suggestions.iter().any(|(cmd, _)| cmd == "/help-cmds"),
        "expected /help-cmds in autocomplete suggestions, got: {:?}",
        suggestions
            .iter()
            .map(|(c, _)| c.clone())
            .collect::<Vec<_>>()
    );

    // Handler: invoking it pushes a system message containing the cheat
    // sheet markdown (we sniff for distinctive headings rather than the
    // full body so the test stays robust to wording tweaks).
    let messages_before = app.display_messages.len();
    let handled = crate::tui::app::state_ui::handle_info_command(&mut app, "/help-cmds");
    assert!(handled, "/help-cmds handler should claim the input");
    assert_eq!(
        app.display_messages.len(),
        messages_before + 1,
        "/help-cmds should push exactly one display message"
    );
    let pushed = app.display_messages.last().expect("pushed message");
    assert_eq!(pushed.role, "system");
    assert_eq!(pushed.title.as_deref(), Some("/help-cmds"));
    assert!(
        pushed.content.contains("# jcode cheat sheet"),
        "expected cheat-sheet header in message body"
    );
    assert!(
        pushed.content.contains("Multiple terminals / vertical windows"),
        "expected multi-window section in message body"
    );
    assert!(
        pushed.content.contains("`~/.jcode/config.toml`"),
        "expected config path in message body"
    );
}

#[test]
fn test_top_level_command_suggestions_include_catchup_and_back() {
    let app = create_test_app();

    let suggestions = app.get_suggestions_for("/cat");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/catchup"));

    let suggestions = app.get_suggestions_for("/bac");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/back"));

    let suggestions = app.get_suggestions_for("/gi");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/git"));
}

#[test]
fn test_help_topic_suggestions_are_contextual() {
    let app = create_test_app();
    let suggestions = app.get_suggestions_for("/help fi");
    assert_eq!(
        suggestions.first().map(|(cmd, _)| cmd.as_str()),
        Some("/help fix")
    );
}

#[test]
fn test_help_topic_suggestions_include_catchup_topics() {
    let app = create_test_app();

    let suggestions = app.get_suggestions_for("/help cat");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/help catchup"));

    let suggestions = app.get_suggestions_for("/help bac");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/help back"));
}

#[test]
fn test_context_command_reports_session_context_snapshot() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.memory_enabled = true;
        app.swarm_enabled = true;
        app.queue_mode = true;
        app.active_skill = Some("debug".to_string());
        app.queued_messages.push("queued follow-up".to_string());
        app.pending_images
            .push(("image/png".to_string(), "abc".to_string()));
        app.side_panel = crate::side_panel::SidePanelSnapshot {
            focused_page_id: Some("goals".to_string()),
            pages: vec![crate::side_panel::SidePanelPage {
                id: "goals".to_string(),
                title: "Goals".to_string(),
                file_path: "".to_string(),
                format: crate::side_panel::SidePanelPageFormat::Markdown,
                source: crate::side_panel::SidePanelPageSource::Managed,
                content: "goal details".to_string(),
                updated_at_ms: 0,
            }],
        };
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "one".to_string(),
                content: "Inspect context summary".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.input = "/context".to_string();
        app.submit_input();

        let msg = app
            .display_messages()
            .last()
            .expect("missing context report");
        assert_eq!(msg.title.as_deref(), Some("Context"));
        assert!(msg.content.contains("# Session Context"));
        assert!(msg.content.contains("## Prompt / Context Composition"));
        assert!(msg.content.contains("## Compaction"));
        assert!(msg.content.contains("## Session State"));
        assert!(msg.content.contains("## Todos"));
        assert!(msg.content.contains("## Side Panel"));
        assert!(msg.content.contains("Inspect context summary"));
        assert!(msg.content.contains("active skill: debug"));
        assert!(msg.content.contains("queue mode: on"));
    });
}

#[test]
fn test_nested_command_suggestions_filter_partial_suffixes() {
    let app = create_test_app();

    let suggestions = app.get_suggestions_for("/config ed");
    assert_eq!(
        suggestions.first().map(|(cmd, _)| cmd.as_str()),
        Some("/config edit")
    );

    let suggestions = app.get_suggestions_for("/alignment ce");
    assert_eq!(
        suggestions.first().map(|(cmd, _)| cmd.as_str()),
        Some("/alignment centered")
    );

    let suggestions = app.get_suggestions_for("/compact mo se");
    assert_eq!(
        suggestions.first().map(|(cmd, _)| cmd.as_str()),
        Some("/compact mode semantic")
    );

    let suggestions = app.get_suggestions_for("/memory st");
    assert_eq!(
        suggestions.first().map(|(cmd, _)| cmd.as_str()),
        Some("/memory status")
    );

    let suggestions = app.get_suggestions_for("/improve st");
    assert!(
        suggestions.iter().any(|(cmd, _)| cmd == "/improve status"),
        "expected /improve status suggestion"
    );

    let suggestions = app.get_suggestions_for("/refactor st");
    assert!(
        suggestions.iter().any(|(cmd, _)| cmd == "/refactor status"),
        "expected /refactor status suggestion"
    );
}

#[test]
fn test_autocomplete_adds_space_for_nested_argument_commands() {
    let mut app = create_test_app();
    app.input = "/goals sh".to_string();
    app.cursor_pos = app.input.len();

    assert!(app.autocomplete());
    assert_eq!(app.input(), "/goals show ");
}

#[test]
fn test_goals_show_suggestions_include_goal_ids() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("repo");
    std::fs::create_dir_all(&project).expect("project dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let goal = crate::goal::create_goal(
        crate::goal::GoalCreateInput {
            title: "Ship mobile MVP".to_string(),
            scope: crate::goal::GoalScope::Project,
            ..crate::goal::GoalCreateInput::default()
        },
        Some(&project),
    )
    .expect("create goal");

    let mut app = create_test_app();
    app.session.working_dir = Some(project.display().to_string());

    let suggestions = app.get_suggestions_for("/goals show ");
    assert!(
        suggestions
            .iter()
            .any(|(cmd, _)| cmd == &format!("/goals show {}", goal.id))
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

fn configure_test_remote_models(app: &mut App) {
    app.is_remote = true;
    app.remote_provider_model = Some("gpt-5.3-codex".to_string());
    app.remote_available_entries = vec!["gpt-5.3-codex".to_string(), "gpt-5.2-codex".to_string()];
}

fn configure_test_remote_models_with_openai_recommendations(app: &mut App) {
    app.is_remote = true;
    app.remote_provider_model = Some("gpt-5.2".to_string());
    app.remote_available_entries = vec![
        "gpt-5.2".to_string(),
        "gpt-5.5".to_string(),
        "gpt-5.4".to_string(),
        "gpt-5.4-pro".to_string(),
        "gpt-5.3-codex-spark".to_string(),
        "gpt-5.3-codex".to_string(),
        "claude-opus-4-7".to_string(),
    ];
    app.remote_model_options = app
        .remote_available_entries
        .iter()
        .cloned()
        .map(|model| crate::provider::ModelRoute {
            model,
            provider: "OpenAI".to_string(),
            api_method: "openai-oauth".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        })
        .collect();
}

fn configure_test_remote_openrouter_provider_routes(app: &mut App) {
    app.is_remote = true;
    app.remote_provider_name = Some("openrouter".to_string());
    app.remote_provider_model = Some("anthropic/claude-sonnet-4".to_string());
    app.remote_available_entries = vec!["anthropic/claude-sonnet-4".to_string()];
    app.remote_model_options = vec![
        crate::provider::ModelRoute {
            model: "anthropic/claude-sonnet-4".to_string(),
            provider: "auto".to_string(),
            api_method: "openrouter".to_string(),
            available: true,
            detail: "→ Fireworks".to_string(),
            cheapness: None,
        },
        crate::provider::ModelRoute {
            model: "anthropic/claude-sonnet-4".to_string(),
            provider: "Fireworks".to_string(),
            api_method: "openrouter".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        },
        crate::provider::ModelRoute {
            model: "anthropic/claude-sonnet-4".to_string(),
            provider: "OpenAI".to_string(),
            api_method: "openrouter".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        },
    ];
}

#[test]
fn test_model_picker_preview_filter_parsing() {
    assert_eq!(
        App::model_picker_preview_filter("/model"),
        Some(String::new())
    );
    assert_eq!(
        App::model_picker_preview_filter("/model   gpt-5"),
        Some("gpt-5".to_string())
    );
    assert_eq!(
        App::model_picker_preview_filter("   /models codex"),
        Some("codex".to_string())
    );
    assert_eq!(App::model_picker_preview_filter("/modelx"), None);
    assert_eq!(App::model_picker_preview_filter("hello /model"), None);
}

#[test]
fn test_login_picker_preview_filter_parsing() {
    assert_eq!(
        App::login_picker_preview_filter("/login"),
        Some(String::new())
    );
    assert_eq!(
        App::login_picker_preview_filter("/login   zai"),
        Some("zai".to_string())
    );
    assert_eq!(App::login_picker_preview_filter("/loginx"), None);
    assert_eq!(App::login_picker_preview_filter("hello /login"), None);
}

#[test]
fn test_agents_command_opens_agent_picker() {
    let mut app = create_test_app();
    app.input = "/agents".to_string();

    app.submit_input();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("/agents should open the agent picker");
    assert!(
        picker
            .entries
            .iter()
            .any(|entry| entry.name == "Code review")
    );
    assert!(picker.entries.iter().any(|entry| matches!(
        entry.action,
        crate::tui::PickerAction::AgentTarget(crate::tui::AgentModelTarget::Swarm)
    )));
}

#[test]
fn test_agents_command_suggestions_include_targets() {
    let app = create_test_app();
    let suggestions = app.get_suggestions_for("/agents re");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/agents review"));
}

#[test]
fn test_agents_picker_uses_provider_default_when_inherited_model_is_unknown() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.open_agents_picker();

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("/agents should open the agent picker");
        let swarm_entry = picker
            .entries
            .iter()
            .find(|entry| {
                matches!(
                    entry.action,
                    crate::tui::PickerAction::AgentTarget(crate::tui::AgentModelTarget::Swarm)
                )
            })
            .expect("swarm entry should exist");

        assert_eq!(swarm_entry.options[0].provider, "provider default");
    });
}

#[test]
fn test_agent_model_picker_inherit_row_uses_provider_default_when_inherited_model_is_unknown() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        configure_test_remote_models(&mut app);
        app.open_agent_model_picker(crate::tui::AgentModelTarget::Swarm);

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("agent model picker should open");
        let inherit_entry = picker.entries.first().expect("inherit row should exist");

        assert_eq!(inherit_entry.name, "inherit (provider default)");
        assert!(matches!(
            inherit_entry.action,
            crate::tui::PickerAction::AgentModelChoice {
                target: crate::tui::AgentModelTarget::Swarm,
                clear_override: true,
            }
        ));
    });
}
