//! Classify rendered rows by the frame art this renderer draws around code
//! blocks, display math, diffs, and blockquotes, so copy can drop it.
//!
//! The art is `┌─ lang` / `└─` edge rows plus leading `│ ` gutters. Whether a
//! `│ ` is art depends on context, not on the row alone:
//!
//! - Inside a `┌─` ... `└─` frame only one frame gutter (under any enclosing
//!   quote gutters) is art. A further `│ ` belongs to the code.
//! - Outside a frame, leading `│ ` runs are quote gutters, unless the bar sits
//!   on a table's column separator (the run at that column holds a `┼`). Then
//!   it is cell padding, which copy must keep to preserve the column layout.
//!
//! [`classify_frame_rows`] takes a row accessor so callers can classify a
//! selected range without slicing it first. It scans back to a row without any
//! frame glyph, so a selection that starts inside a block still knows which
//! frame it is in. It does not allocate.

use std::ops::Range;

use unicode_width::UnicodeWidthChar;

/// How copy should treat one rendered row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameRow {
    /// No frame art: copy the row unchanged.
    Content,
    /// A frame header or footer row: drop the whole row.
    Edge,
    /// This many leading display columns are frame art: copy only what follows.
    Gutter(usize),
}

const GUTTER: &str = "│ ";

fn has_frame_glyph(text: &str) -> bool {
    text.contains(['│', '┌', '├', '└', '┼'])
}

/// Whether a rendered markdown line is literal text that copy must keep as is.
/// Frame art is always drawn as a leading dim span, so a line whose first
/// visible span is styled otherwise (e.g. inline code `│ tree`) is content.
pub fn line_is_literal_text(line: &ratatui::text::Line<'_>) -> bool {
    line.spans
        .iter()
        .find(|span| !span.content.trim().is_empty())
        .is_some_and(|span| span.style.fg != Some(crate::md_dim_color()))
}

/// `┌─ lang` / `┌─` headers of code, math, and diff frames. Closed boxes
/// (`┌─ title ──┐`) and table-like rows are not frames.
fn is_frame_header(rest: &str) -> bool {
    let text = rest.trim_end();
    text == "┌─" || (text.starts_with("┌─ ") && !text.contains(" │ ") && !text.ends_with('┐'))
}

/// `└─` footers, including the diff summary `└─ (+N -M total)`.
fn is_frame_footer(rest: &str) -> bool {
    let text = rest.trim_end();
    text == "└─" || text.starts_with("└─ (+")
}

/// A bare `│` row (an empty gutter line whose trailing space was trimmed).
fn is_bare_bar(rest: &str) -> bool {
    rest.trim_end() == "│"
}

fn display_width(text: &str) -> usize {
    text.chars().map(|ch| ch.width().unwrap_or(0)).sum()
}

fn char_at_col(text: &str, col: usize) -> Option<char> {
    let mut at = 0usize;
    for ch in text.chars() {
        if at == col {
            return Some(ch);
        }
        at += ch.width().unwrap_or(0);
        if at > col {
            return None;
        }
    }
    None
}

fn has_bar_at(text: &str, col: usize) -> bool {
    matches!(char_at_col(text, col), Some('│' | '┼'))
}

/// The contiguous rows holding a bar at one column, and whether that run is a
/// table (some row has the `┼` header separator at that column).
#[derive(Clone, Copy)]
struct BarRun {
    col: usize,
    end: usize,
    is_table: bool,
}

struct Classifier {
    /// Quote depth of the open `┌─` frame, if any.
    frame_quote_depth: Option<usize>,
    bar_run: Option<BarRun>,
}

impl Classifier {
    fn classify<'a>(
        &mut self,
        idx: usize,
        text: &str,
        row: &impl Fn(usize) -> Option<&'a str>,
    ) -> FrameRow {
        let indent = text.len() - text.trim_start_matches(' ').len();
        let flush = &text[indent..];

        if let Some(depth) = self.frame_quote_depth {
            let kind = Self::frame_body_row(text, flush, depth).unwrap_or(FrameRow::Content);
            if kind == FrameRow::Edge && !flush.contains('├') {
                self.frame_quote_depth = None;
            }
            // Rendered frames always close with `└─`. A row that does not fit
            // the open frame (a soft-wrapped continuation) is copied as is.
            return kind;
        }

        let mut rest = flush;
        let mut depth = 0usize;
        loop {
            if is_frame_header(rest) {
                self.frame_quote_depth = Some(depth);
                return FrameRow::Edge;
            }
            let is_gutter = rest.starts_with(GUTTER) || is_bare_bar(rest);
            if !is_gutter || (depth == 0 && self.is_table_column(idx, indent, row)) {
                break;
            }
            depth += 1;
            match rest.strip_prefix(GUTTER) {
                Some(next) => rest = next,
                None => return FrameRow::Gutter(display_width(text)),
            }
        }

        if depth == 0 {
            FrameRow::Content
        } else {
            FrameRow::Gutter(display_width(text) - display_width(rest))
        }
    }

    /// Classify a row inside an open frame nested under `depth` quote gutters.
    fn frame_body_row(text: &str, flush: &str, depth: usize) -> Option<FrameRow> {
        let mut rest = flush;
        for _ in 0..depth {
            rest = rest.strip_prefix(GUTTER)?;
        }
        // `├─ diff · path` dividers between files of a multi-file diff.
        if is_frame_footer(rest) || rest.starts_with("├─") {
            return Some(FrameRow::Edge);
        }
        if let Some(code) = rest.strip_prefix(GUTTER) {
            return Some(FrameRow::Gutter(display_width(text) - display_width(code)));
        }
        is_bare_bar(rest).then(|| FrameRow::Gutter(display_width(text)))
    }

    /// Whether the bar at `col` on row `idx` is a table column separator.
    /// Computed once per contiguous run of rows with a bar at that column.
    fn is_table_column<'a>(
        &mut self,
        idx: usize,
        col: usize,
        row: &impl Fn(usize) -> Option<&'a str>,
    ) -> bool {
        if let Some(run) = self.bar_run
            && run.col == col
            && idx < run.end
        {
            return run.is_table;
        }
        let bar_at = |at: usize| row(at).is_some_and(|text| has_bar_at(text, col));
        let mut start = idx;
        while start > 0 && bar_at(start - 1) {
            start -= 1;
        }
        let mut end = idx + 1;
        while bar_at(end) {
            end += 1;
        }
        let is_table =
            (start..end).any(|at| row(at).is_some_and(|text| char_at_col(text, col) == Some('┼')));
        self.bar_run = Some(BarRun { col, end, is_table });
        is_table
    }
}

/// Classify rows `range` of a rendered pane whose rows are given by `row`.
///
/// Calls `visit(idx, text, kind)` once per row in `range`, in order, and
/// returns `None` if any row in `range` is missing.
pub fn classify_frame_rows<'a>(
    row: impl Fn(usize) -> Option<&'a str>,
    range: Range<usize>,
    mut visit: impl FnMut(usize, &'a str, FrameRow),
) -> Option<()> {
    // Start from a row with no frame glyph so the frame state is known.
    let mut idx = range.start;
    while idx > 0 && row(idx - 1).is_some_and(has_frame_glyph) {
        idx -= 1;
    }
    let mut classifier = Classifier {
        frame_quote_depth: None,
        bar_run: None,
    };
    while idx < range.end {
        let text = row(idx)?;
        let kind = classifier.classify(idx, text, &row);
        if idx >= range.start {
            visit(idx, text, kind);
        }
        idx += 1;
    }
    Some(())
}

/// The part of `text` from display column `col` on.
fn slice_from_col(text: &str, col: usize) -> &str {
    let mut at = 0usize;
    for (byte, ch) in text.char_indices() {
        if at >= col {
            return &text[byte..];
        }
        at += ch.width().unwrap_or(0);
    }
    ""
}

/// Remove frame art from a block of rendered rows: edge rows are dropped and
/// gutters are trimmed per [`classify_frame_rows`]. The whole block is the
/// context, so pass complete rendered rows rather than a partial selection.
pub fn strip_rendered_frame_decorations(text: &str) -> String {
    if !has_frame_glyph(text) {
        return text.to_string();
    }
    let rows: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<&str> = Vec::with_capacity(rows.len());
    classify_frame_rows(
        |idx| rows.get(idx).copied(),
        0..rows.len(),
        |_, row, kind| match kind {
            FrameRow::Content => out.push(row),
            FrameRow::Edge => {}
            FrameRow::Gutter(width) => out.push(slice_from_col(row, width)),
        },
    );
    out.join("\n")
}
