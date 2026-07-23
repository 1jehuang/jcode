use super::animation::{
    APP_MODE_TRANSITION_DURATION, AnimatedRect, ColorTransition, DESKTOP_REDUCED_MOTION_ENV,
    DesktopReducedMotionEnvGuard, FOCUS_PULSE_DURATION, STATUS_COLOR_TRANSITION_DURATION,
    STATUS_TEXT_TRANSITION_DURATION, SURFACE_TRANSITION_DURATION, StatusTextTransition,
    SurfaceTransitionAnimator, SurfaceVisualFrame, SurfaceVisualTarget,
    VIEWPORT_ANIMATION_DURATION, desktop_reduced_motion_enabled,
    desktop_reduced_motion_enabled_for_env_value,
};
use super::single_session::*;
use super::*;
use std::sync::Mutex;

static DESKTOP_PREFS_ENV_LOCK: Mutex<()> = Mutex::new(());

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
fn desktop_reduced_motion_env_parses_common_flag_values() {
    assert!(!desktop_reduced_motion_enabled_for_env_value(None));
    assert!(!desktop_reduced_motion_enabled_for_env_value(Some(
        OsString::from("")
    )));
    assert!(!desktop_reduced_motion_enabled_for_env_value(Some(
        OsString::from("0")
    )));
    assert!(!desktop_reduced_motion_enabled_for_env_value(Some(
        OsString::from("false")
    )));
    assert!(!desktop_reduced_motion_enabled_for_env_value(Some(
        OsString::from("off")
    )));
    assert!(desktop_reduced_motion_enabled_for_env_value(Some(
        OsString::from("1")
    )));
    assert!(desktop_reduced_motion_enabled_for_env_value(Some(
        OsString::from("true")
    )));
    assert!(desktop_reduced_motion_enabled_for_env_value(Some(
        OsString::from("reduce")
    )));
    assert_eq!(DESKTOP_REDUCED_MOTION_ENV, "JCODE_DESKTOP_REDUCED_MOTION");
}

#[test]
fn desktop_process_role_parses_internal_flags() {
    assert_eq!(
        desktop_process_role_from_args(["jcode-desktop"].into_iter()),
        DesktopProcessRole::StableHost
    );
    assert_eq!(
        desktop_process_role_from_args(["jcode-desktop", "--desktop-host"].into_iter()),
        DesktopProcessRole::StableHost
    );
    assert_eq!(
        desktop_process_role_from_args(["jcode-desktop", "--desktop-app-worker"].into_iter()),
        DesktopProcessRole::AppWorker
    );
    assert_eq!(
        desktop_process_role_from_args(
            ["jcode-desktop", "--desktop-process-role=stable_host"].into_iter()
        ),
        DesktopProcessRole::StableHost
    );
    assert_eq!(
        desktop_process_role_from_args(
            ["jcode-desktop", "--desktop-process-role", "app-worker"].into_iter()
        ),
        DesktopProcessRole::AppWorker
    );
    assert_eq!(
        DesktopProcessRole::Standalone.reload_strategy(),
        DesktopReloadStrategy::FullProcessHandoff
    );
    assert_eq!(
        DesktopProcessRole::StableHost.reload_strategy(),
        DesktopReloadStrategy::AppWorkerRestart
    );
    assert_eq!(
        DesktopProcessRole::AppWorker.reload_strategy(),
        DesktopReloadStrategy::FullProcessHandoff
    );
}

#[test]
fn desktop_modes_map_to_worker_modes() {
    assert_eq!(
        DesktopMode::SingleSession.worker_mode(),
        DesktopWorkerMode::SingleSession
    );
    assert_eq!(
        DesktopMode::WorkspacePrototype.worker_mode(),
        DesktopWorkerMode::Workspace
    );
}

#[test]
fn desktop_worker_init_builds_initial_scene_from_snapshot_and_window() {
    let init = DesktopWorkerInit {
        mode: DesktopWorkerMode::Workspace,
        snapshot: Some(DesktopUiSnapshot::new(
            "workspace",
            "Workspace Title".to_string(),
            None,
            DesktopSurfaceSnapshot::Workspace(DesktopWorkspaceSnapshot {
                input_mode: "normal".to_string(),
                focused_surface_id: 1,
                focused_session_id: None,
                zoomed: false,
                detail_scroll: 0,
                draft: String::new(),
                draft_cursor: 0,
                pending_image_count: 0,
                surfaces: Vec::new(),
            }),
        )),
        window: DesktopWindowState {
            width: 800,
            height: 600,
            scale_factor: 1.5,
            focused: true,
        },
    };

    let scene = desktop_scene_for_worker_init(&init);
    assert_eq!(scene.viewport.size.width, 800.0);
    assert_eq!(scene.viewport.size.height, 600.0);
    assert_eq!(scene.viewport.scale_factor, 1.5);
    assert_eq!(scene.metadata.title.as_deref(), Some("Workspace Title"));
    assert!(scene.metadata.content_ready);
    assert!(matches!(
        scene.display_list.commands.as_slice(),
        [DesktopDisplayCommand::Clear(_)]
    ));
}

#[test]
fn desktop_key_events_preserve_text_and_modifiers_for_worker_input() {
    let key = desktop_key_event_from_winit(
        &Key::Character("x".into()),
        ModifiersState::SHIFT | ModifiersState::CONTROL,
        true,
    );
    assert_eq!(key.key, "x");
    assert_eq!(key.text.as_deref(), Some("x"));
    assert!(key.pressed);
    assert!(key.modifiers.shift);
    assert!(key.modifiers.ctrl);
    assert!(!key.modifiers.alt);
    assert!(!key.modifiers.super_key);

    let enter =
        desktop_key_event_from_winit(&Key::Named(NamedKey::Enter), ModifiersState::empty(), true);
    assert_eq!(enter.key, "Enter");
    assert_eq!(enter.text, None);
}

#[test]
fn desktop_mouse_wheel_events_convert_line_and_pixel_delta() {
    assert_eq!(
        desktop_mouse_wheel_event(MouseScrollDelta::LineDelta(1.0, -2.0)),
        DesktopMouseEvent::Wheel {
            delta_x: 1.0,
            delta_y: -2.0,
        }
    );
    assert_eq!(
        desktop_mouse_wheel_event(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
            3.0, -4.0
        ))),
        DesktopMouseEvent::Wheel {
            delta_x: 3.0,
            delta_y: -4.0,
        }
    );
}

#[test]
fn desktop_session_events_convert_to_worker_wire_events() {
    assert_eq!(
        desktop_session_event_to_wire(&session_launch::DesktopSessionEvent::Status(
            DesktopSessionStatus::external("thinking"),
        )),
        DesktopSessionEventWire::Status {
            message: "thinking".to_string(),
        }
    );
    assert_eq!(
        desktop_session_event_to_wire(&session_launch::DesktopSessionEvent::TextDelta(
            "hello".to_string(),
        )),
        DesktopSessionEventWire::AssistantTextDelta {
            text: "hello".to_string(),
        }
    );
    assert_eq!(
        desktop_session_event_to_wire(&session_launch::DesktopSessionEvent::Done),
        DesktopSessionEventWire::RawJson {
            event_type: "done".to_string(),
            payload: "Done".to_string(),
        }
    );
}

#[test]
fn desktop_app_worker_relaunch_replaces_existing_process_role() {
    let relaunch = DesktopRelaunch {
        binary: PathBuf::from("/tmp/jcode-desktop"),
        args: vec![
            OsString::from("--workspace"),
            OsString::from("--desktop-process-role=stable_host"),
            OsString::from("--foo"),
        ],
    };

    assert_eq!(
        relaunch.for_app_worker().args,
        vec![
            OsString::from("--workspace"),
            OsString::from("--foo"),
            OsString::from("--desktop-process-role"),
            OsString::from("app-worker"),
        ]
    );

    let relaunch = DesktopRelaunch {
        binary: PathBuf::from("/tmp/jcode-desktop"),
        args: vec![
            OsString::from("--desktop-process-role"),
            OsString::from("stable-host"),
            OsString::from("--desktop-app-worker"),
            OsString::from("--new"),
        ],
    };
    assert_eq!(
        relaunch.for_app_worker().args,
        vec![
            OsString::from("--new"),
            OsString::from("--desktop-process-role"),
            OsString::from("app-worker"),
        ]
    );
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
fn desktop_scene_vertices_render_rectangles_and_clear_color() {
    let mut scene = DesktopScene::default();
    scene.push(DesktopDisplayCommand::Clear(DesktopColor::rgba(
        0.1, 0.2, 0.3, 1.0,
    )));
    scene.push(DesktopDisplayCommand::Rect(DesktopRectPaint::filled(
        DesktopSceneRect::new(10.0, 20.0, 30.0, 40.0),
        DesktopColor::rgba(0.8, 0.7, 0.6, 1.0),
    )));

    let mut vertices = Vec::new();
    let clear = desktop_scene_vertices(&scene, PhysicalSize::new(100, 100), &mut vertices)
        .expect("clear color");

    assert!((clear.r - 0.1).abs() < 0.000_001);
    assert!((clear.g - 0.2).abs() < 0.000_001);
    assert!((clear.b - 0.3).abs() < 0.000_001);
    assert_eq!(clear.a, 1.0);
    assert_eq!(vertices.len(), 6);
    assert!(
        vertices
            .iter()
            .all(|vertex| vertex.color == [0.8, 0.7, 0.6, 1.0])
    );
}

#[test]
fn desktop_scene_clear_resets_previous_vertices() {
    let mut scene = DesktopScene::default();
    scene.push(DesktopDisplayCommand::Rect(DesktopRectPaint::filled(
        DesktopSceneRect::new(0.0, 0.0, 10.0, 10.0),
        DesktopColor::rgba(1.0, 0.0, 0.0, 1.0),
    )));
    scene.push(DesktopDisplayCommand::Clear(DesktopColor::rgba(
        0.0, 0.0, 0.0, 1.0,
    )));

    let mut vertices = Vec::new();
    desktop_scene_vertices(&scene, PhysicalSize::new(100, 100), &mut vertices);

    assert!(vertices.is_empty());
}

#[test]
fn desktop_app_build_scene_adds_metadata_and_default_clear() {
    let app = DesktopApp::SingleSession(SingleSessionApp::new(None));
    let scene = app.build_scene(DesktopSceneBuildContext::new(DesktopScene::default()));

    assert_eq!(scene.metadata.title, Some(app.status_title()));
    assert!(scene.metadata.content_ready);
    assert_eq!(scene.display_list.commands.len(), 1);
    assert!(matches!(
        scene.display_list.commands.first(),
        Some(DesktopDisplayCommand::Clear(_))
    ));
}

#[test]
fn desktop_app_build_scene_preserves_existing_display_list() {
    let app = DesktopApp::SingleSession(SingleSessionApp::new(None));
    let mut scene = DesktopScene::default();
    scene.push(DesktopDisplayCommand::Rect(DesktopRectPaint::filled(
        DesktopSceneRect::new(1.0, 2.0, 3.0, 4.0),
        DesktopColor::rgba(0.2, 0.3, 0.4, 1.0),
    )));

    let scene = app.build_scene(DesktopSceneBuildContext::new(scene));

    assert_eq!(scene.display_list.commands.len(), 1);
    assert!(matches!(
        scene.display_list.commands.first(),
        Some(DesktopDisplayCommand::Rect(_))
    ));
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
    let Ok(_guard) = DESKTOP_PREFS_ENV_LOCK.lock() else {
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
            transcript_messages: Vec::new(),
        },
        workspace::SessionCard {
            session_id: "session-b".to_string(),
            title: "bravo".to_string(),
            subtitle: "active".to_string(),
            detail: "2 messages".to_string(),
            preview_lines: vec![],
            detail_lines: vec![],
            transcript_messages: Vec::new(),
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
fn desktop_hot_reload_restarts_default_launched_workspace_as_workspace() -> Result<()> {
    let Ok(_guard) = DESKTOP_PREFS_ENV_LOCK.lock() else {
        anyhow::bail!("desktop hot reload env lock poisoned");
    };
    let temp = unique_desktop_test_dir("desktop-hot-reload-default-workspace")?;
    let state_path = temp.join("desktop-state.json");
    unsafe {
        std::env::set_var("JCODE_DESKTOP_STATE", &state_path);
    }
    let relaunch = DesktopRelaunch {
        binary: PathBuf::from("/old/jcode-desktop"),
        args: Vec::new(),
    };
    let app = DesktopApp::Workspace(Workspace::from_session_cards(vec![
        workspace::SessionCard {
            session_id: "session-visible".to_string(),
            title: "visible session".to_string(),
            subtitle: "active".to_string(),
            detail: "already on screen".to_string(),
            preview_lines: vec!["same content".to_string()],
            detail_lines: vec![],
            transcript_messages: Vec::new(),
        },
    ]));

    let updated = relaunch.for_app(&app, PathBuf::from("/new/jcode-desktop"));

    assert_eq!(updated.binary, PathBuf::from("/new/jcode-desktop"));
    assert_eq!(updated.args, vec![OsString::from("--workspace")]);

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

#[test]
fn desktop_reload_window_placement_roundtrips_position_and_size() {
    let placement = DesktopReloadWindowPlacement {
        position: Some(PhysicalPosition::new(-24, 48)),
        inner_size: PhysicalSize::new(1280, 800),
    };

    let encoded = placement.to_env_value();

    assert_eq!(encoded, "-24,48,1280,800");
    assert_eq!(
        DesktopReloadWindowPlacement::from_env_value(&encoded),
        Some(placement)
    );
}

#[test]
fn desktop_reload_window_placement_allows_size_without_position() {
    let placement = DesktopReloadWindowPlacement {
        position: None,
        inner_size: PhysicalSize::new(1024, 720),
    };

    let encoded = placement.to_env_value();

    assert_eq!(encoded, "_,_,1024,720");
    assert_eq!(
        DesktopReloadWindowPlacement::from_env_value(&encoded),
        Some(placement)
    );
}

#[test]
fn desktop_reload_window_placement_rejects_invalid_values() {
    for raw in [
        "",
        "1,2,3",
        "1,2,3,4,5",
        "_,2,1280,800",
        "1,_,1280,800",
        "x,2,1280,800",
        "1,y,1280,800",
        "1,2,0,800",
        "1,2,1280,0",
        "1,2,32769,800",
        "1,2,1280,32769",
        "1,2,width,800",
        "1,2,1280,height",
    ] {
        assert_eq!(
            DesktopReloadWindowPlacement::from_env_value(raw),
            None,
            "expected {raw:?} to be rejected"
        );
    }
}

#[test]
fn desktop_reload_window_placement_roundtrips_through_handoff_file() -> Result<()> {
    let dir = unique_desktop_test_dir("desktop-reload-placement")?;
    let placement_file = dir.join("placement");
    let placement = DesktopReloadWindowPlacement {
        position: Some(PhysicalPosition::new(320, 180)),
        inner_size: PhysicalSize::new(1440, 900),
    };

    write_desktop_reload_window_placement(&placement_file, placement)?;

    assert_eq!(
        read_desktop_reload_window_placement(&placement_file),
        Some(placement)
    );
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn desktop_reload_handoff_watcher_releases_ready_child() -> Result<()> {
    let dir = desktop_reload_handoff_temp_dir();
    std::fs::create_dir_all(&dir)?;
    let ready_file = dir.join("ready");
    let release_file = dir.join("release");
    let placement_file = dir.join("placement");
    let watcher = DesktopReloadHandoffWatcher {
        ready_file: ready_file.clone(),
        release_file: release_file.clone(),
        placement_file,
        spawned_at: Instant::now(),
    };

    assert_eq!(watcher.poll()?, DesktopReloadHandoffPoll::Waiting);
    std::fs::write(&ready_file, b"ready")?;
    let final_placement = DesktopReloadWindowPlacement {
        position: Some(PhysicalPosition::new(640, 360)),
        inner_size: PhysicalSize::new(1600, 1000),
    };

    assert_eq!(
        watcher.poll_with_placement(Some(final_placement))?,
        DesktopReloadHandoffPoll::Ready
    );
    assert!(release_file.exists());
    assert_eq!(
        read_desktop_reload_window_placement(&watcher.placement_file),
        Some(final_placement)
    );

    watcher.cleanup();
    assert!(!dir.exists());
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
        transcript_messages: Vec::new(),
    };
    let second_card = workspace::SessionCard {
        session_id: "b".to_string(),
        title: "beta".to_string(),
        subtitle: "idle".to_string(),
        detail: "2 msgs".to_string(),
        preview_lines: Vec::new(),
        detail_lines: Vec::new(),
        transcript_messages: Vec::new(),
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
fn automatic_redraw_requires_a_visible_renderable_surface_and_elapsed_timeout() {
    let now = Instant::now();
    let timeout_deadline = now + SURFACE_TIMEOUT_BACKOFF_MIN;

    assert!(desktop_automatic_redraw_allowed(now, true, false, None));
    assert!(!desktop_automatic_redraw_allowed(now, false, false, None));
    assert!(!desktop_automatic_redraw_allowed(now, true, true, None));
    assert!(!desktop_automatic_redraw_allowed(
        now,
        true,
        false,
        Some(timeout_deadline),
    ));
    assert!(desktop_automatic_redraw_allowed(
        timeout_deadline,
        true,
        false,
        Some(timeout_deadline),
    ));
}

#[test]
fn surface_timeout_wake_parks_while_unrenderable_or_occluded() {
    let redraw_at = Instant::now() + SURFACE_TIMEOUT_BACKOFF_MIN;

    assert_eq!(
        desktop_surface_timeout_wake(true, false, Some(redraw_at)),
        Some(redraw_at)
    );
    assert_eq!(
        desktop_surface_timeout_wake(false, false, Some(redraw_at)),
        None
    );
    assert_eq!(
        desktop_surface_timeout_wake(true, true, Some(redraw_at)),
        None
    );
    assert_eq!(desktop_surface_timeout_wake(true, false, None), None);
}

#[test]
fn automatic_redraw_state_defers_and_coalesces_until_rendering_is_allowed() {
    let mut redraw = AutomaticRedrawState::default();

    assert!(!redraw.schedule(false));
    assert!(redraw.pending);
    assert!(!redraw.request_outstanding);
    assert!(!redraw.request_pending(false));

    assert!(redraw.request_pending(true));
    assert!(redraw.request_outstanding);
    assert!(
        !redraw.schedule(true),
        "an outstanding redraw must coalesce"
    );

    redraw.park();
    assert!(redraw.pending, "parking must preserve pending work");
    assert!(
        !redraw.request_outstanding,
        "parking must forget a request the platform may drop"
    );
    assert!(redraw.request_pending(true));

    redraw.begin_redraw(false);
    assert!(redraw.pending, "a gated redraw must preserve pending work");
    assert!(!redraw.request_outstanding);
    assert!(redraw.request_pending(true));

    redraw.begin_redraw(true);
    assert!(!redraw.pending, "a renderable frame consumes pending work");
    assert!(!redraw.request_outstanding);
    assert!(!redraw.request_pending(true));
}

#[test]
fn surface_timeout_gate_defers_every_automatic_request_until_deadline() {
    let now = Instant::now();
    let redraw_at = now + SURFACE_TIMEOUT_BACKOFF_MIN;
    let mut redraw = AutomaticRedrawState::default();

    let allowed_before_deadline =
        desktop_automatic_redraw_allowed(now, true, false, Some(redraw_at));
    assert!(!redraw.schedule(allowed_before_deadline));
    assert!(redraw.pending);

    let allowed_at_deadline =
        desktop_automatic_redraw_allowed(redraw_at, true, false, Some(redraw_at));
    assert!(redraw.request_pending(allowed_at_deadline));
}

#[test]
fn pending_backend_redraw_survives_zero_size_until_a_renderable_frame() {
    let requested_at = Instant::now();
    let mut pending = Some(requested_at);

    assert_eq!(
        consume_pending_backend_redraw_for_render(false, &mut pending),
        None
    );
    assert_eq!(pending, Some(requested_at));
    assert_eq!(
        consume_pending_backend_redraw_for_render(true, &mut pending),
        Some(requested_at)
    );
    assert_eq!(pending, None);
}

#[test]
fn next_animation_redraw_paces_frame_scroll_and_space_hold_at_sixteen_ms() {
    let now = Instant::now();
    let expected = Some(now + DESKTOP_ANIMATION_FRAME_INTERVAL);

    assert_eq!(
        next_animation_redraw_at(now, true, false, false, true),
        expected
    );
    assert_eq!(
        next_animation_redraw_at(now, false, true, false, true),
        expected
    );
    assert_eq!(
        next_animation_redraw_at(now, false, false, true, true),
        expected
    );
    // Once the animation settles, no further redraw is scheduled and the loop
    // can park on ControlFlow::Wait.
    assert_eq!(
        next_animation_redraw_at(now, false, false, false, true),
        None
    );
    // Occlusion and surface timeout gating suppress every automatic animation
    // source until automatic redraws become eligible again.
    assert_eq!(next_animation_redraw_at(now, true, true, true, false), None);
    // The pacing interval must be positive; a zero interval would reintroduce
    // the busy-spin it exists to prevent.
    assert_eq!(DESKTOP_ANIMATION_FRAME_INTERVAL, Duration::from_millis(16));
    assert!(DESKTOP_ANIMATION_FRAME_INTERVAL > Duration::ZERO);
}

#[test]
fn hero_reveal_worker_count_falls_back_to_serial_for_small_images() {
    // Tiny images should not pay thread-spawn overhead.
    assert_eq!(hero_reveal_worker_count(0), 1);
    assert_eq!(hero_reveal_worker_count(1024), 1);
    // Large images should use more than one worker when parallelism is available.
    let big = hero_reveal_worker_count(8 * 1024 * 1024);
    let available = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1);
    assert!(big >= 1);
    assert!(big <= available.max(1));
}

#[test]
fn fill_hero_reveal_values_matches_serial_reference() {
    let width = 64_u32;
    let height = 48_u32;
    let alpha_bounds = HeroMaskPixelBounds {
        min_x: 4,
        min_y: 4,
        max_x: width - 4,
        max_y: height - 4,
    };
    // A handful of normalized stroke segments tracing a rough path.
    let segments = vec![
        WelcomeHeroStrokeSegment {
            start: [0.1, 0.2],
            end: [0.4, 0.5],
            start_progress: 0.0,
            end_progress: 0.4,
        },
        WelcomeHeroStrokeSegment {
            start: [0.4, 0.5],
            end: [0.8, 0.3],
            start_progress: 0.4,
            end_progress: 0.8,
        },
        WelcomeHeroStrokeSegment {
            start: [0.8, 0.3],
            end: [0.9, 0.9],
            start_progress: 0.8,
            end_progress: 1.0,
        },
    ];
    // Mark a checkerboard of lit pixels so both branches exercise lit/unlit.
    let mut glyph_rgba = vec![0_u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            if (x + y) % 3 == 0 {
                let index = ((y * width + x) * 4) as usize;
                glyph_rgba[index] = 200;
            }
        }
    }
    let brush_delay_px = (alpha_bounds.height() * 0.10).max(5.0);

    // Serial reference computed directly here.
    let mut expected = vec![1.0_f32; (width * height) as usize];
    let mut expected_min = f32::INFINITY;
    let mut expected_max = 0.0_f32;
    for y in 0..height {
        for x in 0..width {
            let pixel_index = (y * width + x) as usize;
            if glyph_rgba[pixel_index * 4] <= 2 {
                continue;
            }
            let (path_progress, distance) = nearest_hero_stroke_progress(
                x as f32 + 0.5,
                y as f32 + 0.5,
                alpha_bounds,
                &segments,
            );
            let width_delay = (distance / brush_delay_px).min(1.0) * 0.045;
            let value = (path_progress + width_delay).clamp(0.0, 1.0);
            expected[pixel_index] = value;
            expected_min = expected_min.min(value);
            expected_max = expected_max.max(value);
        }
    }

    // The parallel implementation must produce bit-identical output regardless
    // of how many worker threads it chose.
    let mut actual = vec![1.0_f32; (width * height) as usize];
    let (actual_min, actual_max) = fill_hero_reveal_values(
        &mut actual,
        width,
        height,
        &glyph_rgba,
        alpha_bounds,
        &segments,
        brush_delay_px,
    );

    assert_eq!(
        actual, expected,
        "parallel hero reveal fill must match serial"
    );
    assert_eq!(actual_min.to_bits(), expected_min.to_bits());
    assert_eq!(actual_max.to_bits(), expected_max.to_bits());
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
fn workspace_status_text_transition_animates_mode_changes() {
    let mut workspace = Workspace::fake();
    let mut transition = StatusTextTransition::default();
    let now = Instant::now();
    let nav_text = workspace_status_text(&workspace);

    let first = transition.frame(nav_text.clone(), now);
    assert!(!first.is_active());
    assert_eq!(first.current.text, nav_text);
    assert_eq!(first.current.opacity, 1.0);
    assert!(first.previous.is_none());

    workspace.mode = InputMode::Insert;
    let insert_text = workspace_status_text(&workspace);
    let start = transition.frame(insert_text.clone(), now);
    assert!(start.is_active());
    assert_eq!(start.current.text, insert_text);
    assert_eq!(start.current.opacity, 0.0);
    assert_eq!(start.previous.as_ref().unwrap().text, nav_text);
    assert_eq!(start.previous.as_ref().unwrap().opacity, 1.0);

    let middle = transition.frame(
        insert_text.clone(),
        now + STATUS_TEXT_TRANSITION_DURATION / 2,
    );
    assert!(middle.is_active());
    assert!(middle.current.opacity > 0.0 && middle.current.opacity < 1.0);
    let previous = middle.previous.as_ref().expect("previous status text");
    assert!(previous.opacity > 0.0 && previous.opacity < 1.0);
    assert!(middle.current.y_offset_pixels > 0.0);
    assert!(previous.y_offset_pixels < 0.0);

    let settled = transition.frame(
        insert_text.clone(),
        now + STATUS_TEXT_TRANSITION_DURATION * 2,
    );
    assert!(!settled.is_active());
    assert_eq!(settled.current.text, insert_text);
    assert_eq!(settled.current.opacity, 1.0);
    assert!(settled.previous.is_none());
}

#[test]
fn workspace_status_text_transition_draws_previous_and_current_labels() {
    let mut workspace = Workspace::fake();
    let mut transition = StatusTextTransition::default();
    let now = Instant::now();
    transition.frame(workspace_status_text(&workspace), now);
    workspace.mode = InputMode::Insert;
    let target = workspace_status_text(&workspace);
    transition.frame(target.clone(), now);
    let middle = transition.frame(target, now + STATUS_TEXT_TRANSITION_DURATION / 2);

    let size = PhysicalSize::new(900, 600);
    let status_rect = Rect {
        x: OUTER_PADDING,
        y: OUTER_PADDING,
        width: size.width as f32 - OUTER_PADDING * 2.0,
        height: STATUS_BAR_HEIGHT,
    };
    let mut settled_vertices = Vec::new();
    push_status_text(&mut settled_vertices, &workspace, status_rect, size, None);
    let settled_text_vertex_count = settled_vertices
        .iter()
        .filter(|vertex| vertex.color[..3] == STATUS_TEXT_COLOR[..3])
        .count();

    let mut animated_vertices = Vec::new();
    push_status_text(
        &mut animated_vertices,
        &workspace,
        status_rect,
        size,
        Some(&middle),
    );
    let animated_text_vertices = animated_vertices
        .iter()
        .filter(|vertex| vertex.color[..3] == STATUS_TEXT_COLOR[..3])
        .collect::<Vec<_>>();

    assert!(
        animated_text_vertices.len() > settled_text_vertex_count,
        "transition should draw outgoing and incoming status labels"
    );
    assert!(
        animated_text_vertices
            .iter()
            .any(|vertex| { vertex.color[3] > 0.0 && vertex.color[3] < STATUS_TEXT_COLOR[3] })
    );
}

#[test]
fn workspace_status_preview_viewport_uses_animated_scroll_offset() {
    let workspace = Workspace::fake();
    let size = PhysicalSize::new(900, 600);
    let status_rect = Rect {
        x: OUTER_PADDING,
        y: OUTER_PADDING,
        width: size.width as f32 - OUTER_PADDING * 2.0,
        height: STATUS_BAR_HEIGHT,
    };
    let visible = VisibleColumnLayout {
        visible_columns: 1,
        first_visible_column: 3,
    };
    let column_width = 100.0;
    let column_pitch = column_width + GAP;

    let mut target_vertices = Vec::new();
    push_status_preview(
        &mut target_vertices,
        &workspace,
        workspace.current_workspace(),
        WorkspaceRenderLayout {
            visible,
            column_width,
            scroll_offset: visible.first_visible_column as f32 * column_pitch,
            vertical_scroll_offset: 0.0,
        },
        None,
        &HashMap::new(),
        0.0,
        status_rect,
        size,
    );
    let target_viewport =
        pixel_bounds_for_color(&target_vertices, STATUS_PREVIEW_VIEWPORT_COLOR, size)
            .expect("settled status preview should draw a viewport outline");

    let mut animated_vertices = Vec::new();
    push_status_preview(
        &mut animated_vertices,
        &workspace,
        workspace.current_workspace(),
        WorkspaceRenderLayout {
            visible,
            column_width,
            scroll_offset: 1.5 * column_pitch,
            vertical_scroll_offset: 0.0,
        },
        None,
        &HashMap::new(),
        0.0,
        status_rect,
        size,
    );
    let animated_viewport =
        pixel_bounds_for_color(&animated_vertices, STATUS_PREVIEW_VIEWPORT_COLOR, size)
            .expect("animated status preview should draw a viewport outline");

    assert!(
        animated_viewport.min_x < target_viewport.min_x - 8.0,
        "status preview viewport should track the animated scroll offset instead of jumping to the target column: animated={animated_viewport:?}, target={target_viewport:?}"
    );
}

#[test]
fn workspace_status_preview_focused_tick_uses_focus_pulse() {
    let workspace = Workspace::fake();
    let size = PhysicalSize::new(900, 600);
    let status_rect = Rect {
        x: OUTER_PADDING,
        y: OUTER_PADDING,
        width: size.width as f32 - OUTER_PADDING * 2.0,
        height: STATUS_BAR_HEIGHT,
    };
    let render_layout = workspace_render_layout(&workspace, size, None);
    let focused_color = status_preview_surface_color(0, true, true);

    let mut idle_vertices = Vec::new();
    push_status_preview(
        &mut idle_vertices,
        &workspace,
        workspace.current_workspace(),
        render_layout,
        None,
        &HashMap::new(),
        0.0,
        status_rect,
        size,
    );
    let idle_bounds = pixel_bounds_for_color(&idle_vertices, focused_color, size)
        .expect("focused preview tick should be visible without a pulse");

    let mut pulsed_vertices = Vec::new();
    push_status_preview(
        &mut pulsed_vertices,
        &workspace,
        workspace.current_workspace(),
        render_layout,
        None,
        &HashMap::new(),
        1.0,
        status_rect,
        size,
    );
    let pulsed_bounds = pixel_bounds_for_color(&pulsed_vertices, focused_color, size)
        .expect("focused preview tick should be visible with a pulse");

    assert!(
        pulsed_bounds.max_x - pulsed_bounds.min_x > idle_bounds.max_x - idle_bounds.min_x + 1.0,
        "focus pulse should widen the focused minimap tick: idle={idle_bounds:?}, pulsed={pulsed_bounds:?}"
    );
    assert!(
        pulsed_bounds.max_y - pulsed_bounds.min_y > idle_bounds.max_y - idle_bounds.min_y + 1.0,
        "focus pulse should thicken the focused minimap tick: idle={idle_bounds:?}, pulsed={pulsed_bounds:?}"
    );
}

#[test]
fn workspace_status_preview_draws_exiting_surface_with_fade() {
    let mut workspace = Workspace::fake();
    let size = PhysicalSize::new(900, 600);
    let render_layout = workspace_render_layout(&workspace, size, None);
    let mut transitions = SurfaceTransitionAnimator::default();
    let now = Instant::now();
    let initial_targets = workspace_surface_transition_targets(&workspace, size, render_layout);
    transitions.frame(initial_targets, now);

    let removed = workspace
        .surfaces
        .iter()
        .find(|surface| surface.id == 2)
        .expect("fake workspace surface")
        .clone();
    workspace
        .surfaces
        .retain(|surface| surface.id != removed.id);
    let mut exit_cache = HashMap::new();
    exit_cache.insert(removed.id, removed.clone());
    let updated_layout = workspace_render_layout(&workspace, size, None);
    let updated_targets = workspace_surface_transition_targets(&workspace, size, updated_layout);
    transitions.frame(updated_targets.clone(), now + Duration::from_millis(8));
    let frames = transitions.frame(
        updated_targets,
        now + Duration::from_millis(8) + SURFACE_TRANSITION_DURATION / 2,
    );
    let frames = WorkspaceSurfaceTransitionFrames::new(frames, transitions.is_animating());

    let status_rect = Rect {
        x: OUTER_PADDING,
        y: OUTER_PADDING,
        width: size.width as f32 - OUTER_PADDING * 2.0,
        height: STATUS_BAR_HEIGHT,
    };
    let mut vertices = Vec::new();
    push_status_preview(
        &mut vertices,
        &workspace,
        workspace.current_workspace(),
        updated_layout,
        Some(&frames),
        &exit_cache,
        0.0,
        status_rect,
        size,
    );

    let removed_accent = STATUS_PREVIEW_ACCENTS[removed.color_index % STATUS_PREVIEW_ACCENTS.len()];
    let faded_removed_vertices = vertices
        .iter()
        .filter(|vertex| vertex.color[..3] == removed_accent)
        .filter(|vertex| vertex.color[3] > 0.0 && vertex.color[3] < 0.72)
        .count();

    assert!(
        faded_removed_vertices > 0,
        "status preview should keep removed surfaces visible as fading minimap ticks during surface exit animation"
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
fn reduced_motion_snaps_viewport_to_new_layout_target() {
    let _guard = DesktopReducedMotionEnvGuard::set(true);
    assert!(desktop_reduced_motion_enabled());

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

    assert_eq!(animation.frame(start, now).column_width, 200.0);
    let snapped = animation.frame(target, now);
    assert_eq!(snapped.column_width, 300.0);
    assert_eq!(snapped.scroll_offset, 600.0);
    assert_eq!(snapped.vertical_scroll_offset, 800.0);
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
fn reduced_motion_disables_focus_pulse() {
    let _guard = DesktopReducedMotionEnvGuard::set(true);
    let mut pulse = FocusPulse::default();
    let now = Instant::now();

    assert_eq!(pulse.frame(1, now), 0.0);
    assert_eq!(pulse.frame(2, now), 0.0);
    assert!(!pulse.is_animating());
}

#[test]
fn app_mode_transition_crossfades_and_scales_previous_and_current_vertices() {
    let mut transition = AppModeTransitionState::default();
    let now = Instant::now();
    let previous_vertex = Vertex {
        position: [0.5, -0.25],
        color: [0.1, 0.2, 0.3, 0.8],
    };
    let current_vertex = Vertex {
        position: [-0.25, 0.5],
        color: [0.6, 0.7, 0.8, 0.4],
    };

    assert!(transition.frame("workspace", now).is_none());
    transition.remember_uploaded_vertices(&[previous_vertex]);

    let start = transition
        .frame("single_session", now)
        .expect("mode change should start a transition");
    assert_eq!(start.previous_opacity, 1.0);
    assert_eq!(start.current_opacity, 0.0);
    assert_eq!(transition.previous_vertices().len(), 1);

    let middle = transition
        .frame("single_session", now + APP_MODE_TRANSITION_DURATION / 2)
        .expect("transition should still be active halfway through");
    assert!(middle.previous_opacity > 0.0 && middle.previous_opacity < 1.0);
    assert!(middle.current_opacity > 0.0 && middle.current_opacity < 1.0);
    assert!(middle.previous_scale < 1.0);
    assert!(middle.current_scale > 0.985 && middle.current_scale <= 1.0);

    let mut composed = Vec::new();
    compose_app_mode_transition_vertices(
        &mut composed,
        &[previous_vertex],
        &[current_vertex],
        middle,
    );
    assert_eq!(composed.len(), 2);
    assert!(
        (composed[0].position[0] - previous_vertex.position[0] * middle.previous_scale).abs()
            < 0.000_1
    );
    assert!(
        (composed[0].color[3] - previous_vertex.color[3] * middle.previous_opacity).abs() < 0.000_1
    );
    assert!(
        (composed[1].position[1] - current_vertex.position[1] * middle.current_scale).abs()
            < 0.000_1
    );
    assert!(
        (composed[1].color[3] - current_vertex.color[3] * middle.current_opacity).abs() < 0.000_1
    );

    assert!(
        transition
            .frame("single_session", now + APP_MODE_TRANSITION_DURATION * 2)
            .is_none()
    );
    assert!(transition.previous_vertices().is_empty());
}

#[test]
fn reduced_motion_disables_app_mode_transition() {
    let _guard = DesktopReducedMotionEnvGuard::set(true);
    let mut transition = AppModeTransitionState::default();
    let now = Instant::now();
    transition.frame("workspace", now);
    transition.remember_uploaded_vertices(&[Vertex {
        position: [0.0, 0.0],
        color: [1.0, 1.0, 1.0, 1.0],
    }]);

    assert!(transition.frame("single_session", now).is_none());
    assert!(transition.previous_vertices().is_empty());
}

#[test]
fn surface_transition_animates_panel_reposition_and_entry() {
    let mut transitions = SurfaceTransitionAnimator::default();
    let now = Instant::now();
    let original = SurfaceVisualTarget {
        id: 1,
        rect: AnimatedRect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 120.0,
        },
    };
    let moved = SurfaceVisualTarget {
        id: 1,
        rect: AnimatedRect {
            x: 120.0,
            y: 16.0,
            width: 180.0,
            height: 220.0,
        },
    };

    let first = transitions.frame([original], now);
    let first = surface_visual_frame(&first, 1);
    assert_eq!(first.rect, original.rect);
    assert_eq!(first.opacity, 1.0);
    assert!(!transitions.is_animating());

    let start = transitions.frame([moved], now);
    let start = surface_visual_frame(&start, 1);
    assert_eq!(start.rect, original.rect);
    assert!(transitions.is_animating());

    let middle = transitions.frame([moved], now + SURFACE_TRANSITION_DURATION / 2);
    let middle = surface_visual_frame(&middle, 1);
    assert!(middle.rect.x > original.rect.x);
    assert!(middle.rect.x < moved.rect.x);
    assert!(middle.rect.width > original.rect.width);
    assert!(middle.rect.width < moved.rect.width);

    let entered = SurfaceVisualTarget {
        id: 2,
        rect: AnimatedRect {
            x: 24.0,
            y: 36.0,
            width: 90.0,
            height: 110.0,
        },
    };
    let entry_start = transitions.frame([moved, entered], now + SURFACE_TRANSITION_DURATION / 2);
    let entry_start = surface_visual_frame(&entry_start, 2);
    assert_eq!(entry_start.opacity, 0.0);
    assert!(entry_start.rect.y > entered.rect.y);
    assert!(entry_start.visual_rect().width < entered.rect.width);

    let final_frame = transitions.frame([moved, entered], now + SURFACE_TRANSITION_DURATION * 2);
    let final_moved = surface_visual_frame(&final_frame, 1);
    let final_entered = surface_visual_frame(&final_frame, 2);
    assert_eq!(final_moved.rect, moved.rect);
    assert_eq!(final_entered.rect, entered.rect);
    assert_eq!(final_entered.opacity, 1.0);
    assert!(!transitions.is_animating());
}

#[test]
fn surface_transition_animates_panel_exit_before_removal() {
    let mut transitions = SurfaceTransitionAnimator::default();
    let now = Instant::now();
    let original = SurfaceVisualTarget {
        id: 1,
        rect: AnimatedRect {
            x: 16.0,
            y: 24.0,
            width: 140.0,
            height: 180.0,
        },
    };

    let first = transitions.frame([original], now);
    let first = surface_visual_frame(&first, 1);
    assert!(!first.exiting);
    assert_eq!(first.opacity, 1.0);

    let exit_start = transitions.frame([], now + Duration::from_millis(8));
    let exit_start = surface_visual_frame(&exit_start, 1);
    assert!(exit_start.exiting);
    assert_eq!(exit_start.opacity, 1.0);
    assert_eq!(exit_start.rect, original.rect);
    assert!(transitions.is_animating());

    let exit_middle = transitions.frame(
        [],
        now + Duration::from_millis(8) + SURFACE_TRANSITION_DURATION / 2,
    );
    let exit_middle = surface_visual_frame(&exit_middle, 1);
    assert!(exit_middle.exiting);
    assert!(exit_middle.opacity > 0.0);
    assert!(exit_middle.opacity < 1.0);
    assert!(exit_middle.rect.y < original.rect.y);
    assert!(exit_middle.visual_rect().width < original.rect.width);

    let finished = transitions.frame(
        [],
        now + Duration::from_millis(8) + SURFACE_TRANSITION_DURATION * 2,
    );
    assert!(finished.is_empty());
    assert!(!transitions.is_animating());
}

#[test]
fn reduced_motion_snaps_surface_entries_moves_and_exits() {
    let _guard = DesktopReducedMotionEnvGuard::set(true);
    let mut transitions = SurfaceTransitionAnimator::default();
    let now = Instant::now();
    let original = SurfaceVisualTarget {
        id: 1,
        rect: AnimatedRect {
            x: 16.0,
            y: 24.0,
            width: 140.0,
            height: 180.0,
        },
    };
    let moved = SurfaceVisualTarget {
        id: 1,
        rect: AnimatedRect {
            x: 80.0,
            y: 40.0,
            width: 160.0,
            height: 200.0,
        },
    };

    transitions.frame([original], now);
    let snapped = transitions.frame([moved], now);
    let snapped = surface_visual_frame(&snapped, 1);
    assert_eq!(snapped.rect, moved.rect);
    assert_eq!(snapped.opacity, 1.0);
    assert!(!snapped.exiting);
    assert!(!transitions.is_animating());

    let removed = transitions.frame([], now + Duration::from_millis(1));
    assert!(removed.is_empty());
    assert!(!transitions.is_animating());
}

#[test]
fn surface_transition_retargets_from_current_frame_mid_animation() {
    let mut transitions = SurfaceTransitionAnimator::default();
    let now = Instant::now();
    let original = SurfaceVisualTarget {
        id: 1,
        rect: AnimatedRect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 120.0,
        },
    };
    let first_target = SurfaceVisualTarget {
        id: 1,
        rect: AnimatedRect {
            x: 240.0,
            y: 60.0,
            width: 180.0,
            height: 220.0,
        },
    };
    let second_target = SurfaceVisualTarget {
        id: 1,
        rect: AnimatedRect {
            x: -80.0,
            y: 42.0,
            width: 150.0,
            height: 190.0,
        },
    };

    transitions.frame([original], now);
    transitions.frame([first_target], now);
    let halfway_time = now + SURFACE_TRANSITION_DURATION / 2;
    let halfway = transitions.frame([first_target], halfway_time);
    let halfway = surface_visual_frame(&halfway, 1);

    let retarget_start = transitions.frame([second_target], halfway_time);
    let retarget_start = surface_visual_frame(&retarget_start, 1);
    assert_eq!(retarget_start.rect, halfway.rect);
    assert!(transitions.is_animating());

    let retarget_middle = transitions.frame(
        [second_target],
        halfway_time + SURFACE_TRANSITION_DURATION / 2,
    );
    let retarget_middle = surface_visual_frame(&retarget_middle, 1);
    assert!(retarget_middle.rect.x < halfway.rect.x);
    assert!(retarget_middle.rect.x > second_target.rect.x);
}

#[test]
fn surface_transition_reenters_from_current_exit_frame() {
    let mut transitions = SurfaceTransitionAnimator::default();
    let now = Instant::now();
    let original = SurfaceVisualTarget {
        id: 1,
        rect: AnimatedRect {
            x: 12.0,
            y: 24.0,
            width: 120.0,
            height: 160.0,
        },
    };
    let reentered = SurfaceVisualTarget {
        id: 1,
        rect: AnimatedRect {
            x: 120.0,
            y: 80.0,
            width: 160.0,
            height: 210.0,
        },
    };

    transitions.frame([original], now);
    let exit_start_time = now + Duration::from_millis(6);
    transitions.frame([], exit_start_time);

    let reentry_start = transitions.frame(
        [reentered],
        exit_start_time + SURFACE_TRANSITION_DURATION / 2,
    );
    let reentry_start = surface_visual_frame(&reentry_start, 1);
    assert!(!reentry_start.exiting);
    assert!(reentry_start.opacity > 0.0);
    assert!(reentry_start.opacity < 1.0);
    assert!(reentry_start.rect.y < original.rect.y);

    let settled = transitions.frame(
        [reentered],
        exit_start_time + SURFACE_TRANSITION_DURATION * 2,
    );
    let settled = surface_visual_frame(&settled, 1);
    assert_eq!(settled.rect, reentered.rect);
    assert_eq!(settled.opacity, 1.0);
}

#[test]
fn workspace_zoom_uses_surface_transition_frames() {
    let mut workspace = Workspace::from_session_cards(vec![workspace::SessionCard {
        session_id: "session-alpha".to_string(),
        title: "alpha".to_string(),
        subtitle: "active".to_string(),
        detail: "3 msgs".to_string(),
        preview_lines: Vec::new(),
        detail_lines: Vec::new(),
        transcript_messages: Vec::new(),
    }]);
    let size = PhysicalSize::new(1280, 800);
    let layout = workspace_render_layout(&workspace, size, Some(size));
    let focused_id = workspace.focused_id;
    let unzoomed_target = workspace_surface_transition_targets(&workspace, size, layout)
        .into_iter()
        .find(|target| target.id == focused_id)
        .expect("focused unzoomed target");

    assert_eq!(
        workspace.handle_key(KeyInput::Character("z".to_string())),
        KeyOutcome::Redraw
    );
    assert!(workspace.zoomed);
    let zoomed_target = workspace_surface_transition_targets(&workspace, size, layout)
        .into_iter()
        .find(|target| target.id == focused_id)
        .expect("focused zoomed target");
    assert!(zoomed_target.rect.width > unzoomed_target.rect.width);
    assert!(zoomed_target.rect.x <= unzoomed_target.rect.x);

    let mut transitions = SurfaceTransitionAnimator::default();
    let now = Instant::now();
    transitions.frame([unzoomed_target], now);

    let zoom_start = transitions.frame([zoomed_target], now);
    let zoom_start = surface_visual_frame(&zoom_start, focused_id);
    assert_eq!(zoom_start.rect, unzoomed_target.rect);
    assert!(transitions.is_animating());

    let zoom_mid = transitions.frame([zoomed_target], now + SURFACE_TRANSITION_DURATION / 2);
    let zoom_mid = surface_visual_frame(&zoom_mid, focused_id);
    assert!(zoom_mid.rect.width > unzoomed_target.rect.width);
    assert!(zoom_mid.rect.width < zoomed_target.rect.width);

    let zoomed = transitions.frame([zoomed_target], now + SURFACE_TRANSITION_DURATION * 2);
    let zoomed = surface_visual_frame(&zoomed, focused_id);
    assert_eq!(zoomed.rect, zoomed_target.rect);

    assert_eq!(
        workspace.handle_key(KeyInput::Character("z".to_string())),
        KeyOutcome::Redraw
    );
    assert!(!workspace.zoomed);
    let back_start = transitions.frame([unzoomed_target], now + SURFACE_TRANSITION_DURATION * 2);
    let back_start = surface_visual_frame(&back_start, focused_id);
    assert_eq!(back_start.rect, zoomed_target.rect);
    assert!(transitions.is_animating());
}

#[test]
fn color_transition_interpolates_status_bar_mode_changes() {
    let mut transition = ColorTransition::default();
    let now = Instant::now();
    let nav = [0.10, 0.20, 0.30, 1.0];
    let insert = [0.40, 0.50, 0.60, 1.0];

    assert_eq!(transition.frame(nav, now), nav);
    assert!(!transition.is_animating());

    assert_eq!(transition.frame(insert, now), nav);
    assert!(transition.is_animating());

    let middle = transition.frame(insert, now + STATUS_COLOR_TRANSITION_DURATION / 2);
    assert!(middle[0] > nav[0]);
    assert!(middle[0] < insert[0]);
    assert!(middle[1] > nav[1]);
    assert!(middle[1] < insert[1]);
    assert_eq!(middle[3], 1.0);

    assert_eq!(
        transition.frame(insert, now + STATUS_COLOR_TRANSITION_DURATION),
        insert
    );
    assert!(!transition.is_animating());
}

#[test]
fn reduced_motion_snaps_color_and_streaming_text_arrival() {
    let _guard = DesktopReducedMotionEnvGuard::set(true);
    let mut transition = ColorTransition::default();
    let now = Instant::now();
    let nav = [0.10, 0.20, 0.30, 1.0];
    let insert = [0.40, 0.50, 0.60, 1.0];

    assert_eq!(transition.frame(nav, now), nav);
    assert_eq!(transition.frame(insert, now), insert);
    assert!(!transition.is_animating());

    let style = streaming_text_arrival_style_for_elapsed(Duration::ZERO);
    assert_eq!(style.opacity, 1.0);
    assert_eq!(style.y_offset_pixels, 0.0);
    assert!(!style.active);

    let handoff_style = streaming_text_handoff_style_for_elapsed(Duration::ZERO);
    assert_eq!(handoff_style.opacity, 0.0);
    assert_eq!(handoff_style.y_offset_pixels, 0.0);
    assert!(!handoff_style.active);
    assert_eq!(
        streaming_text_handoff_start_after_len_change(10, 0, true, None, now),
        None
    );
}

fn surface_visual_frame(frames: &[SurfaceVisualFrame], id: u64) -> SurfaceVisualFrame {
    frames
        .iter()
        .copied()
        .find(|frame| frame.id == id)
        .expect("surface visual frame")
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
    assert_eq!(SINGLE_SESSION_USER_FONT_FAMILY, "Kalam");
    assert!(SINGLE_SESSION_HANDWRITING_FONT_FAMILIES.contains(&SINGLE_SESSION_USER_FONT_FAMILY));
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

#[test]
fn single_session_vertices_draw_borderless_composer_and_submit_affordance() {
    let size = PhysicalSize::new(900, 700);
    let empty_app = SingleSessionApp::new(None);
    let empty_vertices = build_single_session_vertices(&empty_app, size, 0.0, 0);

    const REMOVED_COMPOSER_CARD_BACKGROUND_COLOR: [f32; 4] = [0.990, 0.994, 1.000, 0.420];

    assert!(!vertices_have_color(
        &empty_vertices,
        REMOVED_COMPOSER_CARD_BACKGROUND_COLOR
    ));
    assert!(!vertices_have_rgb(
        &empty_vertices,
        COMPOSER_FOCUS_RING_COLOR
    ));
    assert!(vertices_have_color(
        &empty_vertices,
        COMPOSER_PLACEHOLDER_RAIL_COLOR
    ));
    assert!(!vertices_have_color(
        &empty_vertices,
        COMPOSER_SUBMIT_READY_COLOR
    ));

    let mut typed_app = SingleSessionApp::new(None);
    typed_app.handle_key(KeyInput::Character("ship it".to_string()));
    let typed_vertices = build_single_session_vertices(&typed_app, size, 0.0, 0);

    assert!(!vertices_have_color(
        &typed_vertices,
        REMOVED_COMPOSER_CARD_BACKGROUND_COLOR
    ));
    assert!(vertices_have_color(
        &typed_vertices,
        COMPOSER_SUBMIT_READY_COLOR
    ));
    assert!(!vertices_have_color(
        &typed_vertices,
        COMPOSER_PLACEHOLDER_RAIL_COLOR
    ));
}

#[test]
fn single_session_vertices_draw_attachment_chips_near_composer() {
    let size = PhysicalSize::new(900, 700);
    let empty_app = SingleSessionApp::new(None);
    let empty_vertices = build_single_session_vertices(&empty_app, size, 0.0, 0);
    assert!(!vertices_have_color(
        &empty_vertices,
        ATTACHMENT_CHIP_BACKGROUND_COLOR
    ));

    let mut app = SingleSessionApp::new(None);
    app.attach_image("image/png".to_string(), "abc123".to_string());
    let vertices = build_single_session_vertices(&app, size, 0.0, 0);

    assert!(vertices_have_color(
        &vertices,
        ATTACHMENT_CHIP_BACKGROUND_COLOR
    ));
    assert!(vertices_have_color(&vertices, ATTACHMENT_CHIP_ACCENT_COLOR));

    app.clear_attached_images();
    let cleared_vertices = build_single_session_vertices(&app, size, 0.0, 0);
    assert!(!vertices_have_color(
        &cleared_vertices,
        ATTACHMENT_CHIP_BACKGROUND_COLOR
    ));
}

#[test]
fn single_session_vertices_draw_stdin_overlay_chrome_and_submit_affordance() {
    let size = PhysicalSize::new(900, 700);
    let empty_app = SingleSessionApp::new(None);
    let empty_vertices = build_single_session_vertices(&empty_app, size, 0.0, 0);
    assert!(!vertices_have_color(
        &empty_vertices,
        STDIN_OVERLAY_BACKGROUND_COLOR
    ));

    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::StdinRequest {
        request_id: "stdin-1".to_string(),
        prompt: "code:".to_string(),
        is_password: false,
        tool_call_id: "tool-1".to_string(),
    });
    let requested_vertices = build_single_session_vertices(&app, size, 0.0, 0);
    assert!(vertices_have_color(
        &requested_vertices,
        STDIN_OVERLAY_BACKGROUND_COLOR
    ));
    assert!(vertices_have_color(
        &requested_vertices,
        STDIN_OVERLAY_BORDER_COLOR
    ));
    assert!(vertices_have_rgb(
        &requested_vertices,
        STDIN_OVERLAY_INPUT_RAIL_COLOR
    ));
    assert!(!vertices_have_color(
        &requested_vertices,
        STDIN_OVERLAY_SUBMIT_COLOR
    ));

    app.handle_key(KeyInput::Character("ready".to_string()));
    let typed_vertices = build_single_session_vertices(&app, size, 0.0, 0);
    assert!(vertices_have_color(
        &typed_vertices,
        STDIN_OVERLAY_SUBMIT_COLOR
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

    // Pre-token: the activity pill (with pulsing dots) shows while waiting for
    // the first streamed token.
    app.is_processing = true;
    let tick_zero = build_single_session_vertices(&app, PhysicalSize::new(900, 700), 0.0, 0);
    let tick_one = build_single_session_vertices(&app, PhysicalSize::new(900, 700), 0.0, 1);

    assert!(vertices_have_rgb(&tick_zero, NATIVE_SPINNER_HEAD_COLOR));
    assert!(vertices_have_rgb(&tick_one, NATIVE_SPINNER_HEAD_COLOR));
    assert!(vertices_have_color(
        &tick_zero,
        STREAMING_ACTIVITY_PILL_COLOR
    ));
    assert_ne!(
        colors_for_rgb(&tick_zero, NATIVE_SPINNER_HEAD_COLOR),
        colors_for_rgb(&tick_one, NATIVE_SPINNER_HEAD_COLOR)
    );

    let pill_bounds = pixel_bounds_for_color(
        &tick_zero,
        STREAMING_ACTIVITY_PILL_COLOR,
        PhysicalSize::new(900, 700),
    )
    .expect("streaming activity cue should draw an inline pill");
    assert!(
        (pill_bounds.min_x - PANEL_TITLE_LEFT_PADDING).abs() <= 0.5,
        "activity cue should align to the composer lane, got {pill_bounds:?}"
    );
    let body_bottom = single_session_body_bottom_for_total_lines(
        &app,
        PhysicalSize::new(900, 700),
        single_session_rendered_body_lines_for_tick(&app, PhysicalSize::new(900, 700), 0).len(),
    );
    let draft_top = single_session_draft_top_for_app(&app, PhysicalSize::new(900, 700));
    assert!(
        pill_bounds.min_y >= body_bottom - 0.5,
        "activity cue should be below transcript body, got {pill_bounds:?}, body_bottom={body_bottom}"
    );
    assert!(
        pill_bounds.max_y <= draft_top + 0.5,
        "activity cue should stay above composer, got {pill_bounds:?}, draft_top={draft_top}"
    );
    let pill_width = pill_bounds.max_x - pill_bounds.min_x;
    assert!(
        (26.0..=34.1).contains(&pill_width),
        "activity cue pill should stay compact, got {pill_bounds:?}"
    );

    // Once text streams, the pill yields to the tail cursor at the end of the
    // revealed text.
    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "streaming".to_string(),
    ));
    let streaming = build_single_session_vertices(&app, PhysicalSize::new(900, 700), 0.0, 0);
    assert!(
        !vertices_have_color(&streaming, STREAMING_ACTIVITY_PILL_COLOR),
        "activity pill should hide once tokens stream"
    );
    assert!(
        vertices_have_rgb(&streaming, STREAMING_TAIL_CURSOR_COLOR),
        "streaming tail cursor should render at the end of the streamed text"
    );
}

#[test]
fn single_session_motion_geometry_survives_resize_and_text_scale_changes() {
    fn apply_text_scale_steps(app: &mut SingleSessionApp, steps: i8) {
        let direction = steps.signum();
        for _ in 0..steps.unsigned_abs() {
            app.handle_key(KeyInput::AdjustTextScale(direction));
        }
    }

    fn assert_finite_vertices(label: &str, vertices: &[Vertex]) {
        assert!(
            !vertices.is_empty(),
            "{label} should produce primitive vertices"
        );
        for (index, vertex) in vertices.iter().enumerate() {
            assert!(
                vertex
                    .position
                    .iter()
                    .all(|component| component.is_finite()),
                "{label} vertex {index} has non-finite position {:?}",
                vertex.position
            );
            assert!(
                vertex.color.iter().all(|component| component.is_finite()),
                "{label} vertex {index} has non-finite color {:?}",
                vertex.color
            );
        }
    }

    fn assert_case(label: &str, base_app: SingleSessionApp, expected_colors: &[[f32; 4]]) {
        for text_scale_steps in [-2, 0, 3] {
            let mut app = base_app.clone();
            apply_text_scale_steps(&mut app, text_scale_steps);
            for size in [
                PhysicalSize::new(520, 420),
                PhysicalSize::new(900, 700),
                PhysicalSize::new(1320, 860),
            ] {
                let vertices = build_single_session_vertices(&app, size, 0.0, 7);
                let case_label = format!("{label} scale_step={text_scale_steps} size={size:?}");
                assert_finite_vertices(&case_label, &vertices);
                for color in expected_colors {
                    assert!(
                        vertices_have_color(&vertices, *color),
                        "{case_label} should retain color {color:?}"
                    );
                }
            }
        }
    }

    let mut activity_app = SingleSessionApp::new(None);
    activity_app.is_processing = true;
    assert_case(
        "activity cue",
        activity_app,
        &[STREAMING_ACTIVITY_PILL_COLOR],
    );

    let mut attachments_app = SingleSessionApp::new(None);
    attachments_app
        .pending_images
        .push(("image/png".to_string(), "base64-data".to_string()));
    attachments_app.handle_key(KeyInput::Character("caption for image".to_string()));
    assert_case(
        "composer attachments",
        attachments_app,
        &[ATTACHMENT_CHIP_BACKGROUND_COLOR],
    );

    let mut stdin_app = SingleSessionApp::new(None);
    stdin_app.apply_session_event(session_launch::DesktopSessionEvent::StdinRequest {
        request_id: "stdin-resize".to_string(),
        prompt: "multi-line code: ".to_string(),
        is_password: false,
        tool_call_id: "tool-resize".to_string(),
    });
    stdin_app.handle_key(KeyInput::Character("ready".to_string()));
    assert_case(
        "stdin overlay",
        stdin_app,
        &[STDIN_OVERLAY_BACKGROUND_COLOR, STDIN_OVERLAY_SUBMIT_COLOR],
    );

    let mut switcher_app = SingleSessionApp::new(None);
    assert_eq!(
        switcher_app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    switcher_app.apply_session_switcher_cards(vec![
        test_session_card("session-alpha", "alpha", "active"),
        test_session_card("session-beta", "beta", "idle"),
    ]);
    assert_case(
        "session switcher preview panes",
        switcher_app,
        &[
            INLINE_WIDGET_CARD_BACKGROUND_COLOR,
            INLINE_WIDGET_PREVIEW_PANE_BACKGROUND_COLOR,
        ],
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
fn single_session_streaming_handoff_fades_out_after_finish() {
    let start_style = streaming_text_handoff_style_for_elapsed(Duration::ZERO);
    let mid_style = streaming_text_handoff_style_for_elapsed(STREAMING_TEXT_HANDOFF_DURATION / 2);
    let end_style = streaming_text_handoff_style_for_elapsed(STREAMING_TEXT_HANDOFF_DURATION);

    assert!(start_style.active);
    assert_eq!(start_style.y_offset_pixels, 0.0);
    assert!((start_style.opacity - STREAMING_TEXT_HANDOFF_START_OPACITY).abs() < f32::EPSILON);
    assert!(mid_style.active);
    assert!(mid_style.opacity > 0.0);
    assert!(mid_style.opacity < start_style.opacity);
    assert_eq!(end_style.opacity, 0.0);
    assert_eq!(end_style.y_offset_pixels, 0.0);
    assert!(!end_style.active);
}

#[test]
fn single_session_streaming_handoff_starts_only_when_visible_stream_finishes() {
    let now = Instant::now();
    let active = streaming_text_handoff_start_after_len_change(48, 0, true, None, now);
    assert_eq!(active, Some(now));

    assert_eq!(
        streaming_text_handoff_start_after_len_change(48, 0, false, None, now),
        None
    );
    assert_eq!(
        streaming_text_handoff_start_after_len_change(48, 64, true, Some(now), now),
        None
    );

    let during = now + STREAMING_TEXT_HANDOFF_DURATION / 2;
    assert_eq!(
        streaming_text_handoff_start_after_len_change(0, 0, true, active, during),
        active
    );

    let after = now + STREAMING_TEXT_HANDOFF_DURATION + Duration::from_millis(1);
    assert_eq!(
        streaming_text_handoff_start_after_len_change(0, 0, true, active, after),
        None
    );
}

#[test]
fn single_session_streaming_text_fade_does_not_restart_for_each_delta() {
    let first = Instant::now();
    let second = first + Duration::from_millis(40);

    let started = streaming_text_fade_start_after_len_change(0, 5, None, first);
    assert_eq!(started, Some(first));

    let unchanged = streaming_text_fade_start_after_len_change(5, 12, started, second);
    assert_eq!(unchanged, Some(first));
}

#[test]
fn single_session_streaming_text_fade_does_not_restart_for_slow_delta_after_previous_fade_finishes()
{
    let first = Instant::now();
    let later = first + STREAMING_TEXT_FADE_DURATION + Duration::from_millis(1);

    let started = streaming_text_fade_start_after_len_change(0, 5, None, first);
    assert_eq!(started, Some(first));

    let settled = streaming_text_fade_start_after_len_change(5, 12, started, later);
    assert_eq!(settled, None);

    let style = settled
        .map(|started_at| {
            streaming_text_arrival_style_for_elapsed(later.saturating_duration_since(started_at))
        })
        .unwrap_or(StreamingTextArrivalStyle {
            opacity: 1.0,
            y_offset_pixels: 0.0,
            active: false,
        });
    assert_eq!(style.opacity, 1.0);
    assert_eq!(style.y_offset_pixels, 0.0);
    assert!(!style.active);
}

#[test]
fn single_session_streaming_text_fade_stays_idle_after_renderer_clears_finished_fade() {
    let later = Instant::now() + STREAMING_TEXT_FADE_DURATION + Duration::from_millis(1);

    let restarted = streaming_text_fade_start_after_len_change(5, 12, None, later);
    assert_eq!(restarted, None);
}

#[test]
fn single_session_streaming_text_fade_starts_for_new_response_after_reset() {
    let first = Instant::now();
    let next = first + STREAMING_TEXT_FADE_DURATION + Duration::from_millis(1);

    let started = streaming_text_fade_start_after_len_change(0, 5, None, first);
    assert_eq!(started, Some(first));
    assert_eq!(
        streaming_text_fade_start_after_len_change(5, 0, started, first),
        None
    );

    let restarted_for_new_response = streaming_text_fade_start_after_len_change(0, 4, None, next);
    assert_eq!(restarted_for_new_response, Some(next));
}

#[test]
fn single_session_streaming_text_fade_stays_idle_without_response_change() {
    let now = Instant::now();

    assert_eq!(
        streaming_text_fade_start_after_len_change(5, 5, None, now),
        None
    );
}

#[test]
fn single_session_streaming_text_fade_keeps_active_fade_without_response_change() {
    let first = Instant::now();
    let during = first + Duration::from_millis(40);

    let unchanged = streaming_text_fade_start_after_len_change(5, 5, Some(first), during);
    assert_eq!(unchanged, Some(first));
}

#[test]
fn single_session_streaming_text_fade_resets_when_streaming_finishes() {
    let first = Instant::now();
    let started = streaming_text_fade_start_after_len_change(0, 5, None, first);
    assert_eq!(
        streaming_text_fade_start_after_len_change(5, 0, started, first),
        None
    );
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
fn single_session_slash_suggestions_support_tui_style_fuzzy_abbreviations() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/cp".to_string()));

    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::SlashSuggestions)
    );
    let suggestions = app.inline_widget_styled_lines();
    assert!(suggestions.iter().any(|line| {
        line.style == SingleSessionLineStyle::OverlaySelection && line.text.contains("/copy")
    }));

    assert_eq!(app.handle_key(KeyInput::Autocomplete), KeyOutcome::Redraw);
    assert_eq!(app.draft, "/copy");
}

#[test]
fn single_session_raw_slash_tab_completion_uses_fuzzy_after_suggestions_are_dismissed() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/cp".to_string()));
    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::SlashSuggestions)
    );

    assert_eq!(app.handle_key(KeyInput::Escape), KeyOutcome::Redraw);
    assert_eq!(app.active_inline_widget(), None);

    assert_eq!(app.handle_key(KeyInput::Autocomplete), KeyOutcome::Redraw);
    assert_eq!(app.draft, "/copy");
}

#[test]
fn single_session_raw_slash_tab_completion_covers_commands_from_help_table() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/ef".to_string()));
    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::SlashSuggestions)
    );

    assert_eq!(app.handle_key(KeyInput::Escape), KeyOutcome::Redraw);
    assert_eq!(app.active_inline_widget(), None);

    assert_eq!(app.handle_key(KeyInput::Autocomplete), KeyOutcome::Redraw);
    assert_eq!(app.draft, "/effort");
}

#[test]
fn single_session_slash_suggestions_keep_prefix_matches_before_fuzzy_matches() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/c".to_string()));

    let suggestions = app.inline_widget_styled_lines();
    assert!(suggestions.iter().any(|line| {
        line.style == SingleSessionLineStyle::OverlaySelection && line.text.contains("/commands")
    }));
}

#[test]
fn single_session_slash_suggestions_expose_accepted_aliases() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/can".to_string()));

    let suggestions = app
        .inline_widget_styled_lines()
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(suggestions.contains("/cancel"), "{suggestions}");

    assert_eq!(app.handle_key(KeyInput::Autocomplete), KeyOutcome::Redraw);
    assert_eq!(app.draft, "/cancel");
    app.draft.clear();
    app.draft_cursor = 0;

    app.handle_key(KeyInput::Character("/help".to_string()));
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    let help = app
        .inline_widget_styled_lines()
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(help.contains("/session"), "{help}");
    assert!(help.contains("/cancel"), "{help}");
    assert!(help.contains("/force-reload"), "{help}");
    assert!(help.contains("/fonts"), "{help}");
    assert!(help.contains("/info"), "{help}");
    assert!(help.contains("/exit"), "{help}");
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
        app.handle_key(KeyInput::ModelPickerMove(4)),
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
    assert!(vertices_have_color(
        &suggestion_vertices,
        SLASH_SUGGESTIONS_INLINE_CARD_BACKGROUND_COLOR
    ));
    assert!(vertices_have_color(
        &help_vertices,
        INLINE_WIDGET_CARD_BACKGROUND_COLOR
    ));
}

#[test]
fn single_session_slash_suggestions_use_compact_no_wrap_card() {
    let size = PhysicalSize::new(1000, 720);
    let mut app = SingleSessionApp::new(None);

    assert_eq!(
        app.handle_key(KeyInput::Character("/c".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::SlashSuggestions)
    );

    let vertices = build_single_session_vertices(&app, size, 0.0, 0);
    assert!(vertices_have_color(
        &vertices,
        SLASH_SUGGESTIONS_INLINE_CARD_BACKGROUND_COLOR
    ));
    assert!(vertices_have_color(
        &vertices,
        SLASH_SUGGESTIONS_INLINE_SELECTION_BACKGROUND_COLOR
    ));

    let suggestions = app.inline_widget_styled_lines();
    let mut font_system = FontSystem::new();
    let buffers = single_session_text_buffers(&app, size, &mut font_system);
    let inline_buffer = buffers
        .get(4)
        .expect("inline slash suggestion buffer should be present");
    let layout_runs = inline_buffer.layout_runs().collect::<Vec<_>>();
    assert_eq!(
        layout_runs.len(),
        suggestions.len(),
        "slash command suggestions should stay one glyphon row per entry"
    );
    assert!(
        layout_runs
            .iter()
            .any(|run| run.text.contains("/copy [latest|code|transcript]")),
        "long /copy suggestion should remain on one visual row"
    );
}

#[test]
fn read_only_inline_widgets_use_per_widget_visible_height_limits() {
    let mut app = SingleSessionApp::new(None);

    app.show_session_info = true;
    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::SessionInfo)
    );
    assert_eq!(
        app.inline_widget_visible_line_count(),
        app.inline_widget_line_count().min(10)
    );

    app.show_session_info = false;
    assert_eq!(app.handle_key(KeyInput::HotkeyHelp), KeyOutcome::Redraw);
    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::HotkeyHelp)
    );
    assert_eq!(
        app.inline_widget_visible_line_count(),
        app.inline_widget_line_count().min(18)
    );

    app.show_help = false;
    assert_eq!(
        app.handle_key(KeyInput::Character("/".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::SlashSuggestions)
    );
    assert_eq!(
        app.inline_widget_visible_line_count(),
        app.inline_widget_line_count()
            .min(DESKTOP_SLASH_SUGGESTION_ROW_LIMIT + 1)
    );
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
fn single_session_issues_slash_toggles_local_issue_browser() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/issues".to_string()));

    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert!(app.issue_browser_visible());
    assert_eq!(app.side_panel().focus, DesktopSidePanelFocus::IssueList);
    assert!(app.draft.is_empty());
    assert!(app.messages.is_empty());
    assert_eq!(app.side_panel().github_issues.repo, "1jehuang/jcode");
    assert_eq!(
        app.side_panel()
            .github_issues
            .selected_issue()
            .unwrap()
            .number,
        342
    );

    app.handle_key(KeyInput::Character("/issues preview".to_string()));
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert_eq!(app.side_panel().focus, DesktopSidePanelFocus::IssuePreview);

    app.handle_key(KeyInput::Character("/issues off".to_string()));
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert!(!app.issue_browser_visible());
    assert_eq!(app.side_panel().focus, DesktopSidePanelFocus::Chat);
}

#[test]
fn single_session_issue_browser_navigates_focus_list_and_preview() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/issues".to_string()));
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);

    assert_eq!(app.side_panel().focus, DesktopSidePanelFocus::IssueList);
    assert_eq!(
        app.handle_key(KeyInput::Character("j".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.side_panel().github_issues.selected, 1);
    assert_eq!(
        app.side_panel()
            .github_issues
            .selected_issue()
            .unwrap()
            .number,
        337
    );

    assert_eq!(app.handle_key(KeyInput::Autocomplete), KeyOutcome::Redraw);
    assert_eq!(app.side_panel().focus, DesktopSidePanelFocus::IssuePreview);
    assert_eq!(
        app.handle_key(KeyInput::Character("j".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.side_panel().github_issues.preview_scroll, 1);

    assert_eq!(app.handle_key(KeyInput::Autocomplete), KeyOutcome::Redraw);
    assert_eq!(app.side_panel().focus, DesktopSidePanelFocus::Chat);
    assert_eq!(
        app.handle_key(KeyInput::Character("hello".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.draft, "hello");
}

#[test]
fn single_session_issue_browser_investigate_injects_context() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/issues".to_string()));
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    app.handle_key(KeyInput::Character("j".to_string()));

    let outcome = app.handle_key(KeyInput::SubmitDraft);
    let KeyOutcome::StartFreshSession { message, images } = outcome else {
        panic!("expected issue investigation to start a fresh session, got {outcome:?}");
    };
    assert!(images.is_empty());
    assert!(message.contains("GitHub issue mission"));
    assert!(message.contains("Repository: 1jehuang/jcode"));
    assert!(message.contains("Issue: #337"));
    assert!(message.contains("Mission objective: investigate"));
    assert!(message.contains("Operating instructions:"));
    assert_eq!(app.side_panel().focus, DesktopSidePanelFocus::Chat);
    assert_eq!(
        app.side_panel()
            .github_issues
            .selected_issue()
            .unwrap()
            .state,
        GitHubIssueVisualState::Active
    );
    assert!(app.is_processing);
    assert!(app.draft.is_empty());
    assert_eq!(app.messages.len(), 1);
}

#[test]
fn single_session_issue_browser_requests_and_applies_background_sync() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/issues".to_string()));
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert!(app.take_github_issue_sync_request());
    assert!(!app.take_github_issue_sync_request());
    assert!(app.side_panel().github_issue_sync.syncing);

    let synced_browser = GitHubIssueBrowserState {
        repo: "owner/repo".to_string(),
        filter_label: "priority · open · cached now".to_string(),
        selected: 0,
        list_scroll: 0,
        preview_scroll: 0,
        issues: vec![GitHubIssuePreview {
            number: 99,
            priority: "P1".to_string(),
            title: "synced desktop issue".to_string(),
            labels: vec!["bug".to_string()],
            age: "updated now".to_string(),
            comments: 3,
            state: GitHubIssueVisualState::Selected,
            body_lines: vec!["fresh body".to_string()],
            comment_lines: vec!["maintainer: fresh comment".to_string()],
            priority_reason: "test sync".to_string(),
        }],
    };
    app.apply_github_issue_sync_result(Ok(desktop_issue_cache::GitHubIssueSyncSummary {
        repo: "owner/repo".to_string(),
        issue_count: 1,
        fetched_comment_threads: 1,
        comment_fetch_errors: 0,
        cache_path: PathBuf::from("owner_repo.json"),
        elapsed: Duration::from_millis(12),
        browser: synced_browser,
    }));

    assert!(!app.side_panel().github_issue_sync.syncing);
    assert_eq!(app.side_panel().github_issues.repo, "owner/repo");
    assert_eq!(
        app.side_panel()
            .github_issues
            .selected_issue()
            .unwrap()
            .number,
        99
    );
    assert!(
        app.status
            .as_deref()
            .unwrap()
            .contains("synced 1 GitHub issues")
    );
    assert!(
        app.side_panel()
            .github_issue_sync
            .last_message
            .as_deref()
            .unwrap()
            .contains("cache owner_repo.json")
    );

    app.handle_key(KeyInput::Character("r".to_string()));
    assert!(app.take_github_issue_sync_request());
}

#[test]
fn single_session_issue_browser_keeps_cached_data_when_sync_fails() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/issues".to_string()));
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    let original_repo = app.side_panel().github_issues.repo.clone();
    let original_issue = app
        .side_panel()
        .github_issues
        .selected_issue()
        .unwrap()
        .number;

    app.apply_github_issue_sync_result(Err(
        "gh issue list failed because authentication is missing and the message is intentionally long".to_string(),
    ));

    assert!(!app.side_panel().github_issue_sync.syncing);
    assert_eq!(app.side_panel().github_issues.repo, original_repo);
    assert_eq!(
        app.side_panel()
            .github_issues
            .selected_issue()
            .unwrap()
            .number,
        original_issue
    );
    assert!(app.side_panel().github_issue_sync.last_error.is_some());
    assert_eq!(
        app.side_panel().github_issue_sync.last_message.as_deref(),
        Some("Run `gh auth login` or refresh GitHub CLI auth, then press r or Ctrl+R to sync.")
    );
    assert_eq!(
        app.side_panel().github_issue_sync.guidance().as_deref(),
        Some("Run `gh auth login` or refresh GitHub CLI auth, then press r or Ctrl+R to sync.")
    );
    assert!(
        app.status
            .as_deref()
            .unwrap()
            .contains("GitHub issue sync failed")
    );
}

#[test]
fn single_session_issue_browser_guides_missing_gh_sync_failure() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/issues".to_string()));
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);

    app.apply_github_issue_sync_result(Err(
        "GitHub CLI `gh` is not installed or not on PATH".to_string()
    ));

    assert_eq!(
        app.side_panel().github_issue_sync.last_message.as_deref(),
        Some("Install GitHub CLI `gh`, authenticate it, then press r or Ctrl+R to sync.")
    );
    assert_eq!(
        app.side_panel().github_issue_sync.guidance().as_deref(),
        Some("Install GitHub CLI `gh`, authenticate it, then press r or Ctrl+R to sync.")
    );
}

#[test]
fn issue_browser_layout_uses_three_panes_when_wide() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/issues".to_string()));
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);

    let layout = issue_browser_layout(&app, PhysicalSize::new(1440, 900));
    assert_eq!(layout.mode, IssueBrowserLayoutMode::Wide);
    let list = layout.list.expect("wide layout should have issue list");
    let preview = layout
        .preview
        .expect("wide layout should have issue preview");
    assert!(list.x < preview.x);
    assert!(preview.x + preview.width < layout.chat.x);
    assert!(layout.chat.width > 360.0);
}

#[test]
fn issue_browser_layout_collapses_for_medium_and_narrow_windows() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/issues".to_string()));
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);

    let medium = issue_browser_layout(&app, PhysicalSize::new(980, 720));
    assert_eq!(medium.mode, IssueBrowserLayoutMode::Medium);
    let list = medium.list.expect("medium layout should keep list");
    let preview = medium.preview.expect("medium layout should keep preview");
    assert_eq!(list.x, preview.x);
    assert!(list.y + list.height < preview.y);
    assert!(medium.chat.x > list.x + list.width);

    let narrow = issue_browser_layout(&app, PhysicalSize::new(760, 720));
    assert_eq!(narrow.mode, IssueBrowserLayoutMode::Narrow);
    assert!(narrow.list.is_none());
    assert!(narrow.preview.is_none());
    assert_eq!(narrow.chat.width, 760.0);
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
fn single_session_slash_resume_opens_session_switcher_without_sending_prompt() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::Character("/resume".to_string())),
        KeyOutcome::LoadSessionSwitcher
    );

    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::SessionSwitcher)
    );
    assert_eq!(
        app.active_inline_widget_mode(),
        Some(InlineWidgetMode::ReadOnly)
    );
    assert!(app.session_switcher.open);
    assert!(app.session_switcher.loading);
    assert!(app.session_switcher.preview);
    assert_eq!(app.draft, "/resume");
    assert!(app.messages.is_empty());

    app.apply_session_switcher_cards(vec![test_session_card("session_alpha", "alpha", "active")]);
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert!(!app.session_switcher.open);
    assert_eq!(app.live_session_id.as_deref(), Some("session_alpha"));
    assert!(app.draft.is_empty());
    assert!(app.messages.is_empty());
}

#[test]
fn single_session_slash_resume_completion_opens_session_switcher() {
    let mut app = SingleSessionApp::new(None);
    for ch in ["/", "r", "e", "s"] {
        app.handle_key(KeyInput::Character(ch.to_string()));
    }

    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::LoadSessionSwitcher
    );
    assert_eq!(app.draft, "");
    assert!(app.session_switcher.open);
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
    assert!(info.contains("status"));
    assert!(info.contains("model"));
}

#[test]
fn single_session_info_slash_alias_opens_inline_session_info() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("/info".to_string()));

    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert!(app.show_session_info);
    assert!(app.draft.is_empty());
    assert_eq!(
        app.active_inline_widget(),
        Some(InlineWidgetKind::SessionInfo)
    );
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
    assert_eq!(submit("/reload"), KeyOutcome::ForceReload);
    assert_eq!(submit("/force-reload"), KeyOutcome::ForceReload);
    assert_eq!(
        submit("/effort high"),
        KeyOutcome::SetReasoningEffort("high".to_string())
    );
    assert_eq!(
        submit("/effort max"),
        KeyOutcome::SetReasoningEffort("max".to_string())
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
        Some("effort: high · use /effort <none|low|medium|high|xhigh|max>")
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
            provider: Some("Anthropic".to_string()),
            api_method: Some("claude-oauth".to_string()),
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
    assert!(picker.contains("Choose model"));
    assert!(picker.contains("filter \"opus\""));
    assert!(picker.contains("claude-opus-4-5"));
    assert!(picker.contains("Anthropic · claude-oauth · premium"));

    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::SetModel("claude-oauth:claude-opus-4-5".to_string())
    );
    assert!(!app.model_picker.open);
    assert!(app.draft.is_empty());
    assert!(app.messages.is_empty());
}

#[test]
fn single_session_model_slash_preview_accepts_vim_navigation_keys() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::Character("/model".to_string())),
        KeyOutcome::LoadModelCatalog
    );
    app.apply_session_event(session_launch::DesktopSessionEvent::ModelCatalog {
        current_model: None,
        provider_name: Some("Claude".to_string()),
        models: vec![
            session_launch::DesktopModelChoice {
                model: "claude-sonnet-4-5".to_string(),
                provider: Some("Anthropic".to_string()),
                api_method: Some("claude-oauth".to_string()),
                detail: None,
                available: true,
            },
            session_launch::DesktopModelChoice {
                model: "claude-opus-4-5".to_string(),
                provider: Some("Anthropic".to_string()),
                api_method: Some("claude-oauth".to_string()),
                detail: None,
                available: true,
            },
        ],
        reasoning_effort: None,
        service_tier: None,
        compaction_mode: None,
    });

    assert_eq!(
        app.handle_key(KeyInput::Character("j".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.model_picker.selected, 1);
    assert_eq!(app.draft, "/model");
    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::SetModel("claude-oauth:claude-opus-4-5".to_string())
    );
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
        Some(SingleSessionLineStyle::AssistantMedia)
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
        "# Heading\n\n> quoted\n\n![diagram](https://example.com/diagram.png)\n\n| a | b |\n| - | - |\n| c | d |",
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
    assert_eq!(
        first_glyph_color_for_text(body, "🖼 diagram ↗ https://example.com/diagram.png"),
        Some(single_session_line_color(
            SingleSessionLineStyle::AssistantMedia
        ))
    );

    let vertices = build_single_session_vertices(&app, PhysicalSize::new(1200, 760), 0.0, 0);
    assert!(vertices_have_color(
        &vertices,
        MARKDOWN_HEADING_BACKGROUND_COLOR
    ));
    assert!(vertices_have_color(&vertices, QUOTE_CARD_BACKGROUND_COLOR));
    assert!(vertices_have_color(&vertices, TABLE_CARD_BACKGROUND_COLOR));
    assert!(vertices_have_color(
        &vertices,
        MARKDOWN_MEDIA_BACKGROUND_COLOR
    ));
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
    let mut font_system = FontSystem::new();
    let body_buffer = single_session_body_text_buffer_from_lines(
        &mut font_system,
        &body_lines,
        size,
        app.text_scale(),
    );
    let layout_runs = body_buffer.layout_runs().collect::<Vec<_>>();
    let inline_layout_run = layout_runs
        .iter()
        .find(|run| run.line_i == inline_line)
        .expect("inline markdown line should have a glyphon layout run");
    let inline_line_y = body_top + inline_layout_run.line_top;
    let inline_card_height = line_height + 2.0;
    let inline_horizontal_pad = (3.5 * app.text_scale()).clamp(3.0, 6.0);
    let rule_thickness = (1.7 * app.text_scale()).clamp(1.0, 3.0);

    assert_pixel_bounds_close(
        pixel_bounds_for_color(&vertices, MARKDOWN_HEADING_BACKGROUND_COLOR, size)
            .expect("heading card vertices should be present"),
        Rect {
            x: PANEL_TITLE_LEFT_PADDING - 6.0,
            y: body_top + heading_line as f32 * line_height + 3.0,
            width: (size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0 - 6.0).max(1.0),
            height: (line_height - 6.0).max(1.0),
        },
        "heading card",
    );

    let code_span = inline_styled_line
        .inline_spans
        .iter()
        .find(|span| span.kind == SingleSessionInlineSpanKind::Code)
        .expect("code span should be detected");
    let mut code_glyph_left: Option<f32> = None;
    let mut code_glyph_right: Option<f32> = None;
    for glyph in inline_layout_run
        .glyphs
        .iter()
        .enumerate()
        .filter(|(glyph_index, _)| code_span.start <= *glyph_index && *glyph_index < code_span.end)
        .map(|(_, glyph)| glyph)
    {
        code_glyph_left = Some(code_glyph_left.map_or(glyph.x, |current| current.min(glyph.x)));
        code_glyph_right = Some(
            code_glyph_right.map_or(glyph.x + glyph.w, |current| current.max(glyph.x + glyph.w)),
        );
    }
    let (code_glyph_left, code_glyph_right) = code_glyph_left
        .zip(code_glyph_right)
        .expect("code span should have glyph bounds");
    assert_pixel_bounds_close(
        pixel_bounds_for_color(&vertices, INLINE_CODE_BACKGROUND_COLOR, size)
            .expect("inline code pill vertices should be present"),
        Rect {
            x: PANEL_TITLE_LEFT_PADDING + code_glyph_left - inline_horizontal_pad,
            y: inline_line_y + (line_height - inline_card_height) * 0.5,
            width: code_glyph_right - code_glyph_left + inline_horizontal_pad * 2.0,
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
            width: size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0 - 10.0,
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
        to_key_input(&Key::Named(NamedKey::Backspace), ModifiersState::SUPER),
        KeyInput::DeletePreviousWord
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::ArrowUp), ModifiersState::CONTROL),
        KeyInput::RetrieveQueuedDraft
    );
    assert_eq!(
        to_key_input(&Key::Character("v".into()), ModifiersState::ALT),
        KeyInput::PasteText
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
fn desktop_maps_standard_clipboard_shortcuts_across_platform_modifiers() {
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::Paste), ModifiersState::empty()),
        KeyInput::PasteText
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::Cut), ModifiersState::empty()),
        KeyInput::CutInputLine
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::Copy), ModifiersState::empty()),
        KeyInput::CopyLatestResponse
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::Undo), ModifiersState::empty()),
        KeyInput::UndoInput
    );
    assert_eq!(
        to_key_input(&Key::Character("v".into()), ModifiersState::CONTROL),
        KeyInput::PasteText
    );
    assert_eq!(
        to_key_input(
            &Key::Character("v".into()),
            ModifiersState::CONTROL | ModifiersState::SHIFT
        ),
        KeyInput::PasteText
    );
    assert_eq!(
        to_key_input(&Key::Character("v".into()), ModifiersState::SUPER),
        KeyInput::PasteText
    );
    assert_eq!(
        to_key_input(&Key::Character("v".into()), ModifiersState::ALT),
        KeyInput::PasteText
    );
    assert_eq!(
        to_key_input(
            &Key::Character("v".into()),
            ModifiersState::ALT | ModifiersState::SHIFT
        ),
        KeyInput::PasteText
    );
    assert_eq!(
        to_key_input(&Key::Character("x".into()), ModifiersState::SUPER),
        KeyInput::CutInputLine
    );
    assert_eq!(
        to_key_input(&Key::Character("z".into()), ModifiersState::SUPER),
        KeyInput::UndoInput
    );
    assert_eq!(
        to_key_input(&Key::Character("c".into()), ModifiersState::SUPER),
        KeyInput::CopyLatestResponse
    );
    assert_eq!(
        to_key_input(
            &Key::Character("c".into()),
            ModifiersState::SUPER | ModifiersState::SHIFT
        ),
        KeyInput::CopyLatestResponse
    );
    assert_eq!(
        to_key_input(&Key::Character("c".into()), ModifiersState::CONTROL),
        KeyInput::CancelGeneration
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
        to_key_input(&Key::Named(NamedKey::ArrowLeft), ModifiersState::ALT),
        KeyInput::CycleReasoningEffort(-1)
    );
    assert_eq!(
        to_key_input(&Key::Named(NamedKey::ArrowRight), ModifiersState::ALT),
        KeyInput::CycleReasoningEffort(1)
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
        KeyInput::JumpPrompt(-1)
    );
    assert_eq!(
        to_key_input(&Key::Character("j".into()), ModifiersState::SUPER),
        KeyInput::JumpPrompt(1)
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
        to_key_input(&Key::Character(";".into()), ModifiersState::SUPER),
        KeyInput::SpawnSelfDevSession
    );
    assert_eq!(
        to_key_input(&Key::Character("'".into()), ModifiersState::SUPER),
        KeyInput::SpawnHomeSession
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

    assert_eq!(buffers.len(), 8);
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
    assert!(!vertices_have_color(
        &markdown_vertices,
        [0.985, 0.992, 1.000, 0.46]
    ));
    assert!(!vertices_have_color(
        &markdown_vertices,
        [0.055, 0.125, 0.270, 0.18]
    ));
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
    assert_visual_text_contains(&switcher_key, "Resume sessions");
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
                .family(Family::Name(SINGLE_SESSION_USER_FONT_FAMILY))
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
        tool: None,
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
        tool: None,
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

    for (text, style, color_style) in [
        (
            "See https://example.com/docs for details",
            SingleSessionLineStyle::Assistant,
            SingleSessionLineStyle::Assistant,
        ),
        (
            "docs https://example.com",
            SingleSessionLineStyle::AssistantLink,
            SingleSessionLineStyle::AssistantLink,
        ),
        (
            "| command | result |",
            SingleSessionLineStyle::AssistantTable,
            SingleSessionLineStyle::AssistantTable,
        ),
        (
            "Call single_session_render() carefully",
            SingleSessionLineStyle::Assistant,
            SingleSessionLineStyle::Assistant,
        ),
        (
            "こんにちは assistant response",
            SingleSessionLineStyle::Assistant,
            SingleSessionLineStyle::Assistant,
        ),
    ] {
        let lines = [SingleSessionStyledLine::new(text, style)];
        let segments = single_session_styled_text_segments(&lines);
        assert!(
            segments.contains(&(
                text,
                Attrs::new()
                    .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                    .color(single_session_line_color(color_style))
            )),
            "expected {text:?} to use the main monospace font"
        );
    }

    let bullet_lines = [SingleSessionStyledLine::new(
        "• handwritten list prose",
        SingleSessionLineStyle::Assistant,
    )];
    let bullet_segments = single_session_styled_text_segments(&bullet_lines);
    assert!(
        bullet_segments.contains(&(
            "• ",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(text_color(MARKDOWN_LIST_MARKER_COLOR))
        ))
    );
    assert!(
        bullet_segments.contains(&(
            "handwritten list prose",
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
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(single_session_line_color(SingleSessionLineStyle::Assistant))
        ))
    );
    assert!(
        segments.contains(&(
            " before ",
            Attrs::new()
                .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
                .color(single_session_line_color(SingleSessionLineStyle::Assistant))
        ))
    );
    assert!(
        !segments
            .iter()
            .any(|(_, attrs)| attrs.family == Family::Name(SINGLE_SESSION_USER_FONT_FAMILY)),
        "inline-code lines should avoid mixing handwriting and mono fonts: {segments:?}"
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
    assert!(pixel_bounds_for_color(&vertices, INLINE_CODE_BACKGROUND_COLOR, size).is_some());
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
            .any(|line| line.text.contains("format inline code")),
        "narrow fixture should exercise wrapped prose before the inline-code row"
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
    assert!(pixel_bounds_for_color(&vertices, INLINE_CODE_BACKGROUND_COLOR, size).is_some());
}

#[test]
fn assistant_inline_code_pills_enclose_glyphon_highlights_across_resizes() {
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "Here are inline snippets that should stay covered after wrapping: `cargo test`, `KeyOutcome::SetReasoningEffort(\"max\")`, ` `, and `single_session_reasoning_cycle_updates_visible_thinking_status_and_transcript`.",
    ));

    for size in [
        PhysicalSize::new(640, 720),
        PhysicalSize::new(900, 720),
        PhysicalSize::new(1280, 720),
        PhysicalSize::new(640, 840),
    ] {
        let body_lines = single_session_rendered_body_lines_for_tick(&app, size, 0);
        let viewport = single_session_body_viewport_from_lines(&app, size, 0.0, &body_lines);
        let mut font_system = FontSystem::new();
        let body_buffer = single_session_body_text_buffer_from_lines(
            &mut font_system,
            &viewport.lines,
            size,
            app.text_scale(),
        );
        let layout_runs = body_buffer.layout_runs().collect::<Vec<_>>();
        let vertices = build_single_session_vertices(&app, size, 0.0, 0);
        let pill_bounds =
            pixel_bounds_list_for_color(&vertices, INLINE_CODE_BACKGROUND_COLOR, size);

        let typography = single_session_typography_for_scale(app.text_scale());
        let line_height = typography.body_size * typography.body_line_height;
        let horizontal_pad = (3.5 * app.text_scale()).clamp(3.0, 6.0);
        let mut expected_index = 0;

        for (line_index, line) in viewport.lines.iter().enumerate() {
            let Some(layout_run) = layout_runs.get(line_index) else {
                continue;
            };
            for span in line
                .inline_spans
                .iter()
                .filter(|span| span.kind == SingleSessionInlineSpanKind::Code)
            {
                let pill = pill_bounds.get(expected_index).unwrap_or_else(|| {
                    panic!(
                        "missing inline-code pill {expected_index} for line {:?} at size {:?}; found {} pills",
                        line.text,
                        size,
                        pill_bounds.len()
                    )
                });
                let line_y =
                    PANEL_BODY_TOP_PADDING + viewport.top_offset_pixels + layout_run.line_top;
                let (glyph_x, glyph_width) = layout_run
                    .highlight(
                        glyphon::Cursor::new(layout_run.line_i, span.start),
                        glyphon::Cursor::new(layout_run.line_i, span.end),
                    )
                    .unwrap_or_else(|| {
                        panic!(
                            "glyphon highlight missing for inline code span {span:?} in line {:?} at size {:?}; layout_run text={:?}, glyph_count={}",
                            line.text,
                            size,
                            layout_run.text,
                            layout_run.glyphs.len()
                        )
                    });
                let glyph_left = PANEL_TITLE_LEFT_PADDING + glyph_x;
                let glyph_right = glyph_left + glyph_width;
                let glyph_width = (glyph_right - glyph_left).min(size.width as f32 - pill.min_x);
                assert!(
                    pill.min_x <= glyph_left - horizontal_pad + 0.75,
                    "pill should start before glyph highlight plus padding at size {:?}: pill={pill:?}, glyph_left={glyph_left:.2}",
                    size
                );
                assert!(
                    pill.max_x - pill.min_x >= glyph_width - 0.75,
                    "pill should be at least as wide as the inline code run at size {:?}: pill={pill:?}, glyph_width={glyph_width:.2}",
                    size
                );
                assert!(
                    pill.min_y <= line_y + 1.75,
                    "pill should cover the top of the laid-out line at size {:?}: pill={pill:?}, line_y={line_y:.2}",
                    size
                );
                assert!(
                    pill.max_y >= line_y + line_height - 1.75,
                    "pill should cover the bottom of the laid-out line at size {:?}: pill={pill:?}, line_y={line_y:.2}, line_height={line_height:.2}",
                    size
                );
                expected_index += 1;
            }
        }

        assert_eq!(
            pill_bounds.len(),
            expected_index,
            "inline-code pill count must match glyphon-visible code spans at size {size:?}"
        );
    }
}

#[test]
fn assistant_inline_code_pills_align_after_unicode_list_markers() {
    let size = PhysicalSize::new(1000, 720);
    let mut app = SingleSessionApp::new(None);
    app.messages.push(SingleSessionMessage::assistant(
        "- `crates/`: shared modules\n- `jcode-provider-*`: model/provider integrations.",
    ));

    let body_lines = single_session_rendered_body_lines_for_tick(&app, size, 0);
    let viewport = single_session_body_viewport_from_lines(&app, size, 0.0, &body_lines);
    let mut font_system = FontSystem::new();
    let body_buffer = single_session_body_text_buffer_from_lines(
        &mut font_system,
        &viewport.lines,
        size,
        app.text_scale(),
    );
    let layout_runs = body_buffer.layout_runs().collect::<Vec<_>>();
    let vertices = build_single_session_vertices(&app, size, 0.0, 0);
    let pill_bounds = pixel_bounds_list_for_color(&vertices, INLINE_CODE_BACKGROUND_COLOR, size);

    let horizontal_pad = (3.5 * app.text_scale()).clamp(3.0, 6.0);
    let mut expected_index = 0;
    let mut unicode_prefixed_spans = 0;

    for (line_index, line) in viewport.lines.iter().enumerate() {
        let Some(layout_run) = layout_runs.get(line_index) else {
            continue;
        };
        for span in line
            .inline_spans
            .iter()
            .filter(|span| span.kind == SingleSessionInlineSpanKind::Code)
        {
            let pill = pill_bounds.get(expected_index).unwrap_or_else(|| {
                panic!(
                    "missing inline-code pill {expected_index} for line {:?}; found {} pills",
                    line.text,
                    pill_bounds.len()
                )
            });
            let prefix = &line.text[..span.start];
            assert!(
                prefix.chars().any(|ch| ch.len_utf8() > 1),
                "regression fixture should place the code span after a multi-byte list marker: line={:?}, span={span:?}",
                line.text
            );
            let (glyph_x, glyph_width) = layout_run
                .highlight(
                    glyphon::Cursor::new(layout_run.line_i, span.start),
                    glyphon::Cursor::new(layout_run.line_i, span.end),
                )
                .unwrap_or_else(|| {
                    panic!(
                        "glyphon highlight missing for unicode-prefixed inline code span {span:?} in line {:?}",
                        line.text
                    )
                });
            let expected_left = PANEL_TITLE_LEFT_PADDING + glyph_x - horizontal_pad;
            let expected_right =
                (PANEL_TITLE_LEFT_PADDING + glyph_x + glyph_width + horizontal_pad)
                    .min(size.width as f32);
            assert!(
                (pill.min_x - expected_left).abs() <= 1.25,
                "inline-code pill should start at the highlighted code run, not shifted by UTF-8 bytes: line={:?}, pill={pill:?}, expected_left={expected_left:.2}",
                line.text
            );
            assert!(
                (pill.max_x - expected_right).abs() <= 1.25,
                "inline-code pill should end at the highlighted code run, not bleed into following text: line={:?}, pill={pill:?}, expected_right={expected_right:.2}",
                line.text
            );
            expected_index += 1;
            unicode_prefixed_spans += 1;
        }
    }

    assert_eq!(pill_bounds.len(), expected_index);
    assert_eq!(unicode_prefixed_spans, 2);
}

#[test]
fn single_session_cached_body_layout_is_not_reused_across_width_resize() {
    let wide = PhysicalSize::new(1600, 900);
    let narrow = PhysicalSize::new(720, 900);
    let height_only_resize = PhysicalSize::new(1600, 895);
    let scale = 1.0;

    assert!(
        single_session_body_text_buffer_layout_compatible(
            (wide.width, wide.height),
            height_only_resize,
            scale
        ),
        "minor height-only resizes that keep the same visible-line bucket may reuse cached body glyphs"
    );
    assert!(
        !single_session_body_text_buffer_layout_compatible(
            (wide.width, wide.height),
            narrow,
            scale
        ),
        "horizontal resizes must rebuild the body buffer so wrapping and text bounds match the new window width"
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
            tool: None,
        },
        SingleSessionStyledLine {
            text: "  │intent: Run tests                                            │".to_string(),
            style: SingleSessionLineStyle::Tool,
            inline_spans: Vec::new(),
            tool: None,
        },
        SingleSessionStyledLine {
            text: "  plain tool output".to_string(),
            style: SingleSessionLineStyle::Tool,
            inline_spans: Vec::new(),
            tool: None,
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
            for (line_index, line) in lines
                .iter()
                .enumerate()
                .skip(header_run.line)
                .take(header_run.line_count)
            {
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

                let line_text = &line.text;
                let text_glyph_right =
                    geometry.text_left + line_text.chars().count() as f32 * char_width;
                let card_right = geometry.card_rect.x + geometry.card_rect.width;
                if text_glyph_right <= card_right + 0.75 {
                    assert!(
                        text_glyph_right <= card_right + 0.75,
                        "code text must fit horizontally inside the card at line {line_index}, size {size:?}, scale {scale_step}"
                    );
                } else {
                    assert!(
                        geometry.text_left < card_right,
                        "overflowing preformatted code must still begin inside the visible code card at line {line_index}, size {size:?}, scale {scale_step}"
                    );
                }
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

fn pixel_bounds_list_for_color(
    vertices: &[Vertex],
    color: [f32; 4],
    size: PhysicalSize<u32>,
) -> Vec<PixelBounds> {
    let color_vertices = vertices
        .iter()
        .filter(|vertex| vertex.color == color)
        .collect::<Vec<_>>();
    let rounded_rect_vertex_count = (ROUNDED_CORNER_SEGMENTS + 1) * 4 * 3;
    assert_eq!(
        color_vertices.len() % rounded_rect_vertex_count,
        0,
        "expected color vertices to be composed of whole rounded rects"
    );
    color_vertices
        .chunks_exact(rounded_rect_vertex_count)
        .map(|chunk| {
            let mut bounds: Option<PixelBounds> = None;
            for vertex in chunk {
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
            bounds.expect("rounded rect chunk should contain vertices")
        })
        .collect()
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
            (actual_value - expected_value).abs() <= 1.25,
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
        .chain(
            key.inline_widget_preview
                .iter()
                .map(|line| line.text.as_str()),
        )
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
        transcript_messages: Vec::new(),
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

fn buffer_has_layout_run_text(buffer: &Buffer, text: &str) -> bool {
    buffer.layout_runs().any(|run| run.text == text)
}

#[test]
fn single_session_tool_events_expand_context_and_collapse_previous_call() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolStarted {
        id: None,
        name: "bash".to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolInput {
        id: None,
        delta: r#"{"command":"cargo test -p jcode-desktop","timeout":120000,"intent":"Run desktop tests"}"#.to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolExecuting {
        id: None,
        name: "bash".to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolFinished {
        id: None,
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
        id: None,
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
        id: None,
        name: "bash".to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolExecuting {
        id: None,
        name: "bash".to_string(),
    });
    let before_input_cache_key = app.rendered_body_cache_key((900, 700));
    let before_static_cache_key = app.rendered_body_static_cache_key((900, 700));

    app.apply_session_event(session_launch::DesktopSessionEvent::ToolInput {
        id: None,
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
fn single_session_tool_ids_drive_stable_native_card_runs() {
    let mut app = SingleSessionApp::new(None);

    app.apply_session_event(session_launch::DesktopSessionEvent::ToolStarted {
        id: Some("tool-a".to_string()),
        name: "bash".to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolInput {
        id: Some("tool-a".to_string()),
        delta: r#"{"command":"echo a"}"#.to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolExecuting {
        id: Some("tool-a".to_string()),
        name: "bash".to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolFinished {
        id: Some("tool-a".to_string()),
        name: "bash".to_string(),
        summary: "printed a".to_string(),
        is_error: false,
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolStarted {
        id: Some("tool-b".to_string()),
        name: "read".to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolInput {
        id: Some("tool-b".to_string()),
        delta: r#"{"file_path":"src/lib.rs"}"#.to_string(),
    });

    let lines = app.body_styled_lines();
    let runs = single_session_tool_card_runs(&lines);

    assert_eq!(
        runs.len(),
        2,
        "each tool id should produce its own native card: {runs:?}"
    );
    assert_eq!(runs[0].call_id, "tool-a");
    assert_eq!(runs[0].state, SingleSessionToolVisualState::Succeeded);
    assert!(!runs[0].active);
    assert_eq!(runs[1].call_id, "tool-b");
    assert_eq!(runs[1].state, SingleSessionToolVisualState::Preparing);
    assert!(runs[1].active);
    assert!(
        lines[runs[0].line]
            .text
            .contains("✓ bash · done · printed a"),
        "finished tool should keep its compact success header: {:?}",
        lines[runs[0].line]
    );
    assert!(
        lines[runs[1].line].text.contains("○ read · preparing"),
        "new active tool should keep its preparing header: {:?}",
        lines[runs[1].line]
    );
}

#[test]
fn single_session_tool_cards_have_native_geometry_and_success_fill() {
    let mut app = SingleSessionApp::new(None);
    let size = PhysicalSize::new(900, 640);

    app.apply_session_event(session_launch::DesktopSessionEvent::ToolStarted {
        id: Some("tool-card".to_string()),
        name: "bash".to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolInput {
        id: Some("tool-card".to_string()),
        delta: r#"{"command":"cargo check -p jcode-desktop"}"#.to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolFinished {
        id: Some("tool-card".to_string()),
        name: "bash".to_string(),
        summary: "check passed".to_string(),
        is_error: false,
    });

    let lines = single_session_rendered_body_lines_for_tick(&app, size, 0);
    let geometries = single_session_tool_card_geometries(&app, size, &lines);
    let geometry = geometries
        .iter()
        .find(|geometry| geometry.run.call_id == "tool-card")
        .unwrap_or_else(|| panic!("missing tool-card geometry: {geometries:?}"));

    assert_eq!(geometry.run.name, "bash");
    assert_eq!(geometry.run.state, SingleSessionToolVisualState::Succeeded);
    assert_eq!(geometry.run.detail_line_count, 0);
    assert!(geometry.card_rect.height > geometry.line_height * 0.45);
    assert!(geometry.rail_rect.x > geometry.card_rect.x);
    assert!(geometry.rail_rect.height <= geometry.card_rect.height);

    let vertices = build_single_session_vertices(&app, size, 0.0, 0);
    let bounds = pixel_bounds_for_color(&vertices, TOOL_CARD_SUCCESS_BACKGROUND_COLOR, size)
        .expect("success tool card fill should be rendered as native geometry");
    assert!(bounds.max_x > bounds.min_x);
    assert!(bounds.max_y > bounds.min_y);
}

#[test]
fn single_session_tool_event_preserves_prior_streaming_text_order() {
    let mut app = SingleSessionApp::new(None);

    app.apply_session_event(session_launch::DesktopSessionEvent::TextDelta(
        "Before the tool".to_string(),
    ));
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolStarted {
        id: None,
        name: "bash".to_string(),
    });
    app.apply_session_event(session_launch::DesktopSessionEvent::ToolFinished {
        id: None,
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
    assert!(
        body[0].starts_with("  ▸ tools: 1 read, 2 agentgrep, 1 edit · ~"),
        "compact summary should preserve grouped tool counts: {:?}",
        body[0]
    );
    assert!(
        body[0].ends_with(" tokens"),
        "compact summary should preserve approximate token suffix: {:?}",
        body[0]
    );

    let styled = app.body_styled_lines();
    let runs = single_session_tool_card_runs(&styled);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, SingleSessionToolVisualState::Group);
    assert_eq!(runs[0].kind, SingleSessionToolLineKind::GroupSummary);
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
    assert!(help_has_shortcut(
        &help,
        "Super+K/J",
        "jump between user prompts"
    ));
    assert!(help_has_shortcut(
        &help,
        "Super+;",
        "spawn a self-dev jcode session"
    ));
    assert!(help_has_shortcut(
        &help,
        "Super+'",
        "spawn a jcode session in home"
    ));
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
fn single_session_inline_widget_close_keeps_render_snapshot_until_exit_finishes() {
    let mut app = SingleSessionApp::new(None);

    assert_eq!(app.handle_key(KeyInput::HotkeyHelp), KeyOutcome::Redraw);
    assert_eq!(
        app.render_inline_widget_kind(),
        Some(InlineWidgetKind::HotkeyHelp)
    );
    assert!(
        app.render_inline_widget_styled_lines()
            .iter()
            .any(|line| line.text == "desktop shortcuts")
    );

    assert_eq!(app.handle_key(KeyInput::Escape), KeyOutcome::Redraw);
    assert_eq!(app.active_inline_widget(), None);
    assert!(app.inline_widget_styled_lines().is_empty());
    assert_eq!(
        app.render_inline_widget_kind(),
        Some(InlineWidgetKind::HotkeyHelp)
    );
    assert!(app.render_inline_widget_reveal_progress() > 0.0);
    assert!(app.has_frame_animation());
    assert!(
        app.render_inline_widget_styled_lines()
            .iter()
            .any(|line| line.text == "desktop shortcuts")
    );

    app.finish_inline_widget_exit_animation_for_test();
    assert_eq!(app.render_inline_widget_kind(), None);
    assert!(app.render_inline_widget_styled_lines().is_empty());
    assert!(!app.has_frame_animation());
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
fn single_session_reasoning_cycle_updates_visible_thinking_status_and_transcript() {
    let mut app = SingleSessionApp::new(None);

    assert_eq!(
        app.handle_key(KeyInput::CycleReasoningEffort(1)),
        KeyOutcome::CycleReasoningEffort(1)
    );
    app.apply_session_event(session_launch::DesktopSessionEvent::Status(
        session_launch::DesktopSessionStatus::ReasoningEffort("high".to_string()),
    ));

    assert_eq!(app.status.as_deref(), Some("thinking level: high"));
    assert_eq!(app.reasoning_effort(), Some("high"));
    assert!(
        app.body_lines()
            .join("\n")
            .contains("thinking level set to high")
    );
}

#[test]
fn single_session_reasoning_cycle_previews_locally_without_backend_roundtrip() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::ModelCatalog {
        current_model: Some("gpt-5-codex".to_string()),
        provider_name: Some("OpenAI".to_string()),
        models: Vec::new(),
        reasoning_effort: Some("medium".to_string()),
        service_tier: None,
        compaction_mode: None,
    });

    assert_eq!(
        app.preview_reasoning_effort_cycle(1),
        ReasoningEffortCycleOutcome::Set("high".to_string())
    );
    assert_eq!(app.status.as_deref(), Some("thinking level: high"));
    assert_eq!(app.reasoning_effort(), Some("high"));

    assert_eq!(
        app.preview_reasoning_effort_cycle(-1),
        ReasoningEffortCycleOutcome::Set("medium".to_string())
    );
    assert_eq!(app.status.as_deref(), Some("thinking level: medium"));
    assert_eq!(app.reasoning_effort(), Some("medium"));
}

#[test]
fn single_session_reasoning_cycle_clamps_and_normalizes_max_aliases() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::ModelCatalog {
        current_model: Some("gpt-5-codex".to_string()),
        provider_name: Some("OpenAI".to_string()),
        models: Vec::new(),
        reasoning_effort: Some("xhigh".to_string()),
        service_tier: None,
        compaction_mode: None,
    });

    assert_eq!(
        app.preview_reasoning_effort_cycle(1),
        ReasoningEffortCycleOutcome::AlreadyAtLimit {
            effort: "xhigh".to_string(),
            limit: "max",
        }
    );
    assert_eq!(
        app.status.as_deref(),
        Some("thinking level: xhigh (already at max)")
    );

    assert_eq!(
        app.preview_reasoning_effort_set("max"),
        Some("xhigh".to_string())
    );
    assert_eq!(app.status.as_deref(), Some("thinking level: xhigh"));
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
                provider: Some("Anthropic".to_string()),
                api_method: Some("claude-oauth".to_string()),
                detail: Some("active account".to_string()),
                available: true,
            },
            session_launch::DesktopModelChoice {
                model: "claude-opus-4-5".to_string(),
                provider: Some("Anthropic".to_string()),
                api_method: Some("claude-oauth".to_string()),
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
    assert!(picker.contains("Choose model"));
    assert!(picker.contains("Current  Claude · claude-sonnet-4-5"));
    assert!(picker.contains("type to filter"));
    assert!(picker.contains("2 models"));
    assert!(picker.contains("   claude-sonnet-4-5"));
    assert!(picker.contains("   claude-opus-4-5"));
    assert!(picker.contains("Anthropic"));
    assert!(picker.contains("claude-oauth"));

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
        KeyOutcome::SetModel("claude-oauth:claude-opus-4-5".to_string())
    );
    assert!(!app.model_picker.open);
}

#[test]
fn single_session_model_picker_accepts_vim_navigation_keys() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::OpenModelPicker),
        KeyOutcome::LoadModelCatalog
    );
    app.apply_session_event(session_launch::DesktopSessionEvent::ModelCatalog {
        current_model: None,
        provider_name: Some("Claude".to_string()),
        models: vec![
            session_launch::DesktopModelChoice {
                model: "claude-sonnet-4-5".to_string(),
                provider: Some("Anthropic".to_string()),
                api_method: Some("claude-oauth".to_string()),
                detail: None,
                available: true,
            },
            session_launch::DesktopModelChoice {
                model: "claude-opus-4-5".to_string(),
                provider: Some("Anthropic".to_string()),
                api_method: Some("claude-oauth".to_string()),
                detail: None,
                available: true,
            },
        ],
        reasoning_effort: None,
        service_tier: None,
        compaction_mode: None,
    });

    assert_eq!(
        app.handle_key(KeyInput::Character("j".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.model_picker.selected, 1);
    assert_eq!(
        app.handle_key(KeyInput::Character("k".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.model_picker.selected, 0);
    assert_eq!(
        app.handle_key(KeyInput::Character("G".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.model_picker.selected, 1);
    assert_eq!(
        app.handle_key(KeyInput::SubmitDraft),
        KeyOutcome::SetModel("claude-oauth:claude-opus-4-5".to_string())
    );
}

#[test]
fn single_session_model_picker_filter_supports_fuzzy_abbreviations() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::OpenModelPicker),
        KeyOutcome::LoadModelCatalog
    );
    app.apply_session_event(session_launch::DesktopSessionEvent::ModelCatalog {
        current_model: None,
        provider_name: Some("OpenAI".to_string()),
        models: vec![
            session_launch::DesktopModelChoice {
                model: "gpt-5-codex".to_string(),
                provider: Some("openai".to_string()),
                api_method: Some("responses".to_string()),
                detail: Some("coding model".to_string()),
                available: true,
            },
            session_launch::DesktopModelChoice {
                model: "claude-opus-4-5".to_string(),
                provider: Some("Anthropic".to_string()),
                api_method: Some("claude-oauth".to_string()),
                detail: Some("premium".to_string()),
                available: true,
            },
        ],
        reasoning_effort: None,
        service_tier: None,
        compaction_mode: None,
    });

    assert_eq!(
        app.handle_key(KeyInput::Character("g5c".to_string())),
        KeyOutcome::Redraw
    );
    let picker = app
        .inline_widget_styled_lines()
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(picker.contains("filter \"g5c\""));
    assert!(picker.contains("gpt-5-codex"), "{picker}");
    assert!(!picker.contains("claude-opus-4-5"), "{picker}");
}

#[test]
fn single_session_model_picker_filter_tolerates_transposed_typo() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::OpenModelPicker),
        KeyOutcome::LoadModelCatalog
    );
    app.apply_session_event(session_launch::DesktopSessionEvent::ModelCatalog {
        current_model: None,
        provider_name: Some("OpenAI".to_string()),
        models: vec![
            session_launch::DesktopModelChoice {
                model: "gpt-5-codex".to_string(),
                provider: Some("openai".to_string()),
                api_method: Some("responses".to_string()),
                detail: Some("coding model".to_string()),
                available: true,
            },
            session_launch::DesktopModelChoice {
                model: "claude-opus-4-5".to_string(),
                provider: Some("Anthropic".to_string()),
                api_method: Some("claude-oauth".to_string()),
                detail: Some("premium".to_string()),
                available: true,
            },
        ],
        reasoning_effort: None,
        service_tier: None,
        compaction_mode: None,
    });

    assert_eq!(
        app.handle_key(KeyInput::Character("codxe".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.model_picker.filtered_indices(), &[0]);
}

#[test]
fn single_session_model_picker_exact_match_outranks_longer_prefix() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::OpenModelPicker),
        KeyOutcome::LoadModelCatalog
    );
    app.apply_session_event(session_launch::DesktopSessionEvent::ModelCatalog {
        current_model: None,
        provider_name: Some("OpenAI".to_string()),
        models: vec![
            session_launch::DesktopModelChoice {
                model: "gpt-5.5".to_string(),
                provider: Some("openai".to_string()),
                api_method: Some("responses".to_string()),
                detail: None,
                available: true,
            },
            session_launch::DesktopModelChoice {
                model: "gpt-5".to_string(),
                provider: Some("openai".to_string()),
                api_method: Some("responses".to_string()),
                detail: None,
                available: true,
            },
        ],
        reasoning_effort: None,
        service_tier: None,
        compaction_mode: None,
    });

    assert_eq!(
        app.handle_key(KeyInput::Character("gpt-5".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.model_picker.filtered_indices(), &[1, 0]);
}

#[test]
fn single_session_model_picker_filter_finds_synthetic_current_choice() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::OpenModelPicker),
        KeyOutcome::LoadModelCatalog
    );
    app.apply_session_event(session_launch::DesktopSessionEvent::ModelCatalog {
        current_model: Some("custom-local-model".to_string()),
        provider_name: Some("Local".to_string()),
        models: vec![session_launch::DesktopModelChoice {
            model: "gpt-5-codex".to_string(),
            provider: Some("openai".to_string()),
            api_method: Some("responses".to_string()),
            detail: Some("coding model".to_string()),
            available: true,
        }],
        reasoning_effort: None,
        service_tier: None,
        compaction_mode: None,
    });

    assert_eq!(
        app.handle_key(KeyInput::Character("current".to_string())),
        KeyOutcome::Redraw
    );
    let picker = app
        .inline_widget_styled_lines()
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(picker.contains("custom-local-model"), "{picker}");
    assert!(!picker.contains("gpt-5-codex"), "{picker}");
}

#[test]
fn single_session_model_picker_cold_start_benchmark() {
    use std::time::Instant;

    let mut app = SingleSessionApp::new(None);
    let models = (0..5_000)
        .map(|index| session_launch::DesktopModelChoice {
            model: format!("provider-family-model-{index:04}"),
            provider: Some(format!("provider-{}", index % 17)),
            api_method: Some("responses".to_string()),
            detail: Some(format!("benchmark detail tier {}", index % 11)),
            available: true,
        })
        .collect::<Vec<_>>();

    let open_started = Instant::now();
    assert_eq!(
        app.handle_key(KeyInput::OpenModelPicker),
        KeyOutcome::LoadModelCatalog
    );
    let open_elapsed = open_started.elapsed();

    let catalog_started = Instant::now();
    app.apply_session_event(session_launch::DesktopSessionEvent::ModelCatalog {
        current_model: Some("provider-family-model-2500".to_string()),
        provider_name: Some("Benchmark".to_string()),
        models,
        reasoning_effort: None,
        service_tier: None,
        compaction_mode: None,
    });
    let catalog_elapsed = catalog_started.elapsed();

    let first_render_started = Instant::now();
    let lines = app.inline_widget_styled_lines();
    let first_render_elapsed = first_render_started.elapsed();
    assert!(lines.iter().any(|line| line.text.contains("5000 models")));

    let filter_started = Instant::now();
    assert_eq!(
        app.handle_key(KeyInput::Character("2500".to_string())),
        KeyOutcome::Redraw
    );
    let filter_elapsed = filter_started.elapsed();

    let filtered_render_started = Instant::now();
    let filtered_lines = app.inline_widget_styled_lines();
    let filtered_render_elapsed = filtered_render_started.elapsed();
    assert!(
        filtered_lines
            .iter()
            .any(|line| line.text.contains("provider-family-model-2500"))
    );

    eprintln!(
        "model_picker_cold_start_benchmark open={open_elapsed:?} catalog={catalog_elapsed:?} first_render={first_render_elapsed:?} filter={filter_elapsed:?} filtered_render={filtered_render_elapsed:?}"
    );
}

#[test]
fn desktop_model_choice_switch_spec_preserves_route_identity_state_space() {
    let choice =
        |model: &str, provider: &str, api_method: &str| session_launch::DesktopModelChoice {
            model: model.to_string(),
            provider: Some(provider.to_string()),
            api_method: Some(api_method.to_string()),
            detail: None,
            available: true,
        };

    for (model, provider, api_method, expected) in [
        ("gpt-5.5", "OpenAI", "openai-oauth", "openai-oauth:gpt-5.5"),
        ("gpt-5.5", "OpenAI", "openai-api-key", "openai-api:gpt-5.5"),
        (
            "claude-opus-4-6",
            "Anthropic",
            "claude-oauth",
            "claude-oauth:claude-opus-4-6",
        ),
        (
            "claude-opus-4-6",
            "claude",
            "oauth",
            "claude-oauth:claude-opus-4-6",
        ),
        (
            "claude-opus-4-6",
            "Anthropic",
            "claude-api",
            "claude-api:claude-opus-4-6",
        ),
        (
            "glm-51-nvfp4",
            "Comtegra GPU Cloud",
            "openai-compatible:comtegra",
            "comtegra:glm-51-nvfp4",
        ),
        (
            "claude-sonnet-4-6",
            "Copilot",
            "copilot",
            "copilot:claude-sonnet-4-6",
        ),
        ("gpt-5.5", "Cursor", "cursor", "cursor:gpt-5.5"),
        (
            "anthropic.claude-sonnet-4-6",
            "Bedrock",
            "bedrock",
            "bedrock:anthropic.claude-sonnet-4-6",
        ),
        (
            "gemini-3-pro",
            "Antigravity",
            "https",
            "antigravity:gemini-3-pro",
        ),
        (
            "openai/gpt-5.5",
            "OpenAI",
            "openrouter",
            "openai/gpt-5.5@OpenAI",
        ),
    ] {
        assert_eq!(
            desktop_model_choice_switch_spec(&choice(model, provider, api_method)),
            expected,
            "{provider} via {api_method} should not collapse to a bare model"
        );
    }
}

#[test]
fn model_choice_dedupe_preserves_distinct_variants_and_first_occurrence() {
    let choices = vec![
        session_launch::DesktopModelChoice {
            model: "gpt-5".to_string(),
            provider: Some("openai".to_string()),
            api_method: Some("responses".to_string()),
            detail: Some("fast".to_string()),
            available: true,
        },
        session_launch::DesktopModelChoice {
            model: "gpt-5".to_string(),
            provider: Some("openai".to_string()),
            api_method: Some("responses".to_string()),
            detail: Some("fast".to_string()),
            available: false,
        },
        session_launch::DesktopModelChoice {
            model: "gpt-5".to_string(),
            provider: Some("openai".to_string()),
            api_method: Some("chat".to_string()),
            detail: Some("fast".to_string()),
            available: true,
        },
        session_launch::DesktopModelChoice {
            model: "gpt-5-mini".to_string(),
            provider: Some("openai".to_string()),
            api_method: Some("responses".to_string()),
            detail: Some("fast".to_string()),
            available: true,
        },
    ];

    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(session_launch::DesktopSessionEvent::ModelCatalog {
        current_model: None,
        provider_name: Some("OpenAI".to_string()),
        models: choices,
        reasoning_effort: None,
        service_tier: None,
        compaction_mode: None,
    });

    let deduped = &app.model_picker.choices;

    assert_eq!(deduped.len(), 3);
    assert!(deduped[0].available, "first duplicate entry should win");
    assert_eq!(deduped[1].api_method.as_deref(), Some("chat"));
    assert_eq!(deduped[2].model, "gpt-5-mini");
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
    assert!(switcher.contains("Resume sessions"));
    assert!(switcher.contains("Recent sessions"));
    assert!(switcher.contains("Preview"));
    assert!(switcher.contains("alpha"));
    assert!(switcher.contains("beta"));
    assert!(switcher.contains("Assistant  alpha response"));

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
fn single_session_session_switcher_uses_split_rail_and_preview_text_areas() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    app.apply_session_switcher_cards(vec![test_session_card("session_alpha", "alpha", "active")]);

    let size = PhysicalSize::new(1100, 760);
    let key = single_session_text_key(&app, size);
    assert!(
        key.inline_widget
            .iter()
            .any(|line| line.text.contains("active session"))
    );
    assert!(
        key.inline_widget_preview
            .iter()
            .any(|line| line.text.contains("Preview"))
    );
    assert!(
        key.inline_widget_preview
            .iter()
            .any(|line| line.text.contains("Assistant  alpha response"))
    );

    let mut font_system = FontSystem::new();
    let buffers = single_session_text_buffers(&app, size, &mut font_system);
    let areas = single_session_text_areas_for_app(&app, &buffers, size);
    let rail_area = areas
        .iter()
        .find(|area| std::ptr::eq(area.buffer, &buffers[4]))
        .expect("resume rail text area");
    let preview_area = areas
        .iter()
        .find(|area| std::ptr::eq(area.buffer, &buffers[7]))
        .expect("resume preview text area");

    assert!(preview_area.left > rail_area.left + 240.0);
    assert!(preview_area.bounds.right > preview_area.bounds.left);
    assert!(preview_area.bounds.bottom > preview_area.bounds.top);
    assert!(buffer_has_layout_run_text(
        &buffers[7],
        "Assistant  alpha response"
    ));
}

#[test]
fn single_session_session_switcher_renders_tui_style_cards_and_role_preview() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    app.apply_session_switcher_cards(vec![workspace::SessionCard {
        session_id: "session_design".to_string(),
        title: "Design Session".to_string(),
        subtitle: "active · claude-sonnet-4-5".to_string(),
        detail: "8 msgs · just now · jcode".to_string(),
        preview_lines: vec!["user compact card prompt".to_string()],
        detail_lines: vec![
            "user first question".to_string(),
            "asst thoughtful answer".to_string(),
            "tool bash completed".to_string(),
            "sys system note".to_string(),
        ],
        transcript_messages: Vec::new(),
    }]);

    let styled = app.inline_widget_styled_lines();
    let switcher = styled
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(switcher.contains("Resume sessions · 1 sessions · all"));
    assert!(switcher.contains("Recent sessions · focused · newest first"));
    assert!(switcher.contains("Preview · selected session transcript"));
    assert!(switcher.contains("active session · Design Session"));
    assert!(switcher.contains("Status active · Model claude-sonnet-4-5"));
    assert!(switcher.contains("latest prompt: compact card prompt"));
    assert!(switcher.contains("Prompt 1  first question"));
    assert!(switcher.contains("Assistant  thoughtful answer"));
    assert!(switcher.contains("Tool  bash completed"));
    assert!(switcher.contains("System  system note"));

    let style_containing = |needle: &str| {
        styled
            .iter()
            .find(|line| line.text.contains(needle))
            .map(|line| line.style)
    };
    assert_eq!(
        style_containing("Prompt 1  first question"),
        Some(SingleSessionLineStyle::User)
    );
    assert_eq!(
        style_containing("Assistant  thoughtful answer"),
        Some(SingleSessionLineStyle::Assistant)
    );
    assert_eq!(
        style_containing("Tool  bash completed"),
        Some(SingleSessionLineStyle::Tool)
    );
}

#[test]
fn single_session_vertices_draw_session_switcher_preview_panes() {
    let mut app = SingleSessionApp::new(None);
    let size = PhysicalSize::new(1180, 760);
    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    app.apply_session_switcher_cards(vec![test_session_card("session_alpha", "alpha", "active")]);

    let vertices = build_single_session_vertices(&app, size, 0.0, 0);

    assert!(vertices_have_color(
        &vertices,
        INLINE_WIDGET_PREVIEW_PANE_BACKGROUND_COLOR
    ));
    assert!(vertices_have_color(
        &vertices,
        INLINE_WIDGET_PREVIEW_PANE_FOCUS_COLOR
    ));
}

#[test]
fn single_session_session_switcher_filter_supports_fuzzy_abbreviations() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    app.apply_session_switcher_cards(vec![
        test_session_card("session_alpha", "alpha-notes", "active"),
        test_session_card("session_ticket", "ticket-workspace", "closed"),
    ]);

    assert_eq!(
        app.handle_key(KeyInput::Character("tkw".to_string())),
        KeyOutcome::Redraw
    );
    let switcher = app
        .inline_widget_styled_lines()
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(app.session_switcher.filter, "tkw");
    assert!(switcher.contains("filter: tkw"), "{switcher}");
    assert!(switcher.contains("ticket-workspace"), "{switcher}");
    assert!(!switcher.contains("alpha-notes"), "{switcher}");
}

#[test]
fn single_session_session_switcher_filter_tolerates_transposed_typo() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    app.apply_session_switcher_cards(vec![
        test_session_card("session_alpha", "alpha-notes", "active"),
        test_session_card("session_ticket", "ticket-workspace", "closed"),
    ]);

    assert_eq!(
        app.handle_key(KeyInput::Character("tikcet".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.session_switcher.filtered_indices(), &[1]);
}

#[test]
fn single_session_session_switcher_filter_reports_visible_match_count() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    app.apply_session_switcher_cards(vec![
        test_session_card("session_alpha", "alpha", "active"),
        test_session_card("session_beta", "beta", "closed"),
    ]);

    assert_eq!(
        app.handle_key(KeyInput::Character("beta".to_string())),
        KeyOutcome::Redraw
    );
    let switcher = app
        .inline_widget_styled_lines()
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(switcher.contains("filter: beta"), "{switcher}");
    assert!(
        switcher.contains("Resume sessions · 1/2 sessions · filter beta"),
        "{switcher}"
    );
}

#[test]
fn single_session_resume_switcher_reopens_without_stale_filter_but_refresh_preserves_it() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    app.apply_session_switcher_cards(vec![
        test_session_card("session_alpha", "alpha", "active"),
        test_session_card("session_beta", "beta", "closed"),
    ]);

    assert_eq!(
        app.handle_key(KeyInput::Character("beta".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.session_switcher.filter, "beta");

    assert_eq!(
        app.handle_key(KeyInput::RefreshSessions),
        KeyOutcome::LoadSessionSwitcher
    );
    assert_eq!(
        app.session_switcher.filter, "beta",
        "explicit refresh should keep the user's current filter"
    );

    assert_eq!(app.handle_key(KeyInput::Escape), KeyOutcome::Redraw);
    assert!(!app.session_switcher.open);
    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    assert_eq!(
        app.session_switcher.filter, "",
        "fresh /resume opens must not inherit a stale filter that hides sessions"
    );
}

#[test]
fn single_session_resume_slash_preview_accepts_vim_navigation_keys() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::Character("/resume".to_string())),
        KeyOutcome::LoadSessionSwitcher
    );
    app.apply_session_switcher_cards(vec![
        test_session_card("session_alpha", "alpha", "active"),
        test_session_card("session_beta", "beta", "closed"),
    ]);

    assert_eq!(
        app.handle_key(KeyInput::Character("j".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.session_switcher.selected, 1);
    assert_eq!(app.draft, "/resume");
    assert_eq!(
        app.handle_key(KeyInput::Character("l".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.session_switcher.focus, SessionSwitcherPane::Preview);
    assert_eq!(
        app.handle_key(KeyInput::Character("h".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.session_switcher.focus, SessionSwitcherPane::Sessions);
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert_eq!(app.live_session_id.as_deref(), Some("session_beta"));
}

#[test]
fn single_session_resume_picker_switches_to_preview_pane_and_opens_terminal() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    app.apply_session_switcher_cards(vec![
        test_session_card("session_alpha", "alpha", "active"),
        test_session_card("session_beta", "beta", "closed"),
    ]);

    assert_eq!(
        app.handle_key(KeyInput::MoveCursorRight),
        KeyOutcome::Redraw
    );
    let switcher = app
        .inline_widget_styled_lines()
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(switcher.contains("focus: preview"));
    assert!(switcher.contains("Preview · focused · selected session transcript"));

    assert_eq!(app.handle_key(KeyInput::MoveCursorLeft), KeyOutcome::Redraw);
    assert_eq!(
        app.handle_key(KeyInput::ModelPickerMove(1)),
        KeyOutcome::Redraw
    );
    assert_eq!(
        app.handle_key(KeyInput::QueueDraft),
        KeyOutcome::OpenSession {
            session_id: "session_beta".to_string(),
            title: "beta".to_string(),
        }
    );
}

#[test]
fn single_session_resume_picker_accepts_vim_navigation_keys() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    app.apply_session_switcher_cards(vec![
        test_session_card("session_alpha", "alpha", "active"),
        test_session_card("session_beta", "beta", "closed"),
    ]);

    assert_eq!(
        app.handle_key(KeyInput::Character("j".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.session_switcher.selected, 1);
    assert_eq!(
        app.handle_key(KeyInput::Character("k".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.session_switcher.selected, 0);
    assert_eq!(
        app.handle_key(KeyInput::Character("l".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.session_switcher.focus, SessionSwitcherPane::Preview);
    assert_eq!(
        app.handle_key(KeyInput::Character("h".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.session_switcher.focus, SessionSwitcherPane::Sessions);
    assert_eq!(
        app.handle_key(KeyInput::Character("G".to_string())),
        KeyOutcome::Redraw
    );
    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert_eq!(app.live_session_id.as_deref(), Some("session_beta"));
}

#[test]
fn switcher_resume_defers_transcript_hydration_off_key_path() {
    let mut app = SingleSessionApp::new(None);
    assert_eq!(
        app.handle_key(KeyInput::OpenSessionSwitcher),
        KeyOutcome::LoadSessionSwitcher
    );
    app.apply_session_switcher_cards(vec![test_session_card("session_alpha", "alpha", "active")]);

    assert_eq!(app.handle_key(KeyInput::SubmitDraft), KeyOutcome::Redraw);
    assert_eq!(app.live_session_id.as_deref(), Some("session_alpha"));
    assert_eq!(
        app.take_pending_transcript_hydration().as_deref(),
        Some("session_alpha"),
        "switcher resume should queue hydration for the event loop instead of \
         blocking the key handler on a disk parse"
    );
    assert_eq!(app.take_pending_transcript_hydration(), None);

    // A hydrated transcript for the live session applies...
    let applied = app.apply_hydrated_transcript(
        "session_alpha",
        Ok(Some(vec![session_data::SessionTranscriptMessage {
            role: "user".to_string(),
            content: "hydrated prompt".to_string(),
        }])),
    );
    assert!(applied);
    assert!(app.body_lines().join("\n").contains("hydrated prompt"));

    // ...but a stale result for a different session is dropped.
    let stale = app.apply_hydrated_transcript(
        "session_other",
        Ok(Some(vec![session_data::SessionTranscriptMessage {
            role: "user".to_string(),
            content: "stale prompt".to_string(),
        }])),
    );
    assert!(!stale);
    assert!(!app.body_lines().join("\n").contains("stale prompt"));
}

#[test]
fn single_session_resumed_transcript_hydration_replaces_card_preview() {
    let mut app =
        SingleSessionApp::new(Some(test_session_card("session_alpha", "alpha", "closed")));

    app.apply_resumed_session_transcript(vec![
        session_data::SessionTranscriptMessage {
            role: "user".to_string(),
            content: "previous prompt".to_string(),
        },
        session_data::SessionTranscriptMessage {
            role: "assistant".to_string(),
            content: "previous answer".to_string(),
        },
    ]);

    let body = app.body_lines().join("\n");
    assert!(body.contains("previous prompt"));
    assert!(body.contains("previous answer"));
    assert!(!body.contains("assistant alpha response"));
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
            .contains("active session · current · alpha")
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
            .contains("Current  OpenAI · gpt-5.4")
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
    // Exhaustive DFS over action sequences. Depth 4 (~11k traces) covers every
    // length-<=4 action ordering and stays tractable under the default debug
    // test profile; depth 5 is ~111k traces and made `cargo test -p jcode-desktop`
    // appear to hang for over a minute in debug builds.
    visit(&mut trace, 4);
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
fn desktop_reload_notice_is_visible_without_replacing_window() {
    let mut app = fresh_single_session_app();

    show_desktop_reload_notice(&mut app);

    let snapshot = app.debug_snapshot();
    assert_eq!(snapshot.mode, "single_session");
    assert_eq!(snapshot.status.as_deref(), Some("desktop UI reloaded"));
    assert!(snapshot.body_text.contains("desktop UI reloaded"));
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
fn desktop_user_facing_surfaces_are_marked_beta() {
    // Window/status titles, help text, and in-app version labels must all
    // carry the beta channel until the desktop app graduates.
    assert!(DESKTOP_PRODUCT_NAME.contains("Beta"));
    assert!(desktop_help_text().contains("Beta"));
    assert!(SingleSessionApp::new(None).status_title().contains("Beta"));
    assert!(
        crate::single_session_render::desktop_header_version_label()
            .starts_with(DESKTOP_RELEASE_CHANNEL)
    );
    assert!(
        crate::single_session_render::fresh_welcome_version_label()
            .starts_with(DESKTOP_RELEASE_CHANNEL)
    );
    let workspace = workspace::Workspace::fake();
    assert!(workspace.status_title().contains("Beta"));
}

#[test]
fn desktop_help_text_documents_desktop_options() {
    let help = desktop_help_text();

    assert!(help.starts_with(DESKTOP_PRODUCT_NAME));
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
fn single_session_scroll_motion_animates_keyboard_and_snap_targets() {
    let mut motion = SingleSessionScrollMotion::default();
    let now = Instant::now();

    let initial = motion.frame(0.0, now);
    assert_eq!(initial.visual_scroll_lines, 0.0);
    assert_eq!(initial.smooth_scroll_lines, 0.0);
    assert!(!initial.active);

    let start = motion.frame(12.0, now);
    assert_eq!(start.visual_scroll_lines, 0.0);
    assert_eq!(start.smooth_scroll_lines, -12.0);
    assert!(start.active);

    let middle = motion.frame(12.0, now + SINGLE_SESSION_SCROLL_ANIMATION_DURATION / 2);
    assert!(middle.visual_scroll_lines > 0.0);
    assert!(middle.visual_scroll_lines < 12.0);
    assert!(middle.smooth_scroll_lines < 0.0);
    assert!(middle.active);

    let interrupted = motion.frame(0.0, now + SINGLE_SESSION_SCROLL_ANIMATION_DURATION / 2);
    assert_eq!(interrupted.visual_scroll_lines, middle.visual_scroll_lines);
    assert!(interrupted.smooth_scroll_lines > 0.0);
    assert!(interrupted.active);

    let settled = motion.frame(0.0, now + SINGLE_SESSION_SCROLL_ANIMATION_DURATION * 2);
    assert_eq!(settled.visual_scroll_lines, 0.0);
    assert_eq!(settled.smooth_scroll_lines, 0.0);
    assert!(!settled.active);
}

#[test]
fn reduced_motion_disables_single_session_scroll_motion() {
    let _guard = DesktopReducedMotionEnvGuard::set(true);
    let mut motion = SingleSessionScrollMotion::default();
    let now = Instant::now();

    assert_eq!(motion.frame(0.0, now).visual_scroll_lines, 0.0);
    let snapped = motion.frame(12.0, now + Duration::from_millis(16));
    assert_eq!(snapped.visual_scroll_lines, 12.0);
    assert_eq!(snapped.smooth_scroll_lines, 0.0);
    assert!(!snapped.active);
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
fn fallback_draft_caret_position_uses_text_scale() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("hello\nworld".to_string()));
    app.handle_key(KeyInput::AdjustTextScale(1));
    let size = PhysicalSize::new(640, 480);

    let caret = approximate_draft_caret_position(&app, size);
    let typography = single_session_typography_for_scale(app.text_scale());
    let expected_y = single_session_draft_top_for_app(&app, size)
        + typography.code_size * typography.code_line_height;

    assert_eq!(caret.y, expected_y);
    assert_eq!(caret.height, typography.code_size * 1.12);
}

#[test]
fn composer_draft_buffer_rebuilds_when_user_font_changes() {
    let mut app = SingleSessionApp::new(None);
    app.handle_key(KeyInput::Character("stale input".to_string()));
    let size = PhysicalSize::new(640, 480);
    let mut font_system = FontSystem::new();
    let stale_key = single_session_text_key(&app, size);
    let old_buffers = single_session_text_buffers_from_key(&stale_key, size, &mut font_system);

    let mut previous_key = stale_key.clone();
    previous_key.draft = "typed input".to_string();
    previous_key.user_font_family = SINGLE_SESSION_USER_FONT_FAMILY;

    let mut next_key = previous_key.clone();
    next_key.user_font_family = "Patrick Hand";

    let buffers = single_session_text_buffers_from_key_reusing_unchanged(
        &next_key,
        Some(&previous_key),
        old_buffers,
        false,
        size,
        &mut font_system,
    );

    assert!(buffer_has_layout_run_text(&buffers[2], "typed input"));
    assert!(!buffer_has_layout_run_text(&buffers[2], "stale input"));
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
            .any(|line| line.text.contains("Choose model"))
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
    let inline_area = areas
        .iter()
        .find(|area| std::ptr::eq(area.buffer, &buffers[4]))
        .expect("inline model picker text area");
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
    // The welcome hero/version chrome is shifted up by the welcome timeline
    // offset while an inline widget is open, so the unshifted
    // handwritten_welcome_bounds cannot be compared against the inline area
    // directly. The version label renders below the hero with the same
    // offset applied, so staying below its bounds keeps the picker clear of
    // the hero as well (asserted above against version_area).
    assert!(
        inline_area.top >= version_area.bounds.bottom as f32,
        "fresh inline picker must stay below the offset welcome chrome"
    );
    assert!(
        inline_area.bounds.bottom > inline_area.bounds.top,
        "fresh inline picker should keep a visible clipped lane"
    );

    let vertices = build_single_session_vertices(&app, size, 0.0, 0);
    let inline_card_vertices = positions_for_color(&vertices, MODEL_PICKER_CARD_BACKGROUND_COLOR);
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
fn inline_widget_card_never_overlaps_body_clip_during_reveal() {
    let kinds = [
        InlineWidgetKind::HotkeyHelp,
        InlineWidgetKind::ModelPicker,
        InlineWidgetKind::SessionSwitcher,
        InlineWidgetKind::SessionInfo,
        InlineWidgetKind::SlashSuggestions,
    ];
    let sizes = [
        PhysicalSize::new(1000, 720),
        PhysicalSize::new(760, 520),
        PhysicalSize::new(640, 420),
    ];
    let reveal_steps = [0.0_f32, 0.25, 0.5, 0.75, 1.0];

    for size in sizes {
        let body_base_bottom = single_session_body_bottom(size);
        for kind in kinds {
            let visible_lines = kind.visible_line_limit().clamp(1, 8);
            for activity_reserved_height in [0.0, 22.0] {
                for reveal_progress in reveal_steps {
                    let Some((body_bottom, card_top)) =
                        inline_widget_body_and_card_vertical_geometry_for_test(
                            size,
                            Some(kind),
                            1.0,
                            body_base_bottom,
                            visible_lines,
                            420.0,
                            reveal_progress,
                            activity_reserved_height,
                        )
                    else {
                        assert!(reveal_progress <= 0.001);
                        continue;
                    };

                    assert!(
                        body_bottom.ceil() <= card_top + 0.001,
                        "{kind:?} overlaps body during reveal: size={size:?} progress={reveal_progress} activity={activity_reserved_height} body_bottom={body_bottom} card_top={card_top}"
                    );
                }
            }
        }
    }
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
        transcript_messages: Vec::new(),
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

    assert_eq!(app.status_title(), "Jcode Desktop (Beta) · fresh session");
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
        transcript_messages: Vec::new(),
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
        transcript_messages: Vec::new(),
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
fn workspace_focused_session_promotes_to_single_session_app() {
    let mut app = DesktopApp::Workspace(Workspace::from_session_cards(vec![
        workspace::SessionCard {
            session_id: "session_alpha".to_string(),
            title: "alpha".to_string(),
            subtitle: "active".to_string(),
            detail: "3 msgs".to_string(),
            preview_lines: vec!["user hello".to_string()],
            detail_lines: vec!["assistant hi".to_string()],
            transcript_messages: Vec::new(),
        },
    ]));

    assert!(app.promote_focused_workspace_session());
    let snapshot = app.debug_snapshot();
    assert_eq!(snapshot.mode, "single_session");
    assert_eq!(snapshot.live_session_id.as_deref(), Some("session_alpha"));
    assert!(snapshot.body_text.contains("user hello"));
}

#[test]
fn workspace_session_panel_app_uses_single_session_body_and_focused_draft() {
    let mut workspace = Workspace::from_session_cards(vec![workspace::SessionCard {
        session_id: "session_alpha".to_string(),
        title: "alpha".to_string(),
        subtitle: "active".to_string(),
        detail: "3 msgs".to_string(),
        preview_lines: vec!["user hello".to_string()],
        detail_lines: vec!["assistant hi".to_string()],
        transcript_messages: Vec::new(),
    }]);
    workspace.handle_key(KeyInput::Character("i".to_string()));
    workspace.handle_key(KeyInput::Character("draft text".to_string()));

    let surface = workspace.focused_surface().expect("focused surface");
    let app = workspace_single_session_app_for_surface(&workspace, surface)
        .expect("session surface should become a single-session app");
    let body = app.body_lines().join("\n");

    assert_eq!(app.live_session_id.as_deref(), Some("session_alpha"));
    assert_eq!(app.draft, "draft text");
    assert!(body.contains("single session mode"));
    assert!(body.contains("user hello"));
    assert!(body.contains("assistant hi"));
    assert!(!body.contains("session metadata"));
}

#[test]
fn workspace_session_panel_prefers_filtered_transcript_over_card_preview() {
    let workspace = Workspace::from_session_cards(vec![workspace::SessionCard {
        session_id: "session_alpha".to_string(),
        title: "alpha".to_string(),
        subtitle: "active".to_string(),
        detail: "3 msgs".to_string(),
        preview_lines: vec![
            "user <system-reminder>raw startup context</system-reminder>".to_string(),
        ],
        detail_lines: vec!["expanded transcript raw fallback".to_string()],
        transcript_messages: vec![
            workspace::SessionTranscriptMessage {
                role: "user".to_string(),
                content: "clean workspace prompt".to_string(),
            },
            workspace::SessionTranscriptMessage {
                role: "assistant".to_string(),
                content: "clean workspace answer".to_string(),
            },
        ],
    }]);

    let surface = workspace.focused_surface().expect("focused surface");
    let app = workspace_single_session_app_for_surface(&workspace, surface)
        .expect("session surface should become a single-session app");
    let body = app.body_lines().join("\n");

    assert!(body.contains("clean workspace prompt"));
    assert!(body.contains("clean workspace answer"));
    assert!(!body.contains("system-reminder"));
    assert!(!body.contains("raw startup context"));
    assert!(!body.contains("single session mode"));
    assert!(!body.contains("recent transcript"));
    assert!(!body.contains("expanded transcript raw fallback"));
}

#[test]
fn workspace_session_panel_composes_single_session_geometry() {
    let workspace = Workspace::from_session_cards(vec![workspace::SessionCard {
        session_id: "session_alpha".to_string(),
        title: "alpha".to_string(),
        subtitle: "active".to_string(),
        detail: "3 msgs".to_string(),
        preview_lines: vec!["user hello".to_string()],
        detail_lines: vec!["assistant hi".to_string()],
        transcript_messages: Vec::new(),
    }]);
    let size = PhysicalSize::new(1280, 800);
    let render_layout = workspace_render_layout(&workspace, size, None);
    let focused_id = workspace.focused_id;
    let mut panel_rect = None;
    for_each_visible_workspace_surface(
        &workspace,
        size,
        render_layout,
        0.0,
        |surface, rect, _, _| {
            if surface.id == focused_id {
                panel_rect = Some(rect);
            }
        },
    );
    let rect = panel_rect.expect("focused panel rect");
    let panel_app = workspace_single_session_app_for_surface(
        &workspace,
        workspace.focused_surface().expect("focused surface"),
    )
    .expect("single-session panel app");
    let panel_size = workspace_panel_size(rect);
    let rendered_body_lines =
        single_session_rendered_body_lines_for_tick(&panel_app, panel_size, 0);
    let child_vertices = build_single_session_vertices_with_cached_body(
        &panel_app,
        panel_size,
        0.0,
        0,
        0.0,
        1.0,
        &rendered_body_lines,
    );

    let mut vertices = Vec::new();
    build_vertices_into(
        WorkspaceVertexBuildParams {
            workspace: &workspace,
            size,
            render_layout,
            focus_pulse: 0.0,
            space_hold_progress: None,
            surface_frames: None,
            exiting_surfaces: &HashMap::new(),
            workspace_panel_cache: None,
            status_color: workspace_status_bar_target_color(&workspace),
            status_text_frame: None,
        },
        &mut vertices,
    );

    assert!(
        vertices.len() >= child_vertices.len() + 6,
        "workspace should include the child single-session primitive geometry"
    );

    let mut expected_panel_vertices = Vec::new();
    append_child_vertices_to_parent_with_opacity(
        &mut expected_panel_vertices,
        &child_vertices,
        panel_size,
        rect,
        size,
        1.0,
    );
    let actual_panel_vertices = &vertices[vertices.len() - expected_panel_vertices.len()..];
    assert_eq!(
        actual_panel_vertices.len(),
        expected_panel_vertices.len(),
        "workspace panel should contribute exactly the transformed single-session primitive"
    );
    for (index, (actual, expected)) in actual_panel_vertices
        .iter()
        .zip(expected_panel_vertices.iter())
        .enumerate()
    {
        assert!(
            (actual.position[0] - expected.position[0]).abs() < 0.000_01
                && (actual.position[1] - expected.position[1]).abs() < 0.000_01,
            "workspace session panel vertex {index} position diverged from transformed single-session primitive: actual={:?} expected={:?}",
            actual.position,
            expected.position
        );
        assert!(
            actual
                .color
                .iter()
                .zip(expected.color.iter())
                .all(|(actual, expected)| (actual - expected).abs() < 0.000_01),
            "workspace session panel vertex {index} color diverged from transformed single-session primitive: actual={:?} expected={:?}",
            actual.color,
            expected.color
        );
    }
}

#[test]
fn workspace_session_panel_reuses_single_session_primitive_exactly() {
    let mut workspace = Workspace::from_session_cards(vec![workspace::SessionCard {
        session_id: "session_alpha".to_string(),
        title: "alpha".to_string(),
        subtitle: "active".to_string(),
        detail: "3 msgs".to_string(),
        preview_lines: vec!["user hello".to_string()],
        detail_lines: vec!["assistant hi".to_string()],
        transcript_messages: Vec::new(),
    }]);
    workspace.handle_key(KeyInput::Character("i".to_string()));
    workspace.handle_key(KeyInput::Character("draft text".to_string()));

    let size = PhysicalSize::new(1280, 800);
    let render_layout = workspace_render_layout(&workspace, size, None);
    let mut panel_rect = None;
    for_each_visible_workspace_surface(
        &workspace,
        size,
        render_layout,
        0.0,
        |surface, rect, _, _| {
            if workspace.is_focused(surface.id) {
                panel_rect = Some(rect);
            }
        },
    );
    let rect = panel_rect.expect("focused panel rect");
    let app = workspace_single_session_app_for_surface(
        &workspace,
        workspace.focused_surface().expect("focused surface"),
    )
    .expect("workspace session surface should map to a single-session app");
    assert_eq!(app.draft, "draft text");

    let panel_size = workspace_panel_size(rect);
    let rendered_body_lines = single_session_rendered_body_lines_for_tick(&app, panel_size, 0);
    let single_session_vertices = build_single_session_vertices_with_cached_body(
        &app,
        panel_size,
        0.0,
        0,
        0.0,
        1.0,
        &rendered_body_lines,
    );
    let mut expected_panel_vertices = Vec::new();
    append_child_vertices_to_parent_with_opacity(
        &mut expected_panel_vertices,
        &single_session_vertices,
        panel_size,
        rect,
        size,
        1.0,
    );

    let mut workspace_vertices = Vec::new();
    build_vertices_into(
        WorkspaceVertexBuildParams {
            workspace: &workspace,
            size,
            render_layout,
            focus_pulse: 0.0,
            space_hold_progress: None,
            surface_frames: None,
            exiting_surfaces: &HashMap::new(),
            workspace_panel_cache: None,
            status_color: workspace_status_bar_target_color(&workspace),
            status_text_frame: None,
        },
        &mut workspace_vertices,
    );

    let actual_panel_vertices =
        &workspace_vertices[workspace_vertices.len() - expected_panel_vertices.len()..];
    assert_eq!(actual_panel_vertices.len(), expected_panel_vertices.len());
    for (index, (actual, expected)) in actual_panel_vertices
        .iter()
        .zip(expected_panel_vertices.iter())
        .enumerate()
    {
        assert!(
            (actual.position[0] - expected.position[0]).abs() < 0.000_01
                && (actual.position[1] - expected.position[1]).abs() < 0.000_01,
            "workspace session panel vertex {index} position diverged from standalone single-session primitive"
        );
        assert!(
            actual
                .color
                .iter()
                .zip(expected.color.iter())
                .all(|(actual, expected)| (actual - expected).abs() < 0.000_01),
            "workspace session panel vertex {index} color diverged from standalone single-session primitive"
        );
    }
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
        transcript_messages: Vec::new(),
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
        transcript_messages: Vec::new(),
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

fn streaming_cache_test_app() -> SingleSessionApp {
    let mut app = SingleSessionApp::new(Some(test_session_card(
        "streaming-cache-session",
        "cache session",
        "active",
    )));
    app.messages
        .push(SingleSessionMessage::assistant("settled response"));
    app.streaming_response = "streaming partial response".to_string();
    app
}

fn streaming_primitive_geometry_cache_key_for_test(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    spinner_tick: u64,
    activity_cue_motion_cache_key: u64,
) -> Option<u64> {
    let rendered_body_lines = single_session_rendered_body_lines_for_tick(app, size, spinner_tick);
    single_session_streaming_primitive_geometry_cache_key(
        app,
        size,
        0.0,
        spinner_tick,
        0.0,
        1.0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        activity_cue_motion_cache_key,
        0,
        Some(app.rendered_body_cache_key((size.width, size.height))),
        rendered_body_lines.len(),
    )
}

#[test]
fn streaming_primitive_geometry_cache_key_is_stable_for_idle_motion_inputs() {
    let size = PhysicalSize::new(900, 700);
    let app = streaming_cache_test_app();

    let first = streaming_primitive_geometry_cache_key_for_test(&app, size, 0, 0)
        .expect("plain streaming transcript should be cacheable");
    let second = streaming_primitive_geometry_cache_key_for_test(&app, size, 0, 0)
        .expect("same streaming transcript should stay cacheable");
    assert_eq!(first, second);

    let spinner_changed = streaming_primitive_geometry_cache_key_for_test(&app, size, 1, 0)
        .expect("spinner tick remains eligible but invalidates animated dots");
    assert_ne!(first, spinner_changed);

    let motion_changed = streaming_primitive_geometry_cache_key_for_test(&app, size, 0, 99)
        .expect("activity cue motion remains eligible but invalidates geometry");
    assert_ne!(first, motion_changed);

    let mut scrolled = app.clone();
    scrolled.body_scroll_lines = 2.0;
    let scrolled_key = streaming_primitive_geometry_cache_key_for_test(&scrolled, size, 0, 0)
        .expect("scroll position remains eligible but invalidates geometry");
    assert_ne!(first, scrolled_key);
}

#[test]
fn streaming_primitive_geometry_cache_key_rejects_dynamic_overlay_states() {
    let size = PhysicalSize::new(900, 700);
    let base = streaming_cache_test_app();
    assert!(streaming_primitive_geometry_cache_key_for_test(&base, size, 0, 0).is_some());

    let mut no_streaming = base.clone();
    no_streaming.streaming_response.clear();
    assert!(streaming_primitive_geometry_cache_key_for_test(&no_streaming, size, 0, 0).is_none());

    let mut help = base.clone();
    help.show_help = true;
    assert!(streaming_primitive_geometry_cache_key_for_test(&help, size, 0, 0).is_none());

    let mut model_picker = base.clone();
    model_picker.model_picker.open = true;
    assert!(streaming_primitive_geometry_cache_key_for_test(&model_picker, size, 0, 0).is_none());

    let mut session_switcher = base.clone();
    session_switcher.session_switcher.open = true;
    assert!(
        streaming_primitive_geometry_cache_key_for_test(&session_switcher, size, 0, 0).is_none()
    );

    let mut stdin_overlay = base.clone();
    stdin_overlay.stdin_response = Some(StdinResponseState {
        request_id: "request".to_string(),
        prompt: "continue?".to_string(),
        is_password: false,
        tool_call_id: "tool".to_string(),
        input: String::new(),
    });
    assert!(streaming_primitive_geometry_cache_key_for_test(&stdin_overlay, size, 0, 0).is_none());

    let mut welcome = SingleSessionApp::new(None);
    welcome.streaming_response = "fresh welcome stream".to_string();
    assert!(streaming_primitive_geometry_cache_key_for_test(&welcome, size, 0, 0).is_none());
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
        transcript_messages: Vec::new(),
    }]);
    let expected = workspace.preferences();
    let (tx, rx) = mpsc::channel();

    queue_desktop_preferences_save(&workspace, &Some(tx));

    assert_eq!(rx.try_recv().ok(), Some(expected));
    assert!(rx.try_recv().is_err());
}
