//! Backend-neutral width measurement and line wrapping.
//!
//! Wrapping needs to know how "wide" text is, but width is backend-specific:
//! the terminal measures in monospace columns, the desktop measures in pixels
//! via font metrics. We abstract that behind [`WidthMeasure`] so the wrapping
//! algorithm itself is shared.

use crate::model::{StyledLine, StyledSpan};

/// Measures the display width of text in a backend's units.
pub trait WidthMeasure {
    /// Width of a whole string in this backend's units.
    fn measure(&self, text: &str) -> usize;

    /// Width of a single `char`. Defaults to measuring a one-char string, but
    /// backends can override for speed.
    fn measure_char(&self, ch: char) -> usize {
        let mut buf = [0u8; 4];
        self.measure(ch.encode_utf8(&mut buf))
    }
}

/// Terminal-style measurer: Unicode display columns (what ratatui uses).
#[derive(Debug, Clone, Copy, Default)]
pub struct ColumnWidth;

impl WidthMeasure for ColumnWidth {
    fn measure(&self, text: &str) -> usize {
        use unicode_width::UnicodeWidthStr;
        text.width()
    }

    fn measure_char(&self, ch: char) -> usize {
        use unicode_width::UnicodeWidthChar;
        ch.width().unwrap_or(0)
    }
}

/// Wrap a single styled line to `max_width` (in the measurer's units),
/// preserving span styling across the wrap boundary. Wrapping prefers to break
/// at whitespace; words longer than `max_width` are hard-split.
///
/// Alignment is carried onto every produced line.
pub fn wrap_line<M: WidthMeasure>(
    line: &StyledLine,
    max_width: usize,
    measure: &M,
) -> Vec<StyledLine> {
    if max_width == 0 {
        return vec![line.clone()];
    }
    if measure.measure(&line.plain_text()) <= max_width {
        return vec![line.clone()];
    }

    let mut out: Vec<StyledLine> = Vec::new();
    let mut cur: Vec<StyledSpan> = Vec::new();
    let mut cur_width = 0usize;

    let push_line = |out: &mut Vec<StyledLine>, spans: Vec<StyledSpan>| {
        out.push(StyledLine {
            spans,
            alignment: line.alignment,
        });
    };

    for span in &line.spans {
        // Split the span text into word / whitespace runs, keeping whitespace
        // so we can drop a trailing space at a wrap point but keep interior
        // spacing.
        for token in tokenize(&span.text) {
            let tok_width = measure.measure(token);
            let is_ws = token.chars().all(char::is_whitespace);

            if cur_width + tok_width <= max_width {
                push_token(&mut cur, span, token);
                cur_width += tok_width;
                continue;
            }

            // Token doesn't fit on the current line.
            if is_ws {
                // Whitespace at a wrap boundary is dropped; start a fresh line.
                if !cur.is_empty() {
                    push_line(&mut out, std::mem::take(&mut cur));
                    cur_width = 0;
                }
                continue;
            }

            // Flush what we have, then place the word (hard-splitting if the
            // word itself is wider than max_width).
            if !cur.is_empty() {
                push_line(&mut out, std::mem::take(&mut cur));
                cur_width = 0;
            }

            if tok_width <= max_width {
                push_token(&mut cur, span, token);
                cur_width += tok_width;
            } else {
                // Hard-split an over-long word by char width.
                let mut chunk = String::new();
                let mut chunk_w = 0usize;
                for ch in token.chars() {
                    let cw = measure.measure_char(ch);
                    if chunk_w + cw > max_width && !chunk.is_empty() {
                        push_token(&mut cur, span, &chunk);
                        push_line(&mut out, std::mem::take(&mut cur));
                        chunk.clear();
                        chunk_w = 0;
                    }
                    chunk.push(ch);
                    chunk_w += cw;
                }
                if !chunk.is_empty() {
                    push_token(&mut cur, span, &chunk);
                    cur_width = chunk_w;
                }
            }
        }
    }

    if !cur.is_empty() {
        push_line(&mut out, cur);
    }
    if out.is_empty() {
        out.push(StyledLine {
            spans: vec![],
            alignment: line.alignment,
        });
    }
    out
}

/// Wrap many lines.
pub fn wrap_lines<M: WidthMeasure>(
    lines: &[StyledLine],
    max_width: usize,
    measure: &M,
) -> Vec<StyledLine> {
    lines
        .iter()
        .flat_map(|l| wrap_line(l, max_width, measure))
        .collect()
}

fn push_token(cur: &mut Vec<StyledSpan>, src: &StyledSpan, text: &str) {
    // Merge into the previous span when it shares styling, to avoid span
    // fragmentation across token boundaries.
    if let Some(last) = cur.last_mut()
        && last.role == src.role
        && last.fill == src.fill
        && last.attrs == src.attrs
    {
        last.text.push_str(text);
        return;
    }
    cur.push(StyledSpan {
        text: text.to_string(),
        role: src.role,
        fill: src.fill,
        attrs: src.attrs,
    });
}

/// Split text into alternating non-whitespace / whitespace runs.
fn tokenize(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut prev_ws: Option<bool> = None;
    for (idx, ch) in text.char_indices() {
        let is_ws = ch.is_whitespace();
        match prev_ws {
            Some(p) if p != is_ws => {
                out.push(&text[start..idx]);
                start = idx;
            }
            _ => {}
        }
        prev_ws = Some(is_ws);
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}
