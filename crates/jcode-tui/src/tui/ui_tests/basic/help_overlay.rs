/// The /model picker's agent chords must stay advertised in the help overlay:
/// Alt+S (swarm) and Alt+P (memory) replaced the Ctrl+S swarm shortcut that
/// collided with the global input-stash chord. Alt+M stays free for picker
/// features other branches add, so it is intentionally absent here.
#[test]
fn help_overlay_lists_agent_model_chords() {
    let _lock = viewport_snapshot_test_lock();

    let state = TestState {
        help_scroll: Some(0),
        ..Default::default()
    };
    let mut terminal = Terminal::new(TestBackend::new(80, 200)).expect("test terminal");
    terminal
        .draw(|frame| crate::tui::ui::draw(frame, &state))
        .expect("help overlay frame");

    let buffer = terminal.backend().buffer();
    let text: String = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Alt+S"), "swarm chord missing from help:\n{text}");
    assert!(text.contains("Alt+P"), "memory chord missing from help:\n{text}");
    assert!(
        text.contains("Select swarm model"),
        "swarm chord description missing from help:\n{text}"
    );
    assert!(
        text.contains("Select memory model"),
        "memory chord description missing from help:\n{text}"
    );
}
