//! Companion to `latex_draw_path_latency.rs` (#735).
//!
//! Lives in its own test binary on purpose: the sibling test points the
//! renderer at a hanging toolchain via process-global `JCODE_*_COMMAND`
//! environment variables, and the background render worker reads them
//! asynchronously. Sharing a process would let that stub leak into this test.

#![cfg(all(unix, feature = "mermaid-renderer"))]

use std::time::{Duration, Instant};

/// The other half of the contract: a deferred render must actually resolve.
/// The placeholder is only acceptable if the background worker finishes, bumps
/// the shared deferred-render epoch (which every markdown/body cache layer
/// treats as an invalidation signal), and a subsequent render yields the image.
///
/// Requires a working TeX toolchain; skipped when one is not installed.
#[test]
fn a_deferred_formula_resolves_into_an_image_once_the_worker_finishes() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    assert_formula_resolves(format!("$$q_{{{nonce}}} = 1$$\n"));
}

#[test]
fn a_gaussian_integral_nested_in_a_list_resolves_into_an_image() {
    assert_formula_resolves(concat!(
        "- Gaussian integral:\n\n",
        "  \\[\n",
        "  \\int_{-\\infty}^{\\infty} e^{-x^2}\\,dx=\\sqrt{\\pi}\n",
        "  \\]\n",
    ));
}

fn assert_formula_resolves(text: impl Into<String>) {
    if which("latex").is_none() || which("dvipng").is_none() {
        eprintln!("skipping: no latex/dvipng toolchain available");
        return;
    }
    // Ensure the real toolchain is used even if an earlier test in this binary
    // pointed the renderer at a stub.
    unsafe {
        std::env::remove_var("JCODE_LATEX_COMMAND");
        std::env::remove_var("JCODE_DVIPNG_COMMAND");
        std::env::remove_var("JCODE_PDFLATEX_COMMAND");
        std::env::remove_var("JCODE_PDFTOCAIRO_COMMAND");
    }

    let text = text.into();

    let render = || {
        jcode_tui_mermaid::with_image_protocol_override(Some(true), || {
            jcode_tui_markdown::render_markdown_with_width(&text, Some(90))
        })
    };

    let first = render();
    let pending = first
        .iter()
        .any(jcode_tui_markdown::line_is_mermaid_pending_placeholder);
    if !pending {
        // Already cached from a previous run: the invariant under test cannot
        // be observed, but the end state (an image, not a placeholder) still
        // must hold.
        assert!(
            rendered_a_formula_image(&first),
            "expected a rendered image"
        );
        return;
    }

    // Poll for the worker to publish the artifact. Generous bound: this is a
    // real TeX invocation, and the point is that the *draw thread* never waits.
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let lines = render();
        if !lines
            .iter()
            .any(jcode_tui_markdown::line_is_mermaid_pending_placeholder)
        {
            assert!(
                rendered_a_formula_image(&lines),
                "deferred render finished but produced no image: {lines:?}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "deferred LaTeX render never resolved; the placeholder would be permanent"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// True when `lines` carry a real rendered formula artifact.
///
/// A terminal with graphics gets an image placeholder. A test process has no
/// image picker, so the same successful artifact is described by its measured
/// pixel geometry instead. Either proves the background render published a
/// usable PNG, which is what this test is about; what it must never be is the
/// "rendering math..." placeholder or a Unicode-only fallback.
fn rendered_a_formula_image(lines: &[ratatui::text::Line<'static>]) -> bool {
    let has_placeholder = lines.iter().any(|line| {
        jcode_tui_mermaid::parse_image_placeholder(line).is_some()
            || jcode_tui_mermaid::parse_inline_image_placeholder(line).is_some()
    });
    let describes_geometry = lines.iter().any(|line| {
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        text.contains("px (image protocols not available)")
    });
    has_placeholder || describes_geometry
}

fn which(bin: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(bin))
            .find(|candidate| candidate.is_file())
    })
}
