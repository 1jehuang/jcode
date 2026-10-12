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
//! Border text is fitted to the available width: the left slot is truncated
//! with an ellipsis, and the right slot is dropped entirely rather than
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
    let right = right
        .as_ref()
        .map(padded)
        .filter(|line| left_w + gap + line.width() <= avail);

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

/// Truncate `line` to `max` display columns, ending in `… ` when cut.
/// Returns `None` when there is no room for anything meaningful.
pub(crate) fn fit(line: &Line<'static>, max: usize) -> Option<Line<'static>> {
    if line.width() <= max {
        return Some(line.clone());
    }
    if max < 4 {
        return None;
    }
    let budget = max - 2; // room for "…" and the closing pad
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
    let tail_style = spans.last().map(|s| s.style).unwrap_or_default();
    spans.push(Span::styled("…", tail_style));
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
    fn right_slot_is_dropped_before_it_collides() {
        let f = Framed::body(vec![Line::from("x")])
            .title("a long headline here")
            .title_right("secondary");
        let rows = render(&f, 26);
        assert!(rows[0].contains("a long headline"), "{}", rows[0]);
        assert!(!rows[0].contains("secondary"), "{}", rows[0]);
    }

    #[test]
    fn left_slot_truncates_with_ellipsis() {
        let f = Framed::body(vec![Line::from("x")]).title("abcdefghijklmnopqrstuvwxyz");
        let rows = render(&f, 12);
        assert!(rows[0].contains('…'), "{}", rows[0]);
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
