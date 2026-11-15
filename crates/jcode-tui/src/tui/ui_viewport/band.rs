//! Band wrap-around for the pinned Overview HUD.
//!
//! The HUD covers the top rows and right-hand columns of the chat viewport.
//! Rows inside the band re-wrap at the narrower width while rows below keep
//! the prepared full width, so the transcript wraps around the panel instead
//! of losing a full column for every row.
//!
//! Copy and selection stay exact: every display row carries a `CopyRowBinding`
//! back into the wide (full-width) frame, which remains the single source of
//! truth for copied text.

use ratatui::prelude::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Band geometry for one frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PinnedBand {
    /// Display rows the HUD covers, counting from the viewport top.
    pub(crate) height: u16,
    /// Columns the HUD occupies on the right edge.
    pub(crate) reserve: u16,
}

impl PinnedBand {
    /// Width transcript rows wrap to inside the band.
    pub(crate) fn narrow_width(&self, content_width: u16) -> usize {
        (content_width.saturating_sub(self.reserve)).max(1) as usize
    }

    /// Content-area rows the HUD covers after the synthetic top-band rows
    /// (prompt preview, pinned todos) have taken their share.
    pub(crate) fn content_rows(&self, top_band_lines: u16) -> u16 {
        self.height.saturating_sub(top_band_lines)
    }
}

/// How one display row maps back into the wide frame for copy and selection:
/// `wide_column(rel)` is the wide line's display column for row-local column
/// `rel`. Continuation rows clamp their re-seeded gutter into the row's copy
/// start, matching how the wide frame itself collapses gutters through
/// `wrapped_copy_offset`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CopyRowBinding {
    /// Wide-frame line this display row renders.
    pub(crate) wide_line: usize,
    /// Added to the row-local column before clamping.
    pub(crate) shift: usize,
    /// Lower clamp: the wide column this row's copyable content starts at.
    pub(crate) floor: usize,
    /// Upper clamp: one past the row's last wide content column.
    pub(crate) ceil: usize,
}

impl CopyRowBinding {
    /// Wide-frame column for a row-local display column.
    pub(crate) fn wide_column(&self, rel_col: usize) -> usize {
        (self.shift + rel_col).clamp(self.floor.min(self.ceil), self.ceil)
    }
}

/// Band split of one frame's visible wide rows.
pub(crate) struct BandSplit {
    /// Exactly `visible_height` rows: narrow-wrapped rows in the band, the
    /// straddle remainder, identity full-width rows, then blank padding.
    pub(crate) lines: Vec<Line<'static>>,
    /// Per display row, the wide-frame mapping for copy and selection.
    pub(crate) bindings: Vec<CopyRowBinding>,
    /// Wide rows consumed; the visible window is `[scroll, scroll + wide_fit)`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) wide_fit: usize,
    /// Display rows the band covers at the top of the content area.
    pub(crate) band_rows: usize,
    /// Whether one wide line straddled the band boundary (its remainder row
    /// renders directly below the band).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) straddled: bool,
    /// Rows with real content; rows past this are blank padding.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) real_rows: usize,
}

/// One narrow-wrapped piece of a wide line. `start`/`end` are the wide line's
/// display columns the fragment's content covers; the re-seeded gutter prefix
/// is excluded and maps onto `start`, like the wide frame's own gutter.
struct Fragment {
    line: Line<'static>,
    start: usize,
    end: usize,
}

/// Narrow wrap of one wide line plus `prefix`, the re-seeded gutter width the
/// wrap engine adds to every continuation fragment (0 for plain lines and the
/// balanced path).
struct WrappedRow {
    fragments: Vec<Fragment>,
    prefix: usize,
}

/// Wrap `line` at `width`, giving each fragment its wide-line column range.
///
/// Fragment content is conserved along both wrap paths. The token path is
/// strictly contiguous (`start[n] == end[n-1]`), with every continuation
/// re-seeding a gutter prefix of `prefix` cells. The balanced path (Center/
/// Right alignment) drops exactly one break space per boundary, so its
/// fragments end that many cells left of the cumulative-width sum. Ranges
/// rebuild from cumulative fragment widths alone using these two per-line
/// constants.
fn wrap_line_columns(line: &Line<'static>, width: usize) -> WrappedRow {
    let wide_width = line.width();
    if width == 0 || wide_width <= width {
        return WrappedRow {
            fragments: vec![Fragment {
                line: line.clone(),
                start: 0,
                end: wide_width,
            }],
            prefix: 0,
        };
    }
    let wrapped = crate::tui::markdown::wrap_line(line.clone(), width);
    let count = wrapped.len();
    let widths: Vec<usize> = wrapped.iter().map(Line::width).collect();
    let total: usize = widths.iter().sum();
    // Re-seeded gutter cells each continuation adds beyond the wide content.
    // A negative deficit (balanced path) pins the correction at zero and the
    // boundary loop below tightens the ranges instead.
    let prefix = count
        .checked_sub(1)
        .filter(|_| total >= wide_width)
        .map(|n| (total - wide_width) / n)
        .unwrap_or(0);
    // Cells the balanced path drops at each boundary: the token path fully
    // conserves (total == wide_width + (n-1)*prefix), the balanced path
    // conserves minus one break space per boundary.
    let boundary_drop = wide_width
        .saturating_add((count.saturating_sub(1)) * prefix)
        .saturating_sub(total)
        .min(count.saturating_sub(1));
    let per_boundary_drop = match count.checked_sub(1) {
        Some(boundaries) if boundaries > 0 => boundary_drop / boundaries,
        _ => 0,
    };
    let mut fragments = Vec::with_capacity(count);
    let mut prev_end = 0usize;
    let mut cum = 0usize;
    for (i, fragment) in wrapped.into_iter().enumerate() {
        cum += widths[i];
        let end = cum
            .saturating_sub(i * prefix)
            .saturating_add(i * per_boundary_drop)
            .min(wide_width);
        // The token path is contiguous; the balanced path skips the break
        // space it dropped at each earlier boundary, so fragment k starts one
        // dropped cell past the previous fragment's end.
        let start = if i == 0 {
            prev_end
        } else {
            prev_end.saturating_add(per_boundary_drop)
        };
        fragments.push(Fragment {
            line: fragment,
            start,
            end,
        });
        prev_end = end;
    }
    WrappedRow { fragments, prefix }
}

/// Leading spans covering `width` display cells, cutting the boundary span.
fn leading_spans(spans: &[Span<'static>], width: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0usize;
    for span in spans {
        if used >= width {
            break;
        }
        let span_width = UnicodeWidthStr::width(span.content.as_ref());
        if used + span_width <= width {
            out.push(span.clone());
        } else {
            out.push(span_prefix(span, width - used));
        }
        used += span_width;
    }
    out
}

/// Leading cells of `span` up to `width` display columns.
fn span_prefix(span: &Span<'static>, width: usize) -> Span<'static> {
    let mut text = String::new();
    let mut used = 0usize;
    for ch in span.content.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > width {
            break;
        }
        text.push(ch);
        used += ch_width;
    }
    Span::styled(text, span.style)
}

/// Cells of `span` from display column `skip` to its end.
fn span_suffix(span: &Span<'static>, skip: usize) -> Span<'static> {
    let mut text = String::new();
    let mut used = 0usize;
    for ch in span.content.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used >= skip {
            text.push(ch);
        } else {
            used += ch_width;
        }
    }
    Span::styled(text, span.style)
}

/// The line's spans from display column `col` onward, cut mid-span when the
/// column lands inside a span. Keeps the line's alignment.
fn slice_line_from(line: &Line<'static>, col: usize) -> Line<'static> {
    let mut out = Vec::new();
    let mut used = 0usize;
    for span in &line.spans {
        let span_width = UnicodeWidthStr::width(span.content.as_ref());
        if used + span_width <= col {
            used += span_width;
            continue;
        }
        if used < col {
            out.push(span_suffix(span, col - used));
        } else {
            out.push(span.clone());
        }
        used += span_width;
    }
    let mut sliced = Line::from(out);
    if let Some(alignment) = line.alignment {
        sliced = sliced.alignment(alignment);
    }
    sliced
}

/// Split the visible wide rows into band rows and full-width rows.
///
/// The band occupies the top band rows of the content area. Wide rows are
/// consumed in order: rows whose narrow fragments fit entirely within the
/// band are re-wrapped at the band width; the one wide line that straddles
/// the band boundary fills the rest of the band with its first fragments and
/// continues full-width directly below, so only one wide line ever straddles;
/// wide lines below render exactly as prepared.
pub(super) fn split_visible_rows(
    wide_rows: Vec<Line<'static>>,
    scroll: usize,
    visible_height: usize,
    band: PinnedBand,
    top_band_lines: u16,
    content_width: u16,
    copy_offset: &dyn Fn(usize) -> usize,
) -> BandSplit {
    let band_rows = (band.content_rows(top_band_lines) as usize).min(visible_height);
    let narrow = band.narrow_width(content_width);
    let total_input = wide_rows.len();
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(visible_height);
    let mut bindings: Vec<CopyRowBinding> = Vec::with_capacity(visible_height);
    let mut wide_cursor = 0usize;
    let mut wide_consumed = 0usize;
    let mut straddled = false;
    // (wide line, re-seeded prefix spans, continuation column, prefix width, wide width)
    let mut remainder: Option<(usize, Vec<Span<'static>>, usize, usize, usize)> = None;

    while lines.len() < band_rows && wide_cursor < total_input {
        let wide_line = scroll + wide_cursor;
        let copy_start = copy_offset(wide_line);
        let wrapped = wrap_line_columns(&wide_rows[wide_cursor], narrow);
        let space_left = band_rows - lines.len();
        let count = wrapped.fragments.len();
        let take = count.min(space_left);
        let straddling = take < count;
        let meta: Vec<(usize, usize)> = wrapped
            .fragments
            .iter()
            .map(|fragment| (fragment.start, fragment.end))
            .collect();
        let mut prefix_spans = Vec::new();
        if straddling && wrapped.prefix > 0 {
            prefix_spans = leading_spans(&wrapped.fragments[1].line.spans, wrapped.prefix);
        }
        for (i, fragment) in wrapped.fragments.into_iter().enumerate().take(take) {
            let (start, end) = meta[i];
            let binding = if i == 0 {
                CopyRowBinding {
                    wide_line,
                    shift: 0,
                    floor: copy_start,
                    ceil: end,
                }
            } else {
                CopyRowBinding {
                    wide_line,
                    shift: start.saturating_sub(wrapped.prefix),
                    floor: start,
                    ceil: end,
                }
            };
            lines.push(fragment.line);
            bindings.push(binding);
        }
        if straddling {
            straddled = true;
            let wide_width = wide_rows[wide_cursor].width();
            remainder = Some((
                wide_line,
                prefix_spans,
                meta[take].0,
                wrapped.prefix,
                wide_width,
            ));
            break;
        }
        wide_cursor += 1;
        wide_consumed += 1;
    }

    // Straddle remainder: the wide line's tail continues full-width below the
    // band, repeating the gutter so the join reads like the wide frame's own
    // continuation rows.
    if let Some((wide_line, prefix_spans, continuation, prefix, wide_width)) = remainder {
        let sliced = slice_line_from(&wide_rows[wide_cursor], continuation);
        let mut spans = prefix_spans;
        spans.extend(sliced.spans);
        let mut line = Line::from(spans);
        if let Some(alignment) = wide_rows[wide_cursor].alignment {
            line = line.alignment(alignment);
        }
        lines.push(line);
        bindings.push(CopyRowBinding {
            wide_line,
            shift: continuation.saturating_sub(prefix),
            floor: continuation,
            ceil: wide_width,
        });
        wide_cursor += 1;
        wide_consumed += 1;
    }

    let mut rest = wide_rows.into_iter().enumerate().skip(wide_cursor);
    while lines.len() < visible_height {
        let Some((offset, line)) = rest.next() else {
            break;
        };
        let wide_line = scroll + offset;
        let copy_start = copy_offset(wide_line);
        let wide_width = line.width();
        bindings.push(CopyRowBinding {
            wide_line,
            shift: 0,
            floor: copy_start,
            ceil: wide_width,
        });
        lines.push(line);
        wide_consumed += 1;
    }

    let real_rows = lines.len();
    let mut padding = 0usize;
    while lines.len() < visible_height {
        // Blank rows below the last wide line (bottom-anchor overshoot).
        // Bind past the frame's end so copy treats them as empty.
        bindings.push(CopyRowBinding {
            wide_line: scroll + wide_consumed + padding,
            shift: 0,
            floor: 0,
            ceil: 0,
        });
        lines.push(Line::default());
        padding += 1;
    }

    BandSplit {
        lines,
        bindings,
        wide_fit: wide_consumed,
        band_rows,
        straddled,
        real_rows,
    }
}

/// Wide rows whose band rendering fits in `visible_height` display rows
/// starting at `scroll`. Mirrors `split_visible_rows` accounting so the
/// fixpoint and the split can never disagree.
pub(super) fn wide_rows_that_fit(
    scroll: usize,
    total_lines: usize,
    visible_height: usize,
    band: PinnedBand,
    top_band_lines: u16,
    content_width: u16,
    materialize: &dyn Fn(usize, usize) -> Vec<Line<'static>>,
) -> usize {
    let remaining = total_lines.saturating_sub(scroll);
    let band_rows = (band.content_rows(top_band_lines) as usize).min(visible_height);
    if band_rows == 0 || remaining == 0 {
        return visible_height.min(remaining);
    }
    // Only the band's rows need content; identity rows below are one row each.
    let narrow = band.narrow_width(content_width);
    let need = band_rows.min(remaining);
    let rows = materialize(scroll, scroll + need);
    let mut consumed = 0usize;
    let mut display = 0usize;
    let mut straddled = false;
    let mut idx = 0usize;
    while display < band_rows && idx < rows.len() {
        let wrapped = wrap_line_columns(&rows[idx], narrow);
        let space_left = band_rows - display;
        if wrapped.fragments.len() > space_left {
            straddled = true;
            consumed += 1;
            display = band_rows;
        } else {
            display += wrapped.fragments.len();
            consumed += 1;
            idx += 1;
        }
    }
    let below = visible_height - display;
    consumed += below.saturating_sub(straddled as usize);
    consumed.min(remaining)
}

/// Display rows the packed view needs when the band re-wraps its top rows.
/// Mirrors `split_visible_rows` accounting at scroll 0 with no viewport cap:
/// band rows expand to their fragment count, the straddling line adds one
/// remainder row, and every line below the band stays identity (one row each).
pub(crate) fn packed_display_height(
    start: usize,
    total_lines: usize,
    band: PinnedBand,
    top_band_lines: u16,
    content_width: u16,
    materialize: &dyn Fn(usize, usize) -> Vec<Line<'static>>,
) -> usize {
    let visible_total = total_lines.saturating_sub(start);
    if visible_total == 0 {
        return 0;
    }
    let band_rows = band.content_rows(top_band_lines) as usize;
    if band_rows == 0 {
        return visible_total;
    }
    let narrow = band.narrow_width(content_width);
    let need = band_rows.min(visible_total);
    let rows = materialize(start, start + need);
    let mut display = 0usize;
    let mut consumed = 0usize;
    for line in &rows {
        if display >= band_rows {
            break;
        }
        let wrapped = wrap_line_columns(line, narrow);
        let space_left = band_rows - display;
        if wrapped.fragments.len() > space_left {
            // Straddle: the fragments fill the band and the remainder adds
            // one full-width row below it, so the line costs space_left + 1.
            display += space_left + 1;
            consumed += 1;
            break;
        }
        display += wrapped.fragments.len();
        consumed += 1;
    }
    display + visible_total.saturating_sub(consumed)
}

/// Bottom-anchor fixpoint for the band: the largest scroll whose band
/// rendering shows the tail. Starts from the naive no-band max and only moves
/// up, since undershooting would cut the tail while overshooting only pads
/// blank rows. Monotone by construction: `wide_rows_that_fit` is capped at
/// `total - scroll`, so `next >= scroll` always.
pub(super) fn bottom_anchor_max_scroll(
    total_lines: usize,
    naive_max_scroll: usize,
    visible_height: usize,
    band: PinnedBand,
    top_band_lines: u16,
    content_width: u16,
    materialize: &dyn Fn(usize, usize) -> Vec<Line<'static>>,
) -> usize {
    let mut scroll = naive_max_scroll.min(total_lines);
    for _ in 0..8 {
        let fits = wide_rows_that_fit(
            scroll,
            total_lines,
            visible_height,
            band,
            top_band_lines,
            content_width,
            materialize,
        );
        let next = total_lines.saturating_sub(fits);
        if next <= scroll {
            break;
        }
        scroll = next;
    }
    scroll
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn wrap_line_columns_plain_line_is_contiguous() {
        let line = Line::from("word ".repeat(12));
        let wrapped = wrap_line_columns(&line, 20);
        assert!(wrapped.fragments.len() > 1);
        assert_eq!(wrapped.prefix, 0);
        for pair in wrapped.fragments.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        assert_eq!(wrapped.fragments.last().unwrap().end, line.width());
        for fragment in &wrapped.fragments {
            assert!(fragment.line.width() <= 20);
        }
    }

    #[test]
    fn wrap_line_columns_splits_oversized_words_per_char() {
        let line = Line::from("x".repeat(25));
        let wrapped = wrap_line_columns(&line, 10);
        assert_eq!(wrapped.prefix, 0);
        let starts: Vec<usize> = wrapped.fragments.iter().map(|f| f.start).collect();
        let ends: Vec<usize> = wrapped.fragments.iter().map(|f| f.end).collect();
        assert_eq!(starts, vec![0, 10, 20]);
        assert_eq!(ends, vec![10, 20, 25]);
    }

    #[test]
    fn wrap_line_columns_gutter_line_reseeds_prefix() {
        let line = Line::from(vec![Span::raw("│ "), Span::raw("quote text ".repeat(6))]);
        let wrapped = wrap_line_columns(&line, 12);
        assert!(wrapped.fragments.len() >= 2);
        assert_eq!(wrapped.prefix, 2);
        for pair in wrapped.fragments.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        let widths: Vec<usize> = wrapped.fragments.iter().map(|f| f.line.width()).collect();
        let sum: usize = widths.iter().sum();
        assert_eq!(
            sum.saturating_sub(line.width()),
            (widths.len() - 1) * wrapped.prefix
        );
        let prefix = leading_spans(&wrapped.fragments[1].line.spans, wrapped.prefix);
        assert_eq!(row_text(&Line::from(prefix)), "│ ");
    }

    #[test]
    fn wrap_line_columns_balanced_path_drops_one_space_per_boundary() {
        let line = Line::from("alpha beta gamma delta epsilon zeta")
            .alignment(ratatui::prelude::Alignment::Center);
        let wrapped = wrap_line_columns(&line, 15);
        assert!(wrapped.fragments.len() > 1);
        assert_eq!(wrapped.prefix, 0);
        // The balanced path drops the break space at each boundary: fragment
        // k starts one cell past fragment k-1's end, and the gaps total the
        // dropped spaces so the last fragment still ends at the wide width.
        for pair in wrapped.fragments.windows(2) {
            assert_eq!(pair[1].start, pair[0].end + 1);
        }
        assert_eq!(wrapped.fragments.last().unwrap().end, line.width());
        // Exactly one break space is dropped per boundary, and everything
        // else is conserved: the fragments' total width is the wide line minus
        // one cell per boundary.
        let widths: Vec<usize> = wrapped.fragments.iter().map(|f| f.line.width()).collect();
        let total: usize = widths.iter().sum();
        assert_eq!(total, line.width() - (wrapped.fragments.len() - 1));
        let text: String = wrapped
            .fragments
            .iter()
            .map(|f| row_text(&f.line))
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(UnicodeWidthStr::width(text.as_str()), total);
    }

    #[test]
    fn split_wraps_band_rows_and_keeps_identity_below() {
        let wide = vec![
            Line::from("alpha beta"),
            Line::from("gamma"),
            Line::from("tail one"),
            Line::from("z"),
        ];
        let band = PinnedBand {
            height: 2,
            reserve: 5,
        };
        let split = split_visible_rows(wide, 0, 4, band, 0, 15, &|_| 0);
        assert_eq!(split.lines.len(), 4);
        assert_eq!(split.band_rows, 2);
        assert!(!split.straddled);
        assert_eq!(split.wide_fit, 4);
        assert_eq!(split.real_rows, 4);
        assert_eq!(split.bindings[0].wide_line, 0);
        assert_eq!(split.bindings[2].wide_line, 2);
        assert_eq!(split.bindings[3].wide_line, 3);
        assert_eq!(split.bindings[2].wide_column(0), 0);
        assert_eq!(split.bindings[2].wide_column(7), 7);
    }

    #[test]
    fn split_straddle_continues_below_band() {
        let wide = vec![Line::from("word ".repeat(4)), Line::from("next")];
        let band = PinnedBand {
            height: 2,
            reserve: 3,
        };
        let split = split_visible_rows(wide, 0, 4, band, 0, 10, &|_| 0);
        assert!(split.straddled);
        assert_eq!(split.band_rows, 2);
        assert_eq!(split.lines.len(), 4);
        let text = |i: usize| row_text(&split.lines[i]);
        assert_eq!(text(0), "word ");
        assert_eq!(text(1), "word ");
        assert_eq!(text(2), "word word ");
        assert_eq!(text(3), "next");
        let remainder = split.bindings[2];
        assert_eq!(remainder.wide_line, 0);
        assert_eq!(remainder.shift, 10);
        assert_eq!(remainder.floor, 10);
        assert_eq!(remainder.ceil, 20);
        assert_eq!(remainder.wide_column(0), 10);
        assert_eq!(remainder.wide_column(9), 19);
        assert_eq!(split.bindings[3].wide_line, 1);
        assert_eq!(split.wide_fit, 2);
    }

    #[test]
    fn straddle_remainder_repeats_gutter_prefix() {
        let wide = vec![
            Line::from(vec![Span::raw("│ "), Span::raw("aa ".repeat(5))]),
            Line::from("next"),
        ];
        let band = PinnedBand {
            height: 1,
            reserve: 4,
        };
        let split = split_visible_rows(wide, 0, 4, band, 0, 12, &|_| 0);
        assert!(split.straddled);
        let text = |i: usize| row_text(&split.lines[i]);
        assert_eq!(text(0), "│ aa aa ");
        assert_eq!(text(1), "│ aa aa aa ");
        let remainder = split.bindings[1];
        assert_eq!(remainder.shift, 6);
        assert_eq!(remainder.floor, 8);
        assert_eq!(remainder.wide_column(0), 8);
        assert_eq!(remainder.wide_column(2), 8);
        assert_eq!(remainder.wide_column(3), 9);
    }

    #[test]
    fn split_pads_short_content_past_end() {
        let wide = vec![Line::from("only")];
        let band = PinnedBand {
            height: 0,
            reserve: 5,
        };
        let split = split_visible_rows(wide, 0, 3, band, 0, 20, &|_| 0);
        assert_eq!(split.lines.len(), 3);
        assert_eq!(split.real_rows, 1);
        assert_eq!(split.wide_fit, 1);
        assert_eq!(split.bindings[1].wide_line, 1);
        assert_eq!(split.bindings[2].wide_line, 2);
        assert_eq!(split.bindings[1].wide_column(0), 0);
    }

    #[test]
    fn fragment_zero_collapses_copy_offset_region() {
        let wide = vec![Line::from(vec![Span::raw("│ "), Span::raw("hello world")])];
        let band = PinnedBand {
            height: 0,
            reserve: 0,
        };
        let split = split_visible_rows(wide, 0, 1, band, 0, 40, &|_| 2);
        let binding = split.bindings[0];
        assert_eq!(binding.wide_column(0), 2);
        assert_eq!(binding.wide_column(1), 2);
        assert_eq!(binding.wide_column(5), 5);
    }

    #[test]
    fn content_rows_subtracts_top_band_lines() {
        let band = PinnedBand {
            height: 5,
            reserve: 4,
        };
        assert_eq!(band.content_rows(2), 3);
        assert_eq!(band.content_rows(7), 0);
    }

    #[test]
    fn packed_display_height_expands_band_rows_only() {
        let rows: Vec<Line<'static>> = (0..10).map(|_| Line::from("y".repeat(27))).collect();
        let materialize = |start: usize, end: usize| -> Vec<Line<'static>> {
            rows[start.min(rows.len())..end.min(rows.len())].to_vec()
        };
        let band = PinnedBand {
            height: 2,
            reserve: 17,
        };
        // Narrow width is 10, so each 27-cell line takes 3 fragments in the
        // band. Two band rows fit only the first line (straddle), whose
        // remainder adds one row below the band; the other 9 lines stay
        // identity.
        assert_eq!(packed_display_height(0, 10, band, 0, 27, &materialize), 12);
        // No band rows: identity height.
        assert_eq!(packed_display_height(0, 10, band, 2, 27, &materialize), 10);
        // Short transcript: the first line straddles (3 fragments, 2 band
        // rows), filling the band with 2 fragments plus one remainder row,
        // and the second line stays identity: 3 + 1 = 4 display rows.
        assert_eq!(packed_display_height(0, 2, band, 0, 27, &materialize), 4);
        // Terminal-clear window: the first visible line straddles, and the
        // remaining 4 identity lines follow: 3 + 4 = 7.
        assert_eq!(packed_display_height(5, 10, band, 0, 27, &materialize), 7);
    }

    #[test]
    fn packed_display_height_matches_split_accounting() {
        let rows: Vec<Line<'static>> = (0..8).map(|_| Line::from("z".repeat(40))).collect();
        let materialize = |start: usize, end: usize| -> Vec<Line<'static>> {
            rows[start.min(rows.len())..end.min(rows.len())].to_vec()
        };
        let band = PinnedBand {
            height: 3,
            reserve: 25,
        };
        let display = packed_display_height(0, 8, band, 0, 40, &materialize);
        // The same content rendered through the split with a viewport tall
        // enough to show it all must occupy exactly `display` rows.
        let split = split_visible_rows(materialize(0, 8), 0, display, band, 0, 40, &|_| 0);
        assert_eq!(split.real_rows, display);
        assert_eq!(split.wide_fit, 8);
    }

    #[test]
    fn bottom_anchor_max_scroll_grows_when_band_eats_rows() {
        let rows: Vec<Line<'static>> = (0..20).map(|_| Line::from("x".repeat(30))).collect();
        let materialize = |start: usize, end: usize| -> Vec<Line<'static>> {
            rows[start.min(rows.len())..end.min(rows.len())].to_vec()
        };
        let band = PinnedBand {
            height: 3,
            reserve: 20,
        };
        let naive = 20usize.saturating_sub(10);
        let resolved = bottom_anchor_max_scroll(20, naive, 10, band, 0, 30, &materialize);
        assert!(resolved >= naive);
        assert_eq!(resolved, 12);
        assert_eq!(
            wide_rows_that_fit(resolved, 20, 10, band, 0, 30, &materialize),
            8
        );
    }

    #[test]
    fn wide_rows_that_fit_matches_split() {
        let rows: Vec<Line<'static>> = (0..12).map(|_| Line::from("y".repeat(27))).collect();
        let materialize = |start: usize, end: usize| -> Vec<Line<'static>> {
            rows[start.min(rows.len())..end.min(rows.len())].to_vec()
        };
        let band = PinnedBand {
            height: 2,
            reserve: 17,
        };
        for scroll in [0usize, 3, 7] {
            let fits = wide_rows_that_fit(scroll, 12, 6, band, 0, 27, &materialize);
            let split = split_visible_rows(
                materialize(scroll, scroll + 6),
                scroll,
                6,
                band,
                0,
                27,
                &|_| 0,
            );
            assert_eq!(fits, split.wide_fit, "scroll {scroll}");
            assert_eq!(split.lines.len(), 6);
        }
    }
}
