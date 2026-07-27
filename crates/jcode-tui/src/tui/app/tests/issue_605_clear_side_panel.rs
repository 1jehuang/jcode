// Issue #605: `/clear` must drop the discarded session's side panel pages.
//
// There are two `/clear` implementations. The one users actually hit in the
// product TUI is the `RemoteClient` handler in `remote/key_handling.rs`; the
// `reset_current_session` path in `commands_review.rs` only runs under replay,
// the test harness, or a dropped server connection. Both now funnel the side
// panel reset through `clear_side_panel_for_new_session`, so cover that shared
// seam plus the caller that regressed.

use crate::tui::app::tests::create_test_app;

fn side_panel_snapshot_with_one_page() -> crate::side_panel::SidePanelSnapshot {
    crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("page-from-old-session".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "page-from-old-session".to_string(),
            title: "Old session goals".to_string(),
            file_path: "/tmp/old-session-goals.md".to_string(),
            format: Default::default(),
            source: Default::default(),
            content: "leftover content".to_string(),
            updated_at_ms: 1,
        }],
    }
}

#[test]
fn clear_side_panel_for_new_session_drops_pages_and_focus() {
    let mut app = create_test_app();
    app.apply_side_panel_snapshot(side_panel_snapshot_with_one_page());
    assert!(
        app.side_panel.has_pages(),
        "precondition: the panel should hold the old session's page"
    );

    crate::tui::app::commands_review::clear_side_panel_for_new_session(&mut app);

    assert!(
        !app.side_panel.has_pages(),
        "/clear must drop the discarded session's side panel pages (#605)"
    );
    assert!(
        app.side_panel.focused_page_id.is_none(),
        "/clear must drop the focused page id"
    );
}

#[test]
fn reset_current_session_also_clears_the_side_panel() {
    // The replay / test-harness / disconnected path was already correct; pin it
    // so the shared helper refactor did not regress it.
    let mut app = create_test_app();
    app.apply_side_panel_snapshot(side_panel_snapshot_with_one_page());
    assert!(app.side_panel.has_pages());

    crate::tui::app::commands_review::reset_current_session(&mut app);

    assert!(
        !app.side_panel.has_pages(),
        "reset_current_session must still clear the side panel"
    );
    assert!(app.side_panel.focused_page_id.is_none());
}
