#![cfg(all(unix, feature = "mermaid-renderer"))]
use jcode_tui_markdown::{line_is_mermaid_pending_placeholder, render_markdown_with_width};
use jcode_tui_mermaid::{
    D2Options, configure_d2, force_test_kitty_picker, with_image_protocol_override,
};
use std::time::{Duration, Instant};

#[test]
fn missing_binary_deferred_failure_becomes_visible_source_not_permanent_pending() {
    force_test_kitty_picker();
    configure_d2(
        true,
        D2Options {
            binary: "/nonexistent/jcode-d2-missing-tool".into(),
            ..Default::default()
        },
    );
    with_image_protocol_override(Some(true), || {
        let markdown = "```d2\na -> b: keep this source\n```";
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let lines = render_markdown_with_width(markdown, Some(120));
            if !lines.iter().any(line_is_mermaid_pending_placeholder) {
                let text = lines
                    .iter()
                    .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
                    .collect::<String>();
                assert!(text.contains("a -> b: keep this source"));
                assert!(text.contains("D2:") && text.contains("not found"), "{text}");
                assert!(!text.contains("mermaid error"));
                break;
            }
            assert!(Instant::now() < deadline, "missing D2 stayed pending");
            std::thread::sleep(Duration::from_millis(10));
        }
    });
}
