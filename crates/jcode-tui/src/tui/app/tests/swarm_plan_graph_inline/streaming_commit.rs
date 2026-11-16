use super::*;

#[test]
fn test_commit_with_emptied_stream_buffer_clears_streaming_preview_diagram() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    let hash: u64 = 0x0005_17EA_11ED_0004;
    seed_streaming_preview(&mut app, hash);

    // Simulate a TextReplace-style rewrite that drops the fenced block the
    // preview was rendered from, leaving the buffer empty while the preview
    // slot is still occupied.
    app.replace_streaming_text(String::new());
    assert_eq!(
        crate::tui::mermaid::get_active_diagrams()
            .first()
            .map(|d| d.hash),
        Some(hash),
        "precondition: replace_streaming_text alone leaves the preview live"
    );

    let committed = app.commit_pending_streaming_assistant_message();

    assert!(!committed, "empty buffer commits nothing");
    assert_streaming_preview_cleared(hash, "commit with emptied stream buffer");
    crate::tui::mermaid::clear_active_diagrams();
}
