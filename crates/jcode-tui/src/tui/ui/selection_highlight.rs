//! Shared copy-selection highlight rendering.
//!
//! One implementation for every pane that supports drag-to-select (chat
//! viewport, side pane, file-diff pane, full-screen overlays, and the prompt
//! composer) so the selection style stays visually identical across the UI.

use super::{accent_color, blend_color, rgb};
use ratatui::prelude::*;

pub(crate) fn selection_bg_for(base_bg: Option<Color>) -> Color {
    let fallback = rgb(32, 38, 48);
    // A selection is an active interaction, not passive decoration. Use a
    // deliberately strong fill so it remains obvious over syntax highlighting,
    // diff backgrounds, and dim assistant text.
    blend_color(base_bg.unwrap_or(fallback), accent_color(), 0.58)
}

pub(crate) fn selection_fg_for(base_fg: Option<Color>) -> Option<Color> {
    base_fg.map(|fg| blend_color(fg, Color::White, 0.32))
}

/// Apply a copy-selection highlight to a single display line between
/// `[start_col, end_col)` (display columns). Zero-width characters inherit the
/// selection of the cell they attach to; wide characters are selected when any
/// of their cells overlap the range.
pub(crate) fn highlight_line_selection(
    line: &Line<'static>,
    start_col: usize,
    end_col: usize,
) -> Line<'static> {
    if end_col <= start_col {
        return line.clone();
    }

    let mut rebuilt: Vec<Span<'static>> = Vec::new();
    let mut current_text = String::new();
    let mut current_style: Option<Style> = None;
    let mut col = 0usize;

    let flush = |rebuilt: &mut Vec<Span<'static>>, text: &mut String, style: &mut Option<Style>| {
        if !text.is_empty() {
            let span = match style.take() {
                Some(style) => Span::styled(std::mem::take(text), style),
                None => Span::raw(std::mem::take(text)),
            };
            rebuilt.push(span);
        }
    };

    for span in &line.spans {
        let mut selected_style = span.style.bg(selection_bg_for(span.style.bg));
        if let Some(fg) = selection_fg_for(span.style.fg) {
            selected_style = selected_style.fg(fg);
        }
        for ch in span.content.chars() {
            let width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            let selected = if width == 0 {
                col > start_col && col <= end_col
            } else {
                col < end_col && col.saturating_add(width) > start_col
            };

            let style = if selected { selected_style } else { span.style };

            if current_style == Some(style) {
                current_text.push(ch);
            } else {
                flush(&mut rebuilt, &mut current_text, &mut current_style);
                current_text.push(ch);
                current_style = Some(style);
            }

            col = col.saturating_add(width);
        }
    }

    flush(&mut rebuilt, &mut current_text, &mut current_style);

    Line {
        spans: rebuilt,
        style: line.style,
        alignment: line.alignment,
    }
}
