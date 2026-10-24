use super::*;

fn app() -> App {
    let mut app = super::super::tests::create_test_app();
    app.is_remote = true;
    app.remote_session_id = Some("panel-test-a".into());
    app
}

fn inventory(session: &str, revision: u64) -> crate::side_panel::SidePanelSnapshot {
    crate::side_panel::SidePanelSnapshot {
        session_id: Some(session.into()),
        revision,
        focus_revision: 1,
        focused_page_id: Some("first".into()),
        pages: ["first", "second"]
            .into_iter()
            .map(|id| crate::side_panel::SidePanelPage {
                id: id.into(),
                title: id.into(),
                content: format!("# {id}"),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

fn select_second(app: &mut App) {
    app.remember_side_panel_viewport();
    app.side_panel.focused_page_id = Some("second".into());
    app.last_side_panel_focus_id = Some("second".into());
    app.diff_pane_scroll = 17;
    app.diff_pane_scroll_x = 6;
    app.side_panel_image_zoom_percent = 140;
    app.diff_pane_auto_scroll = false;
}

fn assert_second(app: &App) {
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("second"));
    assert_eq!(app.diff_pane_scroll, 17);
    assert_eq!(app.diff_pane_scroll_x, 6);
    assert_eq!(app.side_panel_image_zoom_percent, 140);
    assert!(!app.diff_pane_auto_scroll);
}

fn history(
    session: &str,
    snapshot: crate::side_panel::SidePanelSnapshot,
) -> crate::protocol::ServerEvent {
    serde_json::from_value(serde_json::json!({
        "type": "history", "id": 45, "session_id": session, "messages": [],
        "side_panel": snapshot,
    }))
    .unwrap()
}

#[test]
fn side_panel_model_success_failure_and_catalog_handlers_keep_local_view() {
    let mut app = app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.remote_provider_name = Some("anthropic".into());
    app.remote_provider_model = Some("claude-test-one".into());
    app.set_side_panel_snapshot(inventory("panel-test-a", 0));
    select_second(&mut app);
    for (provider, model, error) in [
        ("anthropic", "claude-test-two", None),
        ("openai", "gpt-test", None),
        ("openai", "missing-model", Some("not available")),
    ] {
        let event = serde_json::from_value(serde_json::json!({
            "type": "model_changed", "id": 42, "provider_name": provider,
            "model": model, "error": error,
        }))
        .unwrap();
        app.handle_server_event(event, &mut remote);
        assert_second(&app);
        // The typed catalog uses History but carries no panel inventory.
        app.handle_server_event(history("panel-test-a", Default::default()), &mut remote);
        assert_second(&app);
    }
}

#[test]
fn side_panel_same_session_history_handler_preserves_selection_and_viewport() {
    let mut app = app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.set_side_panel_snapshot(inventory("panel-test-a", 1));
    select_second(&mut app);
    let mut updated = inventory("panel-test-a", 2);
    updated.pages[1].content = "changed body".into();
    app.handle_server_event(history("panel-test-a", updated), &mut remote);
    assert_second(&app);
    assert_eq!(app.side_panel.pages[1].content, "changed body");
}

#[test]
fn side_panel_session_a_b_a_restores_view_without_cross_session_pages() {
    let mut app = app();
    app.set_side_panel_snapshot(inventory("panel-test-a", 1));
    select_second(&mut app);
    app.side_panel_fullscreen = true;
    app.remote_session_id = Some("panel-test-b".into());
    let mut b = inventory("panel-test-b", 1);
    b.pages.clear();
    b.focused_page_id = None;
    app.set_side_panel_snapshot(b);
    assert!(app.side_panel.pages.is_empty());
    assert!(!app.side_panel_fullscreen);
    assert!(!app.side_panel_user_hidden);
    app.remote_session_id = Some("panel-test-a".into());
    app.set_side_panel_snapshot(inventory("panel-test-a", 1));
    assert_second(&app);
    assert!(app.side_panel_fullscreen);
}

#[test]
fn side_panel_unavailable_retains_last_good_but_empty_inventory_deletes() {
    let mut app = app();
    app.set_side_panel_snapshot(inventory("panel-test-a", 4));
    select_second(&mut app);
    app.set_side_panel_snapshot(crate::side_panel::SidePanelSnapshot {
        session_id: Some("panel-test-a".into()),
        unavailable: Some("loading".into()),
        ..Default::default()
    });
    assert_second(&app);
    assert_eq!(app.side_panel.pages.len(), 2);
    assert_eq!(app.side_panel.unavailable.as_deref(), Some("loading"));
    app.set_side_panel_snapshot(crate::side_panel::SidePanelSnapshot {
        session_id: Some("panel-test-a".into()),
        revision: 5,
        ..Default::default()
    });
    assert!(app.side_panel.pages.is_empty());
    app.set_side_panel_snapshot(inventory("panel-test-a", 4));
    assert!(
        app.side_panel.pages.is_empty(),
        "old revision must not resurrect deleted pages"
    );
    app.set_side_panel_snapshot(inventory("different-session", 99));
    assert!(app.side_panel.pages.is_empty());
}

#[test]
fn side_panel_explicit_focus_reveals_but_content_update_does_not() {
    let mut app = app();
    app.set_side_panel_snapshot(inventory("panel-test-a", 1));
    select_second(&mut app);
    app.dismiss_side_panel();
    app.set_side_panel_snapshot(inventory("panel-test-a", 2));
    assert!(app.side_panel_user_hidden);
    assert!(app.side_panel.focused_page_id.is_none());
    let mut focused = inventory("panel-test-a", 3);
    focused.focus_revision = 2;
    app.set_side_panel_snapshot(focused);
    assert!(!app.side_panel_user_hidden);
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("first"));
}

#[test]
fn side_panel_reexec_file_restores_hidden_and_viewport() {
    let mut first = app();
    first.set_side_panel_snapshot(inventory("panel-test-a", 1));
    select_second(&mut first);
    first.dismiss_side_panel();
    first.save_panel_view_for_reload();
    let mut second = app();
    second.set_side_panel_snapshot(inventory("panel-test-a", 1));
    assert!(second.side_panel_user_hidden);
    assert_eq!(second.last_side_panel_focus_id.as_deref(), Some("second"));
    second.toggle_side_panel();
    assert_second(&second);
}

#[test]
fn side_panel_markdown_link_is_registered_in_session_inventory() {
    let mut app = app();
    app.is_remote = false;
    let repository = tempfile::tempdir().unwrap();
    std::fs::write(repository.path().join("note.md"), "# retained link").unwrap();
    app.session.working_dir = Some(repository.path().display().to_string());
    assert!(app.try_open_repository_markdown_link("note.md"));
    let snapshot = crate::side_panel::snapshot_for_session(&app.session.id).unwrap();
    assert_eq!(snapshot.pages.len(), 1);
    assert_eq!(snapshot.pages[0].content, "# retained link");
    assert_eq!(app.side_panel.focused_page_id, snapshot.focused_page_id);
}

#[test]
fn side_panel_ctrl_r_inherits_provenance_but_new_session_starts_empty() {
    let mut app = app();
    app.is_remote = false;
    let old_session = app.session.id.clone();
    let snapshot = crate::side_panel::write_markdown_page(
        &old_session,
        "recovery",
        Some("Recovery"),
        "saved body",
        true,
    )
    .unwrap();
    app.set_side_panel_snapshot(snapshot);
    app.diff_pane_scroll = 19;
    app.recover_session_without_tools();
    assert_ne!(app.session.id, old_session);
    let page = app.side_panel.focused_page().unwrap();
    assert_eq!(page.content, "saved body");
    assert_eq!(
        page.status.restored_from_session.as_deref(),
        Some(old_session.as_str())
    );
    assert!(page.status.stale_reason.is_some());
    assert_eq!(app.diff_pane_scroll, 19);
    app.session.id = crate::id::new_id("panel-fresh");
    app.load_session_panels(&app.session.id.clone());
    assert!(app.side_panel.pages.is_empty());
    assert_eq!(
        crate::side_panel::snapshot_for_session(&old_session)
            .unwrap()
            .pages
            .len(),
        1
    );
}

#[test]
fn side_panel_spawned_replacement_loads_validated_old_pid_view() {
    let mut first = app();
    first.set_side_panel_snapshot(inventory("panel-test-a", 1));
    select_second(&mut first);
    let old_pid = std::process::id().wrapping_add(10_000);
    let path = PanelSessions::reload_path(old_pid).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec(&first.saved_panel_sessions()).unwrap(),
    )
    .unwrap();
    let mut replacement = app();
    let handoff = old_pid.to_string();
    let restored_pid = PanelSessions::handoff_pid(Some(&handoff));
    assert_eq!(restored_pid, old_pid);
    assert_ne!(restored_pid, std::process::id());
    replacement.restore_panel_sessions(PanelSessions::restore_for_pid(restored_pid));
    assert!(!path.exists(), "handoff is consumed once");
    replacement.set_side_panel_snapshot(inventory("panel-test-a", 1));
    assert_second(&replacement);
    assert_eq!(
        PanelSessions::handoff_pid(Some("../other")),
        std::process::id()
    );
    assert_eq!(
        PanelSessions::handoff_pid(Some("4294967296")),
        std::process::id()
    );
}

#[test]
fn side_panel_reexec_serialization_restores_client_view() {
    let mut first = app();
    first.set_side_panel_snapshot(inventory("panel-test-a", 1));
    select_second(&mut first);
    first.side_panel_fullscreen = true;
    let bytes = serde_json::to_vec(&first.saved_panel_sessions()).unwrap();
    let mut second = app();
    second.restore_panel_sessions(serde_json::from_slice(&bytes).unwrap());
    second.set_side_panel_snapshot(inventory("panel-test-a", 1));
    assert_second(&second);
    assert!(second.side_panel_fullscreen);
}

#[test]
fn side_panel_reexec_unavailable_then_same_focus_revision_restores_view() {
    let mut first = app();
    first.set_side_panel_snapshot(inventory("panel-test-a", 4));
    select_second(&mut first);
    first.side_panel_fullscreen = true;
    let bytes = serde_json::to_vec(&first.saved_panel_sessions()).unwrap();
    let mut second = app();
    second.restore_panel_sessions(serde_json::from_slice(&bytes).unwrap());
    for _ in 0..2 {
        second.set_side_panel_snapshot(crate::side_panel::SidePanelSnapshot {
            session_id: Some("panel-test-a".into()),
            unavailable: Some("loading".into()),
            ..Default::default()
        });
        assert!(second.side_panel.pages.is_empty());
        assert_eq!(
            second.saved_panel_sessions().views["panel-test-a"]
                .selected
                .as_deref(),
            Some("second")
        );
    }
    // Reloading once more during the outage must preserve the same metadata.
    let bytes = serde_json::to_vec(&second.saved_panel_sessions()).unwrap();
    let saved: PanelSessions = serde_json::from_slice(&bytes).unwrap();
    assert!(saved.snapshots.is_empty());
    second.set_side_panel_snapshot(inventory("panel-test-a", 4));
    assert_second(&second);
    assert!(second.side_panel_fullscreen);
    let mut third = app();
    third.restore_panel_sessions(saved);
    third.set_side_panel_snapshot(inventory("panel-test-a", 4));
    assert_second(&third);
    assert!(third.side_panel_fullscreen);
}

#[test]
fn side_panel_snapshot_cache_bounds_inactive_sessions_without_losing_views() {
    let mut app = app();
    for index in 0..8 {
        let session = format!("cache-{index}");
        app.remote_session_id = Some(session.clone());
        app.set_side_panel_snapshot(inventory(&session, 1));
        select_second(&mut app);
    }
    assert_eq!(app.panel_sessions.snapshots.len(), 5);
    assert!(!app.panel_sessions.snapshots.contains_key("cache-0"));
    assert!(app.panel_sessions.snapshots.contains_key("cache-7"));
    assert_eq!(app.panel_sessions.views.len(), 8);
    app.remote_session_id = Some("cache-0".into());
    app.set_side_panel_snapshot(inventory("cache-0", 1));
    assert_second(&app);
}

#[test]
fn side_panel_snapshot_cache_bounds_markdown_and_pdf_bytes_but_keeps_active() {
    let mut sessions = PanelSessions::default();
    sessions.active = Some("active".into());
    for (id, pdf) in [("markdown", false), ("pdf", true), ("active", true)] {
        let mut snapshot = inventory(id, 1);
        let body = "x".repeat(33 * 1024 * 1024);
        if pdf {
            snapshot.pages[0].pdf_data = Some(body);
        } else {
            snapshot.pages[0].content = body;
        }
        sessions.snapshots.insert(id.into(), snapshot);
        sessions.snapshot_order.push(id.into());
    }
    sessions.trim_snapshot_cache();
    assert!(!sessions.snapshots.contains_key("markdown"));
    assert!(sessions.snapshots.contains_key("pdf"));
    assert!(sessions.snapshots.contains_key("active"));
    assert_eq!(sessions.snapshots.len(), 2);
}

#[test]
fn side_panel_idle_handler_redraws_only_changed_inventory() {
    let mut app = app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let event = |snapshot| crate::protocol::ServerEvent::SidePanelState { snapshot };
    assert!(app.handle_server_event(event(inventory("panel-test-a", 1)), &mut remote));
    assert!(!app.handle_server_event(event(inventory("panel-test-a", 1)), &mut remote));
    assert!(!app.handle_server_event(event(inventory("panel-test-a", 0)), &mut remote));
    assert!(!app.handle_server_event(event(inventory("different", 99)), &mut remote));
    app.dismiss_side_panel();
    assert!(!app.handle_server_event(event(inventory("panel-test-a", 1)), &mut remote));
    let unavailable = crate::side_panel::SidePanelSnapshot {
        session_id: Some("panel-test-a".into()),
        unavailable: Some("offline".into()),
        ..Default::default()
    };
    assert!(app.handle_server_event(event(unavailable.clone()), &mut remote));
    assert!(!app.handle_server_event(event(unavailable), &mut remote));
    let mut closed = inventory("panel-test-a", 2);
    closed.pages.pop();
    assert!(app.handle_server_event(event(closed), &mut remote));
    let mut reopened = inventory("panel-test-a", 3);
    reopened.pages[1].content = "reopened archived body".into();
    reopened.focus_revision = 2;
    reopened.focused_page_id = Some("second".into());
    assert!(app.handle_server_event(event(reopened.clone()), &mut remote));
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("second"));
    assert!(!app.handle_server_event(event(reopened), &mut remote));
}

#[test]
fn side_panel_reexec_accepts_backup_inventory_but_live_client_rejects_older() {
    let mut first = app();
    first.set_side_panel_snapshot(inventory("panel-test-a", 5));
    select_second(&mut first);
    let saved = serde_json::to_vec(&first.saved_panel_sessions()).unwrap();
    let mut second = app();
    second.restore_panel_sessions(serde_json::from_slice(&saved).unwrap());
    second.set_side_panel_snapshot(inventory("panel-test-a", 4));
    assert_second(&second);
    assert!(
        second
            .side_panel
            .pages
            .iter()
            .all(|page| page.status.stale_reason.is_some())
    );
    assert_eq!(second.side_panel.revision, 4);
    assert!(!second.set_side_panel_snapshot_changed(inventory("panel-test-a", 3)));
    assert_eq!(second.side_panel.revision, 4);
    second.set_side_panel_snapshot(inventory("panel-test-a", 5));
    assert_second(&second);
    assert!(
        second
            .side_panel
            .pages
            .iter()
            .all(|page| page.status.stale_reason.is_none())
    );
    assert!(!second.set_side_panel_snapshot_changed(inventory("panel-test-a", 4)));
    assert_eq!(second.side_panel.revision, 5);
}
