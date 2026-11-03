use super::*;

#[test]
fn test_expired_status_notice_requests_a_redraw_once() {
    let mut app = create_test_app();
    app.set_status_notice("Waiting for reload handoff...");
    assert!(
        !app.retire_expired_status_notice(),
        "a fresh notice must stay on screen"
    );

    app.status_notice = Some((
        "Waiting for reload handoff...".to_string(),
        crate::tui::app::helpers::backdated_now(
            crate::tui::app::STATUS_NOTICE_TTL + std::time::Duration::from_millis(50),
        ),
    ));
    assert!(
        app.retire_expired_status_notice(),
        "an expired notice must request a frame so it disappears without a keypress"
    );
    assert!(app.status_notice.is_none());
    assert!(
        !app.retire_expired_status_notice(),
        "retiring is one-shot, so idle ticks stay cheap afterwards"
    );
}
