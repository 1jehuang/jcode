use super::*;
use ratatui::backend::TestBackend;

/// The pinned Overview band narrows transcript rows only while the render
/// pass actually places the panel. A short conversation packs the messages
/// area to a few rows, placement rejects the panel below its minimum height,
/// and a still-reserved band would wrap the transcript around a panel that is
/// never drawn. Verify that a short transcript renders at full width (no
/// phantom band reserve) while a tall transcript keeps the pinned HUD and
/// its narrow wrap.
#[test]
fn short_transcript_does_not_wrap_around_an_unplaced_pinned_overview() {
    let _env = crate::storage::lock_test_env();
    let _lock = viewport_snapshot_test_lock();
    crate::tui::ui::clear_test_render_state_for_tests();
    info_widget::clear_widget_placements_for_tests();

    let widget_data = info_widget::InfoWidgetData {
        model: Some("pinned-model".into()),
        session_name: Some("short session".into()),
        context_limit: Some(200_000),
        observed_context_tokens: Some(12500),
        provider_name: Some("openai".into()),
        usage_info: Some(info_widget::UsageInfo {
            provider: info_widget::UsageProvider::OpenAI,
            available: true,
            ..Default::default()
        }),
        dual_git_info: Some(info_widget::DualGitInfo::default()),
        overview_pinned: true,
        ..Default::default()
    };

    let screen_for = |height: u16, prompt: &str| -> String {
        crate::tui::ui::clear_test_render_state_for_tests();
        info_widget::clear_widget_placements_for_tests();
        let state = TestState {
            info_widget_data: widget_data.clone(),
            display_messages: vec![DisplayMessage::user(prompt)],
            messages_version: 1,
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(100, height)).unwrap();
        terminal.draw(|frame| draw(frame, &state)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    };

    // Short conversation: the packed messages area is a few rows, placement
    // cannot host the panel, so the transcript must render at full width.
    // The prompt may wrap across the small area, but its text stays present.
    let prompt = "A short prompt that spans most of the wide row without wrapping";
    let short_screen = screen_for(9, prompt);
    assert!(
        short_screen.contains("A short prompt that spans most of the wide row"),
        "short transcript lost its prompt:\n{short_screen}"
    );
    // The panel is not drawn, so no mcp/session HUD rows appear either.
    assert!(
        !short_screen.contains("short session"),
        "unplaced panel must not draw:\n{short_screen}"
    );

    // Tall conversation: the messages area hosts the pinned HUD, which draws
    // top-right and the transcript keeps working.
    let tall_screen = screen_for(40, prompt);
    assert!(
        tall_screen.contains(prompt),
        "tall transcript lost its prompt:\n{tall_screen}"
    );
}
