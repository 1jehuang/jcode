//! End-to-end wire proof for the macOS glyph-atlas corruption fix (#330).
//!
//! Renders a colored buffer through a *real* `CrosstermBackend` (the same
//! backend the shipped binary uses) and inspects the actual ANSI bytes it
//! writes. Under glyph-safe mode, animated colors are quantized to
//! `Color::Indexed`, which must serialize as `38;5;<n>` (256-color) and never
//! as `38;2;r;g;b` (truecolor). The truecolor churn is what overflows the
//! terminal's GPU glyph atlas and garbles letters into boxes.

use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::Cell;
use ratatui::style::Color;

/// Drive the backend to draw a single cell with the given fg color and return
/// the raw bytes it emitted to the writer.
fn emitted_bytes_for(fg: Color) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    {
        let mut backend = CrosstermBackend::new(&mut out);
        let mut cell = Cell::default();
        cell.set_symbol("X");
        cell.set_fg(fg);
        let content = [(0u16, 0u16, &cell)];
        backend.draw(content.into_iter()).expect("draw");
        backend.flush().expect("flush");
    }
    out
}

#[test]
fn indexed_color_emits_256_sgr_not_truecolor() {
    // What glyph-safe quantization produces (xterm-256 index).
    let bytes = emitted_bytes_for(Color::Indexed(111));
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("38;5;111"),
        "expected 256-color SGR in emitted bytes, got: {text:?}"
    );
    assert!(
        !text.contains("38;2;"),
        "256-color path must never emit truecolor SGR, got: {text:?}"
    );
}

#[test]
fn truecolor_color_emits_truecolor_sgr() {
    // Sanity check: the truecolor path (robust terminals) still works and is
    // precisely what we suppress on fragile terminals.
    let bytes = emitted_bytes_for(Color::Rgb(138, 180, 248));
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("38;2;138;180;248"),
        "expected truecolor SGR, got: {text:?}"
    );
}
