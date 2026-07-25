//! Soft wrapping for the composer.
//!
//! The composer works in *visual rows*: a long logical line wraps into several
//! rows so text never spills outside the well. Wrapping is a pure function of
//! (text, width, char width), which lets rendering, layout sizing, and mouse
//! hit-testing all agree on the same rows instead of each deriving their own.
//!
//! Wrapping is word-aware: it breaks at whitespace when it can, and mid-word
//! only when a single word is wider than the line.

/// One visual row: a byte range of the source text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub start: usize,
    pub end: usize,
    /// True when this row ends because of an explicit newline rather than a
    /// soft wrap. Used to place the caret after a trailing newline.
    pub hard_break: bool,
}

impl Row {
    /// The text of this row.
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start..self.end]
    }

    /// Whether a byte offset belongs to this row. The row that owns the very
    /// end of the text also owns the end offset, so the caret has a home.
    pub fn contains(&self, offset: usize, is_last: bool) -> bool {
        if is_last {
            offset >= self.start && offset <= self.end
        } else {
            offset >= self.start && offset < self.end
        }
    }
}

/// Wrap `text` into visual rows no wider than `max_chars` characters.
///
/// `max_chars` is a character budget rather than a pixel width: the composer
/// uses a monospace family, so this stays exact while keeping the function
/// pure and cheap to test.
pub fn wrap(text: &str, max_chars: usize) -> Vec<Row> {
    let max_chars = max_chars.max(1);
    let mut rows = Vec::new();
    let mut line_start = 0usize;

    // Split on explicit newlines first: those are content, not wrapping.
    loop {
        let rest = &text[line_start..];
        let (line, hard_break) = match rest.find('\n') {
            Some(index) => (&rest[..index], true),
            None => (rest, false),
        };
        wrap_line(line_start, line, max_chars, hard_break, &mut rows);
        match rest.find('\n') {
            Some(index) => line_start += index + 1,
            None => break,
        }
        // A trailing newline opens a final empty row so the caret can sit on it.
        if line_start == text.len() {
            rows.push(Row {
                start: line_start,
                end: line_start,
                hard_break: false,
            });
            break;
        }
    }

    if rows.is_empty() {
        rows.push(Row {
            start: 0,
            end: 0,
            hard_break: false,
        });
    }
    rows
}

/// Wrap one newline-free line, appending its rows.
fn wrap_line(offset: usize, line: &str, max_chars: usize, hard_break: bool, rows: &mut Vec<Row>) {
    if line.is_empty() {
        rows.push(Row {
            start: offset,
            end: offset,
            hard_break,
        });
        return;
    }

    let mut row_start = 0usize; // byte offset within `line`
    while row_start < line.len() {
        let remaining = &line[row_start..];
        let char_count = remaining.chars().count();
        if char_count <= max_chars {
            rows.push(Row {
                start: offset + row_start,
                end: offset + line.len(),
                hard_break,
            });
            return;
        }

        // Byte offset just past the last character that fits.
        let limit = remaining
            .char_indices()
            .nth(max_chars)
            .map(|(index, _)| index)
            .unwrap_or(remaining.len());

        // Prefer breaking after the last whitespace within the budget, so
        // words stay intact.
        let break_at = remaining[..limit]
            .rfind(char::is_whitespace)
            .map(|index| {
                // Break *after* the whitespace run so the next row does not
                // start with a space.
                let after = remaining[index..]
                    .char_indices()
                    .find(|(_, c)| !c.is_whitespace())
                    .map(|(delta, _)| index + delta)
                    .unwrap_or(limit);
                (index, after)
            })
            // A single word longer than the line: break mid-word rather than
            // overflowing the well.
            .filter(|(index, _)| *index > 0)
            .unwrap_or((limit, limit));

        let (row_end, next_start) = break_at;
        rows.push(Row {
            start: offset + row_start,
            end: offset + row_start + row_end,
            hard_break: false,
        });
        row_start += next_start;
    }

    // The line ended exactly on a wrap boundary; a trailing row keeps the
    // caret placeable at the end.
    if let Some(last) = rows.last() {
        if last.end < offset + line.len() {
            rows.push(Row {
                start: offset + row_start,
                end: offset + line.len(),
                hard_break,
            });
        } else if hard_break {
            // Mark the final row as ending on the newline.
            let index = rows.len() - 1;
            rows[index].hard_break = true;
        }
    }
}

/// The row index containing `offset`, and the character column within it.
pub fn row_col_at(rows: &[Row], source: &str, offset: usize) -> (usize, usize) {
    for (index, row) in rows.iter().enumerate() {
        let is_last = index + 1 == rows.len();
        if row.contains(offset, is_last) {
            let column = source[row.start..offset.max(row.start).min(row.end)]
                .chars()
                .count();
            return (index, column);
        }
    }
    let last = rows.len().saturating_sub(1);
    let column = rows
        .get(last)
        .map(|row| row.text(source).chars().count())
        .unwrap_or(0);
    (last, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(source: &str, rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| row.text(source).to_string())
            .collect()
    }

    #[test]
    fn short_text_is_a_single_row() {
        let text = "hello";
        assert_eq!(texts(text, &wrap(text, 20)), vec!["hello"]);
    }

    #[test]
    fn empty_text_still_has_one_row_for_the_caret() {
        let rows = wrap("", 20);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0],
            Row {
                start: 0,
                end: 0,
                hard_break: false
            }
        );
    }

    #[test]
    fn explicit_newlines_start_new_rows() {
        let text = "one\ntwo";
        assert_eq!(texts(text, &wrap(text, 20)), vec!["one", "two"]);
    }

    #[test]
    fn a_trailing_newline_opens_an_empty_row() {
        let text = "one\n";
        let rows = wrap(text, 20);
        assert_eq!(texts(text, &rows), vec!["one", ""]);
    }

    #[test]
    fn long_text_wraps_at_whitespace() {
        let text = "alpha bravo charlie";
        // Budget of 12 fits "alpha bravo" but not "charlie".
        assert_eq!(texts(text, &wrap(text, 12)), vec!["alpha bravo", "charlie"]);
    }

    #[test]
    fn wrapped_rows_never_exceed_the_budget() {
        let text = "the quick brown fox jumps over the lazy dog again and again and again";
        for budget in 4..40 {
            for row in wrap(text, budget) {
                let count = row.text(text).chars().count();
                assert!(
                    count <= budget,
                    "row {:?} is {count} chars, over the {budget} budget",
                    row.text(text)
                );
            }
        }
    }

    #[test]
    fn a_word_longer_than_the_line_breaks_mid_word() {
        // Otherwise it would overflow the composer well.
        let text = "supercalifragilistic";
        let rows = wrap(text, 6);
        assert!(rows.len() > 1);
        for row in &rows {
            assert!(row.text(text).chars().count() <= 6);
        }
        assert_eq!(texts(text, &rows).concat(), text);
    }

    #[test]
    fn wrapping_preserves_every_character() {
        // No text may be lost or duplicated by wrapping.
        for text in [
            "alpha bravo charlie delta",
            "one\ntwo three four five six",
            "supercalifragilisticexpialidocious and more",
            "héllo wörld with accénts and more text here",
        ] {
            for budget in [3usize, 5, 8, 13, 21] {
                let rows = wrap(text, budget);
                let joined: String = rows
                    .iter()
                    .map(|row| row.text(text))
                    .collect::<Vec<_>>()
                    .join("");
                let stripped: String = text.chars().filter(|c| *c != '\n').collect();
                let rejoined: String = joined.chars().filter(|c| !c.is_whitespace()).collect();
                let expected: String = stripped.chars().filter(|c| !c.is_whitespace()).collect();
                assert_eq!(
                    rejoined, expected,
                    "wrapping {text:?} at {budget} lost or duplicated characters"
                );
            }
        }
    }

    #[test]
    fn rows_are_ordered_and_never_overlap() {
        let text = "alpha bravo charlie delta echo foxtrot";
        let rows = wrap(text, 9);
        for pair in rows.windows(2) {
            assert!(pair[0].end <= pair[1].start, "rows overlap: {pair:?}");
        }
        assert_eq!(rows.first().map(|r| r.start), Some(0));
        assert_eq!(rows.last().map(|r| r.end), Some(text.len()));
    }

    #[test]
    fn row_boundaries_stay_on_char_boundaries() {
        let text = "héllo wörld with accénts everywhere";
        for budget in 2..20 {
            for row in wrap(text, budget) {
                assert!(text.is_char_boundary(row.start), "{row:?} split a char");
                assert!(text.is_char_boundary(row.end), "{row:?} split a char");
            }
        }
    }

    #[test]
    fn the_cursor_maps_to_a_row_and_column() {
        let text = "alpha bravo charlie";
        let rows = wrap(text, 12);
        assert_eq!(row_col_at(&rows, text, 0), (0, 0));
        assert_eq!(row_col_at(&rows, text, 5), (0, 5));
        // Offset 12 is the start of "charlie" on row 1.
        assert_eq!(row_col_at(&rows, text, 12), (1, 0));
        // The end of the text lives on the last row.
        assert_eq!(row_col_at(&rows, text, text.len()), (1, 7));
    }

    #[test]
    fn every_offset_maps_to_some_row() {
        // A cursor with no row would make the caret vanish.
        for text in ["", "a", "alpha bravo charlie delta", "one\ntwo\n"] {
            let rows = wrap(text, 7);
            for offset in 0..=text.len() {
                if !text.is_char_boundary(offset) {
                    continue;
                }
                let (row, _) = row_col_at(&rows, text, offset);
                assert!(row < rows.len(), "offset {offset} in {text:?} had no row");
            }
        }
    }

    #[test]
    fn a_zero_budget_does_not_hang_or_panic() {
        let rows = wrap("abc", 0);
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|row| row.text("abc").chars().count() <= 1));
    }
}
