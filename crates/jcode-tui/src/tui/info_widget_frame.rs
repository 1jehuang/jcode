//! Border chrome for info widgets.
//!
//! Every info widget is drawn inside a rounded border whose top and bottom
//! rows used to be empty lines. Headers, summaries, overflow counts, legends,
//! and page dots live there instead, so the body rows are spent on detail.
//!
//! Layout convention:
//! - top-left: what the widget is plus its headline number
//! - top-right: a secondary stat
//! - bottom-left / bottom-right: overflow counts, legends, meters, page dots
//!
//! Border text is fitted to the available width: the left slot is cut at the
//! edge with no ellipsis, and the right slot is dropped entirely rather than
//! colliding with the left.

use crate::tui::color_support::rgb;
use ratatui::prelude::*;
use ratatui::widgets::Block;
use unicode_width::UnicodeWidthStr;

/// Widget name on the border: brighter than the border, still quiet.
pub(crate) fn label(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(rgb(175, 175, 190)).bold())
}

/// Secondary border text.
pub(crate) fn dim(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(rgb(125, 125, 140)))
}

/// Overflow marker such as `+3 more`.
pub(crate) fn more(hidden: usize) -> Span<'static> {
    Span::styled(
        format!("+{hidden} more"),
        Style::default().fg(rgb(110, 110, 125)),
    )
}

/// Hard-wrap an over-long row at display-cell width, keeping span styles.
/// Unlike a word wrapper, the first line is always full: a long single word
/// never jumps to its own line, leaving the marker row nearly blank. Used by
/// overview sections whose rows must wrap instead of truncating: the row is
/// built to intentionally exceed `width`, then reflowed here. Continuation
/// lines get `indent` leading spaces, reserved from their own budget.
pub(crate) fn wrap_row_lines(
    line: Line<'static>,
    width: usize,
    indent: usize,
) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![line];
    }
    if line.width() <= width {
        return vec![line];
    }
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cursor = 0usize;
    let mut first = true;
    for span in line.spans {
        let mut text = span.content.to_string();
        while !text.is_empty() {
            let budget = if first {
                width
            } else {
                width.saturating_sub(indent)
            };
            if cursor >= budget {
                out.push(Line::from(std::mem::take(&mut cur)));
                cursor = 0;
                first = false;
                continue;
            }
            let remaining = budget - cursor;
            let (head, rest) = split_at_cells(&text, remaining);
            let head_w = UnicodeWidthStr::width(head.as_str());
            cur.push(Span::styled(head, span.style));
            cursor += head_w;
            text = rest;
            if head_w == 0 {
                break;
            }
        }
    }
    if !cur.is_empty() {
        out.push(Line::from(cur));
    }
    for continuation in out.iter_mut().skip(1) {
        continuation.spans.insert(0, Span::raw(" ".repeat(indent)));
    }
    out
}

/// Split `text` at no more than `max_cells` display cells.
fn split_at_cells(text: &str, max_cells: usize) -> (String, String) {
    if max_cells == 0 {
        return (String::new(), text.to_string());
    }
    let mut w = 0usize;
    for (idx, ch) in text.char_indices() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > max_cells {
            return (text[..idx].to_string(), text[idx..].to_string());
        }
        w += cw;
    }
    (text.to_string(), String::new())
}

/// A widget's rendered body plus the text it wants on its border.
#[derive(Debug, Default, Clone)]
pub(crate) struct Framed {
    pub title: Option<Line<'static>>,
    pub title_right: Option<Line<'static>>,
    pub footer: Option<Line<'static>>,
    pub footer_right: Option<Line<'static>>,
    pub lines: Vec<Line<'static>>,
}

impl Framed {
    pub(crate) fn body(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            ..Default::default()
        }
    }

    pub(crate) fn title(mut self, line: impl Into<Line<'static>>) -> Self {
        self.title = Some(line.into());
        self
    }

    pub(crate) fn title_right(mut self, line: impl Into<Line<'static>>) -> Self {
        self.title_right = Some(line.into());
        self
    }

    pub(crate) fn footer(mut self, line: impl Into<Line<'static>>) -> Self {
        self.footer = Some(line.into());
        self
    }

    pub(crate) fn footer_right(mut self, line: impl Into<Line<'static>>) -> Self {
        self.footer_right = Some(line.into());
        self
    }

    /// Keep existing footer details, but never leave a visible frame's bottom
    /// border empty when neither slot contains text.
    pub(crate) fn footer_if_empty(mut self, line: impl Into<Line<'static>>) -> Self {
        if ![&self.footer, &self.footer_right]
            .into_iter()
            .flatten()
            .any(|line| {
                line.spans
                    .iter()
                    .any(|span| !span.content.trim().is_empty())
            })
        {
            self.footer = Some(line.into());
        }
        self
    }

    /// A widget whose body is empty would render as a bare border with text
    /// on it, which placement treats as border-only. Pull the headline back
    /// into the body so there is always at least one content row.
    pub(crate) fn settle(mut self) -> Self {
        if self.lines.is_empty() {
            if let Some(line) = self.title.take() {
                self.lines.push(line);
            } else if let Some(line) = self.footer.take() {
                self.lines.push(line);
            }
        }
        self
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Attach the border text to `block`, fitted to a widget `outer_width`
    /// columns wide (including the two corner cells).
    pub(crate) fn apply<'a>(&self, block: Block<'a>, outer_width: u16) -> Block<'a> {
        let avail = usize::from(outer_width.saturating_sub(2));
        let block = place(block, &self.title, &self.title_right, avail, true);
        place(block, &self.footer, &self.footer_right, avail, false)
    }

    /// Every line including border text, in reading order. For tests that
    /// assert on what a widget shows regardless of where it is drawn.
    #[cfg(test)]
    pub(crate) fn all_lines(&self) -> Vec<Line<'static>> {
        let mut out = Vec::new();
        out.extend(self.title.clone());
        out.extend(self.title_right.clone());
        out.extend(self.lines.iter().cloned());
        out.extend(self.footer.clone());
        out.extend(self.footer_right.clone());
        out
    }
}

fn padded(line: &Line<'static>) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len() + 2);
    spans.push(Span::raw(" "));
    spans.extend(line.spans.iter().cloned());
    spans.push(Span::raw(" "));
    Line::from(spans)
}

fn place<'a>(
    block: Block<'a>,
    left: &Option<Line<'static>>,
    right: &Option<Line<'static>>,
    avail: usize,
    top: bool,
) -> Block<'a> {
    let left = left.as_ref().and_then(|line| fit(&padded(line), avail));
    let left_w = left.as_ref().map_or(0, Line::width);
    // Keep at least one border cell between the two slots.
    let gap = usize::from(left_w > 0);
    // A right slot sharing the border with a left slot is secondary and is
    // dropped before it collides. Alone, it truncates like the left slot.
    let right = right.as_ref().map(padded).and_then(|line| {
        if left_w == 0 {
            fit(&line, avail)
        } else {
            Some(line).filter(|line| left_w + gap + line.width() <= avail)
        }
    });

    let mut block = block;
    for (line, alignment) in [(left, Alignment::Left), (right, Alignment::Right)] {
        let Some(line) = line else { continue };
        let line = line.alignment(alignment);
        block = if top {
            block.title_top(line)
        } else {
            block.title_bottom(line)
        };
    }
    block
}

/// Cut `line` to `max` display columns, keeping the closing pad. No ellipsis:
/// in a narrow margin every cell is better spent on text.
/// Returns `None` when there is no room for anything meaningful.
pub(crate) fn fit(line: &Line<'static>, max: usize) -> Option<Line<'static>> {
    if line.width() <= max {
        return Some(line.clone());
    }
    if max < 4 {
        return None;
    }
    let budget = max - 1; // room for the closing pad
    let mut used = 0usize;
    let mut spans: Vec<Span<'static>> = Vec::new();
    'outer: for span in &line.spans {
        let mut kept = String::new();
        for ch in span.content.chars() {
            let w = UnicodeWidthStr::width(ch.encode_utf8(&mut [0u8; 4]) as &str);
            if used + w > budget {
                if !kept.is_empty() {
                    spans.push(Span::styled(kept, span.style));
                }
                break 'outer;
            }
            used += w;
            kept.push(ch);
        }
        spans.push(Span::styled(kept, span.style));
    }
    spans.push(Span::raw(" "));
    Some(Line::from(spans))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::widgets::{BorderType, Borders};

    fn render(framed: &Framed, width: u16) -> Vec<String> {
        let backend = TestBackend::new(width, 3);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let block = framed.apply(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded),
                    width,
                );
                f.render_widget(block, f.area());
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..3)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn left_and_right_titles_share_the_top_border() {
        let f = Framed::body(vec![Line::from("x")])
            .title("Todos 3/7")
            .title_right("conf high");
        let rows = render(&f, 30);
        assert!(rows[0].starts_with("╭ Todos 3/7 "), "{}", rows[0]);
        assert!(rows[0].ends_with(" conf high ╮"), "{}", rows[0]);
    }

    #[test]
    fn wrap_row_lines_fills_the_first_line_before_breaking() {
        let line = Line::from(vec![
            Span::raw("M "),
            Span::styled(
                "crates/jcode-tui/src/tui/info_widget_git.rs",
                Style::default(),
            ),
        ]);
        let out = wrap_row_lines(line, 20, 3);
        let text: Vec<String> = out
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        // First line must be full: a word wrapper would push the whole path to
        // the second line and leave only the marker behind.
        assert!(text[0].starts_with("M crates/j"), "{:?}", text);
        assert!(text.len() >= 3, "{:?}", text);
        for (i, row) in text.iter().enumerate() {
            let w = unicode_width::UnicodeWidthStr::width(row.as_str());
            assert!(w <= 20, "row {i} width {w} > 20: {row:?}");
        }
        // Continuations indent under the marker prefix.
        assert!(text[1].starts_with("   "), "{:?}", text);
    }

    #[test]
    fn wrap_row_lines_keeps_short_rows_single() {
        let line = Line::from("M short_path.rs +1 −2");
        let out = wrap_row_lines(line, 40, 3);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn right_slot_is_dropped_before_it_collides() {
        let f = Framed::body(vec![Line::from("x")])
            .title("a long headline here")
            .title_right("secondary");
        let rows = render(&f, 26);
        assert!(rows[0].contains("a long headline"), "{}", rows[0]);
        assert!(!rows[0].contains("secondary"), "{}", rows[0]);
    }

    #[test]
    fn left_slot_is_cut_at_the_edge_without_ellipsis() {
        let f = Framed::body(vec![Line::from("x")]).title("abcdefghijklmnopqrstuvwxyz");
        let rows = render(&f, 12);
        assert_eq!(rows[0], "╭ abcdefgh ╮");
        assert_eq!(rows[0].chars().count(), 12);
        assert!(rows[0].ends_with('╮'), "{}", rows[0]);
    }

    #[test]
    fn footer_renders_on_bottom_border() {
        let f = Framed::body(vec![Line::from("x")])
            .footer("+3 more")
            .footer_right("● edited");
        let rows = render(&f, 30);
        assert!(rows[2].starts_with("╰ +3 more "), "{}", rows[2]);
        assert!(rows[2].ends_with(" ● edited ╯"), "{}", rows[2]);
    }

    #[test]
    fn missing_or_blank_footers_receive_fallback_text() {
        for framed in [
            Framed::body(vec![Line::from("x")]),
            Framed::body(vec![Line::from("x")])
                .footer("  ")
                .footer_right(""),
        ] {
            let framed = framed.footer_if_empty(dim("Working tree"));
            for width in [8, 12, 30] {
                let rows = render(&framed, width);
                assert!(rows[2].contains("Wor"), "{}", rows[2]);
                assert_eq!(UnicodeWidthStr::width(rows[2].as_str()), usize::from(width));
            }
        }
    }

    #[test]
    fn fallback_preserves_existing_footer_slots() {
        for framed in [
            Framed::body(vec![Line::from("x")]).footer("+3 more"),
            Framed::body(vec![Line::from("x")]).footer_right("● edited"),
            Framed::body(vec![Line::from("x")])
                .footer("+3 more")
                .footer_right("● edited"),
        ] {
            let before = render(&framed, 30);
            let after = render(&framed.footer_if_empty(dim("fallback")), 30);
            assert_eq!(before, after);
        }
    }

    #[test]
    fn lone_right_footer_is_cut_at_the_edge_without_ellipsis() {
        let f = Framed::body(vec![Line::from("x")]).footer_right("abcdefghijklmnopqrstuvwxyz");
        let rows = render(&f, 12);
        assert!(!rows[2].contains('…'), "{}", rows[2]);
        assert!(rows[2].contains("abcdefgh"), "{}", rows[2]);
        assert_eq!(rows[2].chars().count(), 12);
        assert!(rows[2].ends_with('╯'), "{}", rows[2]);
    }

    #[test]
    fn settle_moves_title_into_empty_body() {
        let f = Framed::default().title("only").settle();
        assert_eq!(f.lines.len(), 1);
        assert!(f.title.is_none());
    }

    #[test]
    fn tiny_widths_never_panic() {
        let f = Framed::body(vec![Line::from("x")])
            .title("headline")
            .title_right("r")
            .footer("f")
            .footer_right("fr");
        for w in 2..10 {
            let _ = render(&f, w);
        }
    }
}
