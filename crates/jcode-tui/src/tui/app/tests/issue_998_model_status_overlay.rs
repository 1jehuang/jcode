#[test]
fn issue_998_connected_station_routes_model_status_overlay_keys() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.model_status_scroll = Some(0);

    rt.block_on(app.handle_remote_key(KeyCode::PageDown, KeyModifiers::empty(), &mut remote))
        .unwrap();
    assert_eq!(app.model_status_scroll, Some(20));

    rt.block_on(app.handle_remote_key(KeyCode::Esc, KeyModifiers::empty(), &mut remote))
        .unwrap();
    assert_eq!(app.model_status_scroll, None);
}

#[test]
fn issue_998_disconnected_station_routes_model_status_overlay_keys() {
    let mut app = create_test_app();
    app.model_status_scroll = Some(0);

    super::remote::handle_disconnected_key(&mut app, KeyCode::PageDown, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.model_status_scroll, Some(20));

    super::remote::handle_disconnected_key(&mut app, KeyCode::Char('q'), KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.model_status_scroll, None);
}
