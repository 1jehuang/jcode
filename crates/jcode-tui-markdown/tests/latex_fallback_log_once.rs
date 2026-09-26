//! Without a TeX toolchain, every redraw re-reported the cached render failure,
//! and empty math blocks produced a second error that alternated with it. The
//! previous "same as the last error" check never matched, so the log grew by
//! about 13k lines a day. Own test binary: it points the renderer at missing
//! executables through process-global environment variables.

#![cfg(all(unix, feature = "mermaid-renderer"))]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static LOGGED: AtomicUsize = AtomicUsize::new(0);

#[test]
fn a_missing_toolchain_and_empty_math_log_at_most_once_across_redraws() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-tex");
    unsafe {
        std::env::set_var("JCODE_LATEX_COMMAND", &missing);
        std::env::set_var("JCODE_DVIPNG_COMMAND", &missing);
        std::env::set_var("JCODE_PDFLATEX_COMMAND", &missing);
        std::env::set_var("JCODE_PDFTOCAIRO_COMMAND", &missing);
    }
    jcode_tui_markdown::set_latex_log_hook(|_| {
        LOGGED.fetch_add(1, Ordering::SeqCst);
    });

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let text: String = (0..50)
        .map(|i| format!("Step {i}:\n\n$$x_{{{nonce}{i}}} = {i}$$\n\n$$  $$\n\n"))
        .collect();
    let render = || {
        jcode_tui_mermaid::with_image_protocol_override(Some(true), || {
            jcode_tui_markdown::render_markdown_with_width(&text, Some(90))
        })
    };

    // Redraw until the background worker has failed every formula, then keep
    // redrawing: each frame reads the cached failures again.
    let deadline = Instant::now() + Duration::from_secs(30);
    while render().iter().any(|line| {
        line.to_string()
            .contains(jcode_tui_markdown::MATH_PENDING_PLACEHOLDER_TEXT)
    }) {
        assert!(
            Instant::now() < deadline,
            "LaTeX renders still pending after 30s; the fallback was never exercised"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    for _ in 0..20 {
        render();
    }

    let logged = LOGGED.load(Ordering::SeqCst);
    assert_eq!(
        logged, 1,
        "LaTeX fallback logged {logged} lines for 100 math blocks; each distinct error must be \
         logged once per process and empty math must not reach the renderer"
    );
}
