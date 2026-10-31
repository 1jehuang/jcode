fn seed_stale_clear_usage(app: &mut App) {
    app.streaming.streaming_input_tokens = 40_000;
    app.streaming.streaming_output_tokens = 2_000;
    app.streaming.streaming_cache_read_tokens = Some(30_000);
    app.streaming.streaming_cache_creation_tokens = Some(5_000);
    app.streaming.streaming_context_stale = true;
    app.streaming.streaming_usage_call_reset_pending = true;
    app.kv_cache.current_api_usage_recorded = true;
}

fn assert_clear_usage_reset(app: &App) {
    assert_eq!(app.current_stream_context_tokens(), None);
    assert_eq!(app.streaming.streaming_input_tokens, 0);
    assert_eq!(app.streaming.streaming_output_tokens, 0);
    assert_eq!(app.streaming.streaming_cache_read_tokens, None);
    assert_eq!(app.streaming.streaming_cache_creation_tokens, None);
    assert!(!app.streaming.streaming_context_stale);
    assert!(!app.streaming.streaming_usage_call_reset_pending);
    assert!(!app.kv_cache.current_api_usage_recorded);
}

fn seed_stale_clear_image(app: &mut App) -> u64 {
    app.remote_side_pane_images = vec![crate::session::RenderedImage {
        history_message_index: None,
        media_type: "image/png".to_string(),
        data: "stale-image".to_string(),
        label: Some("stale.png".to_string()),
        source: crate::session::RenderedImageSource::UserInput,
        anchor: None,
    }];
    let _ = crate::tui::TuiState::side_pane_images_signature(app);
    app.expanded_images_version
}

fn assert_clear_image_reset(app: &App, previous_version: u64) {
    assert!(app.remote_side_pane_images.is_empty());
    assert_eq!(app.side_pane_images_signature_cache.get(), None);
    assert!(app.expanded_images.is_empty());
    assert_eq!(
        app.expanded_images_version,
        previous_version.wrapping_add(1)
    );
}

#[test]
fn local_clear_resets_provider_reported_context_usage() {
    let mut app = create_test_app();
    seed_stale_clear_usage(&mut app);
    seed_stale_clear_swarm_plan(&mut app);
    let image_version = seed_stale_clear_image(&mut app);

    assert!(super::commands::handle_session_command(&mut app, "/clear"));

    assert_clear_usage_reset(&app);
    assert_clear_swarm_plan_reset(&app);
    assert_clear_image_reset(&app, image_version);
}

#[test]
fn remote_clear_resets_provider_reported_context_usage() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    app.is_remote = true;
    seed_stale_clear_usage(&mut app);
    seed_stale_clear_swarm_plan(&mut app);
    let image_version = seed_stale_clear_image(&mut app);
    app.input = "/clear".to_string();
    app.cursor_pos = app.input.len();

    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .expect("remote /clear should succeed");

    assert_clear_usage_reset(&app);
    assert_clear_swarm_plan_reset(&app);
    assert_clear_image_reset(&app, image_version);
}

fn seed_stale_clear_swarm_plan(app: &mut App) {
    app.swarm_plan_items = vec![crate::plan::PlanItem {
        content: "old session task".to_string(),
        status: "queued".to_string(),
        priority: "high".to_string(),
        id: "old-task".to_string(),
        subsystem: None,
        file_scope: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
    }];
    app.swarm_plan_version = Some(19);
    app.swarm_plan_swarm_id = Some("old-swarm".to_string());
}

fn assert_clear_swarm_plan_reset(app: &App) {
    assert!(app.swarm_plan_items.is_empty());
    assert_eq!(app.swarm_plan_version, None);
    assert_eq!(app.swarm_plan_swarm_id, None);
}
