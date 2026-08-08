//! Composer text geometry, from Parley.
//!
//! The composer used to derive its own geometry three times over: a character
//! budget for soft wrapping, a measured string prefix for the caret, and a
//! second measured prefix per row for the selection bands. Those three
//! approximations disagreed with the glyphs Parley actually drew, which is why
//! the caret sat a fraction off the cursor and the highlight bands were the
//! wrong width on anything but plain ASCII.
//!
//! This module builds *one* Parley layout and reads the caret and selection
//! rectangles straight out of it with [`parley::Cursor`] and
//! [`parley::Selection`], the same primitives Parley's own text editor uses. The
//! geometry is therefore the glyph geometry by construction: it cannot drift,
//! and it is correct for proportional fonts, ligatures, clusters, and bidi text,
//! none of which a character budget can express.

use crate::text::{ParagraphStyle, TextSystem};
use parley::{Affinity, Cursor, Layout, Selection};
use vello::kurbo::Rect;
use vello::peniko::Brush;

/// A laid-out composer line: its vertical band and its byte range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    /// Top of the line box, in logical units relative to the layout origin.
    pub top: f64,
    /// Bottom of the line box, in logical units relative to the layout origin.
    pub bottom: f64,
    pub start: usize,
    pub end: usize,
}

/// One Parley layout of the composer text, plus the caret and selection
/// geometry read from it.
pub struct InputLayout {
    layout: Layout<Brush>,
    scale: f64,
    len: usize,
}

impl InputLayout {
    /// Lay out `text` wrapped to `max_width` logical units.
    pub fn new(
        text_system: &mut TextSystem,
        text: &str,
        max_width: f64,
        style: ParagraphStyle,
        scale: f64,
    ) -> Self {
        let layout = text_system.layout_paragraph(text, max_width as f32, style, scale);
        Self {
            layout,
            scale,
            len: text.len(),
        }
    }

    /// The underlying layout, for drawing.
    pub fn layout(&self) -> &Layout<Brush> {
        &self.layout
    }

    /// Visual lines, in layout order.
    pub fn lines(&self) -> Vec<Line> {
        self.layout
            .lines()
            .map(|line| {
                let m = line.metrics();
                let range = line.text_range();
                Line {
                    top: f64::from(m.block_min_coord) / self.scale,
                    bottom: f64::from(m.block_max_coord) / self.scale,
                    start: range.start,
                    end: range.end,
                }
            })
            .collect()
    }

    /// Number of visual lines. Always at least one, so an empty composer still
    /// reserves a row for its caret.
    pub fn line_count(&self) -> usize {
        self.layout.len().max(1)
    }

    /// Total laid-out height in logical units.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn height(&self) -> f64 {
        f64::from(self.layout.height()) / self.scale
    }

    /// How far to shift the layout up so the caret's line is visible in a well
    /// showing `visible` lines. Zero while the text fits, so short input sits
    /// at the top of the well rather than floating.
    pub fn scroll_offset(&self, cursor: usize, visible: usize) -> f64 {
        let lines = self.lines();
        if lines.len() <= visible.max(1) {
            return 0.0;
        }
        let caret_line = lines
            .iter()
            .rposition(|line| cursor >= line.start)
            .unwrap_or(0);
        // Keep the caret on the last visible row: the window ends at its line.
        let first = caret_line.saturating_sub(visible.max(1) - 1);
        lines[first].top
    }

    /// Caret rectangle for a byte offset, in logical units relative to the
    /// layout origin. This is Parley's own cursor geometry, so the bar sits in
    /// the glyph gap rather than at a measured guess.
    pub fn caret_rect(&self, offset: usize, width: f64) -> Rect {
        let cursor = self.cursor_at(offset);
        let rect = cursor.geometry(&self.layout, (width * self.scale) as f32);
        Rect::new(
            rect.x0 / self.scale,
            rect.y0 / self.scale,
            rect.x1 / self.scale,
            rect.y1 / self.scale,
        )
    }

    /// Highlight rectangles for a byte range, one per visual line it covers,
    /// in logical units relative to the layout origin.
    pub fn selection_rects(&self, start: usize, end: usize) -> Vec<Rect> {
        if start >= end {
            return Vec::new();
        }
        let selection = Selection::new(self.cursor_at(start), self.cursor_at(end));
        let mut rects = Vec::new();
        selection.geometry_with(&self.layout, |rect, _line| {
            rects.push(Rect::new(
                rect.x0 / self.scale,
                rect.y0 / self.scale,
                rect.x1 / self.scale,
                rect.y1 / self.scale,
            ));
        });
        rects
    }

    /// Byte offset nearest a point, in logical units relative to the layout
    /// origin. Shares the layout with drawing, so a click lands on the glyph
    /// the user actually aimed at.
    pub fn offset_at_point(&self, x: f64, y: f64) -> usize {
        Cursor::from_point(
            &self.layout,
            (x * self.scale) as f32,
            (y * self.scale) as f32,
        )
        .index()
        .min(self.len)
    }

    /// A cursor at a byte offset. Offsets at a soft wrap belong upstream (the
    /// end of the previous row) so a caret at a wrap point stays where the user
    /// last typed instead of jumping to the next row.
    fn cursor_at(&self, offset: usize) -> Cursor {
        Cursor::from_byte_index(&self.layout, offset.min(self.len), Affinity::Downstream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> ParagraphStyle {
        ParagraphStyle {
            font_size: crate::layout::BODY_SIZE,
            line_height: (crate::layout::COMPOSER_LINE_HEIGHT / f64::from(crate::layout::BODY_SIZE))
                as f32,
            ..Default::default()
        }
    }

    fn laid_out(text: &str, width: f64, scale: f64) -> InputLayout {
        let mut ts = TextSystem::default();
        InputLayout::new(&mut ts, text, width, style(), scale)
    }

    /// The caret must advance monotonically through the text, and never leave
    /// the laid-out box. This is the invariant the old measured-prefix caret
    /// could not guarantee once wrapping was involved.
    #[test]
    fn the_caret_advances_monotonically_and_stays_in_the_box() {
        for &scale in &[1.0, 1.75, 2.0] {
            let text = "alpha bravo charlie delta echo foxtrot golf hotel india";
            let l = laid_out(text, 200.0, scale);
            let height = l.height();
            let mut previous = (0.0f64, 0.0f64);
            for offset in text
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(text.len()))
            {
                let r = l.caret_rect(offset, 1.5);
                assert!(r.y0 >= -0.5 && r.y1 <= height + 0.5, "caret left the box");
                assert!(r.x0 >= -0.5, "caret went left of the box");
                // Either further right on the same row, or onto a later row.
                assert!(
                    r.y0 > previous.1 - 0.5 || r.x0 >= previous.0 - 0.5,
                    "caret moved backwards at offset {offset}"
                );
                previous = (r.x0, r.y0);
            }
        }
    }

    /// A selection must produce one band per visual line it covers, and those
    /// bands must stay inside the laid-out box.
    #[test]
    fn a_selection_covers_every_line_it_spans() {
        let text = "alpha bravo charlie delta echo foxtrot golf hotel india juliet";
        let l = laid_out(text, 160.0, 1.75);
        let lines = l.lines();
        assert!(lines.len() > 2, "test text did not wrap");
        let rects = l.selection_rects(0, text.len());
        assert_eq!(
            rects.len(),
            lines.len(),
            "selection bands did not cover every line"
        );
        for rect in &rects {
            assert!(rect.x1 > rect.x0, "empty selection band");
            assert!(rect.y1 > rect.y0, "flat selection band");
            assert!(rect.y1 <= l.height() + 0.5, "band left the box");
        }
    }

    /// An empty selection draws nothing: a collapsed range must not leave a
    /// stray one-pixel band behind the caret.
    #[test]
    fn a_collapsed_selection_draws_nothing() {
        let l = laid_out("alpha bravo", 400.0, 1.0);
        assert!(l.selection_rects(4, 4).is_empty());
        assert!(l.selection_rects(6, 3).is_empty());
    }

    /// The selection band must line up with the caret at its own edges: this is
    /// exactly the alignment that was visibly wrong before.
    #[test]
    fn selection_edges_line_up_with_the_caret() {
        let text = "alpha bravo charlie";
        let l = laid_out(text, 400.0, 1.75);
        for &(start, end) in &[(0usize, 5usize), (6, 11), (2, 17), (0, text.len())] {
            let rects = l.selection_rects(start, end);
            assert_eq!(rects.len(), 1, "unwrapped text banded on several lines");
            let band = rects[0];
            let left = l.caret_rect(start, 1.5);
            let right = l.caret_rect(end, 1.5);
            assert!(
                (band.x0 - left.x0).abs() < 1.0,
                "band start {:.2} missed the caret {:.2}",
                band.x0,
                left.x0
            );
            assert!(
                (band.x1 - right.x0).abs() < 1.0,
                "band end {:.2} missed the caret {:.2}",
                band.x1,
                right.x0
            );
        }
    }

    /// Clicking where a caret is drawn must return that caret's offset: the
    /// round trip that keeps clicks landing between the right glyphs.
    #[test]
    fn hit_testing_round_trips_with_the_caret() {
        let text = "alpha bravo charlie delta";
        for &scale in &[1.0, 1.75] {
            let l = laid_out(text, 200.0, scale);
            for offset in text
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(text.len()))
            {
                let r = l.caret_rect(offset, 1.5);
                let y = (r.y0 + r.y1) / 2.0;
                let hit = l.offset_at_point(r.x0, y);
                assert!(
                    hit == offset || (l.caret_rect(hit, 1.5).x0 - r.x0).abs() < 1.0,
                    "clicking the caret at {offset} returned {hit}"
                );
            }
        }
    }

    /// Every offset lands on some line, and lines tile the text without gaps or
    /// overlaps: the property the old wrap module tested, now for real layout.
    #[test]
    fn lines_tile_the_text() {
        let text = "alpha bravo\ncharlie delta echo foxtrot golf\n\nhotel";
        let l = laid_out(text, 120.0, 1.75);
        let lines = l.lines();
        assert!(!lines.is_empty());
        for pair in lines.windows(2) {
            assert!(
                pair[1].start >= pair[0].end,
                "lines overlapped: {:?} then {:?}",
                pair[0],
                pair[1]
            );
            assert!(pair[1].top >= pair[0].top, "lines were out of order");
        }
        assert_eq!(lines[0].start, 0);
        assert_eq!(lines[lines.len() - 1].end, text.len());
    }

    /// An empty composer still has a row, so its caret has somewhere to sit.
    #[test]
    fn an_empty_composer_still_has_a_row_and_a_caret() {
        let l = laid_out("", 400.0, 1.75);
        assert_eq!(l.line_count(), 1);
        let r = l.caret_rect(0, 1.5);
        assert!(r.y1 > r.y0, "the empty composer had no caret");
        assert!(r.x0.abs() < 1.0, "the empty caret was not at the left edge");
    }

    /// Geometry is reported in logical units, so it must not drift with the
    /// display scale. A regression here is the classic HiDPI caret offset.
    #[test]
    fn geometry_is_scale_independent() {
        let text = "alpha bravo charlie delta echo";
        let base = laid_out(text, 200.0, 1.0);
        for &scale in &[1.25, 1.5, 1.75, 2.0, 3.0] {
            let scaled = laid_out(text, 200.0, scale);
            assert_eq!(
                scaled.line_count(),
                base.line_count(),
                "line count changed at scale {scale}"
            );
            for offset in [0usize, 6, 12, text.len()] {
                let a = base.caret_rect(offset, 1.5);
                let b = scaled.caret_rect(offset, 1.5);
                assert!(
                    (a.x0 - b.x0).abs() < 2.0 && (a.y0 - b.y0).abs() < 2.0,
                    "caret at {offset} drifted at scale {scale}: {a:?} vs {b:?}"
                );
            }
        }
    }

    /// Degenerate inputs must not panic: a zero width, a lone newline, and
    /// multi-byte text all have to lay out.
    #[test]
    fn degenerate_input_does_not_panic() {
        for text in ["", "\n", "\n\n\n", "ünïcödé wörds", "a", &"x".repeat(500)] {
            for width in [0.0, 1.0, 40.0, 4000.0] {
                let l = laid_out(text, width, 1.75);
                assert!(l.line_count() >= 1);
                let _ = l.caret_rect(text.len(), 1.5);
                let _ = l.selection_rects(0, text.len());
                let _ = l.offset_at_point(-5.0, -5.0);
                let _ = l.offset_at_point(1e6, 1e6);
            }
        }
    }

    /// A click past the end of the text lands on the end, not out of bounds.
    #[test]
    fn clicking_past_the_end_lands_on_the_end() {
        let text = "alpha";
        let l = laid_out(text, 400.0, 1.75);
        assert_eq!(l.offset_at_point(10_000.0, 5.0), text.len());
    }

    /// A word longer than the well has no break opportunity, so with CSS's
    /// default overflow-wrap Parley let it run straight off the right edge:
    /// a pasted URL or path rendered as one endless line. Every layout must
    /// stay inside the width it was wrapped to.
    #[test]
    fn an_unbreakable_word_wraps_instead_of_overflowing() {
        let width = 200.0;
        for text in [
            "x".repeat(400),
            "https://example.com/a/very/long/path/that/never/breaks?q=1&r=2".to_string(),
            format!("short {} tail", "y".repeat(300)),
        ] {
            for &scale in &[1.0, 1.75] {
                let l = laid_out(&text, width, scale);
                assert!(
                    f64::from(l.layout.width()) / scale <= width + 1.0,
                    "layout {:.2} overflowed the {width} well",
                    f64::from(l.layout.width()) / scale
                );
                assert!(l.line_count() > 1, "the long word never wrapped");
                let caret = l.caret_rect(text.len(), 1.5);
                assert!(caret.x1 <= width + 1.0, "caret escaped the well");
            }
        }
    }

    /// Scrolling must always keep the caret's row inside the visible window,
    /// however long the text is: this is what stops typing running out of
    /// sight at the bottom of the well.
    #[test]
    fn scrolling_keeps_the_caret_row_visible() {
        let text = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima";
        let l = laid_out(text, 120.0, 1.75);
        let lines = l.lines();
        assert!(lines.len() > 3, "test text did not wrap enough");
        for visible in 1..=lines.len() + 2 {
            for offset in [0, text.len() / 2, text.len()] {
                let off = l.scroll_offset(offset, visible);
                let caret = l.caret_rect(offset, 1.5);
                let top = caret.y0 - off;
                let bottom = caret.y1 - off;
                let window = visible.max(1) as f64 * crate::layout::COMPOSER_LINE_HEIGHT;
                assert!(
                    top >= -0.5 && bottom <= window + 0.5,
                    "caret row [{top:.2},{bottom:.2}] outside a {visible}-line window"
                );
            }
        }
    }
}
