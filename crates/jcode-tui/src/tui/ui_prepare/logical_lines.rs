use crate::tui::ui::WrappedLineMap;
use ratatui::text::Line;

pub(super) fn content_prefers_display_as_logical_lines(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with('|') && trimmed.matches('|').count() >= 2
    })
}

pub(super) fn semantic_swarm_line_text(plain: &str) -> (String, usize) {
    let trimmed = plain.trim_start_matches(' ');
    if let Some(rest) = trimmed.strip_prefix("│ ") {
        let prefix_width = unicode_width::UnicodeWidthStr::width(plain)
            .saturating_sub(unicode_width::UnicodeWidthStr::width(rest));
        (rest.to_string(), prefix_width)
    } else {
        (plain.to_string(), 0)
    }
}

pub(super) fn map_display_lines_to_logical_lines(
    display_lines: &[Line<'static>],
    logical_plain_lines: &[String],
    raw_base: usize,
) -> Option<Vec<WrappedLineMap>> {
    let mut maps = Vec::with_capacity(display_lines.len());
    let mut logical_idx = 0usize;
    let mut logical_col = 0usize;

    for line in display_lines {
        while logical_idx < logical_plain_lines.len() {
            let logical_width =
                unicode_width::UnicodeWidthStr::width(logical_plain_lines[logical_idx].as_str());
            if logical_col < logical_width || logical_width == 0 {
                break;
            }
            logical_idx += 1;
            logical_col = 0;
        }

        let logical_text = logical_plain_lines.get(logical_idx)?;
        let logical_width = unicode_width::UnicodeWidthStr::width(logical_text.as_str());
        let display_width = line.width();
        let remaining = logical_width.saturating_sub(logical_col);
        if display_width > remaining {
            return None;
        }

        maps.push(WrappedLineMap {
            raw_line: raw_base + logical_idx,
            start_col: logical_col,
            end_col: logical_col + display_width,
        });
        logical_col += display_width;
    }

    Some(maps)
}
