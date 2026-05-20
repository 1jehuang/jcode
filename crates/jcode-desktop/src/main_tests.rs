use super::animation::{FOCUS_PULSE_DURATION, VIEWPORT_ANIMATION_DURATION};
use super::single_session::*;
use super::*;
use std::sync::Mutex;

#[test]
fn desktop_frame_profile_is_opt_in_and_recognizes_trace_modes() {
    assert!(!desktop_frame_profile_enabled(None));
    assert!(!desktop_frame_profile_enabled(Some("")));
    assert!(!desktop_frame_profile_enabled(Some("off")));
    assert!(!desktop_frame_profile_enabled(Some("0")));
    assert!(desktop_frame_profile_enabled(Some("1")));
    assert!(desktop_frame_profile_enabled(Some("true")));
    assert!(desktop_frame_profile_enabled(Some("all")));
    assert!(desktop_frame_profile_enabled(Some("trace")));
    assert!(!desktop_frame_profile_log_all(None));
    assert!(!desktop_frame_profile_log_all(Some("1")));
    assert!(desktop_frame_profile_log_all(Some("all")));
    assert!(desktop_frame_profile_log_all(Some("TRACE")));
}

#[test]
fn desktop_config_parses_positive_millisecond_durations_only() {
    assert_eq!(
        parse_positive_duration_millis("8.5"),
        Some(Duration::from_secs_f64(0.0085))
    );
    assert_eq!(
        parse_positive_duration_millis(" 250 "),
        Some(Duration::from_millis(250))
    );
    assert_eq!(parse_positive_duration_millis("0"), None);
    assert_eq!(parse_positive_duration_millis("-1"), None);
    assert_eq!(parse_positive_duration_millis("NaN"), None);
    assert_eq!(parse_positive_duration_millis("inf"), None);
    assert_eq!(parse_positive_duration_millis("nope"), None);
}

#[test]
fn desktop_platform_warnings_only_fire_for_less_supported_targets() {
    assert_eq!(
        desktop_platform_support_warning(DesktopPlatform::Linux),
        None
    );
    assert_eq!(
        desktop_platform_support_warning(DesktopPlatform::Macos),
        None
    );
    assert!(desktop_platform_support_warning(DesktopPlatform::Windows).is_some());
    assert!(desktop_platform_support_warning(DesktopPlatform::Other).is_some());
}

#[test]
fn desktop_hot_reload_rewrites_resume_to_live_session() {
    let relaunch = DesktopRelaunch {
        binary: PathBuf::from("/old/jcode-desktop"),
        args: vec![
            OsString::from("--fullscreen"),
            OsString::from("--resume"),
            OsString::from("stale-session"),
            OsString::from("--startup-log"),
        ],
    };
    let mut single_session = SingleSessionApp::new(None);
    single_session.initialize_resumed_session("live-session");
    let app = DesktopApp::SingleSession(single_session);

    let updated = relaunch.for_app(&app, PathBuf::from("/new/jcode-desktop"));

    assert_eq!(updated.binary, PathBuf::from("/new/jcode-desktop"));
    assert_eq!(
        updated.args,
        vec![
            OsString::from("--fullscreen"),
            OsString::from("--startup-log"),
            OsString::from("--resume"),
            OsString::from("live-session"),
        ]
    );
}

#[test]
fn desktop_hot_reload_drops_resume_when_current_app_is_fresh() {
    let relaunch = DesktopRelaunch {
        binary: PathBuf::from("/old/jcode-desktop"),
        args: vec![
            OsString::from("--resume=stale-session"),
            OsString::from("--fullscreen"),
        ],
    };
    let app = fresh_single_session_app();

    let updated = relaunch.for_app(&app, PathBuf::from("/new/jcode-desktop"));

    assert_eq!(updated.args, vec![OsString::from("--fullscreen")]);
}

#[test]
fn desktop_hot_reload_persists_workspace_focus_before_spawn() -> Result<()> {
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    let Ok(_guard) = ENV_LOCK.lock() else {
        anyhow::bail!("desktop hot reload env lock poisoned");
    };
    let temp = unique_desktop_test_dir("desktop-hot-reload-workspace-state")?;
    let state_path = temp.join("desktop-state.json");
    unsafe {
        std::env::set_var("JCODE_DESKTOP_STATE", &state_path);
    }

    let relaunch = DesktopRelaunch {
        binary: PathBuf::from("/old/jcode-desktop"),
        args: vec![OsString::from("--workspace")],
    };
    let cards = vec![
        workspace::SessionCard {
            session_id: "session-a".to_string(),
            title: "alpha".to_string(),
            subtitle: "active".to_string(),
            detail: "1 message".to_string(),
            preview_lines: vec![],
            detail_lines: vec![],
        },
        workspace::SessionCard {
            session_id: "session-b".to_string(),
            title: "bravo".to_string(),
            subtitle: "active".to_string(),
            detail: "2 messages".to_string(),
            preview_lines: vec![],
            detail_lines: vec![],
        },
    ];
    let mut workspace = Workspace::from_session_cards(cards);
    workspace.apply_preferences(workspace::DesktopPreferences {
        panel_size: PanelSizePreset::ThreeQuarter,
        focused_session_id: Some("session-b".to_string()),
        workspace_lane: 0,
        space_hold_toggle_ms: 333,
    });
    let app = DesktopApp::Workspace(workspace);

    let updated = relaunch.for_app(&app, PathBuf::from("/new/jcode-desktop"));

    assert_eq!(updated.args, vec![OsString::from("--workspace")]);
    let saved = desktop_prefs::load_preferences()?.expect("workspace preferences saved");
    assert_eq!(saved.focused_session_id.as_deref(), Some("session-b"));
    assert_eq!(saved.panel_size, PanelSizePreset::ThreeQuarter);
    assert_eq!(saved.space_hold_toggle_ms, 333);

    unsafe {
        std::env::remove_var("JCODE_DESKTOP_STATE");
    }
    std::fs::remove_dir_all(temp)?;
    Ok(())
}

#[test]
fn desktop_hot_reload_prefers_newer_selfdev_binary() -> Result<()> {
    let temp = unique_desktop_test_dir("desktop-hot-reload-candidate")?;
    let current = temp.join("installed").join(desktop_binary_name());
    let selfdev = desktop_selfdev_binary_path(&temp);
    std::fs::create_dir_all(current.parent().unwrap())?;
    std::fs::write(&current, b"old")?;
    std::fs::create_dir_all(selfdev.parent().unwrap())?;
    let current_modified = std::fs::metadata(&current)?.modified()?;
    for attempt in 0..10 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::fs::write(&selfdev, format!("new-{attempt}"))?;
        if std::fs::metadata(&selfdev)?.modified()? > current_modified {
            break;
        }
    }
    assert!(std::fs::metadata(&selfdev)?.modified()? > current_modified);

    assert_eq!(
        desktop_reload_binary_candidate_from(&current, &temp),
        selfdev
    );

    std::fs::remove_dir_all(temp)?;
    Ok(())
}

fn unique_desktop_test_dir(name: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!(
        "jcode-{name}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[test]
fn primitive_vertex_buffer_capacity_grows_and_shrinks_with_hysteresis() {
    assert_eq!(primitive_vertex_capacity_for_len(0), 0);
    assert_eq!(
        primitive_vertex_capacity_for_len(1),
        PRIMITIVE_VERTEX_BUFFER_MIN_CAPACITY
    );
    assert_eq!(
        primitive_vertex_capacity_for_len(PRIMITIVE_VERTEX_BUFFER_MIN_CAPACITY + 1),
        (PRIMITIVE_VERTEX_BUFFER_MIN_CAPACITY + 1).next_power_of_two()
    );
    assert!(!primitive_vertex_buffer_should_reallocate(
        PRIMITIVE_VERTEX_BUFFER_MIN_CAPACITY,
        0,
    ));
    assert!(!primitive_vertex_buffer_should_reallocate(
        PRIMITIVE_VERTEX_BUFFER_MIN_CAPACITY,
        PRIMITIVE_VERTEX_BUFFER_MIN_CAPACITY / 2,
    ));
    assert!(primitive_vertex_buffer_should_reallocate(128, 129));
    assert!(!primitive_vertex_buffer_should_reallocate(4096, 1024));
    assert!(primitive_vertex_buffer_should_reallocate(4096, 1023));
}

#[test]
fn streaming_text_renderer_releases_only_after_streaming_buffer_disappears() {
    assert!(!streaming_text_renderer_should_release(true, true, true));
    assert!(!streaming_text_renderer_should_release(false, false, false));
    assert!(streaming_text_renderer_should_release(false, true, false));
    assert!(streaming_text_renderer_should_release(false, false, true));
}

#[test]
fn workspace_vertex_capacity_hint_scales_with_surface_count() {
    let first_card = workspace::SessionCard {
        session_id: "a".to_string(),
        title: "alpha".to_string(),
        subtitle: "active".to_string(),
        detail: "1 msg".to_string(),
        preview_lines: Vec::new(),
        detail_lines: Vec::new(),
    };
    let second_card = workspace::SessionCard {
        session_id: "b".to_string(),
        title: "beta".to_string(),
        subtitle: "idle".to_string(),
        detail: "2 msgs".to_string(),
        preview_lines: Vec::new(),
        detail_lines: Vec::new(),
    };
    let mut workspace = Workspace::from_session_cards(vec![first_card.clone()]);

    assert_eq!(
        workspace_vertex_capacity_hint(&workspace),
        WORKSPACE_BASE_VERTEX_CAPACITY_HINT + WORKSPACE_SURFACE_VERTEX_CAPACITY_HINT
    );

    workspace = Workspace::from_session_cards(vec![first_card, second_card]);
    assert_eq!(
        workspace_vertex_capacity_hint(&workspace),
        WORKSPACE_BASE_VERTEX_CAPACITY_HINT + WORKSPACE_SURFACE_VERTEX_CAPACITY_HINT * 2
    );
}

#[test]
fn desktop_background_wake_only_tracks_active_frame_animation() {
    let now = Instant::now();

    assert_eq!(
        desktop_background_wake(now, true, true),
        Some(now + BACKGROUND_POLL_INTERVAL)
    );
    assert_eq!(desktop_background_wake(now, true, false), None);
    assert_eq!(desktop_background_wake(now, false, true), None);
}

#[test]
fn desktop_async_job_slots_are_bounded_and_released() -> Result<()> {
    let counter = std::sync::atomic::AtomicUsize::new(0);
    let first = try_acquire_desktop_async_job_slot(&counter, 2)?;
    let second = try_acquire_desktop_async_job_slot(&counter, 2)?;

    assert!(try_acquire_desktop_async_job_slot(&counter, 2).is_err());
    drop(first);
    let third = try_acquire_desktop_async_job_slot(&counter, 2)?;
    assert!(try_acquire_desktop_async_job_slot(&counter, 2).is_err());
    drop(second);
    drop(third);
    assert_eq!(counter.load(std::sync::atomic::Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn quarter_size_preset_follows_quarter_screen_width_steps() {
    let monitor_width = Some(2000);

    assert_eq!(inferred_visible_column_count(500, monitor_width, 0.25), 1);
    assert_eq!(inferred_visible_column_count(1000, monitor_width, 0.25), 2);
    assert_eq!(inferred_visible_column_count(1500, monitor_width, 0.25), 3);
    assert_eq!(inferred_visible_column_count(2000, monitor_width, 0.25), 4);
}

#[test]
fn preferred_panel_size_limits_visible_column_count() {
    let monitor_width = Some(2000);

    assert_eq!(inferred_visible_column_count(2000, monitor_width, 0.25), 4);
    assert_eq!(inferred_visible_column_count(2000, monitor_width, 0.50), 2);
    assert_eq!(inferred_visible_column_count(2000, monitor_width, 0.75), 1);
    assert_eq!(inferred_visible_column_count(2000, monitor_width, 1.00), 1);

    assert_eq!(inferred_visible_column_count(500, monitor_width, 0.25), 1);
    assert_eq!(inferred_visible_column_count(500, monitor_width, 1.00), 1);
}

#[test]
fn visible_column_count_tolerates_window_manager_gaps() {
    let monitor_width = Some(2000);

    assert_eq!(inferred_visible_column_count(1940, monitor_width, 0.25), 4);
    assert_eq!(inferred_visible_column_count(970, monitor_width, 0.25), 2);
    assert_eq!(inferred_visible_column_count(1940, monitor_width, 0.50), 2);
}

#[test]
fn visible_column_count_is_clamped_and_safe_without_monitor() {
    assert_eq!(inferred_visible_column_count(1, Some(2000), 0.25), 1);
    assert_eq!(inferred_visible_column_count(3000, Some(2000), 0.25), 4);
    assert_eq!(inferred_visible_column_count(1000, Some(0), 0.25), 1);
    assert_eq!(inferred_visible_column_count(1000, None, 0.25), 1);
}

#[test]
fn desktop_surface_size_renderable_requires_non_zero_dimensions() {
    assert!(desktop_surface_size_is_renderable(PhysicalSize::new(1, 1)));
    assert!(!desktop_surface_size_is_renderable(PhysicalSize::new(0, 1)));
    assert!(!desktop_surface_size_is_renderable(PhysicalSize::new(1, 0)));
    assert!(!desktop_surface_size_is_renderable(PhysicalSize::new(0, 0)));
}

#[test]
fn desktop_canvas_uses_owned_static_surface_lifetime() {
    fn accepts_concrete_canvas_type<T>() {}
    fn assert_static_surface(_: Option<wgpu::Surface<'static>>) {}

    accepts_concrete_canvas_type::<Canvas>();
    assert_static_surface(None);
}

#[test]
fn surface_timeout_backoff_doubles_until_cap_and_resets() {
    let mut backoff = SurfaceTimeoutBackoff::default();
    let delays = (0..8)
        .map(|_| backoff.record_timeout().0)
        .collect::<Vec<_>>();

    assert_eq!(delays[0], SURFACE_TIMEOUT_BACKOFF_MIN);
    assert_eq!(delays[1], SURFACE_TIMEOUT_BACKOFF_MIN * 2);
    assert_eq!(delays[2], SURFACE_TIMEOUT_BACKOFF_MIN * 4);
    assert!(delays.windows(2).all(|pair| pair[1] >= pair[0]));
    assert!(
        delays
            .iter()
            .all(|delay| *delay <= SURFACE_TIMEOUT_BACKOFF_MAX)
    );

    backoff.reset();
    assert_eq!(backoff.record_timeout().0, SURFACE_TIMEOUT_BACKOFF_MIN);
}

#[test]
fn workspace_status_text_includes_build_hash() {
    let mut workspace = Workspace::fake();

    assert_eq!(
        workspace_status_text(&workspace),
        format!("NAV P25 {}", desktop_build_hash_label())
    );

    workspace.mode = InputMode::Insert;
    assert_eq!(
        workspace_status_text(&workspace),
        format!("INS P25 {}", desktop_build_hash_label())
    );
}

#[test]
fn viewport_animation_interpolates_to_new_layout_target() {
    let mut animation = AnimatedViewport::default();
    let now = Instant::now();
    let visible = VisibleColumnLayout {
        visible_columns: 2,
        first_visible_column: 0,
    };
    let start = WorkspaceRenderLayout {
        visible,
        column_width: 200.0,
        scroll_offset: 0.0,
        vertical_scroll_offset: 0.0,
    };
    let target = WorkspaceRenderLayout {
        visible: VisibleColumnLayout {
            visible_columns: 2,
            first_visible_column: 2,
        },
        column_width: 300.0,
        scroll_offset: 600.0,
        vertical_scroll_offset: 800.0,
    };

    let first_frame = animation.frame(start, now);
    assert_eq!(first_frame.column_width, 200.0);
    assert_eq!(first_frame.scroll_offset, 0.0);
    assert_eq!(first_frame.vertical_scroll_offset, 0.0);
    assert!(!animation.is_animating());

    let transition_start = animation.frame(target, now);
    assert_eq!(transition_start.column_width, 200.0);
    assert_eq!(transition_start.scroll_offset, 0.0);
    assert_eq!(transition_start.vertical_scroll_offset, 0.0);
    assert!(animation.is_animating());

    let middle = animation.frame(target, now + VIEWPORT_ANIMATION_DURATION / 2);
    assert!(middle.column_width > 200.0);
    assert!(middle.column_width < 300.0);
    assert!(middle.scroll_offset > 0.0);
    assert!(middle.scroll_offset < 600.0);
    assert!(middle.vertical_scroll_offset > 0.0);
    assert!(middle.vertical_scroll_offset < 800.0);

    let final_frame = animation.frame(target, now + VIEWPORT_ANIMATION_DURATION);
    assert_eq!(final_frame.column_width, 300.0);
    assert_eq!(final_frame.scroll_offset, 600.0);
    assert_eq!(final_frame.vertical_scroll_offset, 800.0);
    assert!(!animation.is_animating());
}

#[test]
fn focus_pulse_runs_when_focused_surface_changes() {
    let mut pulse = FocusPulse::default();
    let now = Instant::now();

    assert_eq!(pulse.frame(1, now), 0.0);
    assert!(!pulse.is_animating());

    let start = pulse.frame(2, now);
    assert!(start > 0.0);
    assert!(pulse.is_animating());

    let middle = pulse.frame(2, now + FOCUS_PULSE_DURATION / 2);
    assert!(middle > 0.0);
    assert!(middle < start);

    let end = pulse.frame(2, now + FOCUS_PULSE_DURATION);
    assert_eq!(end, 0.0);
    assert!(!pulse.is_animating());
}

#[test]
fn bitmap_text_normalization_sanitizes_panel_titles() {
    assert_eq!(
        normalize_bitmap_text("fox · coordinator"),
        "FOX COORDINATOR"
    );
    assert_eq!(normalize_bitmap_text("agent-12"), "AGENT-12");
    assert_eq!(bitmap_text_width("NAV", 2.0), 34.0);
}

#[test]
fn bitmap_text_wrapping_breaks_on_words() {
    assert_eq!(
        wrap_bitmap_text("ONE TWO THREE", 1.0, bitmap_char_advance(1.0) * 7.0),
        vec!["ONE TWO", "THREE"]
    );
}

#[test]
fn bitmap_text_wrapping_splits_long_words() {
    assert_eq!(
        wrap_bitmap_text("ABCDEFGHI", 1.0, bitmap_char_advance(1.0) * 4.0),
        vec!["ABCD", "EFGH", "I"]
    );
}

#[test]
fn single_session_typography_targets_jetbrains_mono_light_nerd() {
    assert_eq!(SINGLE_SESSION_FONT_FAMILY, "JetBrainsMono Nerd Font");
    assert_eq!(SINGLE_SESSION_FONT_WEIGHT, "Light");
    assert!(SINGLE_SESSION_FONT_FALLBACKS.contains(&"monospace"));
    assert_eq!(SINGLE_SESSION_DEFAULT_FONT_SIZE, 22.0);
    assert_eq!(
        SINGLE_SESSION_TITLE_FONT_SIZE,
        SINGLE_SESSION_DEFAULT_FONT_SIZE
    );
    assert_eq!(
        SINGLE_SESSION_ASSISTANT_FONT_FAMILY,
        SINGLE_SESSION_FONT_FAMILY
    );
    assert_eq!(SINGLE_SESSION_WELCOME_FONT_FAMILY, "Homemade Apple");
    assert_eq!(SINGLE_SESSION_BODY_FONT_SIZE, SINGLE_SESSION_CODE_FONT_SIZE);
    assert_eq!(
        SINGLE_SESSION_META_FONT_SIZE,
        SINGLE_SESSION_DEFAULT_FONT_SIZE
    );
    assert_eq!(SINGLE_SESSION_CODE_FONT_SIZE, SINGLE_SESSION_BODY_FONT_SIZE);
    const {
        assert!(SINGLE_SESSION_BODY_LINE_HEIGHT > SINGLE_SESSION_CODE_LINE_HEIGHT);
        assert!(SINGLE_SESSION_CODE_LINE_HEIGHT > SINGLE_SESSION_META_LINE_HEIGHT);
    }
}

#[test]
fn single_session_vertices_include_a_draft_caret() {
    let mut app = SingleSessionApp::new(None);
    let empty_vertices = build_single_session_vertices(&app, PhysicalSize::new(640, 480), 0.0, 0);
    app.handle_key(KeyInput::Character("abc".to_string()));
    let mut typed_vertices =
        build_single_session_vertices(&app, PhysicalSize::new(640, 480), 0.0, 0);
    push_single_session_caret(&mut typed_vertices, &app, PhysicalSize::new(640, 480), None);

    assert!(!empty_vertices.is_empty());
    assert!(
        typed_vertices
            .iter()
            .any(|vertex| vertex.color == SINGLE_SESSION_CARET_COLOR)
    );
}

#[test]
fn single_session_caret_visibility_follows_overlay_state() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("abc".to_string()));

    assert_eq!(app.active_overlay_state(), SingleSessionOverlay::None);
    assert!(single_session_caret_visible_for_frame(&app, 0));
    assert!(!single_session_caret_visible_for_frame(&app, 3));

    assert_eq!(
        app.handle_key(KeyInput::OpenModelPicker),
        KeyOutcome::LoadModelCatalog
    );
    assert_eq!(
        app.active_overlay_state(),
        SingleSessionOverlay::Inline {
            kind: InlineWidgetKind::ModelPicker,
            mode: InlineWidgetMode::Interactive,
        }
    );
    assert!(!single_session_caret_visible_for_frame(&app, 0));

    let mut preview_app = SingleSessionApp::new(None);
    assert_eq!(
        preview_app.handle_key(KeyInput::Character("/model opus".to_string())),
        KeyOutcome::LoadModelCatalog
    );
    assert_eq!(
        preview_app.active_overlay_state(),
        SingleSessionOverlay::Inline {
            kind: InlineWidgetKind::ModelPicker,
            mode: InlineWidgetMode::ReadOnly,
        }
    );
    assert!(single_session_caret_visible_for_frame(&preview_app, 0));

    let mut help_app = SingleSessionApp::new(None);
    assert_eq!(
        help_app.handle_key(KeyInput::HotkeyHelp),
        KeyOutcome::Redraw
    );
    assert!(!single_session_caret_visible_for_frame(&help_app, 0));
}

#[test]
fn stdin_request_closes_conflicting_inline_overlays() {
    let mut app = SingleSessionApp::new(None);

    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    assert!(app.session_switcher.open);
    app.apply_session_event(session_launch::DesktopSessionEvent::StdinRequest {
        request_id: "stdin-1".to_string(),
        prompt: "Password:".to_string(),
        is_password: true,
        tool_call_id: "tool-1".to_string(),
    });

    assert_eq!(
        app.active_overlay_state(),
        SingleSessionOverlay::StdinResponse
    );
    assert!(!app.session_switcher.open);
    assert!(!app.model_picker.open);
    assert!(!app.show_help);
    assert!(!app.show_session_info);
    assert!(!single_session_caret_visible_for_frame(&app, 0));
}

#[test]
fn single_session_vertices_do_not_draw_input_underline() {
    let fresh_app = SingleSessionApp::new(None);
    let fresh_vertices =
        build_single_session_vertices(&fresh_app, PhysicalSize::new(900, 700), 0.0, 0);
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::SessionStarted {
        session_id: "composer_line".to_string(),
    });
    let vertices = build_single_session_vertices(&app, PhysicalSize::new(900, 700), 0.0, 0);
    let old_composer_line_color = [0.060, 0.085, 0.145, 0.34];
    let outline_color = panel_accent_color(single_session_surface(None).color_index, true);

    assert!(!vertices_have_color(&vertices, old_composer_line_color));
    assert!(!vertices_have_color(
        &fresh_vertices,
        old_composer_line_color
    ));
    assert!(!vertices_have_bottom_center_rule(&vertices, outline_color));
    assert!(!vertices_have_bottom_center_rule(
        &fresh_vertices,
        outline_color
    ));
}

fn vertices_have_bottom_center_rule(vertices: &[Vertex], color: [f32; 4]) -> bool {
    vertices.iter().any(|vertex| {
        vertex.color == color && vertex.position[1] <= -0.99 && vertex.position[0].abs() < 0.85
    })
}

#[test]
fn fresh_single_session_restores_dominant_welcome_hero_without_input_hline() {
    let mut app = SingleSessionApp::new(None);
    let size = PhysicalSize::new(900, 700);
    let tick_zero = build_single_session_vertices(&app, size, 0.0, 0);

    assert!(vertices_have_color(&tick_zero, WELCOME_AURORA_BLUE));
    assert_runtime_welcome_hero_available(&app, size);
    assert!(!vertices_have_color(
        &tick_zero,
        [0.060, 0.085, 0.145, 0.34]
    ));

    app.handle_key(KeyInput::Character("hello".to_string()));
    let typed = build_single_session_vertices(&app, size, 0.0, 18);
    assert!(vertices_have_color(&typed, WELCOME_AURORA_BLUE));
    assert_runtime_welcome_hero_available(&app, size);
    assert!(!vertices_have_color(&typed, [0.060, 0.085, 0.145, 0.34]));
}

#[test]
fn fresh_single_session_offers_crashed_recovery_without_auto_opening() {
    let mut app = SingleSessionApp::new(None);
    app.set_recovery_session_count(3);

    let lines = app.body_styled_lines();
    let body = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(body.contains("Found 3 crashed session(s)"));
    assert!(body.contains("Press Ctrl+R"));
    assert_eq!(
        app.handle_key(KeyInput::RefreshSessions),
        KeyOutcome::RestoreCrashedSessions
    );
}

#[test]
fn fresh_single_session_without_crashes_keeps_refresh_as_redraw() {
    let mut app = SingleSessionApp::new(None);

    assert_eq!(
        app.handle_key(KeyInput::RefreshSessions),
        KeyOutcome::Redraw
    );
    assert!(
        !app.body_styled_lines()
            .iter()
            .any(|line| line.text.contains("crashed session"))
    );
}

#[test]
fn single_session_active_work_uses_streaming_activity_cue_geometry() {
    let mut app = SingleSessionApp::new(None);
    let idle = build_single_session_vertices(&app, PhysicalSize::new(900, 700), 0.0, 0);
    assert!(!vertices_have_rgb(&idle, NATIVE_SPINNER_HEAD_COLOR));

    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "streaming".to_string(),
    ));
    let tick_zero = build_single_session_vertices(&app, PhysicalSize::new(900, 700), 0.0, 0);
    let tick_one = build_single_session_vertices(&app, PhysicalSize::new(900, 700), 0.0, 1);

    assert!(vertices_have_rgb(&tick_zero, NATIVE_SPINNER_HEAD_COLOR));
    assert!(vertices_have_rgb(&tick_one, NATIVE_SPINNER_HEAD_COLOR));
    assert_ne!(
        colors_for_rgb(&tick_zero, NATIVE_SPINNER_HEAD_COLOR),
        colors_for_rgb(&tick_one, NATIVE_SPINNER_HEAD_COLOR)
    );
}

#[test]
fn single_session_streaming_response_does_not_draw_line_reveal_shimmer() {
    let mut app = SingleSessionApp::new(None);
    let size = PhysicalSize::new(900, 700);
    const REMOVED_SHIMMER_SOFT_COLOR: [f32; 4] = [0.220, 0.520, 0.780, 0.055];
    const REMOVED_SHIMMER_CORE_COLOR: [f32; 4] = [0.220, 0.520, 0.780, 0.115];

    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "streaming answer".to_string(),
    ));
    let vertices = build_single_session_vertices(&app, size, 0.0, 0);

    assert!(!vertices_have_color(&vertices, REMOVED_SHIMMER_SOFT_COLOR));
    assert!(!vertices_have_color(&vertices, REMOVED_SHIMMER_CORE_COLOR));
}

#[test]
fn single_session_streaming_text_fades_in() {
    let start_style = streaming_text_arrival_style_for_elapsed(Duration::from_millis(0));
    let mid_style = streaming_text_arrival_style_for_elapsed(STREAMING_TEXT_FADE_DURATION / 2);
    let end_style = streaming_text_arrival_style_for_elapsed(STREAMING_TEXT_FADE_DURATION);
    let (start_opacity, start_active) =
        streaming_text_fade_opacity_for_elapsed(Duration::from_millis(0));
    let (mid_opacity, mid_active) =
        streaming_text_fade_opacity_for_elapsed(STREAMING_TEXT_FADE_DURATION / 2);
    let (end_opacity, end_active) =
        streaming_text_fade_opacity_for_elapsed(STREAMING_TEXT_FADE_DURATION);

    assert!(start_active);
    assert!(mid_active);
    assert!(!end_active);
    assert!((start_opacity - STREAMING_TEXT_FADE_START_OPACITY).abs() < f32::EPSILON);
    assert!(mid_opacity > start_opacity);
    assert!(mid_opacity < 1.0);
    assert!((end_opacity - 1.0).abs() < f32::EPSILON);
    assert_eq!(start_style.opacity, start_opacity);
    assert_eq!(
        start_style.y_offset_pixels,
        STREAMING_TEXT_RISE_START_OFFSET_PIXELS
    );
    assert!(mid_style.y_offset_pixels < start_style.y_offset_pixels);
    assert!(mid_style.y_offset_pixels > 0.0);
    assert_eq!(end_style.y_offset_pixels, 0.0);
    assert!(!end_style.active);
}

#[test]
fn single_session_streaming_text_opacity_scales_rich_text_segments() {
    let lines = vec![SingleSessionStyledLine::new(
        "streaming answer",
        SingleSessionLineStyle::Assistant,
    )];
    let segments = single_session_styled_text_segments_with_opacity(&lines, 0.5);
    let (_, attrs) = segments
        .iter()
        .find(|(text, _)| *text == "streaming answer")
        .expect("streaming assistant segment should be present");
    let (_, _, _, alpha) = attrs
        .color_opt
        .expect("assistant segment should have an explicit color")
        .as_rgba_tuple();

    assert_eq!(alpha, 128);
}

#[test]
fn single_session_ctrl_backspace_deletes_previous_word() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello desktop world".to_string()));

    assert_eq!(
        app.handle_key(KeyInput::DeletePreviousWord),
        KeyOutcome::Redraw
    );
    assert_eq!(app.draft, "hello desktop ");
}

#[test]
fn single_session_supports_tui_like_word_movement_delete_and_undo() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello desktop world".to_string()));

    assert_eq!(
        app.handle_key(KeyInput::MoveCursorWordLeft),
        KeyOutcome::Redraw
    );
    assert_eq!(app.draft_cursor, "hello desktop ".len());

    assert_eq!(
        app.handle_key(KeyInput::MoveCursorWordRight),
        KeyOutcome::Redraw
    );
    assert_eq!(app.draft_cursor, app.draft.len());

    app.handle_key(KeyInput::MoveCursorWordLeft);
    assert_eq!(app.handle_key(KeyInput::DeleteNextWord), KeyOutcome::Redraw);
    assert_eq!(app.draft, "hello desktop ");

    assert_eq!(app.handle_key(KeyInput::UndoInput), KeyOutcome::Redraw);
    assert_eq!(app.draft, "hello desktop world");
}

#[test]
fn single_session_cursor_editing_inserts_and_deletes_in_middle() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("helo".to_string()));
    app.handle_key(KeyInput::MoveCursorLeft);
    app.handle_key(KeyInput::Character("l".to_string()));

    assert_eq!(app.draft, "hello");
    assert_eq!(app.draft_cursor, 4);

    app.handle_key(KeyInput::DeleteNextChar);
    assert_eq!(app.draft, "hell");
}

#[test]
fn single_session_escape_clears_idle_draft_and_undo_restores() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("keep this".to_string()));

    assert_eq!(app.handle_key(KeyInput::Escape), KeyOutcome::Redraw);
    assert!(app.draft.is_empty());
    assert_eq!(app.draft_cursor, 0);

    assert_eq!(app.handle_key(KeyInput::UndoInput), KeyOutcome::Redraw);
    assert_eq!(app.draft, "keep this");
    assert_eq!(app.draft_cursor, "keep this".len());
}

#[test]
fn single_session_tab_autocompletes_desktop_slash_command() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/cop".to_string()));

    assert_eq!(app.handle_key(KeyInput::Autocomplete), KeyOutcome::Redraw);
    assert_eq!(app.draft, "/copy");
    assert_eq!(app.draft_cursor, "/copy".len());

    assert_eq!(app.handle_key(KeyInput::UndoInput), KeyOutcome::Redraw);
    assert_eq!(app.draft, "/cop");
}

#[test]
fn single_session_slash_suggestions_filter_select_and_submit() {
    let mut app = SingleSessionApp::new(None);

    assert_eq!(
        app.handle_key(KeyInput::Character("/c".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::SlashSuggestions)
    );
    assert_eq!(
        app.active_inline_widget_mode(),
        Some(InlineWidgetMode::ReadOnly)
    );
    assert!(app.should_draw_composer_caret());
    assert!(app.active_inline_widget_uses_card_chrome());

    let suggestions = app.inline_widget_styled_lines();
    let suggestion_text = suggestions
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(suggestion_text.contains("slash command suggestions"));
    assert!(suggestion_text.contains("/clear"));
    assert!(suggestion_text.contains("/copy [latest|code|transcript]"));
    assert!(
        !suggestions
            .iter()
            .any(|line| line.text.trim_start().starts_with("/help "))
    );
    assert!(suggestions.iter().any(|line| {
        line.style == SingleSessionLineStyle::OverlaySelection && line.text.contains("/commands")
    }));

    assert_eq!(
        app.handle_key(KeyInput::ModelPickerMove(3)),
        KeyOutcome::Redraw
    );
    let suggestions = app.inline_widget_styled_lines();
    assert!(suggestions.iter().any(|line| {
        line.style == SingleSessionLineStyle::OverlaySelection && line.text.contains("/copy")
    }));

    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert!(app.draft.is_empty());
    assert_eq!(app.status.as_deref(), Some("no assistant response to copy"));
    assert!(app.messages.is_empty());
}

#[test]
fn single_session_slash_suggestions_escape_dismisses_until_draft_changes() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/m".to_string()));

    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::SlashSuggestions)
    );
    assert_eq!(app.handle_key(KeyInput::Escape), KeyOutcome::Redraw);
    assert_eq!(app.draft, "/m");
    assert_eq!(app.active_inline_widget(), None);

    assert_eq!(
        app.handle_key(KeyInput::Character("o".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.draft, "/mo");
    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::SlashSuggestions)
    );
}

#[test]
fn single_session_slash_suggestions_use_inline_card_geometry() {
    let size = PhysicalSize::new(1000, 720);
    let mut base = SingleSessionApp::new(Some(test_session_card(
        "slash_suggestions_geometry",
        "Slash Geometry",
        "ready",
    )));

    assert_eq!(
        base.handle_key(KeyInput::Character("/c".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(
        base.active_inline_widget(),
        Some(InlineWidgetKind::SlashSuggestions)
    );
    assert!(base.active_inline_widget_uses_card_chrome());
    let suggestion_vertices = build_single_session_vertices(&base, size, 0.0, 0);
    assert!(!suggestion_vertices.is_empty());

    assert_eq!(base.handle_key(KeyInput::HotkeyHelp), KeyOutcome::Redraw);
    assert_eq!(
        base.active_inline_widget(),
        Some(InlineWidgetKind::HotkeyHelp)
    );
    assert!(base.active_inline_widget_uses_card_chrome());
    let help_vertices = build_single_session_vertices(&base, size, 0.0, 0);
    assert!(help_vertices.len() >= suggestion_vertices.len());
}

#[test]
fn single_session_composer_uses_next_prompt_number() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(app.next_prompt_number(), 1);
    assert_eq!(app.composer_prompt(), "1› ");
    assert_eq!(app.composer_text(), "1› ");

    app.scroll_body_lines(1.0);
    app.scroll_body_lines(2.0);
    app.scroll_body_to_bottom();

    app.handle_key(KeyInput::Character("hello".to_string()));
    assert_eq!(app.composer_text(), "1› hello");
    assert_eq!(app.composer_cursor_line_byte_index(), (0, "1› hello".len()));
    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession {
            message: "hello".to_string(),
            images: Vec::new()
        }
    );

    assert_eq!(app.next_prompt_number(), 2);
    assert_eq!(app.composer_text(), "2› ");
}

#[test]
fn single_session_slash_help_opens_help_without_sending_prompt() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/help".to_string()));

    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert!(app.show_help);
    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::HotkeyHelp)
    );
    assert_eq!(
        app.active_inline_widget_mode(),
        Some(InlineWidgetMode::ReadOnly)
    );
    assert!(app.draft.is_empty());
    assert!(app.messages.is_empty());
    let help = app
        .inline_widget_styled_lines()
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(help.contains("slash commands"));
    assert!(help.contains("/model [name]"));
    assert!(help.contains("/fast [on|off|status]"));
}

#[test]
fn single_session_commands_alias_opens_help_without_sending_prompt() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/commands".to_string()));

    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert!(app.show_help);
    assert!(app.draft.is_empty());
    assert!(app.messages.is_empty());
    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::HotkeyHelp)
    );
}

#[test]
fn single_session_info_hotkey_toggles_inline_session_stats() {
    let mut app = SingleSessionApp::new(Some(test_session_card(
        "session_info_1234567890",
        "Info Session",
        "ready",
    )));
    app.messages
        .push(SingleSessionMessage::user("what happened?"));
    app.messages
        .push(SingleSessionMessage::assistant("a useful answer"));
    app.messages.push(SingleSessionMessage::tool("read file"));
    app.streaming_response = "still streaming".to_string();
    app.set_status_label("receiving");
    app.model_picker.current_model = Some("claude-sonnet-4-5".to_string());
    app.model_picker.provider_name = Some("Claude".to_string());

    assert_eq!(
        app.handle_key(KeyInput::ToggleSessionInfo),
        KeyOutcome::Redraw
    );
    assert!(app.show_session_info);
    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::SessionInfo)
    );
    assert_eq!(
        app.active_inline_widget_mode(),
        Some(InlineWidgetMode::ReadOnly)
    );
    let info = app
        .inline_widget_styled_lines()
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(info.contains("session info"));
    assert!(info.contains("Info Session"));
    assert!(info.contains("session_info_1234567890"));
    assert!(info.contains("receiving"));
    assert!(info.contains("Claude · claude-sonnet-4-5"));
    assert!(info.contains("3 total · 1 user · 1 assistant · 1 tool"));
    assert!(info.contains("streaming 15 chars"));

    assert_eq!(app.handle_key(KeyInput::Escape), KeyOutcome::Redraw);
    assert!(!app.show_session_info);
    assert!(app.inline_widget_styled_lines().is_empty());
}

#[test]
fn single_session_info_hotkey_changes_render_cache_and_hides_welcome_body() {
    let size = PhysicalSize::new(1000, 720);
    let mut app = SingleSessionApp::new(None);
    let before_key = app.rendered_body_cache_key((size.width, size.height));
    let before_static_key = app.rendered_body_static_cache_key((size.width, size.height));

    assert_eq!(
        app.handle_key(KeyInput::ToggleSessionInfo),
        KeyOutcome::Redraw
    );

    assert_ne!(
        before_key,
        app.rendered_body_cache_key((size.width, size.height))
    );
    assert_ne!(
        before_static_key,
        app.rendered_body_static_cache_key((size.width, size.height))
    );
    assert!(!app.is_welcome_timeline_visible());
    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::SessionInfo)
    );
    assert!(
        app.inline_widget_styled_lines()
            .iter()
            .any(|line| line.text.contains("session info"))
    );
}

#[test]
fn single_session_status_slash_opens_inline_session_info() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/status".to_string()));

    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert!(app.show_session_info);
    assert!(app.draft.is_empty());
    assert!(
        app.messages.is_empty(),
        "/status should not append a transcript meta row"
    );
    let info = app
        .inline_widget_styled_lines()
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(info.contains("fresh / not started"));
    assert!(info.contains("tokens"));
}

#[test]
fn single_session_slash_model_with_argument_requests_model_switch() {
    let mut app = SingleSessionApp::new(None);
    app.draft = "/model gpt-5.5".to_string();
    app.draft_cursor = app.draft.len();

    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::SetModel("gpt-5.5".to_string())
    );
    assert!(app.draft.is_empty());
    assert!(app.messages.is_empty());
}

#[test]
fn single_session_slash_server_setting_commands_return_control_outcomes() {
    let submit = |command: &str| {
        let mut app = SingleSessionApp::new(Some(test_session_card(
            "server_setting_session",
            "Settings Session",
            "ready",
        )));
        app.initialize_resumed_session("server_setting_session");
        app.handle_key(KeyInput::Character(command.to_string()));
        let outcome = app.handle_key(KeyInput::SubmitDraft);
        assert!(app.draft.is_empty());
        outcome
    };

    assert_eq!(
        submit("/refresh-model-list"),
        KeyOutcome::RefreshModelCatalog
    );
    assert_eq!(
        submit("/effort high"),
        KeyOutcome::SetReasoningEffort("high".to_string())
    );

    assert_eq!(
        submit("/fast on"),
        KeyOutcome::SetServiceTier("priority".to_string())
    );

    assert_eq!(
        submit("/fast off"),
        KeyOutcome::SetServiceTier("off".to_string())
    );

    assert_eq!(
        submit("/transport websocket"),
        KeyOutcome::SetTransport("websocket".to_string())
    );

    assert_eq!(submit("/compact"), KeyOutcome::CompactSession);

    assert_eq!(
        submit("/compact mode semantic"),
        KeyOutcome::SetCompactionMode("semantic".to_string())
    );

    assert_eq!(
        submit("/rename Demo Title"),
        KeyOutcome::RenameSession(Some("Demo Title".to_string()))
    );

    assert_eq!(submit("/rename --clear"), KeyOutcome::RenameSession(None));
    assert_eq!(submit("/clear"), KeyOutcome::ClearServerSession);
}

#[test]
fn single_session_slash_setting_status_uses_runtime_metadata() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::ModelCatalog {
        current_model: Some("gpt-5.1".to_string()),
        provider_name: Some("OpenAI".to_string()),
        models: Vec::new(),
        reasoning_effort: Some("high".to_string()),
        service_tier: Some("priority".to_string()),
        compaction_mode: Some("semantic".to_string()),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::Status(
        session_launch::DesktopSessionStatus::Transport("websocket".to_string()),
    ));

    app.handle_key(KeyInput::Character("/effort status".to_string()));
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert_eq!(
        app.status.as_deref(),
        Some("effort: high · use /effort <none|low|medium|high|xhigh>")
    );

    app.handle_key(KeyInput::Character("/fast status".to_string()));
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert_eq!(
        app.status.as_deref(),
        Some("fast mode: priority · use /fast <on|off|status>")
    );

    app.handle_key(KeyInput::Character("/transport status".to_string()));
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert_eq!(
        app.status.as_deref(),
        Some("transport: websocket · use /transport <auto|https|websocket>")
    );

    app.handle_key(KeyInput::Character("/compact mode status".to_string()));
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert_eq!(
        app.status.as_deref(),
        Some("compaction: semantic · use /compact mode <reactive|proactive|semantic>")
    );
}

#[test]
fn single_session_rename_event_updates_title_and_meta_status() {
    let mut app = SingleSessionApp::new(Some(test_session_card(
        "rename_event_session",
        "Old Title",
        "ready",
    )));

    app.apply_session_event(session_launch::DesktopSessionEvent::SessionRenamed {
        title: Some("New Title".to_string()),
        display_title: "New Title".to_string(),
    });

    assert_eq!(
        app.session.as_ref().map(|session| session.title.as_str()),
        Some("New Title")
    );
    assert_eq!(app.status.as_deref(), Some("session renamed"));
    assert!(
        app.body_lines()
            .join("\n")
            .contains("renamed session to New Title")
    );
}

#[test]
fn single_session_typing_model_slash_opens_preview_picker_without_submitting() {
    let mut app = SingleSessionApp::new(None);

    assert_eq!(
        app.handle_key(KeyInput::Character("/model opus".to_string())),
        KeyOutcome::LoadModelCatalog
    );
    assert!(app.model_picker.open);
    assert!(app.model_picker.preview);
    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::ModelPicker)
    );
    assert_eq!(
        app.active_inline_widget_mode(),
        Some(InlineWidgetMode::ReadOnly)
    );
    assert_eq!(app.draft, "/model opus");
    assert_eq!(app.model_picker.filter, "opus");

    app.apply_session_event(session_launch::DesktopSessionEvent::ModelCatalog {
        current_model: Some("claude-sonnet-4-5".to_string()),
        provider_name: Some("Claude".to_string()),
        models: vec![session_launch::DesktopModelChoice {
            model: "claude-opus-4-5".to_string(),
            provider: Some("claude".to_string()),
            api_method: Some("oauth".to_string()),
            detail: Some("premium".to_string()),
            available: true,
        }],
        reasoning_effort: None,
        service_tier: None,
        compaction_mode: None,
    });

    let body = app.body_lines().join("\n");
    assert!(!body.contains("MODEL"));
    let picker = app
        .inline_widget_styled_lines()
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(picker.contains("Model picker"));
    assert!(picker.contains("filter \"opus\""));
    assert!(picker.contains("claude-opus-4-5"));
    assert!(picker.contains("claude · oauth · premium"));

    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::SetModel("claude-opus-4-5".to_string())
    );
    assert!(!app.model_picker.open);
    assert!(app.draft.is_empty());
    assert!(app.messages.is_empty());
}

#[test]
fn single_session_unknown_slash_command_stays_local() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/definitely-not-real".to_string()));

    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert!(app.messages.is_empty());
    assert_eq!(app.draft, "/definitely-not-real");
    assert!(
        app.status
            .as_deref()
            .is_some_and(|status| status.contains("unknown desktop slash command"))
    );
}

#[test]
fn single_session_slash_copy_uses_latest_assistant_response_without_submitting() {
    let mut app = SingleSessionApp::new(None);
    app.messages
        .push(SingleSessionMessage::assistant("completed answer"));
    app.handle_key(KeyInput::Character("/copy".to_string()));

    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::CopyLatestResponse("completed answer".to_string())
    );
    assert!(app.draft.is_empty());
    assert_eq!(app.messages.len(), 1);

    app.handle_key(KeyInput::Character("/copy".to_string()));
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "streaming answer".to_string(),
    ));
    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::CopyLatestResponse("streaming answer".to_string())
    );
}

#[test]
fn single_session_slash_copy_reports_missing_response_locally() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/copy".to_string()));

    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert!(app.messages.is_empty());
    assert!(app.draft.is_empty());
    assert_eq!(app.status.as_deref(), Some("no assistant response to copy"));
}

#[test]
fn single_session_rich_copy_search_and_media_wiring_are_available() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "# Result\n\nAlpha text.\n\n```rust\nfn main() { let value = 42; }\n```",
    ));
    app.messages.push(SingleSessionMessage::tool(
        "▾ shell running: cargo test\n  input: cargo test\n  \\x1b[32mok\\x1b[0m",
    ));

    app.handle_key(KeyInput::Character("/copy code".to_string()));
    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::CopyText {
            text: "fn main() { let value = 42; }\n".to_string(),
            success_notice: "copied latest code block",
        }
    );

    app.handle_key(KeyInput::Character("/search alpha".to_string()));
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert_eq!(app.status.as_deref(), Some("1 match(es) for \"alpha\""));

    app.pending_images
        .push(("image/png".to_string(), "base64-data".to_string()));
    app.handle_key(KeyInput::Character("attached".to_string()));
    assert!(matches!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession { .. }
    ));
    let document = app.rich_transcript_document();
    assert!(document.blocks.iter().any(|block| matches!(
        block.kind,
        desktop_rich_text::TranscriptBlockKind::ImageAttachment { .. }
    )));
    assert!(
        document
            .jumps
            .iter()
            .any(|jump| jump.kind == desktop_rich_text::TranscriptJumpKind::Media)
    );
}

#[test]
fn single_session_rich_transcript_virtualizes_and_copies_transcript() {
    let mut app = SingleSessionApp::new(None);
    for index in 0..120 {
        app.messages.push(SingleSessionMessage::assistant(format!(
            "line {index}\n\n```json\n{{\"index\": {index}}}\n```"
        )));
    }

    let document = app.rich_transcript_document();
    let window =
        desktop_rich_text::VirtualLineWindow::for_viewport(document.total_lines, 50, 10, 3);
    assert!(window.start < window.end);
    assert!(window.end - window.start <= 16);

    app.handle_key(KeyInput::Character("/copy transcript".to_string()));
    match app.handle_key(KeyInput::SubmitDraft) {
        KeyOutcome::CopyText {
            text,
            success_notice,
        } => {
            assert_eq!(success_notice, "copied transcript");
            assert!(text.contains("line 0"));
            assert!(text.contains("line 119"));
        }
        other => panic!("expected transcript copy, got {other:?}"),
    }
}

#[test]
fn single_session_transcript_roles_render_without_stringly_labels() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::user("question"));
    app.messages.push(SingleSessionMessage::assistant("answer"));
    app.messages
        .push(SingleSessionMessage::tool("using tool bash"));
    app.messages
        .push(SingleSessionMessage::system("system note"));
    app.messages.push(SingleSessionMessage::meta("meta note"));

    let body = app.body_lines().join("\n");
    assert!(body.contains("1  question"));
    assert!(body.contains("answer"));
    assert!(body.contains("  using tool bash"));
    assert!(body.contains("  system note"));
    assert!(body.contains("  meta note"));
    assert!(!body.contains("user:"));
    assert!(!body.contains("assistant:"));
}

#[test]
fn single_session_assistant_markdown_is_prepared_for_desktop_rendering() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "# Plan\n\n- first\n- second\n\nUse `cargo test`.\n\n```rust\nfn main() {}\n```",
    ));

    let body = app.body_lines().join("\n");
    assert!(body.contains("Plan"));
    assert!(body.contains("• first"));
    assert!(body.contains("• second"));
    assert!(body.contains("Use cargo test."));
    assert!(body.contains("  rust"));
    assert!(body.contains("  fn main() {}"));
    assert!(!body.contains("```"));
}

#[test]
fn single_session_markdown_renderer_handles_rich_commonmark_shapes() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "## Results\n\n> quote line\n> continues\n\n1. first\n2. second\n\n[docs](https://example.com) and **bold** plus _em_.\n\n| name | value |\n| --- | --- |\n| alpha | 42 |\n\n---",
    ));

    let lines = app.body_styled_lines();
    let body = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(body.contains("Results"));
    assert_eq!(
        style_for_text(&lines, "Results"),
        Some(SingleSessionLineStyle::AssistantHeading)
    );
    assert!(body.contains("│ quote line continues"));
    assert_eq!(
        style_for_text(&lines, "│ quote line continues"),
        Some(SingleSessionLineStyle::AssistantQuote)
    );
    assert!(body.contains("1. first"));
    assert!(body.contains("2. second"));
    assert!(body.contains("docs ↗ https://example.com and bold plus em."));
    assert_eq!(
        style_for_text(&lines, "docs ↗ https://example.com and bold plus em."),
        Some(SingleSessionLineStyle::AssistantLink)
    );
    assert!(body.contains("name  │ value"));
    assert!(body.contains("alpha │ 42"));
    assert!(body.contains("──────┼──────"));
    assert_eq!(
        style_for_text(&lines, "alpha │ 42"),
        Some(SingleSessionLineStyle::AssistantTable)
    );
    assert!(body.contains("────────────"));
}

#[test]
fn single_session_markdown_renderer_preserves_media_html_and_table_alignment() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "Text **strong** and *em* and ~~old~~ with <kbd>Esc</kbd>.\n\n![diagram](https://example.com/diagram.png)\n\n<div>raw</div>\n\n| name | count | center |\n| :--- | ---: | :---: |\n| alpha | 42 | ok |",
    ));

    let lines = app.body_styled_lines();
    let body = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(body.contains("Text strong and em and old with <kbd>Esc</kbd>."));
    assert!(body.contains("🖼 diagram ↗ https://example.com/diagram.png"));
    assert_eq!(
        style_for_text(&lines, "🖼 diagram ↗ https://example.com/diagram.png"),
        Some(SingleSessionLineStyle::AssistantLink)
    );
    assert!(body.contains("html │ <div>raw</div>"));
    assert_eq!(
        style_for_text(&lines, "html │ <div>raw</div>"),
        Some(SingleSessionLineStyle::Meta)
    );
    assert!(
        body.contains("╾────"),
        "left alignment should mark the separator: {body}"
    );
    assert!(
        body.contains("────╼"),
        "right alignment should mark the separator: {body}"
    );
    assert!(
        body.contains("alpha │    42 │   ok"),
        "aligned row should pad numeric/center cells: {body}"
    );
}

#[test]
fn single_session_markdown_renderer_scopes_links_and_structures_lists() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "This is\none paragraph with [docs](https://example.com).\n\nNext paragraph.\n\n- [x] shipped\n- [ ] polish\n  - nested",
    ));

    let lines = app.body_styled_lines();
    let body = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(body.contains("This is one paragraph with docs ↗ https://example.com."));
    assert_eq!(
        style_for_text(
            &lines,
            "This is one paragraph with docs ↗ https://example.com."
        ),
        Some(SingleSessionLineStyle::AssistantLink)
    );
    assert_eq!(
        style_for_text(&lines, "Next paragraph."),
        Some(SingleSessionLineStyle::Assistant)
    );
    assert!(body.contains("✓ shipped"));
    assert!(body.contains("☐ polish"));
    assert!(body.contains("  ◦ nested"));
}

#[test]
fn single_session_markdown_renderer_handles_extended_gfm_structures() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "Footnote ref[^1].\n\n[^1]: Footnote body.\n\nTerm\n: definition text\n\n> [!WARNING]\n> pay attention\n\nInline $x+y$.\n\nCLI --flag stays literal.\n\n$$\na=b\n$$",
    ));

    let lines = app.body_styled_lines();
    let body = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(body.contains("Footnote ref[^1]."));
    assert!(body.contains("[^1]: Footnote body."));
    assert_eq!(
        style_for_text(&lines, "[^1]: Footnote body."),
        Some(SingleSessionLineStyle::Meta)
    );
    assert!(body.contains("Term"));
    assert_eq!(
        style_for_text(&lines, "Term"),
        Some(SingleSessionLineStyle::AssistantHeading)
    );
    assert!(body.contains("  : definition text"));
    assert!(body.contains("WARNING │ pay attention"));
    assert_eq!(
        style_for_text(&lines, "WARNING │ pay attention"),
        Some(SingleSessionLineStyle::AssistantQuote)
    );
    assert!(body.contains("Inline x+y."));
    assert!(body.contains("CLI --flag stays literal."));
    assert!(!body.contains("CLI –flag"));
    assert!(body.contains("  $$"));
    assert!(body.contains("  a=b"));
    assert_eq!(
        style_for_text(&lines, "  a=b"),
        Some(SingleSessionLineStyle::Code)
    );
}

#[test]
fn single_session_streaming_markdown_renders_before_done() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "## Streaming\n\n- first\n- [x] shipped\n\n[docs]".to_string(),
    ));
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "(https://example.com)\n\n```rust\nfn main() {}".to_string(),
    ));

    let lines = app.body_styled_lines();
    let body = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(body.contains("Streaming"));
    assert_eq!(
        style_for_text(&lines, "Streaming"),
        Some(SingleSessionLineStyle::AssistantHeading)
    );
    assert!(body.contains("• first"));
    assert!(body.contains("✓ shipped"));
    assert!(body.contains("docs ↗ https://example.com"));
    assert_eq!(
        style_for_text(&lines, "docs ↗ https://example.com"),
        Some(SingleSessionLineStyle::AssistantLink)
    );
    assert!(body.contains("  rust"));
    assert!(body.contains("  fn main() {}"));
    assert!(!body.contains("```"));

    app.apply_session_event(session_launch::DesktopSessionEvent::Done);
    let finished_body = app.body_lines().join("\n");
    assert!(finished_body.contains("Streaming"));
    assert!(finished_body.contains("• first"));
    assert!(finished_body.contains("✓ shipped"));
    assert!(finished_body.contains("docs ↗ https://example.com"));
    assert!(finished_body.contains("  fn main() {}"));
    assert!(!finished_body.contains("```"));
}

#[test]
fn single_session_markdown_structure_uses_distinct_colors_and_cards() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "# Heading\n\n> quoted\n\n| a | b |\n| - | - |\n| c | d |",
    ));
    let mut font_system = FontSystem::new();

    let buffers = single_session_text_buffers(&app, PhysicalSize::new(1200, 760), &mut font_system);
    let body = &buffers[1];
    assert_eq!(
        first_glyph_color_for_text(body, "Heading"),
        Some(single_session_line_color(
            SingleSessionLineStyle::AssistantHeading
        ))
    );
    assert_eq!(
        first_glyph_color_for_text(body, "│ quoted"),
        Some(single_session_line_color(
            SingleSessionLineStyle::AssistantQuote
        ))
    );
    assert_eq!(
        first_glyph_color_for_text(body, "c │ d"),
        Some(single_session_line_color(
            SingleSessionLineStyle::AssistantTable
        ))
    );

    let vertices = build_single_session_vertices(&app, PhysicalSize::new(1200, 760), 0.0, 0);
    assert!(vertices_have_color(
        &vertices,
        MARKDOWN_HEADING_BACKGROUND_COLOR
    ));
    assert!(vertices_have_color(&vertices, QUOTE_CARD_BACKGROUND_COLOR));
    assert!(vertices_have_color(&vertices, TABLE_CARD_BACKGROUND_COLOR));
}

#[test]
fn single_session_markdown_vertices_draw_heading_rule_and_inline_math_affordances() {
    let size = PhysicalSize::new(1000, 720);
    let mut app = SingleSessionApp::new(Some(test_session_card(
        "markdown_geometry",
        "Markdown geometry",
        "active",
    )));
    app.messages.push(SingleSessionMessage::assistant(
        "# Heading\n\nUse `cargo` and $x+y$.\n\n---",
    ));

    let body_lines = single_session_rendered_body_lines_for_tick(&app, size, 0);
    let heading_line = body_lines
        .iter()
        .position(|line| line.text == "Heading")
        .expect("heading line should be present");
    let inline_line = body_lines
        .iter()
        .position(|line| line.text == "Use cargo and x+y.")
        .expect("inline markdown line should be present");
    let inline_styled_line = &body_lines[inline_line];
    let rule_line = body_lines
        .iter()
        .position(|line| line.text == "────────────")
        .expect("horizontal rule line should be present");

    let vertices = build_single_session_vertices(&app, size, 0.0, 0);

    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let char_width = single_session_body_char_width();
    let body_top = PANEL_BODY_TOP_PADDING;
    let inline_line_y = body_top + inline_line as f32 * line_height;
    let inline_card_height = (typography.body_size * 1.10)
        .min(line_height - 5.0)
        .max(typography.body_size * 0.85);
    let inline_horizontal_pad = (3.5 * app.text_scale()).clamp(3.0, 6.0);
    let rule_thickness = (1.7 * app.text_scale()).clamp(1.0, 3.0);

    assert_pixel_bounds_close(
        pixel_bounds_for_color(&vertices, MARKDOWN_HEADING_BACKGROUND_COLOR, size)
            .expect("heading card vertices should be present"),
        Rect {
            x: PANEL_TITLE_LEFT_PADDING - 6.0,
            y: body_top + heading_line as f32 * line_height + 3.0,
            width: (size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0 + 12.0).max(1.0),
            height: (line_height - 6.0).max(1.0),
        },
        "heading card",
    );

    let code_run = single_session_inline_code_runs_for_line(inline_styled_line)
        .into_iter()
        .next()
        .expect("code run should be detected");
    assert_pixel_bounds_close(
        pixel_bounds_for_color(&vertices, INLINE_CODE_BACKGROUND_COLOR, size)
            .expect("inline code pill vertices should be present"),
        Rect {
            x: PANEL_TITLE_LEFT_PADDING + code_run.start_column as f32 * char_width
                - inline_horizontal_pad,
            y: inline_line_y + (line_height - inline_card_height) * 0.5,
            width: code_run.column_count as f32 * char_width + inline_horizontal_pad * 2.0,
            height: inline_card_height,
        },
        "inline code pill",
    );

    let math_run = single_session_inline_math_runs_for_line(inline_styled_line)
        .into_iter()
        .next()
        .expect("math run should be detected");
    assert_pixel_bounds_close(
        pixel_bounds_for_color(&vertices, INLINE_MATH_BACKGROUND_COLOR, size)
            .expect("inline math pill vertices should be present"),
        Rect {
            x: PANEL_TITLE_LEFT_PADDING + math_run.start_column as f32 * char_width
                - inline_horizontal_pad,
            y: inline_line_y + (line_height - inline_card_height) * 0.5,
            width: math_run.column_count as f32 * char_width + inline_horizontal_pad * 2.0,
            height: inline_card_height,
        },
        "inline math pill",
    );

    assert_pixel_bounds_close(
        pixel_bounds_for_color(&vertices, MARKDOWN_RULE_COLOR, size)
            .expect("markdown rule vertices should be present"),
        Rect {
            x: PANEL_TITLE_LEFT_PADDING - 2.0,
            y: body_top + rule_line as f32 * line_height + line_height * 0.5 - rule_thickness * 0.5,
            width: size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0 + 5.0,
            height: rule_thickness,
        },
        "markdown rule",
    );
}

#[test]
fn single_session_header_only_uses_previous_message_title_for_static_preview() {
    let card = test_session_card("session_alpha", "previous user request", "active");
    let mut app = SingleSessionApp::new(Some(card));
    let size = PhysicalSize::new(1000, 720);

    assert!(app.should_show_session_title_header());
    assert_eq!(
        single_session_text_key(&app, size).title,
        "previous user request"
    );

    app.messages.push(SingleSessionMessage::user("live prompt"));
    app.messages
        .push(SingleSessionMessage::assistant("live answer"));

    assert!(!app.should_show_session_title_header());
    assert_eq!(single_session_text_key(&app, size).title, "");
}

#[test]
fn single_session_activity_indicator_appears_only_for_active_work() {
    let mut app = SingleSessionApp::new(None);
    assert!(!app.activity_indicator_active());

    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "streaming".to_string(),
    ));
    assert!(app.activity_indicator_active());

    app.apply_session_event(session_launch::DesktopSessionEvent::Done);
    assert!(!app.activity_indicator_active());

    assert_eq!(
        app.handle_key(KeyInput::OpenModelPicker),
        KeyOutcome::LoadModelCatalog
    );
    assert!(app.activity_indicator_active());
}

#[test]
fn single_session_status_kind_drives_activity_indicator() {
    let mut app = SingleSessionApp::new(None);

    app.apply_session_event(session_launch::DesktopSessionEvent::Status(
        DesktopSessionStatus::SwitchingModel,
    ));

    assert_eq!(app.status.as_deref(), Some("switching model"));
    assert_eq!(
        app.status_kind(),
        Some(&SingleSessionStatus::Backend(
            DesktopSessionStatus::SwitchingModel
        ))
    );
    assert!(app.activity_indicator_active());

    app.apply_session_event(session_launch::DesktopSessionEvent::Done);

    assert_eq!(app.status.as_deref(), Some("ready"));
    assert_eq!(app.status_kind(), Some(&SingleSessionStatus::Ready));
    assert!(!app.activity_indicator_active());
}

#[test]
fn desktop_session_external_status_preserves_legacy_inflight_classification() {
    let mut app = SingleSessionApp::new(None);

    app.apply_session_event(session_launch::DesktopSessionEvent::Status(
        DesktopSessionStatus::external("using tool bash"),
    ));

    assert_eq!(app.status.as_deref(), Some("using tool bash"));
    assert!(matches!(
        app.status_kind(),
        Some(SingleSessionStatus::Backend(DesktopSessionStatus::External {
            label,
            in_flight: true,
        })) if label == "using tool bash"
    ));
    assert!(app.activity_indicator_active());

    app.apply_session_event(session_launch::DesktopSessionEvent::Status(
        DesktopSessionStatus::external("restored 1 crashed session(s)"),
    ));

    assert_eq!(app.status.as_deref(), Some("restored 1 crashed session(s)"));
    assert!(matches!(
        app.status_kind(),
        Some(SingleSessionStatus::Backend(DesktopSessionStatus::External {
            label,
            in_flight: false,
        })) if label == "restored 1 crashed session(s)"
    ));
    assert!(!app.activity_indicator_active());
}

#[test]
fn desktop_space_key_inserts_visible_prompt_space() {
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::Space), ModifiersState::empty()),
        KeyInput::Character(" ".to_string())
    );

    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::Character("hello".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(
        app.handle_key(KeyInput::Character(" ".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(
        app.handle_key(KeyInput::Character("world".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.composer_text(), "1› hello world");
    assert!(
        single_session_text_key(&app, PhysicalSize::new(420, 640))
            .draft
            .contains("hello world")
    );
}

#[test]
fn desktop_arrow_word_navigation_maps_common_modifiers() {
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::ArrowLeft), ModifiersState::CONTROL),
        KeyInput::MoveCursorWordLeft
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::ArrowRight), ModifiersState::CONTROL),
        KeyInput::MoveCursorWordRight
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::ArrowLeft), ModifiersState::ALT),
        KeyInput::CycleReasoningEffort(-1)
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::ArrowRight), ModifiersState::ALT),
        KeyInput::CycleReasoningEffort(1)
    );
}

#[test]
fn desktop_maps_session_info_hotkey() {
    assert_eq!(
        to_key_input(
            &Key::Character("s".into()),
            ModifiersState::CONTROL | ModifiersState::SHIFT
        ),
        KeyInput::ToggleSessionInfo
    );
}

#[test]
fn desktop_maps_control_question_mark_to_hotkey_help() {
    assert_eq!(
        to_key_input(
            &Key::Character("/".into()),
            ModifiersState::CONTROL | ModifiersState::SHIFT
        ),
        KeyInput::HotkeyHelp
    );
    assert_eq!(
        to_key_input(&Key::Character("?".into()), ModifiersState::CONTROL),
        KeyInput::HotkeyHelp
    );
}

#[test]
fn desktop_maps_terminal_editing_shortcuts_from_tui() {
    assert_eq!(
        to_key_input(&Key::Character("b".into()), ModifiersState::CONTROL),
        KeyInput::MoveCursorWordLeft
    );
    assert_eq!(
        to_key_input(&Key::Character("f".into()), ModifiersState::CONTROL),
        KeyInput::MoveCursorWordRight
    );
    assert_eq!(
        to_key_input(&Key::Character("w".into()), ModifiersState::CONTROL),
        KeyInput::DeletePreviousWord
    );
    assert_eq!(
        to_key_input(&Key::Character("x".into()), ModifiersState::CONTROL),
        KeyInput::CutInputLine
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::Backspace), ModifiersState::ALT),
        KeyInput::DeletePreviousWord
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::ArrowUp), ModifiersState::CONTROL),
        KeyInput::RetrieveQueuedDraft
    );
    assert_eq!(
        to_key_input(&Key::Character("v".into()), ModifiersState::ALT),
        KeyInput::AttachClipboardImage
    );
    assert_eq!(
        to_key_input(&Key::Character("d".into()), ModifiersState::CONTROL),
        KeyInput::CancelGeneration
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::Enter), ModifiersState::ALT),
        KeyInput::Enter
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::Tab), ModifiersState::empty()),
        KeyInput::Autocomplete
    );
}

#[test]
fn desktop_maps_remaining_global_shortcuts() {
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::Tab), ModifiersState::CONTROL),
        KeyInput::CycleModel(1)
    );
    assert_eq!(
        to_key_input(
            &Key::Named(NamedKey::Tab),
            ModifiersState::CONTROL | ModifiersState::SHIFT
        ),
        KeyInput::CycleModel(-1)
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::Home), ModifiersState::CONTROL),
        KeyInput::ScrollBodyToTop
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::End), ModifiersState::CONTROL),
        KeyInput::ScrollBodyToBottom
    );
    assert_eq!(
        to_key_input(&Key::Character("k".into()), ModifiersState::SUPER),
        KeyInput::ScrollBodyLines(1)
    );
    assert_eq!(
        to_key_input(&Key::Character("j".into()), ModifiersState::SUPER),
        KeyInput::ScrollBodyLines(-1)
    );
    assert_eq!(
        to_key_input(&Key::Character("[".into()), ModifiersState::CONTROL),
        KeyInput::JumpPrompt(-1)
    );
    assert_eq!(
        to_key_input(&Key::Character("]".into()), ModifiersState::CONTROL),
        KeyInput::JumpPrompt(1)
    );
    assert_eq!(
        to_key_input(
            &Key::Character("k".into()),
            ModifiersState::CONTROL | ModifiersState::SHIFT
        ),
        KeyInput::CopyLatestCodeBlock
    );
    assert_eq!(
        to_key_input(
            &Key::Character("t".into()),
            ModifiersState::CONTROL | ModifiersState::SHIFT
        ),
        KeyInput::CopyTranscript
    );
    assert_eq!(
        to_key_input(&Key::Character("q".into()), ModifiersState::CONTROL),
        KeyInput::ExitApp
    );
    assert_eq!(
        to_key_input(&Key::Character("q".into()), ModifiersState::SUPER),
        KeyInput::ExitApp
    );
}

#[test]
fn desktop_maps_text_scale_shortcuts() {
    assert_eq!(
        to_key_input(&Key::Character("-".into()), ModifiersState::CONTROL),
        KeyInput::AdjustTextScale(-1)
    );
    assert_eq!(
        to_key_input(&Key::Character("=".into()), ModifiersState::CONTROL),
        KeyInput::AdjustTextScale(1)
    );
    assert_eq!(
        to_key_input(&Key::Character("+".into()), ModifiersState::CONTROL),
        KeyInput::AdjustTextScale(1)
    );
    assert_eq!(
        to_key_input(&Key::Character("0".into()), ModifiersState::CONTROL),
        KeyInput::ResetTextScale
    );
}

#[test]
fn single_session_draft_selection_extracts_text_and_highlights_prompt_offset() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello\nworld".to_string()));

    app.begin_draft_selection(SelectionPoint { line: 0, column: 2 });
    app.update_draft_selection(SelectionPoint { line: 1, column: 3 });

    assert_eq!(
        app.draft_selection_segments(),
        vec![
            SelectionLineSegment {
                line: 0,
                start_column: app.composer_prompt().chars().count() + 2,
                end_column: app.composer_prompt().chars().count() + 5,
            },
            SelectionLineSegment {
                line: 1,
                start_column: 0,
                end_column: 3,
            },
        ]
    );

    let vertices = build_single_session_vertices(&app, PhysicalSize::new(900, 700), 0.0, 0);
    assert!(vertices_have_color(&vertices, SELECTION_HIGHLIGHT_COLOR));
    assert_eq!(app.selected_draft_text(), Some("llo\nwor".to_string()));
}

#[test]
fn single_session_paste_and_typing_replace_draft_selection() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello world".to_string()));
    app.begin_draft_selection(SelectionPoint { line: 0, column: 6 });
    app.update_draft_selection(SelectionPoint {
        line: 0,
        column: 11,
    });

    app.paste_text("there");

    assert_eq!(app.composer_text(), "1› hello there");
    assert_eq!(app.draft_cursor_line_col(), (0, 11));

    app.begin_draft_selection(SelectionPoint { line: 0, column: 6 });
    app.update_draft_selection(SelectionPoint {
        line: 0,
        column: 11,
    });
    assert_eq!(
        app.handle_key(KeyInput::Character("friend".to_string())),
        KeyOutcome::Redraw
    );

    assert_eq!(app.composer_text(), "1› hello friend");
    assert_eq!(app.draft_cursor_line_col(), (0, 12));
}

#[test]
fn single_session_delete_removes_draft_selection() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello world".to_string()));
    app.begin_draft_selection(SelectionPoint { line: 0, column: 5 });
    app.update_draft_selection(SelectionPoint {
        line: 0,
        column: 11,
    });

    assert_eq!(app.handle_key(KeyInput::Backspace), KeyOutcome::Redraw);

    assert_eq!(app.composer_text(), "1› hello");
    assert_eq!(app.draft_cursor_line_col(), (0, 5));
}

#[test]
fn single_session_cut_and_retrieve_queued_draft_match_tui_shortcuts() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("cut me".to_string()));
    assert_eq!(
        app.handle_key(KeyInput::CutInputLine),
        KeyOutcome::CutDraftToClipboard("cut me".to_string())
    );
    assert_eq!(app.composer_text(), "1› ");

    app.is_processing = true;
    app.handle_key(KeyInput::Character("queued".to_string()));
    assert_eq!(app.handle_key(KeyInput::QueueDraft), KeyOutcome::Redraw);
    assert_eq!(app.composer_text(), "1› ");
    assert_eq!(
        app.handle_key(KeyInput::RetrieveQueuedDraft),
        KeyOutcome::Redraw
    );
    assert_eq!(app.composer_text(), "1› queued");
}

#[test]
fn single_session_header_exposes_desktop_app_directory() {
    let mut app = SingleSessionApp::new(Some(test_session_card(
        "session_header",
        "session header",
        "active",
    )));
    app.apply_session_event(session_launch::DesktopSessionEvent::SessionStarted {
        session_id: "session_header".to_string(),
    });
    let key = single_session_text_key(&app, PhysicalSize::new(900, 700));
    let app_directory = std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.parent()
                .map(|directory| directory.display().to_string())
        })
        .unwrap_or_else(|| "unknown app directory".to_string());

    assert!(
        key.version.contains(&app_directory),
        "version label should include the desktop app directory, got {:?}, expected {:?}",
        key.version,
        app_directory
    );
    assert!(
        !key.version.contains(env!("CARGO_PKG_VERSION")),
        "version label should not include the package version, got {:?}",
        key.version
    );
}

#[test]
fn fresh_single_session_startup_puts_greeting_in_welcome_hero() {
    let app = SingleSessionApp::new(None);
    let key = single_session_text_key(&app, PhysicalSize::new(900, 700));

    assert_eq!(key.title, "");
    assert_is_handwritten_welcome_phrase(&key.welcome_hero);
    assert_visual_text_contains(&key, &key.welcome_hero);
    assert!(key.body.is_empty());
    assert_eq!(key.welcome_hint.len(), 1);
    assert!(key.welcome_hint[0].text.contains("Type a message to start"));
}

#[test]
fn single_session_text_buffers_include_header_version_area() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::SessionStarted {
        session_id: "session_header_buffers".to_string(),
    });
    let size = PhysicalSize::new(900, 700);
    let mut font_system = FontSystem::new();
    let buffers = single_session_text_buffers(&app, size, &mut font_system);

    assert_eq!(buffers.len(), 7);
    assert_eq!(single_session_text_areas(&buffers, size).len(), 4);
}

#[test]
fn fresh_welcome_greeting_uses_handwritten_hero_chrome() {
    let app = SingleSessionApp::new(None);
    let key = single_session_text_key(&app, PhysicalSize::new(1000, 720));
    let vertices = build_single_session_vertices(&app, PhysicalSize::new(1000, 720), 0.0, 0);

    assert_is_handwritten_welcome_phrase(&key.welcome_hero);
    assert_visual_text_contains(&key, &key.welcome_hero);
    assert_eq!(key.welcome_hint.len(), 1);
    assert!(vertices_have_color(&vertices, WELCOME_AURORA_BLUE));
}

#[test]
fn fresh_welcome_startup_hint_hides_after_typing() {
    let mut app = SingleSessionApp::new(None);
    let fresh_key = single_session_text_key(&app, PhysicalSize::new(900, 700));
    assert_eq!(fresh_key.welcome_hint.len(), 1);

    app.handle_key(KeyInput::Character("hello".to_string()));
    let typed_key = single_session_text_key(&app, PhysicalSize::new(900, 700));
    assert!(typed_key.welcome_hint.is_empty());
}

#[test]
fn fresh_welcome_handwriting_reveals_over_time() {
    let mask = build_hero_mask_image("Hello there", 640, 180, 96.0)
        .expect("runtime hero mask should be generated from bundled font");
    let early_ink = revealed_hero_mask_pixel_count(&mask, welcome_hero_reveal_progress_for_tick(0));
    let middle_ink =
        revealed_hero_mask_pixel_count(&mask, welcome_hero_reveal_progress_for_tick(4));
    let done_ink = revealed_hero_mask_pixel_count(&mask, 1.0);
    let full_ink = hero_mask_alpha_pixel_count(&mask);

    assert!(early_ink > 0, "first frame should show initial ink");
    assert!(
        early_ink < middle_ink,
        "handwritten ink should grow during reveal: early={early_ink}, middle={middle_ink}"
    );
    assert_eq!(
        done_ink, full_ink,
        "completed reveal should show every runtime font-mask pixel"
    );
}

#[test]
fn welcome_hero_reveal_progress_eases_to_full() {
    let start = welcome_hero_reveal_progress_for_elapsed(Duration::ZERO);
    let middle = welcome_hero_reveal_progress_for_elapsed(Duration::from_millis(675));
    let done = welcome_hero_reveal_progress_for_elapsed(Duration::from_millis(1350));

    assert!(start > 0.0 && start < middle);
    assert!(middle < done);
    assert_eq!(done, 1.0);
    assert!(welcome_hero_reveal_is_active(start));
    assert!(welcome_hero_reveal_is_active(middle));
    assert!(!welcome_hero_reveal_is_active(done));
}

#[test]
fn handwritten_welcome_phrase_set_has_stable_curated_variants() {
    assert_eq!(HANDWRITTEN_WELCOME_PHRASES.len(), 1);
    assert_eq!(handwritten_welcome_phrase(0), "Hello there");
    assert_eq!(
        handwritten_welcome_phrase(HANDWRITTEN_WELCOME_PHRASES.len()),
        handwritten_welcome_phrase(0)
    );
}

#[test]
fn single_session_visual_state_smoke_covers_markdown_spinner_and_switcher() {
    let size = PhysicalSize::new(1200, 760);
    let mut markdown_app = SingleSessionApp::new(Some(test_session_card(
        "session_visual",
        "stale title should hide",
        "active",
    )));
    markdown_app
        .messages
        .push(SingleSessionMessage::user("render this"));
    markdown_app.messages.push(SingleSessionMessage::assistant(
        "# Heading\n\n> quoted\n\n[docs](https://example.com)\n\n| k | v |\n| - | - |\n| color | yes |",
    ));
    markdown_app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "streaming tail".to_string(),
    ));

    let markdown_key = single_session_text_key(&markdown_app, size);
    assert_eq!(markdown_key.title, "");
    assert_visual_text_contains(&markdown_key, "│ quoted");
    assert_visual_text_contains(&markdown_key, "docs ↗ https://example.com");
    assert_visual_text_contains(&markdown_key, "color │ yes");
    assert_visual_text_contains(&markdown_key, "streaming tail");

    let markdown_vertices = build_single_session_vertices(&markdown_app, size, 0.0, 0);
    assert!(vertices_have_color(
        &markdown_vertices,
        QUOTE_CARD_BACKGROUND_COLOR
    ));
    assert!(vertices_have_color(
        &markdown_vertices,
        TABLE_CARD_BACKGROUND_COLOR
    ));

    let mut switcher_app = SingleSessionApp::new(None);
    assert_eq!(
        switcher_app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    let switcher_key = single_session_text_key(&switcher_app, size);
    assert_eq!(switcher_key.title, "");
    assert_visual_text_contains(&switcher_key, "desktop session switcher");
    assert_visual_text_contains(
        &switcher_key,
        "loading recent sessions from ~/.jcode/sessions...",
    );
}

#[test]
fn single_session_body_styled_lines_follow_roles_and_overlays() {
    let mut app = SingleSessionApp::new(None);
    app.messages
        .push(SingleSessionMessage::user("question\nmore context"));
    app.messages.push(SingleSessionMessage::assistant(
        "answer\n\n```rust\nfn main() {}\n```",
    ));
    app.messages.push(SingleSessionMessage::tool("bash done"));
    app.messages
        .push(SingleSessionMessage::meta("model switched"));

    let lines = app.body_styled_lines();
    let segments = single_session_styled_text_segments(&lines);
    assert!(
        segments.contains(&(
            "1",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(user_prompt_number_color(2))
        ))
    );
    assert!(
        segments.contains(&(
            "› ",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(text_color(USER_PROMPT_ACCENT_COLOR))
        ))
    );
    assert!(
        segments.contains(&(
            "question",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(single_session_line_color(SingleSessionLineStyle::User))
        ))
    );
    assert!(
        segments.contains(&(
            "answer",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_ASSISTANT_FONT_FAMILY))
                .color(single_session_line_color(SingleSessionLineStyle::Assistant))
        ))
    );

    assert_eq!(
        style_for_text(&lines, "1  question"),
        Some(SingleSessionLineStyle::User)
    );
    assert_eq!(
        style_for_text(&lines, "   more context"),
        Some(SingleSessionLineStyle::UserContinuation)
    );
    assert_eq!(
        style_for_text(&lines, "answer"),
        Some(SingleSessionLineStyle::Assistant)
    );
    assert_eq!(
        style_for_text(&lines, "  rust"),
        Some(SingleSessionLineStyle::CodeHeader)
    );
    assert_eq!(
        style_for_text(&lines, "  fn main() {}"),
        Some(SingleSessionLineStyle::Code)
    );
    assert_eq!(
        style_for_text(&lines, "  bash done"),
        Some(SingleSessionLineStyle::Tool)
    );
    assert_eq!(
        style_for_text(&lines, "  model switched"),
        Some(SingleSessionLineStyle::Meta)
    );

    app.handle_key(KeyInput::HotkeyHelp);
    let help = app.inline_widget_styled_lines();
    assert_eq!(
        style_for_text(&help, "desktop shortcuts"),
        Some(SingleSessionLineStyle::OverlayTitle)
    );
    assert_eq!(
        style_for_text(
            &help,
            "  Ctrl+V      paste clipboard image when no text is present"
        ),
        Some(SingleSessionLineStyle::Overlay)
    );
}

#[test]
fn assistant_symbol_lines_use_main_font_to_avoid_missing_glyph_boxes() {
    let symbol_line = "docs ↗ https://example.com";
    let symbol_lines = [SingleSessionStyledLine {
        text: symbol_line.to_string(),
        style: SingleSessionLineStyle::AssistantLink,
        inline_spans: Vec::new(),
    }];
    let symbol_segments = single_session_styled_text_segments(&symbol_lines);
    assert!(
        symbol_segments.contains(&(
            symbol_line,
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(single_session_line_color(
                    SingleSessionLineStyle::AssistantLink
                ))
        ))
    );

    let plain_line = "plain assistant prose";
    let plain_lines = [SingleSessionStyledLine {
        text: plain_line.to_string(),
        style: SingleSessionLineStyle::Assistant,
        inline_spans: Vec::new(),
    }];
    let plain_segments = single_session_styled_text_segments(&plain_lines);
    assert!(
        plain_segments.contains(&(
            plain_line,
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_ASSISTANT_FONT_FAMILY))
                .color(single_session_line_color(SingleSessionLineStyle::Assistant))
        ))
    );
}

#[test]
fn glyphon_body_buffer_uses_line_style_colors() {
    let mut app = SingleSessionApp::new(Some(test_session_card(
        "session_colors",
        "colors",
        "active",
    )));
    app.messages.push(SingleSessionMessage::user("question"));
    app.messages.push(SingleSessionMessage::assistant(
        "answer\n\n```rust\nfn main() {}\n```",
    ));
    app.messages.push(SingleSessionMessage::tool("bash done"));
    app.messages
        .push(SingleSessionMessage::meta("model switched"));
    let mut font_system = FontSystem::new();

    let buffers = single_session_text_buffers(&app, PhysicalSize::new(1200, 760), &mut font_system);
    let body = &buffers[1];

    assert_eq!(
        first_glyph_color_for_text(body, "answer"),
        Some(single_session_line_color(SingleSessionLineStyle::Assistant))
    );
    assert_eq!(
        first_glyph_color_for_text(body, "  rust"),
        Some(single_session_line_color(
            SingleSessionLineStyle::CodeHeader
        ))
    );
    assert_eq!(
        first_glyph_color_for_text(body, "  bash done"),
        Some(text_color(TOOL_MUTED_TEXT_COLOR))
    );
    assert_eq!(
        first_glyph_color_for_text(body, "  model switched"),
        Some(single_session_line_color(SingleSessionLineStyle::Meta))
    );
}

#[test]
fn assistant_inline_code_uses_code_text_attrs_inside_prose() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "Use `cargo test` before `cargo clippy`.",
    ));
    let line = app
        .body_styled_lines()
        .into_iter()
        .find(|line| line.text.starts_with("Use "))
        .expect("assistant inline code line should render");

    assert_eq!(line.text, "Use cargo test before cargo clippy.");
    assert_eq!(
        line.inline_spans
            .iter()
            .map(|span| (span.start, span.end, span.kind))
            .collect::<Vec<_>>(),
        vec![
            (4, 14, SingleSessionInlineSpanKind::Code),
            (22, 34, SingleSessionInlineSpanKind::Code),
        ]
    );

    let lines = [line];

    let segments = single_session_styled_text_segments(&lines);

    assert!(
        segments.contains(&(
            "Use ",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_ASSISTANT_FONT_FAMILY))
                .color(single_session_line_color(SingleSessionLineStyle::Assistant))
        ))
    );
    assert!(!segments.iter().any(|(text, _)| text.contains('`')));
    for code_segment in ["cargo test", "cargo clippy"] {
        assert!(
            segments.contains(&(
                code_segment,
                Attrs::new()
                    .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                    .color(single_session_line_color(SingleSessionLineStyle::Code))
            ))
        );
    }
}

#[test]
fn rich_transcript_line_segments_apply_syntax_ansi_and_search_attrs() {
    let line = desktop_rich_text::RichLine {
        block_id: desktop_rich_text::TranscriptBlockId("block".to_string()),
        text: "fn main ok".to_string(),
        style: desktop_rich_text::RichLineStyle::Code,
        spans: vec![
            desktop_rich_text::RichTextSpan {
                start: 0,
                end: 2,
                style: desktop_rich_text::RichSpanStyle::Syntax(
                    desktop_rich_text::SyntaxTokenKind::Keyword,
                ),
            },
            desktop_rich_text::RichTextSpan {
                start: 3,
                end: 7,
                style: desktop_rich_text::RichSpanStyle::SearchMatch,
            },
            desktop_rich_text::RichTextSpan {
                start: 8,
                end: 10,
                style: desktop_rich_text::RichSpanStyle::Ansi(desktop_rich_text::AnsiStyle {
                    foreground: Some(desktop_rich_text::AnsiColor::Green),
                    bold: true,
                    ..desktop_rich_text::AnsiStyle::default()
                }),
            },
        ],
        semantic_role: Some(desktop_rich_text::RichSemanticRole::CodeBlock),
    };

    let segments = rich_line_text_segments(&line);

    assert!(
        segments.contains(&(
            "fn",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(text_color([0.350, 0.145, 0.640, 1.0]))
        ))
    );
    assert!(
        segments.contains(&(
            "main",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(text_color(STATUS_TEXT_ACCENT_COLOR))
                .weight(glyphon::Weight::BOLD)
        ))
    );
    assert!(
        segments.contains(&(
            "ok",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(text_color([0.035, 0.360, 0.220, 1.0]))
                .weight(glyphon::Weight::BOLD)
        ))
    );
}

#[test]
fn assistant_inline_code_runs_and_vertices_draw_code_pills() {
    assert_eq!(
        single_session_inline_code_runs("Use `cargo test` before `cargo clippy`.")
            .into_iter()
            .map(|run| (run.start_column, run.column_count))
            .collect::<Vec<_>>(),
        vec![(4, 12), (24, 14)]
    );

    let parsed_line = SingleSessionStyledLine::with_inline_spans(
        "Use cargo test before cargo clippy.",
        SingleSessionLineStyle::Assistant,
        vec![
            SingleSessionInlineSpan {
                start: 4,
                end: 14,
                kind: SingleSessionInlineSpanKind::Code,
            },
            SingleSessionInlineSpan {
                start: 22,
                end: 34,
                kind: SingleSessionInlineSpanKind::Code,
            },
        ],
    );
    assert_eq!(
        single_session_inline_code_runs_for_line(&parsed_line)
            .into_iter()
            .map(|run| (run.start_column, run.column_count))
            .collect::<Vec<_>>(),
        vec![(4, 10), (22, 12)]
    );

    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "Use `cargo test` before shipping.\n\n```rust\nfn main() {}\n```",
    ));

    let vertices = build_single_session_vertices(&app, PhysicalSize::new(1000, 720), 0.0, 0);
    assert!(vertices_have_color(&vertices, INLINE_CODE_BACKGROUND_COLOR));
    assert!(vertices_have_color(&vertices, CODE_BLOCK_BACKGROUND_COLOR));
}

#[test]
fn assistant_whitespace_only_inline_code_preserves_space_span() {
    let mut app = SingleSessionApp::new(None);
    app.messages
        .push(SingleSessionMessage::assistant("before ` ` after\n\n` `"));

    let body_lines = app.body_styled_lines();
    let inline_line = body_lines
        .iter()
        .find(|line| line.text == "before   after")
        .expect("inline whitespace code should remain in surrounding prose");
    assert_eq!(
        inline_line.inline_spans,
        vec![SingleSessionInlineSpan {
            start: 7,
            end: 8,
            kind: SingleSessionInlineSpanKind::Code,
        }]
    );

    let standalone_line = body_lines
        .iter()
        .find(|line| line.text == " ")
        .expect("standalone whitespace code should render as a one-space line");
    assert_eq!(
        standalone_line.inline_spans,
        vec![SingleSessionInlineSpan {
            start: 0,
            end: 1,
            kind: SingleSessionInlineSpanKind::Code,
        }]
    );
}

#[test]
fn assistant_whitespace_only_inline_code_draws_exact_pill_at_space_column() {
    let size = PhysicalSize::new(1000, 720);
    let mut app = SingleSessionApp::new(None);
    app.messages
        .push(SingleSessionMessage::assistant("before ` ` after"));

    let body_lines = single_session_rendered_body_lines_for_tick(&app, size, 0);
    let inline_line_index = body_lines
        .iter()
        .position(|line| line.text == "before   after")
        .expect("inline whitespace code line should render");
    let inline_line = &body_lines[inline_line_index];
    assert_eq!(
        inline_line.inline_spans,
        vec![SingleSessionInlineSpan {
            start: 7,
            end: 8,
            kind: SingleSessionInlineSpanKind::Code,
        }]
    );
    assert_eq!(
        single_session_inline_code_runs_for_line(inline_line)
            .into_iter()
            .map(|run| (run.start_column, run.column_count))
            .collect::<Vec<_>>(),
        vec![(7, 1)]
    );

    let vertices = build_single_session_vertices(&app, size, 0.0, 0);
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let char_width = single_session_body_char_width();
    let card_height = (typography.body_size * 1.10)
        .min(line_height - 5.0)
        .max(typography.body_size * 0.85);
    let horizontal_pad = (3.5 * app.text_scale()).clamp(3.0, 6.0);

    assert_pixel_bounds_close(
        pixel_bounds_for_color(&vertices, INLINE_CODE_BACKGROUND_COLOR, size)
            .expect("whitespace inline code pill vertices should be present"),
        Rect {
            x: PANEL_TITLE_LEFT_PADDING + 7.0 * char_width - horizontal_pad,
            y: PANEL_BODY_TOP_PADDING
                + inline_line_index as f32 * line_height
                + (line_height - card_height) * 0.5,
            width: char_width + horizontal_pad * 2.0,
            height: card_height,
        },
        "whitespace inline code pill",
    );
}

#[test]
fn assistant_inline_code_pill_matches_glyphon_layout_after_narrow_wrap() {
    let size = PhysicalSize::new(718, 720);
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "Sure, you can use backticks to format inline code like a variable name:\n\n`userName`",
    ));

    let body_lines = single_session_rendered_body_lines_for_tick(&app, size, 0);
    assert!(
        body_lines
            .iter()
            .any(|line| line.text == "format inline code like a variable"),
        "narrow fixture should exercise a line that glyphon used to re-wrap"
    );
    let code_line_index = body_lines
        .iter()
        .position(|line| line.text == "userName")
        .expect("standalone inline code line should render");
    let viewport = single_session_body_viewport_from_lines(&app, size, 0.0, &body_lines);
    assert!(
        code_line_index >= viewport.start_line,
        "code line should be visible in the bottom-aligned narrow viewport"
    );
    let viewport_code_line_index = code_line_index - viewport.start_line;
    let viewport_code_line = viewport
        .lines
        .get(viewport_code_line_index)
        .expect("visible viewport should contain code line");
    assert_eq!(viewport_code_line.text, "userName");
    let code_span = viewport_code_line
        .inline_spans
        .iter()
        .find(|span| span.kind == SingleSessionInlineSpanKind::Code)
        .copied()
        .expect("userName should retain a code span");

    let mut font_system = FontSystem::new();
    let body_buffer = single_session_body_text_buffer_from_lines(
        &mut font_system,
        &viewport.lines,
        size,
        app.text_scale(),
    );
    let layout_runs = body_buffer.layout_runs().collect::<Vec<_>>();
    assert_eq!(
        layout_runs.len(),
        viewport.lines.len(),
        "body buffer must not glyphon-wrap rows that were already explicitly wrapped"
    );
    let glyphon_code_run = &layout_runs[viewport_code_line_index];
    assert_eq!(glyphon_code_run.line_i, viewport_code_line_index);
    assert_eq!(glyphon_code_run.text, "userName");
    let (glyphon_code_x, glyphon_code_width) = glyphon_code_run
        .highlight(
            glyphon::Cursor::new(viewport_code_line_index, code_span.start),
            glyphon::Cursor::new(viewport_code_line_index, code_span.end),
        )
        .expect("glyphon should expose the code span bounds on the same visual row");
    assert!(glyphon_code_x.abs() <= 0.75);
    assert!(glyphon_code_width > 0.0);

    let vertices = build_single_session_vertices(&app, size, 0.0, 0);
    let typography = single_session_typography_for_scale(app.text_scale());
    let line_height = typography.body_size * typography.body_line_height;
    let char_width = single_session_body_char_width();
    let card_height = (typography.body_size * 1.10)
        .min(line_height - 5.0)
        .max(typography.body_size * 0.85);
    let horizontal_pad = (3.5 * app.text_scale()).clamp(3.0, 6.0);
    let code_run = single_session_inline_code_runs_for_line(viewport_code_line)
        .into_iter()
        .next()
        .expect("code card run should be detected");

    assert_pixel_bounds_close(
        pixel_bounds_for_color(&vertices, INLINE_CODE_BACKGROUND_COLOR, size)
            .expect("inline code pill vertices should be present"),
        Rect {
            x: PANEL_TITLE_LEFT_PADDING + code_run.start_column as f32 * char_width
                - horizontal_pad,
            y: PANEL_BODY_TOP_PADDING
                + viewport.top_offset_pixels
                + glyphon_code_run.line_top
                + (line_height - card_height) * 0.5,
            width: code_run.column_count as f32 * char_width + horizontal_pad * 2.0,
            height: card_height,
        },
        "narrow inline code pill",
    );
}

#[test]
fn assistant_markdown_inline_segments_style_semantics_and_task_markers() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "Use **bold** and *em* and ~~old~~ with $x+y$.",
    ));
    let markdown_line = app
        .body_styled_lines()
        .into_iter()
        .find(|line| line.text.starts_with("Use "))
        .expect("assistant markdown line should render");

    assert_eq!(markdown_line.text, "Use bold and em and old with x+y.");
    assert_eq!(
        markdown_line
            .inline_spans
            .iter()
            .map(|span| (span.start, span.end, span.kind))
            .collect::<Vec<_>>(),
        vec![
            (4, 8, SingleSessionInlineSpanKind::Strong),
            (13, 15, SingleSessionInlineSpanKind::Emphasis),
            (20, 23, SingleSessionInlineSpanKind::Strike),
            (29, 32, SingleSessionInlineSpanKind::Math),
        ]
    );

    let lines = [
        markdown_line,
        SingleSessionStyledLine::new("✓ shipped", SingleSessionLineStyle::Assistant),
        SingleSessionStyledLine::new("☐ polish", SingleSessionLineStyle::Assistant),
    ];

    let segments = single_session_styled_text_segments(&lines);

    assert!(
        segments.contains(&(
            "bold",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_ASSISTANT_FONT_FAMILY))
                .color(single_session_line_color(SingleSessionLineStyle::Assistant))
                .weight(glyphon::Weight::BOLD)
        ))
    );
    assert!(
        segments.contains(&(
            "em",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_ASSISTANT_FONT_FAMILY))
                .color(single_session_line_color(SingleSessionLineStyle::Assistant))
                .style(glyphon::Style::Italic)
        ))
    );
    assert!(
        segments.contains(&(
            "old",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_ASSISTANT_FONT_FAMILY))
                .color(text_color(MARKDOWN_STRIKE_TEXT_COLOR))
        ))
    );
    assert!(
        segments.contains(&(
            "x+y",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(single_session_line_color(SingleSessionLineStyle::Code))
        ))
    );
    assert!(
        segments.contains(&(
            "✓ ",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(text_color(MARKDOWN_TASK_DONE_COLOR))
        ))
    );
    assert!(
        segments.contains(&(
            "☐ ",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(text_color(MARKDOWN_TASK_OPEN_COLOR))
        ))
    );
}

#[test]
fn assistant_inline_math_runs_skip_code_spans_and_display_math_markers() {
    assert_eq!(
        single_session_inline_math_runs("Inline $x+y$ and $z$.")
            .into_iter()
            .map(|run| (run.start_column, run.column_count))
            .collect::<Vec<_>>(),
        vec![(7, 5), (17, 3)]
    );
    assert_eq!(
        single_session_inline_math_runs("Display $$x+y$$ is not an inline pill"),
        Vec::new()
    );
    assert_eq!(
        single_session_inline_math_runs("Code `$x$` then $y$.")
            .into_iter()
            .map(|run| (run.start_column, run.column_count))
            .collect::<Vec<_>>(),
        vec![(16, 3)]
    );
}

#[test]
fn single_session_tool_text_segments_use_stateful_colors() {
    let lines = [
        SingleSessionStyledLine {
            text: "  ✓ bash · done · tests passed".to_string(),
            style: SingleSessionLineStyle::Tool,
            inline_spans: Vec::new(),
        },
        SingleSessionStyledLine {
            text: "  │intent: Run tests                                            │".to_string(),
            style: SingleSessionLineStyle::Tool,
            inline_spans: Vec::new(),
        },
        SingleSessionStyledLine {
            text: "  plain tool output".to_string(),
            style: SingleSessionLineStyle::Tool,
            inline_spans: Vec::new(),
        },
    ];

    let segments = single_session_styled_text_segments(&lines);

    assert!(
        segments.contains(&(
            "✓",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(text_color(TOOL_SUCCESS_TEXT_COLOR))
        ))
    );
    assert!(
        segments.contains(&(
            "bash",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(text_color(TOOL_TEXT_COLOR))
        ))
    );
    assert!(
        segments.contains(&(
            "done",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(text_color(TOOL_SUCCESS_TEXT_COLOR))
        ))
    );
    assert!(
        segments.contains(&(
            "tests passed",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(text_color(TOOL_DETAIL_TEXT_COLOR))
        ))
    );
    assert!(
        segments.contains(&(
            "intent: Run tests",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(text_color(TOOL_DETAIL_TEXT_COLOR))
        ))
    );
    assert!(
        segments.contains(&(
            "plain tool output",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(text_color(TOOL_DETAIL_TEXT_COLOR))
        ))
    );
}

#[test]
fn single_session_transcript_card_runs_group_card_styles() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "answer\n\n```rust\nfn main() {}\n```",
    ));
    app.messages.push(SingleSessionMessage::tool("bash done"));
    app.error = Some("boom".to_string());

    let lines = app.body_styled_lines();
    let runs = single_session_transcript_card_runs(&lines);

    let code = runs
        .iter()
        .find(|run| run.style == SingleSessionLineStyle::CodeHeader)
        .expect("code block should have a card run");
    assert_eq!(code.line_count, 2);
    assert_eq!(lines[code.line].text, "  rust");
    assert_eq!(lines[code.line].style, SingleSessionLineStyle::CodeHeader);
    assert_eq!(lines[code.line + 1].style, SingleSessionLineStyle::Code);

    // The language chip participates in the same code-card background run, but
    // is semantically separate from code content so it can be placed/styled as a
    // header instead of being treated as the first source line.
    assert_eq!(
        runs.iter()
            .filter(|run| {
                run.style == SingleSessionLineStyle::CodeHeader
                    || run.style == SingleSessionLineStyle::Code
            })
            .count(),
        1
    );

    // Tool rows are neutral inline transcript rows, not orange card runs.
    assert!(
        runs.iter()
            .all(|run| run.style != SingleSessionLineStyle::Tool)
    );
    assert!(lines.iter().any(|line| line.text == "  bash done"));

    let error = runs
        .iter()
        .find(|run| run.style == SingleSessionLineStyle::Error)
        .expect("error line should have a card run");
    assert_eq!(error.line_count, 1);
    assert_eq!(lines[error.line].text, "error: boom");
}

#[test]
fn code_block_header_placement_is_stable_across_sizes_and_text_scales() {
    let markdown = "before\n\n```text\njcode-desktop native window input uses winit and renders via wgpu\n  indented code stays code\n```\n\nafter";
    let sizes = [
        PhysicalSize::new(520, 420),
        PhysicalSize::new(900, 640),
        PhysicalSize::new(1440, 900),
    ];
    let scale_steps: [i8; 3] = [-2, 0, 3];

    for size in sizes {
        for scale_step in scale_steps {
            let mut app = SingleSessionApp::new(None);
            for _ in 0..scale_step.unsigned_abs() {
                app.handle_key(workspace::KeyInput::AdjustTextScale(scale_step.signum()));
            }
            app.messages.push(SingleSessionMessage::assistant(markdown));

            let lines = single_session_rendered_body_lines_for_tick(&app, size, 0);
            let header_index = lines
                .iter()
                .position(|line| line.text == "  text")
                .unwrap_or_else(|| {
                    panic!("missing code header at size {size:?}, scale {scale_step}")
                });

            assert_eq!(
                lines[header_index].style,
                SingleSessionLineStyle::CodeHeader
            );
            assert!(
                lines[header_index + 1..]
                    .iter()
                    .take_while(|line| line.style == SingleSessionLineStyle::Code)
                    .any(|line| line.text.starts_with("  jcode-desktop")),
                "first code content line should immediately follow header as code at size {size:?}, scale {scale_step}"
            );

            let runs = single_session_transcript_card_runs(&lines);
            let header_run = runs
                .iter()
                .find(|run| run.line <= header_index && header_index < run.line + run.line_count)
                .unwrap_or_else(|| {
                    panic!(
                        "header is not covered by a code card at size {size:?}, scale {scale_step}"
                    )
                });
            assert_eq!(header_run.style, SingleSessionLineStyle::CodeHeader);
            assert!(
                header_run.line_count >= 3,
                "language header and code content should be one contiguous card at size {size:?}, scale {scale_step}"
            );
            assert!(
                lines[header_run.line + 1..header_run.line + header_run.line_count]
                    .iter()
                    .all(|line| line.style == SingleSessionLineStyle::Code),
                "only the first line in the card run may be the language header at size {size:?}, scale {scale_step}"
            );

            let geometry = single_session_transcript_card_geometries(&app, size, &lines)
                .into_iter()
                .find(|geometry| geometry.run == *header_run)
                .unwrap_or_else(|| {
                    panic!("missing code card geometry at size {size:?}, scale {scale_step}")
                });
            let typography = single_session_typography_for_scale(app.text_scale());
            let char_width = single_session_body_char_width_for_scale(app.text_scale());
            let card_bottom = geometry.card_rect.y + geometry.card_rect.height;
            let text_glyph_left = geometry.text_left + 2.0 * char_width;
            assert!(
                geometry.card_rect.x <= text_glyph_left,
                "code text must start inside the card at size {size:?}, scale {scale_step}"
            );
            assert!(
                text_glyph_left - geometry.card_rect.x >= 6.0,
                "code text must keep visible left padding inside the card at size {size:?}, scale {scale_step}"
            );

            let mut previous_glyph_bottom = None;
            for line_index in header_run.line..header_run.line + header_run.line_count {
                let row_offset = line_index - header_run.line;
                let row_top = geometry.card_rect.y - 3.0 + row_offset as f32 * geometry.line_height;
                let glyph_top = row_top + (geometry.line_height - typography.body_size) * 0.5;
                let glyph_bottom = glyph_top + typography.body_size;
                assert!(
                    glyph_top >= geometry.card_rect.y,
                    "line glyph top escaped code card at line {line_index}, size {size:?}, scale {scale_step}"
                );
                assert!(
                    glyph_bottom <= card_bottom,
                    "line glyph bottom escaped code card at line {line_index}, size {size:?}, scale {scale_step}"
                );
                if let Some(previous_glyph_bottom) = previous_glyph_bottom {
                    assert!(
                        previous_glyph_bottom < glyph_top,
                        "code header/content glyph rows overlap at line {line_index}, size {size:?}, scale {scale_step}"
                    );
                }
                previous_glyph_bottom = Some(glyph_bottom);

                let line_text = &lines[line_index].text;
                let text_glyph_right =
                    geometry.text_left + line_text.chars().count() as f32 * char_width;
                assert!(
                    text_glyph_right <= geometry.card_rect.x + geometry.card_rect.width,
                    "code text must fit horizontally inside the card at line {line_index}, size {size:?}, scale {scale_step}"
                );
            }
        }
    }
}

#[test]
fn code_block_header_actual_glyph_rasters_stay_inside_rendered_card() {
    let markdown = "```text\njcode-desktop\n  indented code\n```";
    let sizes = [
        PhysicalSize::new(520, 420),
        PhysicalSize::new(900, 640),
        PhysicalSize::new(1440, 900),
    ];
    let scale_steps: [i8; 3] = [-2, 0, 3];

    for size in sizes {
        for scale_step in scale_steps {
            let mut app = SingleSessionApp::new(Some(test_session_card(
                "code-geometry",
                "Code geometry",
                "ready",
            )));
            for _ in 0..scale_step.unsigned_abs() {
                app.handle_key(workspace::KeyInput::AdjustTextScale(scale_step.signum()));
            }
            app.messages.push(SingleSessionMessage::assistant(markdown));

            let lines = single_session_rendered_body_lines_for_tick(&app, size, 0);
            let header_index = lines
                .iter()
                .position(|line| line.text == "  text")
                .unwrap_or_else(|| {
                    panic!("missing code header at size {size:?}, scale {scale_step}")
                });
            let header_run = single_session_transcript_card_runs(&lines)
                .into_iter()
                .find(|run| run.line <= header_index && header_index < run.line + run.line_count)
                .unwrap_or_else(|| {
                    panic!("missing code card run at size {size:?}, scale {scale_step}")
                });

            let vertices = build_single_session_vertices(&app, size, 0.0, 0);
            let card_bounds = pixel_bounds_for_color(&vertices, CODE_BLOCK_BACKGROUND_COLOR, size)
                .unwrap_or_else(|| {
                    panic!("missing rendered code card at size {size:?}, scale {scale_step}")
                });
            let viewport = single_session_body_viewport_from_lines(&app, size, 0.0, &lines);
            let viewport_header_run = single_session_transcript_card_runs(&viewport.lines)
                .into_iter()
                .find(|run| run.style == SingleSessionLineStyle::CodeHeader)
                .unwrap_or_else(|| {
                    panic!("missing visible code card run at size {size:?}, scale {scale_step}")
                });
            assert_eq!(
                viewport_header_run.line_count, header_run.line_count,
                "code card should be fully visible in the actual glyph fixture at size {size:?}, scale {scale_step}"
            );
            let typography = single_session_typography_for_scale(app.text_scale());
            let line_height = typography.body_size * typography.body_line_height;
            let text_left = card_bounds.min_x + 6.0;
            let text_top = card_bounds.min_y
                - 3.0
                - viewport_header_run.line as f32 * line_height
                - viewport.top_offset_pixels;

            let mut font_system = FontSystem::new();
            let body_buffer = single_session_body_text_buffer_from_lines(
                &mut font_system,
                &viewport.lines,
                size,
                app.text_scale(),
            );
            let layout_runs = body_buffer.layout_runs().collect::<Vec<_>>();

            let mut swash_cache = SwashCache::new();
            let mut previous_raster_bottom = None;
            for line_index in
                viewport_header_run.line..viewport_header_run.line + viewport_header_run.line_count
            {
                let line = &viewport.lines[line_index];
                let layout_run = layout_runs
                    .iter()
                    .find(|run| run.line_i == line_index)
                    .unwrap_or_else(|| {
                        panic!(
                            "missing glyphon layout run for line {line_index}, size {size:?}, scale {scale_step}"
                        )
                    });
                assert_eq!(layout_run.text, line.text);

                let (highlight_x, highlight_width) = layout_run
                    .highlight(
                        glyphon::Cursor::new(line_index, 0),
                        glyphon::Cursor::new(line_index, line.text.len()),
                    )
                    .unwrap_or_else(|| {
                        panic!(
                            "glyphon did not expose text bounds for line {line_index}, size {size:?}, scale {scale_step}"
                        )
                    });
                let highlight_left = text_left + highlight_x;
                let highlight_right = highlight_left + highlight_width;
                assert!(
                    highlight_left >= card_bounds.min_x,
                    "actual glyphon text starts left of rendered card at line {line_index}, size {size:?}, scale {scale_step}"
                );
                assert!(
                    highlight_right <= card_bounds.max_x,
                    "actual glyphon text exceeds rendered card width at line {line_index}, size {size:?}, scale {scale_step}"
                );

                let baseline_y = text_top + layout_run.line_y;
                assert!(
                    baseline_y > card_bounds.min_y && baseline_y < card_bounds.max_y,
                    "actual glyphon baseline escaped rendered card at line {line_index}, size {size:?}, scale {scale_step}"
                );

                let mut raster_bounds: Option<PixelBounds> = None;
                for glyph in layout_run.glyphs {
                    let physical_glyph = glyph.physical((text_left, text_top), 1.0);
                    let Some(image) =
                        swash_cache.get_image_uncached(&mut font_system, physical_glyph.cache_key)
                    else {
                        continue;
                    };
                    if image.placement.width == 0 || image.placement.height == 0 {
                        continue;
                    }
                    let x = physical_glyph.x as f32 + image.placement.left as f32;
                    let y = layout_run.line_y.round() + physical_glyph.y as f32
                        - image.placement.top as f32;
                    let glyph_bounds = PixelBounds {
                        min_x: x,
                        max_x: x + image.placement.width as f32,
                        min_y: y,
                        max_y: y + image.placement.height as f32,
                    };
                    raster_bounds = Some(match raster_bounds {
                        Some(bounds) => PixelBounds {
                            min_x: bounds.min_x.min(glyph_bounds.min_x),
                            max_x: bounds.max_x.max(glyph_bounds.max_x),
                            min_y: bounds.min_y.min(glyph_bounds.min_y),
                            max_y: bounds.max_y.max(glyph_bounds.max_y),
                        },
                        None => glyph_bounds,
                    });
                }
                let raster_bounds = raster_bounds.unwrap_or_else(|| {
                    panic!(
                        "missing actual glyph rasters for line {line_index}, size {size:?}, scale {scale_step}"
                    )
                });
                let tolerance = 1.0;
                assert!(
                    raster_bounds.min_x >= card_bounds.min_x - tolerance
                        && raster_bounds.max_x <= card_bounds.max_x + tolerance,
                    "actual glyph raster escaped rendered card horizontally at line {line_index}, size {size:?}, scale {scale_step}: {raster_bounds:?} vs {card_bounds:?}"
                );
                assert!(
                    raster_bounds.min_y >= card_bounds.min_y - tolerance
                        && raster_bounds.max_y <= card_bounds.max_y + tolerance,
                    "actual glyph raster escaped rendered card vertically at line {line_index}, size {size:?}, scale {scale_step}: {raster_bounds:?} vs {card_bounds:?}"
                );
                if let Some(previous_raster_bottom) = previous_raster_bottom {
                    assert!(
                        previous_raster_bottom < raster_bounds.min_y,
                        "actual glyph rasters overlap between code rows at line {line_index}, size {size:?}, scale {scale_step}"
                    );
                }
                previous_raster_bottom = Some(raster_bounds.max_y);
            }
        }
    }
}

#[test]
fn single_session_vertices_include_transcript_card_backgrounds() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "answer\n\n```rust\nfn main() {}\n```",
    ));
    app.messages.push(SingleSessionMessage::tool("bash done"));
    app.error = Some("boom".to_string());

    let vertices = build_single_session_vertices(&app, PhysicalSize::new(1000, 720), 0.0, 0);

    assert!(vertices_have_color(&vertices, CODE_BLOCK_BACKGROUND_COLOR));
    assert!(vertices_have_color(&vertices, ERROR_CARD_BACKGROUND_COLOR));
}

fn vertices_have_color(vertices: &[Vertex], color: [f32; 4]) -> bool {
    vertices.iter().any(|vertex| vertex.color == color)
}

fn vertices_have_rgb(vertices: &[Vertex], color: [f32; 4]) -> bool {
    vertices
        .iter()
        .any(|vertex| vertex.color[..3] == color[..3])
}

fn assert_runtime_welcome_hero_available(app: &SingleSessionApp, size: PhysicalSize<u32>) {
    let rendered_body_lines = single_session_rendered_body_lines_for_tick(app, size, 0);
    let spec =
        welcome_hero_runtime_mask_spec_for_total_lines(app, size, 0.0, rendered_body_lines.len())
            .expect("fresh welcome hero should be rendered by the runtime font mask");
    assert_eq!(spec.phrase, app.welcome_hero_text());
    assert!(spec.rect.width > size.width as f32 * 0.25);
    assert!(spec.rect.height > 40.0);
    assert!(spec.font_size > 40.0);
}

fn hero_mask_alpha_pixel_count(mask: &HeroMaskImage) -> usize {
    mask.glyph_rgba
        .chunks_exact(4)
        .filter(|pixel| pixel[0] > 2)
        .count()
}

fn revealed_hero_mask_pixel_count(mask: &HeroMaskImage, progress: f32) -> usize {
    let threshold = (progress.clamp(0.0, 1.0) * 255.0).round() as u8;
    mask.glyph_rgba
        .chunks_exact(4)
        .zip(mask.reveal_rgba.chunks_exact(4))
        .filter(|(glyph, reveal)| glyph[0] > 2 && reveal[0] <= threshold)
        .count()
}

fn positions_for_color(vertices: &[Vertex], color: [f32; 4]) -> Vec<[u32; 2]> {
    vertices
        .iter()
        .filter(|vertex| vertex.color == color)
        .map(|vertex| vertex.position.map(f32::to_bits))
        .collect()
}

fn colors_for_rgb(vertices: &[Vertex], color: [f32; 4]) -> Vec<[u32; 4]> {
    vertices
        .iter()
        .filter(|vertex| vertex.color[..3] == color[..3])
        .map(|vertex| vertex.color.map(f32::to_bits))
        .collect()
}

#[derive(Clone, Copy, Debug)]
struct PixelBounds {
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
}

fn pixel_bounds_for_color(
    vertices: &[Vertex],
    color: [f32; 4],
    size: PhysicalSize<u32>,
) -> Option<PixelBounds> {
    let mut bounds: Option<PixelBounds> = None;
    for vertex in vertices.iter().filter(|vertex| vertex.color == color) {
        let x = ndc_x_to_pixel(vertex.position[0], size);
        let y = ndc_y_to_pixel(vertex.position[1], size);
        bounds = Some(match bounds {
            Some(bounds) => PixelBounds {
                min_x: bounds.min_x.min(x),
                max_x: bounds.max_x.max(x),
                min_y: bounds.min_y.min(y),
                max_y: bounds.max_y.max(y),
            },
            None => PixelBounds {
                min_x: x,
                max_x: x,
                min_y: y,
                max_y: y,
            },
        });
    }
    bounds
}

fn assert_pixel_bounds_close(actual: PixelBounds, expected: Rect, label: &str) {
    let expected_bounds = PixelBounds {
        min_x: expected.x,
        max_x: expected.x + expected.width,
        min_y: expected.y,
        max_y: expected.y + expected.height,
    };
    for (axis, actual_value, expected_value) in [
        ("min_x", actual.min_x, expected_bounds.min_x),
        ("max_x", actual.max_x, expected_bounds.max_x),
        ("min_y", actual.min_y, expected_bounds.min_y),
        ("max_y", actual.max_y, expected_bounds.max_y),
    ] {
        assert!(
            (actual_value - expected_value).abs() <= 0.75,
            "{label} {axis} mismatch: actual={actual_value:.2}, expected={expected_value:.2}, bounds={actual:?}"
        );
    }
}

fn ndc_x_to_pixel(x: f32, size: PhysicalSize<u32>) -> f32 {
    (x + 1.0) * 0.5 * size.width.max(1) as f32
}

fn ndc_y_to_pixel(y: f32, size: PhysicalSize<u32>) -> f32 {
    (1.0 - y) * 0.5 * size.height.max(1) as f32
}

fn assert_visual_text_contains(key: &SingleSessionTextKey, expected: &str) {
    let body_lines = key
        .body
        .iter()
        .map(|line| line.text.as_str())
        .chain(key.inline_widget.iter().map(|line| line.text.as_str()))
        .chain(std::iter::once(key.welcome_hero.as_str()))
        .chain(key.welcome_hint.iter().map(|line| line.text.as_str()))
        .collect::<Vec<_>>();
    let body = body_lines.join("\n");
    assert!(
        body.contains(expected),
        "expected visual body to contain {expected:?}, got:\n{body}"
    );
}

fn assert_is_handwritten_welcome_phrase(phrase: &str) {
    assert!(
        HANDWRITTEN_WELCOME_PHRASES.contains(&phrase),
        "unexpected handwritten welcome phrase: {phrase:?}"
    );
}

fn test_session_card(id: &str, title: &str, status: &str) -> workspace::SessionCard {
    workspace::SessionCard {
        session_id: id.to_string(),
        title: title.to_string(),
        subtitle: format!("{status} · test-model"),
        detail: format!("2 msgs · {title}-workspace"),
        preview_lines: vec![format!("user {title} prompt")],
        detail_lines: vec![format!("assistant {title} response")],
    }
}

fn style_for_text(lines: &[SingleSessionStyledLine], text: &str) -> Option<SingleSessionLineStyle> {
    lines
        .iter()
        .find(|line| line.text == text)
        .map(|line| line.style)
}

fn first_glyph_color_for_text(buffer: &Buffer, text: &str) -> Option<TextColor> {
    buffer
        .layout_runs()
        .find(|run| run.text == text)
        .and_then(|run| run.glyphs.first().and_then(|glyph| glyph.color_opt))
}

#[test]
fn single_session_tool_events_expand_context_and_collapse_previous_call() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolStarted {
        name: "bash".to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolInput {
        delta: r#"{"command":"cargo test -p jcode-desktop","timeout":120000,"intent":"Run desktop tests"}"#.to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolExecuting {
        name: "bash".to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolFinished {
        name: "bash".to_string(),
        summary: "tests passed".to_string(),
        is_error: false,
    });

    let body = app.body_lines().join("\n");
    assert!(body.contains("  ✓ bash · done · tests passed"));
    assert!(body.contains("$ cargo test -p jcode-desktop"));
    assert!(!body.contains("    timeout: 120000"));
    assert_eq!(app.status.as_deref(), Some("tool bash done"));

    app.apply_session_event(session_launch::DesktopSessionEvent::ToolStarted {
        name: "read".to_string(),
    });
    let body = app.body_lines().join("\n");
    assert!(body.contains("  ✓ bash · done · tests passed"));
    assert!(!body.contains("Run desktop tests"));
    assert!(body.contains("  ○ read · preparing"));
}

#[test]
fn single_session_running_tool_input_is_visible_and_invalidates_render_cache() {
    let mut app = SingleSessionApp::new(None);

    app.apply_session_event(session_launch::DesktopSessionEvent::ToolStarted {
        name: "bash".to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolExecuting {
        name: "bash".to_string(),
    });
    let before_input_cache_key = app.rendered_body_cache_key((900, 700));
    let before_static_cache_key = app.rendered_body_static_cache_key((900, 700));

    app.apply_session_event(session_launch::DesktopSessionEvent::ToolInput {
        delta: r#"{"command":"sleep 10","intent":"wait while running"}"#.to_string(),
    });

    let body = app.body_lines().join("\n");
    assert!(body.contains("  ● bash · running · $ sleep 10"), "{body}");
    assert!(body.contains("waiting for tool output…"), "{body}");
    assert_ne!(
        app.rendered_body_cache_key((900, 700)),
        before_input_cache_key
    );
    assert_ne!(
        app.rendered_body_static_cache_key((900, 700)),
        before_static_cache_key
    );
}

#[test]
fn single_session_tool_event_preserves_prior_streaming_text_order() {
    let mut app = SingleSessionApp::new(None);

    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "Before the tool".to_string(),
    ));
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolStarted {
        name: "bash".to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolFinished {
        name: "bash".to_string(),
        summary: "done".to_string(),
        is_error: false,
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "After the tool".to_string(),
    ));

    let body = app.body_lines().join("\n");
    let before = body
        .find("Before the tool")
        .expect("streaming text before tool is rendered");
    let tool = body.find("bash").expect("tool message is rendered");
    let after = body
        .find("After the tool")
        .expect("streaming text after tool is rendered");

    assert!(
        before < tool,
        "assistant text that arrived before a tool should stay above the tool: {body}"
    );
    assert!(
        tool < after,
        "assistant text that arrives after a tool should stay below the tool: {body}"
    );
}

#[test]
fn single_session_adjacent_tool_messages_render_as_compact_summary() {
    let mut app = SingleSessionApp::new(None);
    app.messages
        .push(SingleSessionMessage::tool("▸ read done: 100 chars"));
    app.messages
        .push(SingleSessionMessage::tool("▸ agentgrep done: 10 matches"));
    app.messages
        .push(SingleSessionMessage::tool("▸ agentgrep done: 11 matches"));
    app.messages.push(SingleSessionMessage::tool("▸ edit done"));

    let body = app.body_lines();
    assert_eq!(body.len(), 1);
    assert_eq!(
        body[0],
        "  ▸ tools: 1 read, 2 agentgrep, 1 edit · ~23 tokens"
    );
}

#[test]
fn single_session_tool_summary_resets_at_non_tool_messages() {
    let mut app = SingleSessionApp::new(None);
    app.messages
        .push(SingleSessionMessage::tool("▸ read done: 100 chars"));
    app.messages.push(SingleSessionMessage::assistant("ok"));
    app.messages.push(SingleSessionMessage::tool("▸ edit done"));

    let body = app.body_lines().join("\n");
    assert!(body.contains("  ✓ read · done · 100 chars"));
    assert!(body.contains("ok"));
    assert!(body.contains("  ✓ edit · done"));
    assert!(!body.contains("tools:"));
}

#[test]
fn single_session_hotkey_help_toggles_discoverable_shortcuts() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::user("question"));

    assert_eq!(app.handle_key(KeyInput::HotkeyHelp), KeyOutcome::Redraw);
    assert!(app.show_help);
    let help = app
        .inline_widget_styled_lines()
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>();
    assert!(help.iter().any(|line| line == "desktop shortcuts"));
    assert!(help_has_shortcut(
        &help,
        "Ctrl+Enter",
        "queue while running, send when idle"
    ));
    assert!(help_has_shortcut(
        &help,
        "Ctrl+Shift+C",
        "copy latest assistant response"
    ));
    assert!(help_has_shortcut(
        &help,
        "Ctrl+P/O",
        "open recent session switcher"
    ));
    assert!(help_has_shortcut(
        &help,
        "Alt+Up/Down",
        "jump between user prompts"
    ));
    assert!(help_has_shortcut(&help, "Ctrl+Home/End", "jump transcript"));
    assert!(help_has_shortcut(&help, "Super+K/J", "scroll transcript"));
    assert!(help_has_shortcut(
        &help,
        "Ctrl+Shift+K",
        "copy latest code block"
    ));
    assert!(help_has_shortcut(&help, "Ctrl+Shift+T", "copy transcript"));
    assert!(help_has_shortcut(&help, "Ctrl+Tab", "switch to next model"));
    assert!(help_has_shortcut(
        &help,
        "Ctrl+Shift+Tab",
        "switch to previous model"
    ));
    assert!(help_has_shortcut(
        &help,
        "Ctrl+[/]",
        "jump between user prompts"
    ));
    assert!(help_has_shortcut(&help, "q", "close help or session info"));
    assert!(help_has_shortcut(
        &help,
        "Ctrl+Q/Super+Q",
        "quit desktop app"
    ));
    let help_text = help.join("\n");
    assert!(!help_text.contains("desktop queue follow-up pending"));
    assert!(!help_text.contains("1  question"));
    assert!(app.body_lines().join("\n").contains("1  question"));

    assert_eq!(app.handle_key(KeyInput::Escape), KeyOutcome::Redraw);
    assert!(!app.show_help);
    assert!(app.inline_widget_styled_lines().is_empty());
    assert_eq!(app.handle_key(KeyInput::Escape), KeyOutcome::None);
    assert!(app.body_lines().join("\n").contains("1  question"));
}

#[test]
fn single_session_q_closes_read_only_overlays() {
    let mut app = SingleSessionApp::new(None);

    assert_eq!(app.handle_key(KeyInput::HotkeyHelp), KeyOutcome::Redraw);
    assert!(app.show_help);
    assert_eq!(
        app.handle_key(KeyInput::Character("q".to_string())),
        KeyOutcome::Redraw
    );
    assert!(!app.show_help);

    assert_eq!(
        app.handle_key(KeyInput::ToggleSessionInfo),
        KeyOutcome::Redraw
    );
    assert!(app.show_session_info);
    assert_eq!(
        app.handle_key(KeyInput::Character("Q".to_string())),
        KeyOutcome::Redraw
    );
    assert!(!app.show_session_info);
}

#[test]
fn single_session_escape_soft_interrupts_running_generation() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(app.handle_key(KeyInput::Escape), KeyOutcome::None);

    app.is_processing = true;
    assert_eq!(
        app.handle_key(KeyInput::Escape),
        KeyOutcome::CancelGeneration
    );
}

fn help_has_shortcut(lines: &[String], shortcut: &str, description: &str) -> bool {
    lines
        .iter()
        .any(|line| line.contains(shortcut) && line.contains(description))
}

#[test]
fn single_session_model_cycle_updates_status_and_transcript() {
    let mut app = SingleSessionApp::new(None);

    assert_eq!(
        app.handle_key(KeyInput::CycleModel(1)),
        KeyOutcome::CycleModel(1)
    );
    app.apply_session_event(session_launch::DesktopSessionEvent::ModelChanged {
        model: "claude-opus-4-5".to_string(),
        provider_name: Some("Claude".to_string()),
        error: None,
    });

    assert_eq!(
        app.status.as_deref(),
        Some("model: Claude · claude-opus-4-5")
    );
    assert!(
        app.body_lines()
            .join("\n")
            .contains("model switched to Claude · claude-opus-4-5")
    );
}

#[test]
fn single_session_exit_shortcut_requests_exit() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(app.handle_key(KeyInput::ExitApp), KeyOutcome::Exit);
}

#[test]
fn single_session_model_picker_loads_filters_and_selects_model() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "existing transcript stays visible",
    ));

    assert_eq!(
        app.handle_key(KeyInput::OpenModelPicker),
        KeyOutcome::LoadModelCatalog
    );
    assert!(app.model_picker.open);
    assert!(app.model_picker.loading);
    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::ModelPicker)
    );
    assert_eq!(
        app.active_inline_widget_mode(),
        Some(InlineWidgetMode::Interactive)
    );
    assert!(
        app.inline_widget_styled_lines()
            .into_iter()
            .any(|line| line.text.contains("Loading models"))
    );

    app.apply_session_event(session_launch::DesktopSessionEvent::ModelCatalog {
        current_model: Some("claude-sonnet-4-5".to_string()),
        provider_name: Some("Claude".to_string()),
        models: vec![
            session_launch::DesktopModelChoice {
                model: "claude-sonnet-4-5".to_string(),
                provider: Some("claude".to_string()),
                api_method: Some("oauth".to_string()),
                detail: Some("active account".to_string()),
                available: true,
            },
            session_launch::DesktopModelChoice {
                model: "claude-opus-4-5".to_string(),
                provider: Some("claude".to_string()),
                api_method: Some("oauth".to_string()),
                detail: Some("premium".to_string()),
                available: true,
            },
        ],
        reasoning_effort: None,
        service_tier: None,
        compaction_mode: None,
    });

    let body = app.body_lines().join("\n");
    assert!(body.contains("existing transcript stays visible"));
    assert!(!body.contains("MODEL"));
    let picker = app
        .inline_widget_styled_lines()
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(picker.contains("Model picker    current Claude · claude-sonnet-4-5"));
    assert!(picker.contains("type to filter"));
    assert!(picker.contains("2 models"));
    assert!(picker.contains("claude-sonnet-4-5"));
    assert!(picker.contains("claude"));
    assert!(picker.contains("oauth"));

    assert_eq!(
        app.handle_key(KeyInput::Character("opus".to_string())),
        KeyOutcome::Redraw
    );
    let filtered = app
        .inline_widget_styled_lines()
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(filtered.contains("\"opus\""));
    assert!(filtered.contains("claude-opus-4-5"));

    for _ in 0.."opus".len() {
        assert_eq!(app.handle_key(KeyInput::Backspace), KeyOutcome::Redraw);
    }
    assert_eq!(app.model_picker.filter, "");
    assert_eq!(app.handle_key(KeyInput::MoveToLineEnd), KeyOutcome::Redraw);
    assert_eq!(app.model_picker.selected, 1);
    assert_eq!(
        app.handle_key(KeyInput::MoveToLineStart),
        KeyOutcome::Redraw
    );
    assert_eq!(app.model_picker.selected, 0);

    assert_eq!(
        app.handle_key(KeyInput::ModelPickerMove(1)),
        KeyOutcome::Redraw
    );
    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::SetModel("claude-opus-4-5".to_string())
    );
    assert!(!app.model_picker.open);
}

#[test]
fn single_session_session_switcher_loads_filters_and_resumes_session() {
    let mut app = SingleSessionApp::new(None);
    app.messages
        .push(SingleSessionMessage::user("stale live transcript"));
    app.handle_key(KeyInput::Character("pending draft".to_string()));

    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    assert!(app.session_switcher.open);
    assert!(app.session_switcher.loading);
    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::SessionSwitcher)
    );
    assert_eq!(
        app.active_inline_widget_mode(),
        Some(InlineWidgetMode::Interactive)
    );
    assert!(
        app.inline_widget_styled_lines()
            .into_iter()
            .map(|line| line.text)
            .collect::<Vec<_>>()
            .join("\n")
            .contains("loading recent sessions")
    );

    app.apply_session_switcher_cards(vec![
        test_session_card("session_alpha", "alpha", "alpha status"),
        test_session_card("session_beta", "beta", "beta status"),
    ]);
    let switcher = app
        .inline_widget_styled_lines()
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(switcher.contains("desktop session switcher"));
    assert!(switcher.contains("alpha"));
    assert!(switcher.contains("beta"));

    assert_eq!(app.handle_key(KeyInput::MoveToLineEnd), KeyOutcome::Redraw);
    assert_eq!(app.session_switcher.selected, 1);
    assert_eq!(
        app.handle_key(KeyInput::MoveToLineStart),
        KeyOutcome::Redraw
    );
    assert_eq!(app.session_switcher.selected, 0);

    assert_eq!(
        app.handle_key(KeyInput::Character("beta".to_string())),
        KeyOutcome::Redraw
    );
    assert!(
        app.inline_widget_styled_lines()
            .into_iter()
            .map(|line| line.text)
            .collect::<Vec<_>>()
            .join("\n")
            .contains("filter: beta")
    );

    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert!(!app.session_switcher.open);
    assert_eq!(
        app.session
            .as_ref()
            .map(|session| session.session_id.as_str()),
        Some("session_beta")
    );
    assert_eq!(app.live_session_id.as_deref(), Some("session_beta"));
    assert_eq!(app.draft, "pending draft");
    assert_eq!(app.status.as_deref(), Some("resumed beta"));

    let resumed = app.body_lines().join("\n");
    assert!(resumed.contains("beta status"));
    assert!(!resumed.contains("stale live transcript"));
}

#[test]
fn desktop_resume_args_are_parsed() {
    assert_eq!(
        desktop_resume_session_id_from_args(["jcode-desktop", "--resume", "session_beta"]),
        Some("session_beta".to_string())
    );
    assert_eq!(
        desktop_resume_session_id_from_args(["jcode-desktop", "--resume=session_gamma"]),
        Some("session_gamma".to_string())
    );
    assert_eq!(desktop_resume_session_id_from_args(["jcode-desktop"]), None);
}

#[test]
fn initial_single_session_app_marks_resume_visible_before_interaction() {
    let DesktopApp::SingleSession(app) = initial_single_session_app(Some("session_missing")) else {
        panic!("expected single session app");
    };

    assert_eq!(app.live_session_id.as_deref(), Some("session_missing"));
    assert!(
        !app.body_lines()
            .join("\n")
            .contains("What are we building today")
    );
    assert!(app.body_lines().join("\n").contains("resumed session"));
}

#[test]
fn single_session_session_switcher_marks_current_session_and_reloads() {
    let alpha = test_session_card("session_alpha", "alpha", "active");
    let beta = test_session_card("session_beta", "beta", "idle");
    let mut app = SingleSessionApp::new(Some(alpha.clone()));

    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    app.apply_session_switcher_cards(vec![beta, alpha]);

    assert_eq!(app.session_switcher.selected, 1);
    assert!(
        app.inline_widget_styled_lines()
            .into_iter()
            .map(|line| line.text)
            .collect::<Vec<_>>()
            .join("\n")
            .contains("› ✓ alpha")
    );

    assert_eq!(
        app.handle_key(KeyInput::RefreshSessions),
        KeyOutcome::LoadSessionSwitcher
    );
    assert!(app.session_switcher.loading);
    assert_eq!(app.status.as_deref(), Some("loading recent sessions"));
}

#[test]
fn single_session_model_picker_updates_current_model_after_switch() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::OpenModelPicker);

    app.apply_session_event(session_launch::DesktopSessionEvent::ModelChanged {
        model: "gpt-5.4".to_string(),
        provider_name: Some("OpenAI".to_string()),
        error: None,
    });

    assert_eq!(app.model_picker.current_model.as_deref(), Some("gpt-5.4"));
    assert_eq!(app.model_picker.provider_name.as_deref(), Some("OpenAI"));
    assert!(
        app.inline_widget_styled_lines()
            .into_iter()
            .map(|line| line.text)
            .collect::<Vec<_>>()
            .join("\n")
            .contains("Model picker    current OpenAI · gpt-5.4")
    );
}

#[test]
fn single_session_stdin_request_is_visible_in_transcript() {
    let mut app = SingleSessionApp::new(None);

    app.apply_session_event(session_launch::DesktopSessionEvent::StdinRequest {
        request_id: "stdin-1".to_string(),
        prompt: "Password:".to_string(),
        is_password: true,
        tool_call_id: "tool-1".to_string(),
    });

    assert_eq!(app.status.as_deref(), Some("interactive input requested"));
    let body = app.body_lines().join("\n");
    assert!(body.contains("interactive password input requested"));
    assert!(body.contains("prompt: Password:"));
    assert!(body.contains("request: stdin-1"));
    assert!(body.contains("tool: tool-1"));
}

#[test]
fn single_session_stdin_response_masks_password_and_sends_input() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::StdinRequest {
        request_id: "stdin-1".to_string(),
        prompt: "Password:".to_string(),
        is_password: true,
        tool_call_id: "tool-1".to_string(),
    });

    assert_eq!(
        app.handle_key(KeyInput::Character("s3 cr".to_string())),
        KeyOutcome::Redraw
    );
    app.paste_text("et");
    let body = app.body_lines().join("\n");
    assert!(body.contains("input: •••••••"));
    assert!(!body.contains("s3 cr"));

    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::SendStdinResponse {
            request_id: "stdin-1".to_string(),
            input: "s3 cret".to_string()
        }
    );
    assert!(app.stdin_response.is_none());
    assert_eq!(app.status.as_deref(), Some("sending interactive input"));
}

#[test]
fn single_session_attached_image_is_sent_with_next_prompt() {
    let mut app = SingleSessionApp::new(None);
    app.attach_image("image/png".to_string(), "abc123".to_string());

    app.handle_key(KeyInput::Character("describe this".to_string()));

    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession {
            message: "describe this".to_string(),
            images: vec![("image/png".to_string(), "abc123".to_string())]
        }
    );
    assert!(app.pending_images.is_empty());
}

#[test]
fn single_session_clear_attached_images_shortcut_clears_pending_images() {
    let mut app = SingleSessionApp::new(None);
    app.attach_image("image/png".to_string(), "abc123".to_string());

    assert_eq!(
        app.handle_key(KeyInput::ClearAttachedImages),
        KeyOutcome::Redraw
    );
    assert!(app.pending_images.is_empty());
    assert_eq!(app.status.as_deref(), Some("cleared image attachments"));
    assert_eq!(
        app.handle_key(KeyInput::ClearAttachedImages),
        KeyOutcome::None
    );
}

#[test]
fn clipboard_image_paste_is_disabled_while_answering_stdin() {
    let mut app = SingleSessionApp::new(None);
    assert!(app.accepts_clipboard_image_paste());

    app.apply_session_event(session_launch::DesktopSessionEvent::StdinRequest {
        request_id: "stdin-1".to_string(),
        prompt: "Password:".to_string(),
        is_password: true,
        tool_call_id: "tool-1".to_string(),
    });

    assert!(!app.accepts_clipboard_image_paste());
}

#[test]
fn single_session_ctrl_enter_queues_while_processing_then_dequeues() {
    let mut app = SingleSessionApp::new(None);
    app.is_processing = true;
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "working".to_string(),
    ));
    app.handle_key(KeyInput::Character("next prompt".to_string()));

    assert_eq!(app.handle_key(KeyInput::QueueDraft), KeyOutcome::Redraw);
    assert!(app.draft.is_empty());

    app.apply_session_event(session_launch::DesktopSessionEvent::Done);
    assert_eq!(
        app.take_next_queued_draft(),
        Some(("next prompt".to_string(), Vec::new()))
    );
    assert!(app.is_processing);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QueueTraceAction {
    TypeA,
    TypeB,
    CtrlEnter,
    Reloading,
    Reloaded,
    SessionStarted,
    TextDelta,
    Done,
    Error,
    TryDrainQueued,
}

#[derive(Default, Debug)]
struct QueueReferenceModel {
    draft: String,
    processing: bool,
    reloading: bool,
    error: bool,
    queued: Vec<String>,
    sent: Vec<String>,
}

impl QueueReferenceModel {
    fn apply(&mut self, action: QueueTraceAction) -> Option<String> {
        match action {
            QueueTraceAction::TypeA => self.draft.push('a'),
            QueueTraceAction::TypeB => self.draft.push('b'),
            QueueTraceAction::CtrlEnter => {
                let message = self.draft.trim().to_string();
                if message.is_empty() {
                    return None;
                }
                self.draft.clear();
                if self.processing {
                    self.queued.push(message);
                } else {
                    self.processing = true;
                    self.error = false;
                    self.sent.push(message.clone());
                    return Some(message);
                }
            }
            QueueTraceAction::Reloading => {
                // A hot reload is not a turn end. Wait-til-turn-end prompts must stay queued.
                self.processing = true;
                self.reloading = true;
            }
            QueueTraceAction::Reloaded => {
                self.processing = true;
                self.reloading = false;
            }
            QueueTraceAction::SessionStarted => {
                // Reconnection/session-start is also not a turn end.
            }
            QueueTraceAction::TextDelta => self.reloading = false,
            QueueTraceAction::Done => {
                if self.reloading {
                    // A terminal event racing with reload is stale unless the reconnected turn has
                    // produced activity again.
                    self.processing = true;
                } else {
                    self.processing = false;
                }
            }
            QueueTraceAction::Error => {
                self.processing = false;
                self.reloading = false;
                self.error = true;
            }
            QueueTraceAction::TryDrainQueued => {
                if !self.processing && !self.error && !self.queued.is_empty() {
                    let message = self.queued.remove(0);
                    self.processing = true;
                    self.error = false;
                    self.sent.push(message.clone());
                    return Some(message);
                }
            }
        }
        None
    }
}

fn apply_queue_trace_action_to_app(
    app: &mut SingleSessionApp,
    action: QueueTraceAction,
) -> Option<String> {
    match action {
        QueueTraceAction::TypeA => {
            app.handle_key(KeyInput::Character("a".to_string()));
            None
        }
        QueueTraceAction::TypeB => {
            app.handle_key(KeyInput::Character("b".to_string()));
            None
        }
        QueueTraceAction::CtrlEnter => match app.handle_key(KeyInput::QueueDraft) {
            KeyOutcome::StartFreshSession { message, .. }
            | KeyOutcome::SendDraft { message, .. } => Some(message),
            KeyOutcome::Redraw | KeyOutcome::None => None,
            other => panic!("unexpected Ctrl+Enter outcome in queue trace: {other:?}"),
        },
        QueueTraceAction::Reloading => {
            app.apply_session_event(session_launch::DesktopSessionEvent::Reloading {
                new_socket: Some("/tmp/jcode-reload-model-test.sock".to_string()),
            });
            None
        }
        QueueTraceAction::Reloaded => {
            app.apply_session_event(session_launch::DesktopSessionEvent::Reloaded {
                session_id: "reload-model-session".to_string(),
            });
            None
        }
        QueueTraceAction::SessionStarted => {
            app.apply_session_event(session_launch::DesktopSessionEvent::SessionStarted {
                session_id: "reload-model-session".to_string(),
            });
            None
        }
        QueueTraceAction::TextDelta => {
            app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
                "post reload activity".to_string(),
            ));
            None
        }
        QueueTraceAction::Done => {
            app.apply_session_event(session_launch::DesktopSessionEvent::Done);
            None
        }
        QueueTraceAction::Error => {
            app.apply_session_event(session_launch::DesktopSessionEvent::Error(
                "reload failed".to_string(),
            ));
            None
        }
        QueueTraceAction::TryDrainQueued => app
            .take_next_queued_draft()
            .map(|(message, _images)| message),
    }
}

fn assert_queue_trace_state(
    app: &SingleSessionApp,
    model: &QueueReferenceModel,
    real_sent: &[String],
    trace: &[QueueTraceAction],
) {
    assert_eq!(app.draft, model.draft, "draft mismatch for trace {trace:?}");
    assert_eq!(
        app.is_processing, model.processing,
        "processing mismatch for trace {trace:?}"
    );
    assert_eq!(
        app.queued_draft_count(),
        model.queued.len(),
        "queued draft count mismatch for trace {trace:?}"
    );
    assert_eq!(
        app.queued_draft_messages(),
        model.queued,
        "queued draft mismatch for trace {trace:?}"
    );
    assert_eq!(real_sent, model.sent, "sent mismatch for trace {trace:?}");
    assert_eq!(
        app.error.is_some(),
        model.error,
        "error mismatch for trace {trace:?}"
    );

    let body = app.body_lines().join("\n");
    for queued in &model.queued {
        assert!(
            body.contains(&format!("queued prompt: {queued}")),
            "body should show queued prompt {queued:?} for trace {trace:?}: {body}"
        );
    }
}

fn run_queue_trace(trace: &[QueueTraceAction]) {
    let mut app = SingleSessionApp::new(None);
    let mut model = QueueReferenceModel::default();
    let mut real_sent = Vec::new();
    let mut prefix = Vec::new();

    for &action in trace {
        prefix.push(action);
        let expected_send = model.apply(action);
        let actual_send = apply_queue_trace_action_to_app(&mut app, action);
        assert_eq!(
            actual_send, expected_send,
            "send outcome mismatch after trace prefix {prefix:?}"
        );
        if let Some(message) = actual_send {
            real_sent.push(message);
        }
        assert_queue_trace_state(&app, &model, &real_sent, &prefix);
    }
}

fn apply_session_event_batch_then_auto_drain(
    app: &mut DesktopApp,
    events: Vec<session_launch::DesktopSessionEvent>,
) -> Option<String> {
    let stats = apply_desktop_session_event_batch_with_stats(app, events);
    if !stats.visible_changed {
        return None;
    }
    app.take_next_queued_single_session_draft()
        .map(|(message, _images)| message)
}

#[test]
fn single_session_event_loop_auto_drain_ignores_stale_done_after_reload() {
    let mut desktop = DesktopApp::SingleSession(SingleSessionApp::new(None));
    let DesktopApp::SingleSession(app) = &mut desktop else {
        unreachable!();
    };
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "working".to_string(),
    ));
    app.is_processing = true;
    app.handle_key(KeyInput::Character("next".to_string()));
    assert_eq!(app.handle_key(KeyInput::QueueDraft), KeyOutcome::Redraw);

    let drained = apply_session_event_batch_then_auto_drain(
        &mut desktop,
        vec![
            session_launch::DesktopSessionEvent::Reloading {
                new_socket: Some("/tmp/jcode-reload-event-loop-test.sock".to_string()),
            },
            session_launch::DesktopSessionEvent::Done,
        ],
    );

    assert_eq!(
        drained, None,
        "stale Done after reload must not auto-drain queued prompt"
    );
    let DesktopApp::SingleSession(app) = &desktop else {
        unreachable!();
    };
    assert_eq!(app.queued_draft_messages(), vec!["next".to_string()]);
    assert!(app.is_processing);
}

#[test]
fn single_session_event_loop_auto_drain_after_post_reload_activity_done() {
    let mut desktop = DesktopApp::SingleSession(SingleSessionApp::new(None));
    let DesktopApp::SingleSession(app) = &mut desktop else {
        unreachable!();
    };
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "working".to_string(),
    ));
    app.is_processing = true;
    app.handle_key(KeyInput::Character("next".to_string()));
    assert_eq!(app.handle_key(KeyInput::QueueDraft), KeyOutcome::Redraw);

    let drained = apply_session_event_batch_then_auto_drain(
        &mut desktop,
        vec![
            session_launch::DesktopSessionEvent::Reloading {
                new_socket: Some("/tmp/jcode-reload-event-loop-test.sock".to_string()),
            },
            session_launch::DesktopSessionEvent::Reloaded {
                session_id: "reload-model-session".to_string(),
            },
            session_launch::DesktopSessionEvent::TextDelta("resumed".to_string()),
            session_launch::DesktopSessionEvent::Done,
        ],
    );

    assert_eq!(drained, Some("next".to_string()));
    let DesktopApp::SingleSession(app) = &desktop else {
        unreachable!();
    };
    assert!(app.queued_draft_messages().is_empty());
    assert!(
        app.is_processing,
        "draining queued prompt starts the next turn"
    );
}

#[test]
fn single_session_event_loop_auto_drain_allows_done_after_reloaded_without_output() {
    let mut desktop = DesktopApp::SingleSession(SingleSessionApp::new(None));
    let DesktopApp::SingleSession(app) = &mut desktop else {
        unreachable!();
    };
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "working".to_string(),
    ));
    app.is_processing = true;
    app.handle_key(KeyInput::Character("next".to_string()));
    assert_eq!(app.handle_key(KeyInput::QueueDraft), KeyOutcome::Redraw);

    let drained = apply_session_event_batch_then_auto_drain(
        &mut desktop,
        vec![
            session_launch::DesktopSessionEvent::Reloading {
                new_socket: Some("/tmp/jcode-reload-event-loop-test.sock".to_string()),
            },
            session_launch::DesktopSessionEvent::Reloaded {
                session_id: "reload-model-session".to_string(),
            },
            session_launch::DesktopSessionEvent::Done,
        ],
    );

    assert_eq!(drained, Some("next".to_string()));
}

#[test]
fn single_session_event_loop_reload_error_keeps_queue_retryable() {
    let mut desktop = DesktopApp::SingleSession(SingleSessionApp::new(None));
    let DesktopApp::SingleSession(app) = &mut desktop else {
        unreachable!();
    };
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "working".to_string(),
    ));
    app.is_processing = true;
    app.handle_key(KeyInput::Character("retry me".to_string()));
    assert_eq!(app.handle_key(KeyInput::QueueDraft), KeyOutcome::Redraw);

    let drained = apply_session_event_batch_then_auto_drain(
        &mut desktop,
        vec![
            session_launch::DesktopSessionEvent::Reloading {
                new_socket: Some("/tmp/jcode-reload-event-loop-test.sock".to_string()),
            },
            session_launch::DesktopSessionEvent::Error("reload failed".to_string()),
        ],
    );

    assert_eq!(drained, None);
    let DesktopApp::SingleSession(app) = &desktop else {
        unreachable!();
    };
    assert_eq!(app.queued_draft_messages(), vec!["retry me".to_string()]);
}

#[test]
fn single_session_event_loop_multiple_reloads_preserve_queued_prompt_order() {
    let mut desktop = DesktopApp::SingleSession(SingleSessionApp::new(None));
    let DesktopApp::SingleSession(app) = &mut desktop else {
        unreachable!();
    };
    app.is_processing = true;
    app.handle_key(KeyInput::Character("first".to_string()));
    assert_eq!(app.handle_key(KeyInput::QueueDraft), KeyOutcome::Redraw);

    assert_eq!(
        apply_session_event_batch_then_auto_drain(
            &mut desktop,
            vec![
                session_launch::DesktopSessionEvent::Reloading {
                    new_socket: Some("/tmp/jcode-reload-one.sock".to_string()),
                },
                session_launch::DesktopSessionEvent::Reloaded {
                    session_id: "reload-model-session".to_string(),
                },
            ],
        ),
        None
    );

    let DesktopApp::SingleSession(app) = &mut desktop else {
        unreachable!();
    };
    app.handle_key(KeyInput::Character("second".to_string()));
    assert_eq!(app.handle_key(KeyInput::QueueDraft), KeyOutcome::Redraw);
    assert_eq!(
        app.queued_draft_messages(),
        vec!["first".to_string(), "second".to_string()]
    );

    assert_eq!(
        apply_session_event_batch_then_auto_drain(
            &mut desktop,
            vec![
                session_launch::DesktopSessionEvent::Reloading {
                    new_socket: Some("/tmp/jcode-reload-two.sock".to_string()),
                },
                session_launch::DesktopSessionEvent::Reloaded {
                    session_id: "reload-model-session".to_string(),
                },
                session_launch::DesktopSessionEvent::Done,
            ],
        ),
        Some("first".to_string())
    );
    assert_eq!(
        apply_session_event_batch_then_auto_drain(
            &mut desktop,
            vec![session_launch::DesktopSessionEvent::Done],
        ),
        Some("second".to_string())
    );
}

#[test]
fn single_session_reload_queue_golden_trace_waits_for_done_before_drain() {
    run_queue_trace(&[
        QueueTraceAction::Reloading,
        QueueTraceAction::TypeA,
        QueueTraceAction::CtrlEnter,
        QueueTraceAction::SessionStarted,
        QueueTraceAction::TryDrainQueued,
        QueueTraceAction::Done,
        QueueTraceAction::TryDrainQueued,
        QueueTraceAction::Reloaded,
        QueueTraceAction::Done,
        QueueTraceAction::TryDrainQueued,
        QueueTraceAction::TextDelta,
        QueueTraceAction::Done,
        QueueTraceAction::TryDrainQueued,
    ]);
}

#[test]
fn single_session_reload_queue_state_space_matches_reference_model() {
    const ACTIONS: &[QueueTraceAction] = &[
        QueueTraceAction::TypeA,
        QueueTraceAction::TypeB,
        QueueTraceAction::CtrlEnter,
        QueueTraceAction::Reloading,
        QueueTraceAction::Reloaded,
        QueueTraceAction::SessionStarted,
        QueueTraceAction::TextDelta,
        QueueTraceAction::Done,
        QueueTraceAction::Error,
        QueueTraceAction::TryDrainQueued,
    ];

    fn visit(trace: &mut Vec<QueueTraceAction>, depth_remaining: usize) {
        run_queue_trace(trace);
        if depth_remaining == 0 {
            return;
        }
        for &action in ACTIONS {
            trace.push(action);
            visit(trace, depth_remaining - 1);
            trace.pop();
        }
    }

    let mut trace = Vec::new();
    visit(&mut trace, 5);
}

#[test]
fn single_session_paste_text_preserves_spaces() {
    let mut app = SingleSessionApp::new(None);
    app.paste_text("hello  pasted");
    assert_eq!(app.draft, "hello  pasted");
}

#[test]
fn clipboard_text_normalization_preserves_whitespace_but_unifies_newlines() {
    assert_eq!(normalize_clipboard_text("a\r\nb\rc  "), "a\nb\nc  ");
    assert_eq!(
        normalize_clipboard_text("  padded\ttext  "),
        "  padded\ttext  "
    );
}

#[test]
fn paste_clipboard_text_inserts_at_cursor_and_supports_undo() {
    let mut desktop = DesktopApp::SingleSession(SingleSessionApp::new(None));
    let DesktopApp::SingleSession(app) = &mut desktop else {
        unreachable!();
    };
    app.handle_key(KeyInput::Character("hello world".to_string()));
    app.draft_cursor = "hello".len();

    assert!(paste_clipboard_text(&mut desktop, "\r\n  pasted  "));
    let DesktopApp::SingleSession(app) = &mut desktop else {
        unreachable!();
    };
    assert_eq!(app.draft, "hello\n  pasted   world");
    assert_eq!(app.draft_cursor, "hello\n  pasted  ".len());

    assert_eq!(app.handle_key(KeyInput::UndoInput), KeyOutcome::Redraw);
    assert_eq!(app.draft, "hello world");
    assert_eq!(app.draft_cursor, "hello".len());
}

#[test]
fn paste_clipboard_text_routes_to_interactive_stdin_response() {
    let mut desktop = DesktopApp::SingleSession(SingleSessionApp::new(None));
    let DesktopApp::SingleSession(app) = &mut desktop else {
        unreachable!();
    };
    app.apply_session_event(session_launch::DesktopSessionEvent::StdinRequest {
        request_id: "stdin-1".to_string(),
        prompt: "Password:".to_string(),
        is_password: true,
        tool_call_id: "tool-1".to_string(),
    });

    assert!(paste_clipboard_text(&mut desktop, "secret\r\nline"));
    let DesktopApp::SingleSession(app) = &desktop else {
        unreachable!();
    };
    assert_eq!(app.stdin_response.as_ref().unwrap().input, "secret\nline");
    assert!(app.draft.is_empty());
}

#[test]
fn paste_clipboard_text_ignores_empty_text_so_image_fallback_can_run() {
    let mut desktop = DesktopApp::SingleSession(SingleSessionApp::new(None));
    assert!(!paste_clipboard_text(&mut desktop, ""));
    let DesktopApp::SingleSession(app) = &desktop else {
        unreachable!();
    };
    assert!(app.draft.is_empty());
}

#[test]
fn single_session_character_selection_extracts_visible_text() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::user("first"));
    app.messages
        .push(SingleSessionMessage::assistant("second  \nthird"));
    let lines = app.body_lines();
    let second_line = lines
        .iter()
        .position(|line| line == "second")
        .expect("second assistant line");
    let third_line = second_line + 1;

    app.begin_selection(SelectionPoint {
        line: second_line,
        column: 1,
    });
    app.update_selection(SelectionPoint {
        line: third_line,
        column: 2,
    });

    assert_eq!(
        app.selected_text_from_lines(&lines),
        Some(format!(
            "{}\n{}",
            &lines[second_line][1..],
            &lines[third_line][..2]
        ))
    );
    assert_eq!(
        app.selection_segments(&lines),
        vec![
            SelectionLineSegment {
                line: second_line,
                start_column: 1,
                end_column: lines[second_line].chars().count()
            },
            SelectionLineSegment {
                line: third_line,
                start_column: 0,
                end_column: 2
            }
        ]
    );
}

#[test]
fn single_session_character_selection_handles_reverse_unicode_selection() {
    let mut app = SingleSessionApp::new(None);
    let lines = vec!["hello 🦀 world".to_string()];

    app.begin_selection(SelectionPoint { line: 0, column: 9 });
    app.update_selection(SelectionPoint { line: 0, column: 6 });

    assert_eq!(
        app.selected_text_from_lines(&lines),
        Some("🦀 w".to_string())
    );
}

#[test]
fn single_session_body_line_at_y_maps_transcript_region() {
    let size = PhysicalSize::new(800, 600);
    assert_eq!(
        single_session_body_line_at_y(size, PANEL_BODY_TOP_PADDING + 1.0),
        Some(0)
    );
    assert_eq!(single_session_body_line_at_y(size, 1.0), None);
}

#[test]
fn single_session_body_point_at_position_maps_x_to_character_column() {
    let size = PhysicalSize::new(800, 600);
    let lines = vec!["abcdef".to_string()];
    let y = PANEL_BODY_TOP_PADDING + 1.0;
    let char_width = single_session_body_char_width();

    assert_eq!(
        single_session_body_point_at_position(size, PANEL_TITLE_LEFT_PADDING - 4.0, y, &lines),
        Some(SelectionPoint { line: 0, column: 0 })
    );
    assert_eq!(
        single_session_body_point_at_position(
            size,
            PANEL_TITLE_LEFT_PADDING + char_width * 2.4,
            y,
            &lines
        ),
        Some(SelectionPoint { line: 0, column: 2 })
    );
    assert_eq!(
        single_session_body_point_at_position(
            size,
            PANEL_TITLE_LEFT_PADDING + char_width * 99.0,
            y,
            &lines
        ),
        Some(SelectionPoint { line: 0, column: 6 })
    );
}

#[test]
fn single_session_draft_point_at_position_maps_to_cursor_line_column() {
    let size = PhysicalSize::new(900, 700);
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello\nworld".to_string()));
    let typography = single_session_typography_for_scale(app.text_scale());
    let char_width = typography.code_size * 0.58;
    let line_height = typography.code_size * typography.code_line_height;
    let draft_top = single_session_draft_top_for_app(&app, size);
    let prompt_columns = app.composer_prompt().chars().count() as f32;

    assert_eq!(
        single_session_draft_line_col_at_position(
            &app,
            size,
            PANEL_TITLE_LEFT_PADDING + (prompt_columns + 2.0) * char_width,
            draft_top + line_height * 0.5,
        ),
        Some((0, 2))
    );
    assert_eq!(
        single_session_draft_line_col_at_position(
            &app,
            size,
            PANEL_TITLE_LEFT_PADDING + 3.0 * char_width,
            draft_top + line_height * 1.5,
        ),
        Some((1, 3))
    );

    app.set_draft_cursor_line_col(1, 3);
    assert_eq!(app.draft_cursor, "hello\nwor".len());
}

#[test]
fn single_session_prompt_jump_moves_between_user_turns() {
    let mut app = SingleSessionApp::new(None);
    for index in 0..4 {
        app.messages
            .push(SingleSessionMessage::user(format!("question {index}")));
        app.messages
            .push(SingleSessionMessage::assistant(format!("answer {index}")));
    }

    assert_eq!(app.body_scroll_lines, 0.0);
    assert_eq!(app.handle_key(KeyInput::JumpPrompt(-1)), KeyOutcome::Redraw);
    assert!(app.body_scroll_lines > 0.0);
    let older_scroll = app.body_scroll_lines;

    assert_eq!(app.handle_key(KeyInput::JumpPrompt(1)), KeyOutcome::Redraw);
    assert!(app.body_scroll_lines < older_scroll || app.body_scroll_lines == 0.0);
}

#[test]
fn single_session_scroll_shortcuts_move_body_position() {
    let mut app = SingleSessionApp::new(None);
    for index in 0..10 {
        app.messages
            .push(SingleSessionMessage::user(format!("question {index}")));
        app.messages
            .push(SingleSessionMessage::assistant(format!("answer {index}")));
    }

    assert_eq!(
        app.handle_key(KeyInput::ScrollBodyLines(1)),
        KeyOutcome::Redraw
    );
    assert_eq!(app.body_scroll_lines, 1.0);
    assert_eq!(
        app.handle_key(KeyInput::ScrollBodyToBottom),
        KeyOutcome::Redraw
    );
    assert_eq!(app.body_scroll_lines, 0.0);
    assert_eq!(
        app.handle_key(KeyInput::ScrollBodyToTop),
        KeyOutcome::Redraw
    );
    assert!(app.body_scroll_lines > 1.0);
    assert_eq!(
        app.handle_key(KeyInput::ScrollBodyToBottom),
        KeyOutcome::Redraw
    );
    assert_eq!(app.body_scroll_lines, 0.0);
}

#[test]
fn single_session_copy_latest_response_prefers_streaming_text() {
    let mut app = SingleSessionApp::new(None);
    app.messages
        .push(SingleSessionMessage::assistant("completed answer"));
    assert_eq!(
        app.handle_key(KeyInput::CopyLatestResponse),
        KeyOutcome::CopyLatestResponse("completed answer".to_string())
    );

    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "streaming answer".to_string(),
    ));
    assert_eq!(
        app.handle_key(KeyInput::CopyLatestResponse),
        KeyOutcome::CopyLatestResponse("streaming answer".to_string())
    );
}

#[test]
fn single_session_copy_code_and_transcript_shortcuts_use_rich_transcript() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::user("question"));
    app.messages.push(SingleSessionMessage::assistant(
        "Answer\n\n```rust\nfn main() {}\n```",
    ));

    assert_eq!(
        app.handle_key(KeyInput::CopyLatestCodeBlock),
        KeyOutcome::CopyText {
            text: "fn main() {}\n".to_string(),
            success_notice: "copied latest code block",
        }
    );

    match app.handle_key(KeyInput::CopyTranscript) {
        KeyOutcome::CopyText {
            text,
            success_notice,
        } => {
            assert_eq!(success_notice, "copied transcript");
            assert!(text.contains("question"));
            assert!(text.contains("fn main() {}"));
        }
        other => panic!("expected transcript copy, got {other:?}"),
    }

    let mut empty = SingleSessionApp::new(None);
    assert_eq!(
        empty.handle_key(KeyInput::CopyLatestCodeBlock),
        KeyOutcome::Redraw
    );
    assert_eq!(empty.status.as_deref(), Some("no code block to copy"));
}

#[test]
fn single_session_streaming_preserves_manual_scroll_but_submit_follows_bottom() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::user("older"));
    app.messages
        .push(SingleSessionMessage::assistant("older answer"));
    app.scroll_body_lines(12.0);

    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "new token".to_string(),
    ));
    assert_eq!(app.body_scroll_lines, 12.0);

    app.handle_key(KeyInput::Character("new prompt".to_string()));
    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession {
            message: "new prompt".to_string(),
            images: Vec::new()
        }
    );
    assert_eq!(app.body_scroll_lines, 0.0);
}

#[test]
fn single_session_applies_live_server_events_to_visible_body() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello".to_string()));
    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession {
            message: "hello".to_string(),
            images: Vec::new()
        }
    );
    app.apply_session_event(session_launch::DesktopSessionEvent::SessionStarted {
        session_id: "session_desktop_live_123".to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "hi".to_string(),
    ));

    let live_lines = app.body_lines().join("\n");
    assert!(live_lines.contains("1  hello"));
    assert!(live_lines.contains("hi"));
    assert!(!live_lines.contains("user:"));
    assert!(!live_lines.contains("assistant:"));
    assert!(!live_lines.contains("status:"));
    assert!(app.has_background_work());

    app.apply_session_event(session_launch::DesktopSessionEvent::Done);
    assert!(!app.has_background_work());
    let completed_lines = app.body_lines().join("\n");
    assert!(completed_lines.contains("1  hello"));
    assert!(completed_lines.contains("hi"));
    assert!(!completed_lines.contains("assistant:"));
}

#[test]
fn desktop_app_drains_session_events_into_visible_debug_snapshot() {
    let mut app = fresh_single_session_app();
    assert_eq!(
        app.handle_key(KeyInput::Character("hello smoke".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession {
            message: "hello smoke".to_string(),
            images: Vec::new()
        }
    );

    let (event_tx, event_rx) = mpsc::channel();
    event_tx
        .send(session_launch::DesktopSessionEvent::SessionStarted {
            session_id: "session_visible_smoke".to_string(),
        })
        .unwrap();
    event_tx
        .send(session_launch::DesktopSessionEvent::TextDelta(
            "visible assistant response".to_string(),
        ))
        .unwrap();
    assert!(apply_pending_session_events(&mut app, &event_rx));

    let streaming = app.debug_snapshot();
    assert_eq!(streaming.mode, "single_session");
    assert_eq!(
        streaming.live_session_id.as_deref(),
        Some("session_visible_smoke")
    );
    assert!(streaming.is_processing);
    assert!(streaming.body_text.contains("1  hello smoke"));
    assert!(streaming.body_text.contains("visible assistant response"));
    assert!(!streaming.body_text.contains("user:"));
    assert!(!streaming.body_text.contains("assistant:"));
    assert!(!streaming.body_text.contains("status:"));

    event_tx
        .send(session_launch::DesktopSessionEvent::Done)
        .unwrap();
    assert!(apply_pending_session_events(&mut app, &event_rx));
    let completed = app.debug_snapshot();
    assert!(!completed.is_processing);
    assert_eq!(completed.status.as_deref(), Some("ready"));
    assert!(completed.body_text.contains("visible assistant response"));
    assert!(!completed.body_text.contains("assistant:"));
}

#[test]
fn headless_chat_smoke_message_parses_hidden_flag() {
    assert_eq!(
        headless_chat_smoke_message(&[
            "jcode-desktop".to_string(),
            "--headless-chat-smoke".to_string(),
            "reply pong".to_string(),
        ]),
        Some("reply pong".to_string())
    );
    assert_eq!(
        headless_chat_smoke_message(&[
            "jcode-desktop".to_string(),
            "--headless-chat-smoke=reply pong".to_string(),
        ]),
        Some("reply pong".to_string())
    );
    assert_eq!(
        headless_chat_smoke_message(&["jcode-desktop".to_string()]),
        None
    );
}

#[test]
fn desktop_help_text_documents_desktop_options() {
    let help = desktop_help_text();

    assert!(help.contains("Usage:"));
    assert!(help.contains("--fullscreen"));
    assert!(help.contains("--workspace"));
    assert!(help.contains("--startup-log"));
    assert!(help.contains("--startup-benchmark"));
    assert!(help.contains("--headless-chat-smoke <MSG>"));
    assert!(help.contains("--version"));
    assert!(help.contains("--help"));
}

#[test]
fn desktop_startup_flags_enable_logging_and_benchmark_mode() {
    let args = vec!["jcode-desktop".to_string(), "--startup-log".to_string()];
    assert!(startup_log_requested(&args));
    assert!(!startup_benchmark_requested(&args));

    let args = vec![
        "jcode-desktop".to_string(),
        "--startup-benchmark".to_string(),
    ];
    assert!(startup_benchmark_requested(&args));
    assert!(!startup_log_requested(&["jcode-desktop".to_string()]));

    assert!(env_flag_enabled(OsString::from("1")));
    assert!(!env_flag_enabled(OsString::from("false")));
}

#[test]
fn single_session_reload_event_keeps_worker_state_processing() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::Reloading {
        new_socket: Some("/tmp/jcode-reload.sock".to_string()),
    });

    assert!(app.has_background_work());
    assert!(app.body_lines().join("\n").contains("server reloading"));
}

#[test]
fn single_session_scrollback_virtualizes_visible_body_lines() {
    let mut app = SingleSessionApp::new(None);
    for index in 0..32 {
        app.apply_session_event(session_launch::DesktopSessionEvent::TextReplace(format!(
            "message {index}"
        )));
        app.apply_session_event(session_launch::DesktopSessionEvent::Done);
    }
    let size = PhysicalSize::new(640, 480);

    let bottom = single_session_visible_body(&app, size).join("\n");
    assert!(bottom.contains("message 31"));
    assert!(!bottom.contains("message 0"));

    app.scroll_body_lines(24.0);
    let older = single_session_visible_body(&app, size).join("\n");
    assert!(older.contains("message 0") || older.contains("message 1"));
}

#[test]
fn mouse_scroll_delta_maps_to_body_scroll_lines() {
    assert_eq!(
        mouse_scroll_lines(MouseScrollDelta::LineDelta(0.0, 1.0)),
        Some(3.0)
    );
    assert_eq!(
        mouse_scroll_lines(MouseScrollDelta::LineDelta(0.0, -1.0)),
        Some(-3.0)
    );
}

#[test]
fn pixel_scroll_deltas_preserve_fractional_lines() {
    let mut accumulator = ScrollLineAccumulator::default();
    let now = Instant::now();
    let half_line = body_scroll_line_pixels() as f64 * 0.5;

    assert_eq!(
        accumulator.scroll_lines(
            MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(0.0, half_line)),
            now,
        ),
        Some(0.5)
    );
    assert_eq!(
        accumulator.scroll_lines(
            MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(0.0, half_line)),
            now + Duration::from_millis(16),
        ),
        Some(0.5)
    );
}

#[test]
fn scroll_accumulator_keeps_fractional_momentum_after_wheel_input() {
    let mut accumulator = ScrollLineAccumulator::default();
    let now = Instant::now();

    assert_eq!(
        accumulator.scroll_lines(MouseScrollDelta::LineDelta(0.0, 1.0), now),
        Some(3.0)
    );
    assert!(accumulator.is_active());

    let frame = accumulator.frame(now + Duration::from_millis(16));
    assert!(
        frame.active,
        "momentum should continue after the input event"
    );
    assert!(
        frame
            .scroll_lines
            .is_some_and(|lines| lines.abs() >= SCROLL_FRACTIONAL_EPSILON),
        "momentum should emit fractional scroll lines"
    );
}

#[test]
fn pixel_scroll_reversal_and_idle_reset_keep_fractional_deltas() {
    let mut accumulator = ScrollLineAccumulator::default();
    let now = Instant::now();
    let three_quarters = body_scroll_line_pixels() as f64 * 0.75;
    let half_line = body_scroll_line_pixels() as f64 * 0.5;

    assert_scroll_lines_near(
        accumulator.scroll_lines(
            MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(0.0, three_quarters)),
            now,
        ),
        0.75,
    );
    assert_eq!(
        accumulator.scroll_lines(
            MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(0.0, -half_line)),
            now + Duration::from_millis(16),
        ),
        Some(-0.5)
    );
    assert_eq!(
        accumulator.scroll_lines(
            MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(0.0, -half_line)),
            now + Duration::from_millis(32),
        ),
        Some(-0.5)
    );

    assert_scroll_lines_near(
        accumulator.scroll_lines(
            MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(0.0, three_quarters)),
            now + Duration::from_millis(48),
        ),
        0.75,
    );
    assert_scroll_lines_near(
        accumulator.scroll_lines(
            MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(0.0, three_quarters)),
            now + SCROLL_GESTURE_IDLE_RESET + Duration::from_millis(80),
        ),
        0.75,
    );
}

fn assert_scroll_lines_near(actual: Option<f32>, expected: f32) {
    let Some(actual) = actual else {
        panic!("expected scroll lines near {expected}, got None");
    };
    assert!(
        (actual - expected).abs() <= SCROLL_FRACTIONAL_EPSILON,
        "expected scroll lines near {expected}, got {actual}"
    );
}

#[test]
fn mouse_scroll_clamps_to_available_single_session_history() {
    let mut app = SingleSessionApp::new(None);
    let size = PhysicalSize::new(900, 360);
    for index in 0..48 {
        app.messages
            .push(SingleSessionMessage::assistant(format!("message {index}")));
    }
    let mut desktop = DesktopApp::SingleSession(app);
    let mut metrics_cache = SingleSessionScrollMetricsCache::default();

    assert!(desktop.scroll_single_session_body(10_000.0, size, &mut metrics_cache));
    let DesktopApp::SingleSession(app) = &desktop else {
        unreachable!();
    };
    let max_scroll = single_session_body_scroll_metrics(app, size, 0)
        .expect("scroll metrics")
        .max_scroll_lines;
    assert_eq!(app.body_scroll_lines, max_scroll as f32);

    assert!(desktop.scroll_single_session_body(-1.0, size, &mut metrics_cache));
    let DesktopApp::SingleSession(app) = &desktop else {
        unreachable!();
    };
    assert_eq!(app.body_scroll_lines, max_scroll as f32 - 1.0);
}

#[test]
fn fractional_scroll_viewport_keeps_fractional_line_offset() {
    let mut app = SingleSessionApp::new(None);
    let size = PhysicalSize::new(900, 360);
    for index in 0..48 {
        app.messages
            .push(SingleSessionMessage::assistant(format!("message {index}")));
    }

    let normal = single_session_body_viewport_for_tick(&app, size, 0, 0.0);
    app.scroll_body_lines(0.5);
    let fractional = single_session_body_viewport_for_tick(&app, size, 0, 0.0);

    assert!(fractional.top_offset_pixels < 0.0);
    assert_eq!(fractional.lines.len(), normal.lines.len() + 1);
    assert_eq!(&fractional.lines[1..], normal.lines.as_slice());
}

#[test]
fn fractional_scroll_offsets_body_text_area_without_moving_chrome() {
    let mut app = SingleSessionApp::new(Some(test_session_card(
        "session_smooth",
        "smooth",
        "active",
    )));
    let size = PhysicalSize::new(900, 360);
    for index in 0..48 {
        app.messages
            .push(SingleSessionMessage::assistant(format!("message {index}")));
    }
    let mut fractional_app = app.clone();
    fractional_app.scroll_body_lines(0.5);
    let mut font_system = FontSystem::new();
    let key = single_session_text_key(&fractional_app, size);
    let buffers = single_session_text_buffers_from_key(&key, size, &mut font_system);
    let normal_areas = single_session_text_areas_for_app_with_scroll(&app, &buffers, size, 0, 0.0);
    let fractional_areas =
        single_session_text_areas_for_app_with_scroll(&fractional_app, &buffers, size, 0, 0.0);

    let normal_body = normal_areas
        .iter()
        .find(|area| area.bounds.top == PANEL_BODY_TOP_PADDING as i32)
        .expect("normal body text area");
    let fractional_body = fractional_areas
        .iter()
        .find(|area| area.bounds.top == PANEL_BODY_TOP_PADDING as i32)
        .expect("fractional body text area");
    let normal_title = normal_areas
        .iter()
        .find(|area| area.top == PANEL_TITLE_TOP_PADDING && area.bounds.bottom == 64)
        .expect("normal title text area");
    let fractional_title = fractional_areas
        .iter()
        .find(|area| area.top == PANEL_TITLE_TOP_PADDING && area.bounds.bottom == 64)
        .expect("fractional title text area");

    assert_eq!(normal_title.top, PANEL_TITLE_TOP_PADDING);
    assert_eq!(fractional_title.top, PANEL_TITLE_TOP_PADDING);
    assert_eq!(normal_body.top, PANEL_BODY_TOP_PADDING);
    assert!(fractional_body.top < normal_body.top);
}

#[test]
fn fractional_body_bottom_bounds_round_outward() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello desktop".to_string()));
    assert!(matches!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession { .. }
    ));
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "assistant response".to_string(),
    ));
    let size = PhysicalSize::new(900, 640);
    let mut font_system = FontSystem::new();
    let buffers = single_session_text_buffers(&app, size, &mut font_system);
    let areas = single_session_text_areas_for_app(&app, &buffers, size);
    let body_area = areas
        .iter()
        .find(|area| {
            area.bounds.top == PANEL_BODY_TOP_PADDING as i32
                && area.default_color == text_color(ASSISTANT_TEXT_COLOR)
        })
        .expect("body text area");
    let rendered_lines = single_session_rendered_body_lines_for_tick(&app, size, 0);
    let expected_bottom =
        single_session_body_bottom_for_total_lines(&app, size, rendered_lines.len()).ceil() as i32;

    assert_eq!(body_area.bounds.bottom, expected_bottom);
}

#[test]
fn welcome_timeline_body_reserves_composer_lane_clearance() {
    let size = PhysicalSize::new(900, 640);
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello desktop".to_string()));
    assert!(matches!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession { .. }
    ));
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "assistant response".to_string(),
    ));

    let mut font_system = FontSystem::new();
    let buffers = single_session_text_buffers(&app, size, &mut font_system);
    let areas = single_session_text_areas_for_app(&app, &buffers, size);
    let typography = single_session_typography();
    let line_height = typography.body_size * typography.body_line_height;
    let composer_area = areas.first().expect("composer text area");
    let body_area = areas
        .iter()
        .find(|area| area.bounds.top == PANEL_BODY_TOP_PADDING as i32)
        .expect("welcome timeline body text area");
    let body_bottom = body_area.bounds.bottom as f32;
    let composer_top = composer_area.top;

    assert!(
        composer_top - body_bottom >= line_height - 1.0,
        "body text should reserve at least one transcript line before composer lane: body_bottom={body_bottom}, composer_top={composer_top}, line_height={line_height}"
    );
}

#[test]
fn single_session_visible_body_wraps_long_assistant_lines() {
    let mut app = SingleSessionApp::new(Some(test_session_card("session_wrap", "wrap", "active")));
    app.messages.push(SingleSessionMessage::assistant(
        "This assistant response should wrap cleanly within the desktop transcript column instead of running horizontally forever."
    ));
    let size = PhysicalSize::new(360, 480);

    let raw = app.body_styled_lines();
    let visible = single_session_visible_body(&app, size);

    assert_eq!(raw.len(), 1, "model transcript remains one logical line");
    assert!(
        visible.len() > raw.len(),
        "rendered body should split long assistant text into visual lines: {:?}",
        visible
    );
    assert!(visible.iter().all(|line| line.chars().count() <= 40));
}

#[test]
fn long_single_session_transcript_exposes_scrollbar_metrics() {
    let mut app = SingleSessionApp::new(None);
    let size = PhysicalSize::new(900, 360);
    for index in 0..48 {
        app.messages
            .push(SingleSessionMessage::assistant(format!("message {index}")));
    }

    let metrics = single_session_body_scroll_metrics(&app, size, 0).expect("scroll metrics");
    assert!(metrics.total_lines > metrics.visible_lines);
    assert!(metrics.max_scroll_lines > 0);

    let vertices = build_single_session_vertices(&app, size, 0.0, 0);
    assert!(
        vertices
            .iter()
            .any(|vertex| vertex.color == [0.035, 0.065, 0.145, 0.34])
    );
}

#[test]
fn glyphon_caret_position_uses_shaped_draft_buffer() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello".to_string()));
    let size = PhysicalSize::new(640, 480);
    let mut font_system = FontSystem::new();
    let buffers = single_session_text_buffers(&app, size, &mut font_system);

    let caret = glyphon_draft_caret_position(&app, &buffers[2], size)
        .expect("caret position should be available from glyphon layout runs");

    assert!(caret.x > PANEL_TITLE_LEFT_PADDING);
    assert!(caret.y >= single_session_draft_top_for_app(&app, size));
}

#[test]
fn fresh_welcome_uses_dominant_hero_composer_while_drafting() {
    let size = PhysicalSize::new(1000, 720);
    let mut app = SingleSessionApp::new(None);
    let mut font_system = FontSystem::new();
    let buffers = single_session_text_buffers(&app, size, &mut font_system);
    let areas = single_session_text_areas_for_app(&app, &buffers, size);

    assert_eq!(
        areas.first().expect("draft text area").top,
        fresh_welcome_draft_top(size)
    );
    assert_eq!(areas.len(), 5, "fresh welcome shows startup hint chrome");
    assert!(
        areas.first().expect("draft text area").top > handwritten_welcome_bounds(size).1[1],
        "fresh input line should stay visually below the handwritten hero"
    );
    let hero_bottom = handwritten_welcome_bounds(size).1[1];
    let version_area = areas
        .iter()
        .find(|area| area.top > hero_bottom && area.top < fresh_welcome_draft_top(size))
        .expect("version label should sit between hero and composer");
    assert!(
        version_area.top - hero_bottom >= 29.0,
        "version label needs enough clearance from handwriting strokes"
    );

    app.handle_key(KeyInput::Character("hello".to_string()));
    let key = single_session_text_key(&app, size);
    assert!(app.is_fresh_welcome_visible());
    assert_is_handwritten_welcome_phrase(&key.welcome_hero);
    assert_visual_text_contains(&key, &key.welcome_hero);
    assert_eq!(
        single_session_draft_top_for_app(&app, size),
        fresh_welcome_draft_top(size)
    );
}

#[test]
fn completed_welcome_hero_uses_runtime_font_mask_without_overlay() {
    let size = PhysicalSize::new(1000, 720);
    let app = SingleSessionApp::new(None);
    let mut font_system = FontSystem::new();
    let buffers = single_session_text_buffers(&app, size, &mut font_system);

    let completed_vertices =
        build_single_session_vertices_with_scroll_and_reveal(&app, size, 0.0, 0, 0.0, 1.0);
    let completed_areas = single_session_text_areas_for_app(&app, &buffers, size);

    assert!(!vertices_have_color(
        &completed_vertices,
        WELCOME_HANDWRITING_COLOR
    ));
    assert_runtime_welcome_hero_available(&app, size);
    assert!(
        completed_areas
            .iter()
            .all(|area| !std::ptr::eq(area.buffer, &buffers[5])),
        "runtime hero mask owns the final handwritten font pixels without a glyphon overlay"
    );
}

#[test]
fn fresh_welcome_model_picker_only_reserves_inline_lane() {
    let size = PhysicalSize::new(1000, 720);
    let mut app = SingleSessionApp::new(None);

    assert_eq!(
        app.handle_key(KeyInput::Character("/model opus".to_string())),
        KeyOutcome::LoadModelCatalog
    );

    let key = single_session_text_key(&app, size);
    assert!(
        key.fresh_welcome_visible,
        "inline model picker should not hide the welcome hero"
    );
    assert_is_handwritten_welcome_phrase(&key.welcome_hero);
    assert_visual_text_contains(&key, &key.welcome_hero);
    assert!(
        key.body.is_empty(),
        "model picker status should not become transcript/body text"
    );
    assert!(
        key.inline_widget
            .iter()
            .any(|line| line.text.contains("Model picker"))
    );
    assert_eq!(
        single_session_draft_top_for_app(&app, size),
        single_session_draft_top(size),
        "inline picker should move the composer to the normal bottom input lane"
    );

    let mut font_system = FontSystem::new();
    let buffers = single_session_text_buffers(&app, size, &mut font_system);
    let areas = single_session_text_areas_for_app(&app, &buffers, size);
    let draft_area = areas.first().expect("draft text area");
    assert_eq!(draft_area.top, single_session_draft_top(size));
    let inline_area = areas.last().expect("inline model picker text area");
    let version_area = areas
        .iter()
        .find(|area| std::ptr::eq(area.buffer, &buffers[3]))
        .expect("fresh welcome version text area");
    assert!(
        inline_area.top < draft_area.top,
        "fresh inline picker should render above the typed /model command"
    );
    assert!(
        inline_area.left > PANEL_TITLE_LEFT_PADDING,
        "inline picker should leave extra side breathing room: left={}",
        inline_area.left
    );
    assert!(
        inline_area.bounds.right < (size.width as f32 - PANEL_TITLE_LEFT_PADDING) as i32,
        "inline picker should use an intrinsic text width instead of the full panel: right={}",
        inline_area.bounds.right
    );
    assert!(
        inline_area.top >= version_area.bounds.bottom as f32,
        "fresh inline picker should flow below the welcome hero/version chrome instead of overlaying it: inline_top={}, version_bottom={}",
        inline_area.top,
        version_area.bounds.bottom
    );
    assert!(
        inline_area.top > handwritten_welcome_bounds(size).1[1],
        "fresh inline picker must not overlap the handwritten welcome hero"
    );
    assert!(
        inline_area.bounds.bottom > inline_area.bounds.top,
        "fresh inline picker should keep a visible clipped lane"
    );

    let vertices = build_single_session_vertices(&app, size, 0.0, 0);
    let inline_card_vertices = positions_for_color(&vertices, [0.972, 0.982, 1.000, 0.54]);
    assert!(
        !inline_card_vertices.is_empty(),
        "inline picker should draw a rounded card background"
    );
    let min_x = inline_card_vertices
        .iter()
        .map(|position| ndc_x_to_pixel(f32::from_bits(position[0]), size))
        .fold(f32::INFINITY, f32::min);
    let max_x = inline_card_vertices
        .iter()
        .map(|position| ndc_x_to_pixel(f32::from_bits(position[0]), size))
        .fold(f32::NEG_INFINITY, f32::max);
    assert!(
        min_x > PANEL_TITLE_LEFT_PADDING,
        "inline card should start after the normal panel gutter: min_x={min_x}"
    );
    assert!(
        max_x < size.width as f32 - PANEL_TITLE_LEFT_PADDING,
        "inline card should hug the text instead of spanning full width: max_x={max_x}"
    );
}

#[test]
fn fresh_submit_keeps_single_visual_timeline_without_transcript_greeting() {
    let size = PhysicalSize::new(1000, 720);
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello desktop".to_string()));
    assert!(matches!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession { .. }
    ));

    let mut font_system = FontSystem::new();
    let buffers = single_session_text_buffers(&app, size, &mut font_system);
    let areas = single_session_text_areas_for_app(&app, &buffers, size);
    let key = single_session_text_key(&app, size);
    let mut vertices = build_single_session_vertices(&app, size, 0.0, 0);
    push_single_session_caret(&mut vertices, &app, size, buffers.get(2));

    assert_eq!(key.title, "");
    assert_is_handwritten_welcome_phrase(&key.welcome_hero);
    assert_visual_text_contains(&key, &key.welcome_hero);
    assert!(vertices_have_color(&vertices, WELCOME_AURORA_BLUE));
    assert_runtime_welcome_hero_available(&app, size);
    assert!(vertices_have_rgb(&vertices, NATIVE_SPINNER_HEAD_COLOR));
    assert!(
        key.body
            .iter()
            .any(|line| line.text.contains("hello desktop"))
    );
    let submitted_index = key
        .body
        .iter()
        .position(|line| line.text.contains("hello desktop"))
        .expect("submitted prompt should remain visible in the timeline");
    assert!(
        submitted_index > 0
            && key.body[..submitted_index]
                .iter()
                .all(|line| line.text.trim().is_empty()),
        "visual hero spacer should stay before submitted input in the timeline body"
    );
    assert!(
        key.body.iter().all(|line| !HANDWRITTEN_WELCOME_PHRASES
            .iter()
            .any(|phrase| line.text.contains(phrase))),
        "welcome hero must stay visual-only, not become a transcript line"
    );
    assert!(
        areas.len() >= 4,
        "submit should keep welcome timeline chrome instead of switching screens"
    );
    let composer_area = areas.first().expect("composer should prepare first");
    let body_area = areas
        .iter()
        .find(|area| area.bounds.top == PANEL_BODY_TOP_PADDING as i32)
        .expect("welcome body text area");
    assert_eq!(body_area.top, PANEL_BODY_TOP_PADDING);
    assert!(composer_area.top > body_area.top);
    assert!(composer_area.top >= fresh_welcome_draft_top(size));
    assert!(!vertices_have_color(&vertices, [0.060, 0.085, 0.145, 0.34]));
}

#[test]
fn session_attach_does_not_move_submitted_fresh_layout() {
    let size = PhysicalSize::new(1000, 720);
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello desktop".to_string()));
    assert!(matches!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession { .. }
    ));
    let mut font_system = FontSystem::new();
    let before_key = single_session_text_key(&app, size);
    let before_buffers = single_session_text_buffers_from_key(&before_key, size, &mut font_system);
    let before_areas = single_session_text_areas_for_app(&app, &before_buffers, size);

    app.replace_session(Some(workspace::SessionCard {
        session_id: "fresh_session".to_string(),
        title: "fresh session".to_string(),
        subtitle: "active".to_string(),
        detail: "1 msg".to_string(),
        preview_lines: Vec::new(),
        detail_lines: Vec::new(),
    }));

    let after_key = single_session_text_key(&app, size);
    let after_buffers = single_session_text_buffers_from_key(&after_key, size, &mut font_system);
    let after_areas = single_session_text_areas_for_app(&app, &after_buffers, size);

    assert_visual_text_contains(&after_key, &after_key.welcome_hero);
    assert_visual_text_contains(&after_key, "hello desktop");
    assert_eq!(
        before_areas.first().unwrap().top,
        after_areas.first().unwrap().top
    );
    assert_eq!(
        before_areas.last().unwrap().top,
        after_areas.last().unwrap().top
    );
}

#[test]
fn long_transcript_keeps_welcome_visual_only() {
    let size = PhysicalSize::new(900, 360);
    let mut app = SingleSessionApp::new(None);
    for index in 0..48 {
        app.messages
            .push(SingleSessionMessage::assistant(format!("message {index}")));
    }

    let bottom = single_session_visible_body(&app, size).join("\n");
    assert!(
        !HANDWRITTEN_WELCOME_PHRASES
            .iter()
            .any(|phrase| bottom.contains(phrase))
    );
    assert!(bottom.contains("message 47"));

    let metrics = single_session_body_scroll_metrics(&app, size, 0).expect("scroll metrics");
    app.scroll_body_lines(metrics.max_scroll_lines as i32);
    let top = single_session_visible_body(&app, size).join("\n");
    let key = single_session_text_key(&app, size);

    assert!(
        !HANDWRITTEN_WELCOME_PHRASES
            .iter()
            .any(|phrase| top.contains(phrase))
    );
    assert!(!top.contains("message 47"));
    assert_is_handwritten_welcome_phrase(&key.welcome_hero);
    assert_runtime_welcome_hero_available(&app, size);
}

#[test]
fn single_session_without_session_is_native_fresh_draft() {
    let mut app = SingleSessionApp::new(None);

    assert_eq!(app.status_title(), "Jcode · fresh session");
    assert!(!app.status_title().contains("Enter send"));
    assert!(!app.status_title().contains("Ctrl+"));
    assert_eq!(
        app.handle_key(KeyInput::SpawnPanel),
        KeyOutcome::SpawnSession
    );
    assert!(
        single_session_lines(None)
            .iter()
            .any(|line| line.contains("shared desktop session runtime"))
    );
    assert!(
        single_session_lines(None)
            .iter()
            .all(|line| !line.contains("execution is connected"))
    );
}

#[test]
fn fresh_single_session_keeps_welcome_model_and_hero_available() {
    let app = SingleSessionApp::new(None);
    let first = single_session_text_key_for_tick(&app, PhysicalSize::new(900, 700), 0);
    let later = single_session_text_key_for_tick(&app, PhysicalSize::new(900, 700), 42);
    let model_lines = app.body_styled_lines();

    assert!(
        model_lines.is_empty(),
        "fresh welcome greeting should be visual-only, not transcript body: {:?}",
        model_lines
    );
    assert_eq!(first.welcome_hero, later.welcome_hero);
    assert_is_handwritten_welcome_phrase(&first.welcome_hero);
    assert!(first.body.is_empty());
    assert_eq!(first.welcome_hint.len(), 1);
}

#[test]
fn welcome_name_is_optional_and_sanitized() {
    assert_eq!(
        sanitize_welcome_name("  Jeremy Huang  "),
        Some("Jeremy".to_string())
    );
    assert_eq!(sanitize_welcome_name("unknown"), None);
    assert_eq!(sanitize_welcome_name("   "), None);

    let named = welcome_styled_lines(&Some("Jeremy".to_string()), 0, 0);
    assert_eq!(named[0].text, "Welcome, Jeremy");
    let generic = welcome_styled_lines(&None, 0, 0);
    assert_eq!(generic[0].text, "Hello there");
}

#[test]
fn fresh_single_session_submit_requests_backend_session() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello desktop".to_string()));

    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::StartFreshSession {
            message: "hello desktop".to_string(),
            images: Vec::new()
        }
    );
    assert!(app.draft.is_empty());
}

#[test]
fn default_single_session_app_starts_without_attaching_recent_session() {
    let DesktopApp::SingleSession(mut app) = fresh_single_session_app() else {
        panic!("default desktop app should be single-session mode");
    };

    assert!(app.session.is_none());
    assert_eq!(
        app.handle_key(KeyInput::SpawnPanel),
        KeyOutcome::SpawnSession
    );
}

#[test]
fn desktop_mode_defaults_to_single_session_and_gates_workspace_prototype() {
    assert_eq!(
        desktop_mode_from_args(["jcode-desktop"]),
        DesktopMode::SingleSession
    );
    assert_eq!(
        desktop_mode_from_args(["jcode-desktop", "--workspace"]),
        DesktopMode::WorkspacePrototype
    );
}

#[test]
fn single_session_spawn_resets_to_fresh_native_draft() {
    let card = workspace::SessionCard {
        session_id: "session_alpha".to_string(),
        title: "alpha".to_string(),
        subtitle: "active".to_string(),
        detail: "3 msgs".to_string(),
        preview_lines: Vec::new(),
        detail_lines: Vec::new(),
    };
    let mut app = SingleSessionApp::new(Some(card));
    app.handle_key(KeyInput::Character("draft".to_string()));

    app.reset_fresh_session();

    assert!(app.session.is_none());
    assert!(app.draft.is_empty());
    assert_eq!(app.detail_scroll, 0);
    assert!(app.status_title().contains("fresh session"));
}

#[test]
fn single_session_wraps_one_session_card() {
    let card = workspace::SessionCard {
        session_id: "session_alpha".to_string(),
        title: "alpha".to_string(),
        subtitle: "active".to_string(),
        detail: "3 msgs".to_string(),
        preview_lines: vec!["user hello".to_string()],
        detail_lines: vec!["assistant hi".to_string()],
    };
    let mut app = SingleSessionApp::new(Some(card));

    assert_eq!(app.handle_key(KeyInput::Enter), KeyOutcome::Redraw);
    assert_eq!(app.draft, "\n");
    app.handle_key(KeyInput::Character("draft".to_string()));
    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::SendDraft {
            session_id: "session_alpha".to_string(),
            title: "alpha".to_string(),
            message: "draft".to_string(),
            images: Vec::new(),
        }
    );
}

#[test]
fn single_session_surface_is_the_panel_primitive() {
    let card = workspace::SessionCard {
        session_id: "session_alpha".to_string(),
        title: "alpha".to_string(),
        subtitle: "active".to_string(),
        detail: "3 msgs".to_string(),
        preview_lines: Vec::new(),
        detail_lines: Vec::new(),
    };

    let surface = single_session_surface(Some(&card));

    assert_eq!(surface.id, 1);
    assert_eq!(surface.title, "alpha");
    assert_eq!(surface.session_id.as_deref(), Some("session_alpha"));
    assert_eq!((surface.lane, surface.column), (0, 0));
    assert!(
        surface
            .body_lines
            .contains(&"single session mode".to_string())
    );
}

#[test]
fn focused_panel_draft_only_shows_for_focused_insert_panel() {
    let mut workspace = Workspace::from_session_cards(vec![workspace::SessionCard {
        session_id: "a".to_string(),
        title: "alpha".to_string(),
        subtitle: "active".to_string(),
        detail: "1 msg".to_string(),
        preview_lines: Vec::new(),
        detail_lines: Vec::new(),
    }]);
    workspace.handle_key(KeyInput::Character("i".to_string()));
    workspace.handle_key(KeyInput::Character("draft text".to_string()));
    workspace.attach_image("image/png".to_string(), "abc123".to_string());

    assert_eq!(
        focused_panel_draft(&workspace, workspace.focused_id),
        Some("draft text · 1 image".to_string())
    );
    assert_eq!(
        focused_panel_draft(&workspace, workspace.focused_id + 1),
        None
    );
}

#[test]
fn streaming_response_line_count_matches_wrapped_tail_lines() {
    let mut app = SingleSessionApp::new(None);
    app.messages
        .push(SingleSessionMessage::assistant("finished response"));
    app.streaming_response = format!(
        "short line\n{}\nwrapped words {}",
        "x".repeat(512),
        "word ".repeat(96)
    );
    let size = PhysicalSize::new(640, 800);

    let mut rendered_lines = Vec::new();
    append_single_session_streaming_response_rendered_body_lines(&app, size, &mut rendered_lines);

    assert_eq!(
        single_session_streaming_response_rendered_body_line_count(&app, size),
        rendered_lines.len()
    );
}

#[test]
fn rendered_body_cache_key_samples_large_transcript_middle() {
    let mut first = SingleSessionApp::new(None);
    first.messages = (0..64)
        .map(|index| SingleSessionMessage::assistant(format!("assistant message {index:02}")))
        .collect();
    let mut second = first.clone();
    second.messages[34] = SingleSessionMessage::assistant("assistant message XX");
    let size = (1280, 800);

    assert_ne!(
        first.rendered_body_cache_key(size),
        second.rendered_body_cache_key(size)
    );
    assert_ne!(
        first.rendered_body_static_cache_key(size),
        second.rendered_body_static_cache_key(size)
    );
}

#[test]
fn streaming_text_delta_batches_do_not_refresh_session_metadata() {
    let mut app = DesktopApp::SingleSession(SingleSessionApp::new(None));
    app.apply_session_event(session_launch::DesktopSessionEvent::SessionStarted {
        session_id: "session_live".to_string(),
    });

    let stats = apply_desktop_session_event_batch_with_stats(
        &mut app,
        vec![session_launch::DesktopSessionEvent::TextDelta(
            "streaming chunk".to_string(),
        )],
    );

    assert!(stats.visible_changed);
    assert_eq!(stats.text_delta_bytes, "streaming chunk".len());
    assert!(!stats.session_card_refresh_requested);
}

#[test]
fn terminal_session_events_request_async_session_metadata_refresh() {
    let mut app = DesktopApp::SingleSession(SingleSessionApp::new(None));
    app.apply_session_event(session_launch::DesktopSessionEvent::SessionStarted {
        session_id: "session_live".to_string(),
    });

    let stats = apply_desktop_session_event_batch_with_stats(
        &mut app,
        vec![session_launch::DesktopSessionEvent::Done],
    );

    assert!(stats.visible_changed);
    assert!(stats.session_card_refresh_requested);
}

#[test]
fn desktop_preferences_save_is_queued_off_ui_thread() {
    let workspace = Workspace::from_session_cards(vec![workspace::SessionCard {
        session_id: "session_pref".to_string(),
        title: "pref".to_string(),
        subtitle: "active".to_string(),
        detail: "1 msg".to_string(),
        preview_lines: Vec::new(),
        detail_lines: Vec::new(),
    }]);
    let expected = workspace.preferences();
    let (tx, rx) = mpsc::channel();

    queue_desktop_preferences_save(&workspace, &Some(tx));

    assert_eq!(rx.try_recv().ok(), Some(expected));
    assert!(rx.try_recv().is_err());
}
