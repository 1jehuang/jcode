use super::*;

#[test]
fn test_grouped_dm_badge_expands_all_and_any_expanded_badge_collapses_run() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage::user("hi"),
        DisplayMessage::swarm(
            "DM from sheep",
            jcode_tui_messages::encode_collapsible_swarm_content(
                "First preview",
                "First private agent message.",
            ),
        ),
        DisplayMessage::swarm("DM from fox", "Second private agent message."),
    ];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());
    let backend = ratatui::backend::TestBackend::new(90, 30);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    let collapsed = render_and_snap(&app, &mut terminal);
    assert!(!collapsed.contains("private agent message"), "{collapsed}");

    let buffer = terminal.backend().buffer();
    let area = *buffer.area();
    let mut badges = Vec::new();
    for row in 0..area.height {
        for col in 0..area.width {
            if buffer[(col, row)].symbol() == "⊞" {
                badges.push((col, row));
            }
        }
    }
    assert_eq!(badges.len(), 1, "adjacent DMs share one badge: {collapsed}");
    let dm_row = collapsed
        .lines()
        .find(|line| line.contains("DMs ·"))
        .unwrap();
    assert_eq!(dm_row.matches(" tok").count(), 1, "{collapsed}");
    let (column, row) = badges[0];
    assert!(app.try_toggle_swarm_expand_at(column, row));
    for message in &app.display_messages[1..] {
        assert!(
            jcode_tui_messages::parse_collapsible_swarm_content(&message.content)
                .is_some_and(|parsed| parsed.expanded)
        );
    }
    let expanded = render_and_snap(&app, &mut terminal);
    assert!(
        expanded.contains("Second private agent message"),
        "{expanded}"
    );
    assert!(
        expanded.contains("First private agent message"),
        "{expanded}"
    );

    let buffer = terminal.backend().buffer();
    let mut collapse_badges = Vec::new();
    for row in 0..area.height {
        for col in 0..area.width {
            if buffer[(col, row)].symbol() == "⊟" {
                collapse_badges.push((col, row));
            }
        }
    }
    assert_eq!(collapse_badges.len(), 2, "{expanded}");
    // The second card's badge must fold the entire consecutive expanded run,
    // not just the selected card or the suffix starting at that card.
    let (column, row) = collapse_badges[1];
    assert!(app.try_toggle_swarm_expand_at(column, row));
    for message in &app.display_messages[1..] {
        assert!(
            jcode_tui_messages::parse_collapsible_swarm_content(&message.content)
                .is_some_and(|parsed| !parsed.expanded)
        );
    }
    let collapsed_again = render_and_snap(&app, &mut terminal);
    assert!(
        !collapsed_again.contains("private agent message"),
        "{collapsed_again}"
    );
    assert_eq!(collapsed_again.matches('⊞').count(), 1, "{collapsed_again}");
}
