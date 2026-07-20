use ratatui::buffer::Buffer;

pub(crate) fn adapt_buffer_for_emoji_preference(buffer: &mut Buffer) {
    adapt_buffer_for_emoji_enabled(buffer, crate::output_style::emoji_enabled());
}

fn adapt_buffer_for_emoji_enabled(buffer: &mut Buffer, enabled: bool) {
    if enabled {
        return;
    }
    for cell in &mut buffer.content {
        if let std::borrow::Cow::Owned(symbol) =
            crate::output_style::terminal_text_with_emoji(cell.symbol(), enabled)
        {
            cell.set_symbol(&symbol);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{layout::Rect, style::Style};

    #[test]
    fn no_emoji_mode_rewrites_completed_frame_cells_to_ascii() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 24, 1));
        buffer.set_string(0, 0, "🐝 ready ✅ box ─", Style::default());
        adapt_buffer_for_emoji_enabled(&mut buffer, false);
        let rendered = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert_eq!(
            rendered.split_whitespace().collect::<Vec<_>>(),
            vec!["*", "ready", "+", "box", "─"]
        );
        assert!(!rendered.contains(['🐝', '✅']));
    }
}
