use super::*;

#[test]
fn test_pending_split_launch_shows_processing_status_in_ui() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.pending_split_started_at = Some(Instant::now());

    assert!(app.is_processing());
    assert!(crate::tui::TuiState::is_processing(&app));
    assert!(matches!(
        crate::tui::TuiState::status(&app),
        ProcessingStatus::Sending
    ));
    assert!(crate::tui::TuiState::elapsed(&app).is_some());
}

#[test]
fn test_expired_pending_split_launch_no_longer_shows_processing_status() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.pending_split_started_at = Some(Instant::now() - Duration::from_millis(400));

    assert!(!app.is_processing());
    assert!(!crate::tui::TuiState::is_processing(&app));
    assert!(matches!(
        crate::tui::TuiState::status(&app),
        ProcessingStatus::Idle
    ));
    assert!(crate::tui::TuiState::elapsed(&app).is_none());
}
