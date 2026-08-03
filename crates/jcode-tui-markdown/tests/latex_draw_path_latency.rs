//! Regression test for #735 (and its predecessors #428/#732).
//!
//! Reported symptom: pressing Esc while the model streams appears to do
//! nothing. The attached log shows the server cancelling the turn in ~4ms
//! while the client only observes the resulting events 78 seconds later,
//! immediately after a `TUI_RENDER_PHASES prepare=3967ms` frame preceded by
//! a LaTeX toolchain warning. The cause was `render_latex_image` invoking the
//! TeX toolchain synchronously from the markdown renderer, which runs on the
//! TUI draw path: a cold or broken toolchain starves the event loop, so
//! interrupt handling cannot run.
//!
//! This test reproduces the reporter's environment with a deliberately
//! hanging toolchain and asserts that rendering math never blocks the calling
//! thread. Without the deferred worker it blocks for the per-command timeout
//! (8s) on every uncached formula.

#![cfg(all(unix, feature = "mermaid-renderer"))]

use std::io::Write;
use std::time::{Duration, Instant};

#[cfg(all(unix, feature = "mermaid-renderer"))]
fn hanging_stub(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    let mut file = std::fs::File::create(&path).unwrap();
    // Sleeps far longer than the renderer's per-command timeout, emulating a
    // TeX install that stalls regenerating formats (`mktexfmt latex.fmt`).
    file.write_all(b"#!/bin/sh\nsleep 120\nexit 1\n").unwrap();
    drop(file);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(all(unix, feature = "mermaid-renderer"))]
#[test]
fn a_hanging_tex_toolchain_never_blocks_the_markdown_draw_path() {
    let dir = tempfile::tempdir().unwrap();
    let stub = hanging_stub(dir.path(), "hanging-tex");
    // SAFETY: single-threaded test process setup before any render runs.
    unsafe {
        std::env::set_var("JCODE_LATEX_COMMAND", &stub);
        std::env::set_var("JCODE_DVIPNG_COMMAND", &stub);
        std::env::set_var("JCODE_PDFLATEX_COMMAND", &stub);
        std::env::set_var("JCODE_PDFTOCAIRO_COMMAND", &stub);
    }

    // Unique sources so no earlier run left a cached artifact on disk.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let response = format!(
        "Derivation:\n\n$$x_{{{nonce}}}^2 = a$$\n\n$$y_{{{nonce}}} = \\frac{{b}}{{c}}$$\n\n\
         $$z_{{{nonce}}} = \\sqrt{{d}}$$\n"
    );

    // Simulate the streaming draw loop: many renders of growing prefixes, the
    // exact pattern that starved the event loop in the report.
    // Image mode only engages when the terminal advertises an image protocol,
    // which a test process does not; force it on so the LaTeX path is live.
    let started = Instant::now();
    jcode_tui_mermaid::with_image_protocol_override(Some(true), || {
        let mut renderer = jcode_tui_markdown::IncrementalMarkdownRenderer::new(Some(90));
        for end in response
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(response.len()))
        {
            let _ = renderer.update(&response[..end]);
        }
    });
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(5),
        "streaming render of math blocked the calling thread for {elapsed:?} with a hanging \
         TeX toolchain; LaTeX rendering must be deferred to the background worker (#735)"
    );
}
