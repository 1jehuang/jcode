//! Variable-width transcript flow around the selected-session card.
use ratatui::{
    layout::{Alignment, Rect},
    text::{Line, Span},
};

#[derive(Debug, Default)]
pub(super) struct Flow {
    pub lines: Vec<Line<'static>>,
    /// Absolute source-line index for each emitted screen row.
    pub source_rows: Vec<usize>,
    /// Number of completely emitted source lines, starting at `scroll`.
    pub consumed: usize,
}

fn row_width(area: Rect, reserved: Option<Rect>, row: usize) -> usize {
    let y = u32::from(area.y) + row as u32;
    if let Some(card) = reserved
        && card.width > 0
        && y >= u32::from(card.y)
        && y < u32::from(card.y) + u32::from(card.height)
        && u32::from(card.x) < u32::from(area.x) + u32::from(area.width)
        && u32::from(card.x) + u32::from(card.width) > u32::from(area.x)
    {
        return usize::from(
            card.x
                .saturating_sub(area.x)
                .saturating_sub(1)
                .min(area.width),
        );
    }
    usize::from(area.width)
}

/// Split on a UTF-8 boundary, preserving span styles and line metadata.
fn split_bytes(line: &Line<'static>, mut bytes: usize) -> (Line<'static>, Line<'static>) {
    let mut left = line.clone();
    let mut right = line.clone();
    left.spans.clear();
    right.spans.clear();
    for span in &line.spans {
        let n = bytes.min(span.content.len());
        if n > 0 {
            left.spans
                .push(Span::styled(span.content[..n].to_owned(), span.style));
        }
        if n < span.content.len() {
            right
                .spans
                .push(Span::styled(span.content[n..].to_owned(), span.style));
        }
        bytes -= n;
    }
    (left, right)
}

fn plain(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Mirror the live wrapper's hanging gutter, but keep it separate from the
/// unconsumed source so a width change never turns inserted padding into text.
fn hanging_prefix(line: &Line<'static>) -> Vec<Span<'static>> {
    let text = plain(line);
    let leading = text.len() - text.trim_start_matches(char::is_whitespace).len();
    let mut end = leading;
    while text[end..].starts_with("│ ") {
        end += "│ ".len();
    }
    let rest = &text[end..];
    let marker = if rest.starts_with("• ") {
        Some("• ".len())
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        (digits > 0 && rest[digits..].starts_with(". ")).then_some(digits + 2)
    };
    if let Some(mut marker) = marker {
        if rest[marker..].starts_with("[x] ") || rest[marker..].starts_with("[ ] ") {
            marker += 4;
        }
        let mut prefix = split_bytes(line, end).0.spans;
        prefix.push(Span::raw(
            " ".repeat(unicode_width::UnicodeWidthStr::width(&rest[..marker])),
        ));
        prefix
    } else if end > leading || line.alignment == Some(Alignment::Left) {
        split_bytes(line, end).0.spans
    } else {
        Vec::new()
    }
}

pub(super) fn layout(
    lines: &[Line<'static>],
    scroll: usize,
    area: Rect,
    reserved: Option<Rect>,
) -> Flow {
    let mut flow = Flow::default();
    if area.width == 0 || area.height == 0 {
        return flow;
    }
    for (index, source) in lines.iter().enumerate().skip(scroll) {
        let prefix = hanging_prefix(source);
        let prefix_width: usize = prefix.iter().map(Span::width).sum();
        let mut remaining = source.clone();
        let mut continuation = false;
        loop {
            if flow.lines.len() == usize::from(area.height) {
                return flow;
            }
            let width = row_width(area, reserved, flow.lines.len());
            if width == 0 {
                // A fully occluded row advances the canvas, never the source.
                flow.lines.push(Line::default());
                flow.source_rows.push(index);
                continue;
            }
            let mut input = remaining.clone();
            let padding = if continuation && prefix_width < width {
                &prefix[..]
            } else {
                &[]
            };
            input.spans.splice(0..0, padding.iter().cloned());
            let padding_bytes: usize = padding.iter().map(|s| s.content.len()).sum();
            let wrapped = crate::tui::markdown::wrap_line(input.clone(), width);
            let mut first = wrapped.into_iter().next().unwrap_or_default();
            if first.spans.is_empty() && !input.spans.is_empty() {
                first = input;
            }
            // The shared wrapper can retain over-wide whitespace runs or a
            // word that fits alone but not after its hanging prefix.
            if first.width() > width {
                let mut cells = 0;
                let mut bytes = 0;
                for ch in plain(&first).chars() {
                    let next = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                    if cells + next > width && bytes > padding_bytes {
                        break;
                    }
                    cells += next;
                    bytes += ch.len_utf8();
                }
                first = split_bytes(&first, bytes).0;
            }
            first.style = source.style;
            first.alignment = source.alignment;
            let used = plain(&first).len().saturating_sub(padding_bytes);
            flow.lines.push(first);
            flow.source_rows.push(index);
            let total: usize = remaining.spans.iter().map(|s| s.content.len()).sum();
            if used >= total {
                flow.consumed += 1;
                break;
            }
            if used == 0 {
                // Defensive progress guard for a whitespace-only wrapper result.
                return flow;
            }
            remaining = split_bytes(&remaining, used).1;
            continuation = true;
        }
    }
    flow
}

/// At most one source line can start per visible row. Scan only that suffix,
/// rather than walking a potentially enormous saved transcript. A single tall
/// final source line cannot be bottom-aligned by a source-index-only API.
pub(super) fn tail_start(lines: &[Line<'static>], area: Rect, reserved: Option<Rect>) -> usize {
    if lines.is_empty() || area.width == 0 || area.height == 0 {
        return lines.len();
    }
    let last = lines.len() - 1;
    let mut earliest = last;
    for start in (lines.len().saturating_sub(usize::from(area.height))..=last).rev() {
        let flow = layout(lines, start, area, reserved);
        if flow.consumed == lines.len() - start {
            earliest = start;
        }
    }
    earliest
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier, Style};

    #[test]
    fn preserves_whitespace_and_span_boundaries() {
        let source = Line::from(vec![
            Span::styled("alpha ", Style::default().fg(Color::Green)),
            Span::styled("beta  gamma     ", Style::default().fg(Color::Yellow)),
        ]);
        let flow = layout(&[source.clone()], 0, Rect::new(0, 0, 5, 20), None);
        assert_eq!(
            flow.lines.iter().map(plain).collect::<String>(),
            plain(&source)
        );
        assert_eq!(flow.consumed, 1);
        assert!(flow.lines.iter().all(|line| line.width() <= 5));
        let spaces = [Line::from("          ")];
        let flow = layout(&spaces, 0, Rect::new(0, 0, 3, 4), None);
        assert_eq!(
            flow.lines.iter().map(plain).collect::<String>(),
            "          "
        );
        assert_eq!(flow.consumed, 1);
    }

    #[test]
    fn preserves_unicode_and_styles_across_width_change() {
        let style = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
        let text = "界é🙂".repeat(30);
        let source =
            Line::from(Span::styled(text.clone(), style)).style(Style::default().bg(Color::Blue));
        let flow = layout(
            &[source],
            0,
            Rect::new(3, 4, 24, 40),
            Some(Rect::new(10, 4, 17, 3)),
        );
        assert_eq!(flow.consumed, 1);
        assert_eq!(flow.lines.iter().map(plain).collect::<String>(), text);
        assert!(flow.lines.iter().all(|l| l.style.bg == Some(Color::Blue)));
        assert!(
            flow.lines
                .iter()
                .flat_map(|l| &l.spans)
                .all(|s| s.style == style)
        );
        assert!(flow.lines[3].width() > 6);
        assert!(flow.source_rows.iter().all(|&row| row == 0));
    }

    #[test]
    fn continuation_expands_below_card_without_losing_overflow() {
        let source = Line::from("abcdefghijklmnopqrstuvwxyz0123456789");
        let flow = layout(
            &[source.clone()],
            0,
            Rect::new(0, 0, 20, 4),
            Some(Rect::new(6, 0, 14, 2)),
        );
        assert_eq!(
            flow.lines.iter().map(plain).collect::<String>(),
            plain(&source)
        );
        assert_eq!(
            flow.lines.iter().map(Line::width).collect::<Vec<_>>(),
            vec![5, 5, 20, 6]
        );
        assert_eq!(flow.consumed, 1);
    }

    #[test]
    fn hanging_indent_and_absolute_source_indices() {
        let source = Line::from("• abcdefghijklmnopqrstuvwxyz");
        let flow = layout(
            &[Line::from("skip"), source],
            1,
            Rect::new(0, 0, 10, 8),
            None,
        );
        assert!(plain(&flow.lines[1]).starts_with("  "));
        assert!(flow.source_rows.iter().all(|&row| row == 1));
        assert_eq!(flow.consumed, 1);
    }

    #[test]
    fn zero_width_occlusion_does_not_consume_source() {
        let lines = [Line::from("abc")];
        let flow = layout(
            &lines,
            0,
            Rect::new(0, 0, 5, 3),
            Some(Rect::new(0, 0, 5, 2)),
        );
        assert_eq!(flow.lines.len(), 3);
        assert_eq!(plain(&flow.lines[2]), "abc");
        assert_eq!(flow.consumed, 1);
        assert_eq!(layout(&lines, 0, Rect::default(), None).consumed, 0);
        assert_eq!(
            layout(&lines, 9, Rect::new(0, 0, 1, 1), None).lines.len(),
            0
        );
    }

    #[test]
    fn partial_source_is_not_counted_and_tail_scan_is_bounded() {
        let lines = vec![Line::from("short"); 10_000];
        assert_eq!(tail_start(&lines, Rect::new(0, 0, 10, 3), None), 9997);
        let tall = [Line::from("x".repeat(100))];
        assert_eq!(layout(&tall, 0, Rect::new(0, 0, 2, 3), None).consumed, 0);
        assert_eq!(tail_start(&tall, Rect::new(0, 0, 2, 3), None), 0);
        assert_eq!(
            layout(&[Line::from("界界")], 0, Rect::new(0, 0, 1, 4), None).consumed,
            1
        );
    }
}
