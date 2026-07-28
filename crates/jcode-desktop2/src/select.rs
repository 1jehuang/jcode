//! Transcript text selection: highlighting and copying what is on screen.
//!
//! The composer has been selectable since it grew a real editor, but the
//! transcript, the part of the window a user actually wants to quote from, was
//! inert: a drag over a reply did nothing and Ctrl+C copied the composer's
//! line. This module is the missing half.
//!
//! A transcript selection cannot be a byte range the way the composer's is,
//! because the transcript is not one string. It is a list of messages, each of
//! which is a list of Parley layouts (a heading, a paragraph, a code block),
//! and only the *flattened* text of each layout exists as a string. So a
//! position here is the triple that actually identifies a character on screen:
//!
//! ```text
//! (message index, block index, byte offset within that block's text)
//! ```
//!
//! Everything else follows from ordering those triples. Highlight rectangles
//! come from the same [`parley::Selection`] geometry the composer uses, so the
//! bands line up with the glyphs by construction, and copied text is sliced
//! from the very strings the layouts were built from, so what you paste is
//! what you saw rather than the markdown source that produced it.

use crate::transcript::{BLOCK_GAP, LaidBlock, LaidMessage};
use crate::viewport::Viewport;
use parley::{Affinity, Cursor, Selection as ParleySelection};
use vello::kurbo::Rect;

/// A character position in the transcript.
///
/// Ordered by (message, block, offset), which is reading order, so a drag
/// anywhere on screen produces a range simply by sorting its two ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub message: usize,
    pub block: usize,
    pub offset: usize,
}

/// An in-progress or completed transcript selection: where the drag started
/// and where the pointer is now.
///
/// Anchor and focus are kept unsorted so dragging backwards behaves like every
/// other text surface: the anchor stays put and the focus follows the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Position,
    pub focus: Position,
}

impl Selection {
    pub fn new(anchor: Position, focus: Position) -> Self {
        Self { anchor, focus }
    }

    /// The selection in reading order.
    pub fn range(&self) -> (Position, Position) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    /// Whether the selection covers no characters. An empty selection must not
    /// draw a band or claim the clipboard, or a plain click would blank it.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.focus
    }

    /// The byte range of this selection within one block, or `None` when the
    /// block is entirely outside it.
    ///
    /// This is the whole of the "does a triple range intersect a block"
    /// question, in one place, so highlighting and copying can never disagree
    /// about which characters are selected.
    pub fn range_in(&self, message: usize, block: usize, len: usize) -> Option<(usize, usize)> {
        let (start, end) = self.range();
        let here = |p: &Position| p.message == message && p.block == block;
        let before = |p: &Position| (p.message, p.block) < (message, block);
        let after = |p: &Position| (p.message, p.block) > (message, block);

        let from = if here(&start) {
            start.offset.min(len)
        } else if before(&start) {
            0
        } else {
            return None;
        };
        let to = if here(&end) {
            end.offset.min(len)
        } else if after(&end) {
            len
        } else {
            return None;
        };
        (from < to).then_some((from, to))
    }
}

/// A highlight band to draw, in the same logical coordinate space the caller
/// uses to draw the block's glyphs.
pub struct Band {
    pub rect: Rect,
}

/// Rectangles covering `range` within `block`'s layout, in logical units
/// relative to the block's text origin.
///
/// Parley's own selection geometry, exactly as the composer uses it: the
/// highlight is the text's geometry rather than a re-measured guess, so it is
/// right for proportional fonts, clusters, and bidi runs.
pub fn block_bands(block: &LaidBlock, range: (usize, usize), scale: f64) -> Vec<Band> {
    layout_bands(&block.layout, range, scale)
}

/// As [`block_bands`], for a bare Parley layout.
///
/// Split out so the transcript can derive the wash behind an inline code span
/// from the same geometry a selection highlight uses. A code wash and a
/// selection band are the same question asked of the same layout ("where on
/// screen is this byte range?"), and answering it twice was how the two came
/// to disagree by a pixel on HiDPI.
pub fn layout_bands(
    layout: &parley::Layout<vello::peniko::Brush>,
    range: (usize, usize),
    scale: f64,
) -> Vec<Band> {
    let (start, end) = range;
    if start >= end {
        return Vec::new();
    }
    let cursor = |offset: usize| Cursor::from_byte_index(layout, offset, Affinity::Downstream);
    let selection = ParleySelection::new(cursor(start), cursor(end));
    let mut bands = Vec::new();
    selection.geometry_with(layout, |rect, _line| {
        bands.push(Band {
            rect: Rect::new(
                rect.x0 / scale,
                rect.y0 / scale,
                rect.x1 / scale,
                rect.y1 / scale,
            ),
        });
    });
    bands
}

/// The selected text, as it appears on screen.
///
/// Blocks are joined by a blank line, the same separation the transcript draws
/// between them, so a copied code block does not run into the paragraph above
/// it. This is the rendered text, not the markdown source: what you see is
/// what you paste.
pub fn selected_text(laid: &[LaidMessage], selection: &Selection) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for (message_index, message) in laid.iter().enumerate() {
        for (block_index, block) in message.blocks.iter().enumerate() {
            let Some((start, end)) =
                selection.range_in(message_index, block_index, block.source.len())
            else {
                continue;
            };
            let start = floor_boundary(&block.source, start);
            let end = ceil_boundary(&block.source, end);
            parts.push(&block.source[start..end]);
        }
    }
    parts.join("\n\n")
}

/// Round a byte index down to a char boundary. Offsets come from Parley, which
/// already lands on cluster boundaries, but slicing a `String` on a bad index
/// panics and a transcript renders arbitrary model output, so the boundary is
/// enforced here rather than assumed.
fn floor_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

/// Where a point lands in the transcript, in transcript coordinates.
///
/// `y` is relative to the top of the transcript region, matching
/// [`crate::viewport::Placed::top`], and `x` is relative to the message's text
/// left edge. Returns `None` only when there is nothing laid out at all; a
/// point in the margin between two messages resolves to the nearest one, so a
/// drag that strays into a gap keeps extending rather than sticking.
pub fn position_at(view: &Viewport<'_>, x: f64, y: f64, scale: f64) -> Option<Position> {
    let hit = block_at(view, x, y, scale)?;
    let offset = Cursor::from_point(&hit.block.layout, hit.x, hit.y)
        .index()
        .min(hit.block.source.len());
    Some(Position {
        message: hit.message,
        block: hit.block_index,
        offset,
    })
}

/// How much text one gesture grabs. Click places a caret, double click takes
/// the word, triple click takes the line: the granularity ladder every text
/// surface has, and the reason a user does not have to drag precisely to quote
/// a single word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Granularity {
    Character,
    Word,
    Line,
}

impl Granularity {
    /// The granularity for the nth consecutive click at one spot.
    ///
    /// Cycles past three so a fourth click starts over rather than sticking on
    /// whole lines, which is what a user leaning on the button expects.
    pub fn for_click(count: usize) -> Self {
        match count % 3 {
            2 => Self::Word,
            0 => Self::Line,
            _ => Self::Character,
        }
    }
}

/// Consecutive clicks at one spot, which is what decides whether a press is a
/// caret, a word, or a line.
///
/// Its own type rather than a tuple in the app, because "is this the same
/// click again" has three real rules (soon enough, close enough, and not
/// interrupted by a drag) and each one is a bug when it is missing: no time
/// limit and two unrelated clicks grab a word, no distance limit and a click
/// across the window continues the streak, no drag reset and releasing a drag
/// then clicking silently selects a word.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClickStreak {
    last: Option<(std::time::Instant, (f64, f64))>,
    count: usize,
}

/// How far the pointer may move between clicks and still count as the same
/// spot, in logical units. Zero would make a double click impossible on a
/// trackpad, where the finger always drifts a little.
const CLICK_SLOP: f64 = 3.0;

impl ClickStreak {
    /// Record a press and report the granularity it selects with.
    pub fn press(&mut self, x: f64, y: f64, window: std::time::Duration) -> Granularity {
        self.press_at(std::time::Instant::now(), x, y, window)
    }

    /// As [`Self::press`], with an explicit clock so the rules are testable
    /// without sleeping.
    pub fn press_at(
        &mut self,
        now: std::time::Instant,
        x: f64,
        y: f64,
        window: std::time::Duration,
    ) -> Granularity {
        let continues = self.last.is_some_and(|(at, (last_x, last_y))| {
            now.duration_since(at) < window
                && (last_x - x).abs() <= CLICK_SLOP
                && (last_y - y).abs() <= CLICK_SLOP
        });
        self.count = if continues { self.count + 1 } else { 1 };
        self.last = Some((now, (x, y)));
        Granularity::for_click(self.count)
    }

    /// End the streak. A drag is a gesture in its own right, so pressing again
    /// after one is a fresh click rather than the second half of a double.
    pub fn interrupt(&mut self) {
        self.last = None;
        self.count = 0;
    }
}

/// The selection a click of `granularity` makes at a point, or `None` when
/// there is nothing laid out there.
///
/// Word and line ranges come from Parley's own [`ParleySelection`] helpers, so
/// they agree with the cluster and line breaking the text was actually shaped
/// with rather than a hand-rolled scan for spaces, which is wrong for
/// punctuation, CJK, and anything that wraps.
pub fn selection_at(
    view: &Viewport<'_>,
    x: f64,
    y: f64,
    scale: f64,
    granularity: Granularity,
) -> Option<Selection> {
    let hit = block_at(view, x, y, scale)?;
    let range = match granularity {
        Granularity::Character => {
            let offset = Cursor::from_point(&hit.block.layout, hit.x, hit.y)
                .index()
                .min(hit.block.source.len());
            offset..offset
        }
        Granularity::Word => {
            ParleySelection::word_from_point(&hit.block.layout, hit.x, hit.y).text_range()
        }
        Granularity::Line => {
            ParleySelection::line_from_point(&hit.block.layout, hit.x, hit.y).text_range()
        }
    };
    let len = hit.block.source.len();
    let at = |offset: usize| Position {
        message: hit.message,
        block: hit.block_index,
        offset: offset.min(len),
    };
    Some(Selection::new(at(range.start), at(range.end)))
}

/// A resolved pointer hit: which block, and where in its layout, in the
/// physical units Parley's own hit-testing wants.
struct Hit<'a> {
    message: usize,
    block_index: usize,
    block: &'a LaidBlock,
    x: f32,
    y: f32,
}

/// Resolve a transcript point to a block and a point within its layout.
/// Shared by every gesture, so a click, a drag, and a double click can never
/// disagree about which glyph the pointer is over.
fn block_at<'a>(view: &'a Viewport<'_>, x: f64, y: f64, scale: f64) -> Option<Hit<'a>> {
    let placed = nearest_message(view, y)?;
    let local_y = y - placed.top - placed.message.top_padding();
    let block_index = nearest_block(&placed.message.blocks, local_y)?;
    let block = &placed.message.blocks[block_index];
    Some(Hit {
        message: placed.index,
        block_index,
        block,
        x: ((x - block.inset) * scale) as f32,
        y: ((local_y - block.top) * scale) as f32,
    })
}

/// The visible message containing `y`, or the closest one when `y` falls in a
/// gap or off the ends of the conversation.
fn nearest_message<'a, 'b>(
    view: &'a Viewport<'b>,
    y: f64,
) -> Option<&'a crate::viewport::Placed<'b>> {
    view.visible.iter().min_by(|a, b| {
        distance_to(a.top, a.message.height, y).total_cmp(&distance_to(b.top, b.message.height, y))
    })
}

/// The block containing `y` within a message, or the closest one.
fn nearest_block(blocks: &[LaidBlock], y: f64) -> Option<usize> {
    if blocks.is_empty() {
        return None;
    }
    (0..blocks.len()).min_by(|&a, &b| {
        distance_to(blocks[a].top, blocks[a].height + BLOCK_GAP, y).total_cmp(&distance_to(
            blocks[b].top,
            blocks[b].height + BLOCK_GAP,
            y,
        ))
    })
}

/// Vertical distance from `y` to the band `[top, top + height]`; zero inside.
fn distance_to(top: f64, height: f64, y: f64) -> f64 {
    if y < top {
        top - y
    } else if y > top + height {
        y - top - height
    } else {
        0.0
    }
}

/// Lay the model's transcript out through the shared cache. Hit-testing,
/// copying, and drawing all go through this one call, so a click and the pixels
/// it lands on can never come from different layouts.
fn laid_for<'a>(
    painter: &'a mut crate::paint::Painter,
    model: &crate::Model,
    frame: crate::layout::Frame,
) -> &'a [LaidMessage] {
    let style = crate::scene::transcript_body_style(model);
    let width = (frame.column() - crate::transcript::USER_PAD_X * 2.0).max(1.0);
    let crate::paint::Painter {
        text,
        transcript: cache,
    } = painter;
    cache.lay_out(
        text,
        &model.transcript,
        width,
        &model.theme,
        style,
        frame.scale,
    )
}

/// The transcript position under a window point, in logical units.
///
/// The conversion into transcript coordinates lives here rather than at the
/// call site: y is measured from the top of the region and x from the message's
/// text left edge, which is inset by the user card's padding so both roles
/// share one measure.
pub fn position_at_in_frame(
    painter: &mut crate::paint::Painter,
    model: &crate::Model,
    frame: crate::layout::Frame,
    x: f64,
    y: f64,
) -> Option<Position> {
    let region = (frame.body_bottom - frame.body_top).max(1.0);
    let scroll = model.view_scroll();
    let laid = laid_for(painter, model, frame);
    let view = Viewport::new(laid, region, scroll);
    position_at(
        &view,
        x - (frame.left + crate::transcript::USER_PAD_X),
        y - frame.body_top,
        frame.scale,
    )
}

/// The selection a click of `granularity` makes at a window point.
pub fn selection_at_in_frame(
    painter: &mut crate::paint::Painter,
    model: &crate::Model,
    frame: crate::layout::Frame,
    x: f64,
    y: f64,
    granularity: Granularity,
) -> Option<Selection> {
    let region = (frame.body_bottom - frame.body_top).max(1.0);
    let scroll = model.view_scroll();
    let laid = laid_for(painter, model, frame);
    let view = Viewport::new(laid, region, scroll);
    selection_at(
        &view,
        x - (frame.left + crate::transcript::USER_PAD_X),
        y - frame.body_top,
        frame.scale,
        granularity,
    )
}

/// The model's selected transcript text, read from the same cached layouts the
/// renderer drew, so copy returns exactly the characters that were highlighted.
pub fn selection_text_in_frame(
    painter: &mut crate::paint::Painter,
    model: &crate::Model,
    frame: crate::layout::Frame,
) -> Option<String> {
    let selection = model.selection.filter(|selection| !selection.is_empty())?;
    let copied = selected_text(laid_for(painter, model, frame), &selection);
    (!copied.is_empty()).then_some(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{ParagraphStyle, TextSystem};
    use crate::theme::Theme;
    use crate::transcript::{Message, lay_out_message};

    const SCALE: f64 = 1.75;
    const WIDTH: f64 = 600.0;

    fn base() -> ParagraphStyle {
        ParagraphStyle {
            font_size: crate::layout::BODY_SIZE,
            line_height: crate::layout::BODY_LEADING as f32,
            ..Default::default()
        }
    }

    fn lay(sources: &[Message]) -> Vec<LaidMessage> {
        let mut text = TextSystem::default();
        sources
            .iter()
            .map(|message| {
                lay_out_message(
                    &mut text,
                    message,
                    WIDTH,
                    &Theme::print_light(),
                    base(),
                    SCALE,
                )
            })
            .collect()
    }

    fn at(message: usize, block: usize, offset: usize) -> Position {
        Position {
            message,
            block,
            offset,
        }
    }

    /// Reading order is the whole ordering model: a later message is after an
    /// earlier one whatever the byte offsets say.
    #[test]
    fn positions_order_by_message_then_block_then_offset() {
        assert!(at(0, 0, 9) < at(1, 0, 0));
        assert!(at(0, 0, 9) < at(0, 1, 0));
        assert!(at(0, 0, 1) < at(0, 0, 2));
    }

    /// A backwards drag selects the same characters as a forwards one, so the
    /// direction the user happened to move the mouse never changes the result.
    #[test]
    fn a_backwards_drag_selects_the_same_range() {
        let forward = Selection::new(at(0, 0, 2), at(1, 0, 4));
        let backward = Selection::new(at(1, 0, 4), at(0, 0, 2));
        assert_eq!(forward.range(), backward.range());
    }

    /// The intersection rule: blocks strictly inside the selection are taken
    /// whole, the end blocks are cut at their offsets, and blocks outside are
    /// not touched at all.
    #[test]
    fn a_selection_spans_whole_middle_blocks_and_cuts_the_ends() {
        let selection = Selection::new(at(0, 1, 3), at(2, 0, 4));
        assert_eq!(selection.range_in(0, 0, 10), None, "block before the start");
        assert_eq!(selection.range_in(0, 1, 10), Some((3, 10)), "start block");
        assert_eq!(selection.range_in(1, 0, 10), Some((0, 10)), "middle block");
        assert_eq!(selection.range_in(2, 0, 10), Some((0, 4)), "end block");
        assert_eq!(selection.range_in(3, 0, 10), None, "block after the end");
    }

    /// An empty selection selects nothing anywhere: a plain click must not
    /// highlight a band or overwrite the clipboard.
    #[test]
    fn an_empty_selection_covers_nothing() {
        let selection = Selection::new(at(0, 0, 4), at(0, 0, 4));
        assert!(selection.is_empty());
        assert_eq!(selection.range_in(0, 0, 10), None);
        assert_eq!(
            selected_text(&lay(&[Message::assistant("hello")]), &selection),
            ""
        );
    }

    /// Copy yields the *rendered* text. This is the reason `LaidBlock` carries
    /// its source: copying the markdown would paste `**bold**` for text that
    /// was drawn as bold.
    #[test]
    fn copying_yields_rendered_text_not_markdown() {
        let laid = lay(&[Message::assistant("**bold** and `code`")]);
        let len = laid[0].blocks[0].source.len();
        let text = selected_text(&laid, &Selection::new(at(0, 0, 0), at(0, 0, len)));
        assert_eq!(
            text, "bold and code",
            "markdown punctuation leaked into copy"
        );
    }

    /// A selection across messages copies them in reading order, separated the
    /// way they are drawn, rather than concatenated into one run-on line.
    #[test]
    fn copying_across_messages_keeps_them_separate() {
        let laid = lay(&[Message::user("question"), Message::assistant("answer")]);
        let end = laid[1].blocks[0].source.len();
        let text = selected_text(&laid, &Selection::new(at(0, 0, 0), at(1, 0, end)));
        assert_eq!(text, "question\n\nanswer");
    }

    /// A partial selection copies only what it covers.
    #[test]
    fn copying_a_partial_range_copies_only_that_range() {
        let laid = lay(&[Message::assistant("alpha bravo charlie")]);
        let text = selected_text(&laid, &Selection::new(at(0, 0, 6), at(0, 0, 11)));
        assert_eq!(text, "bravo");
    }

    /// Offsets that land mid-character must not panic. A transcript renders
    /// whatever a model emits, so multi-byte text is a real input.
    #[test]
    fn multibyte_text_never_panics_on_a_slice() {
        let laid = lay(&[Message::assistant("ünïcödé 中文 🎉 text")]);
        let len = laid[0].blocks[0].source.len();
        for start in 0..=len {
            for end in start..=len {
                let _ = selected_text(&laid, &Selection::new(at(0, 0, start), at(0, 0, end)));
            }
        }
    }

    /// Bands are real geometry: selecting more text must cover more of the
    /// line, and a selection must never produce zero bands.
    #[test]
    fn bands_grow_with_the_selected_range() {
        let laid = lay(&[Message::assistant("alpha bravo charlie delta")]);
        let block = &laid[0].blocks[0];
        let width = |range| {
            block_bands(block, range, SCALE)
                .iter()
                .map(|band| band.rect.width())
                .sum::<f64>()
        };
        let short = width((0, 5));
        let long = width((0, 20));
        assert!(short > 0.0, "a selection drew no band");
        assert!(long > short, "a longer selection drew a narrower band");
    }

    /// An empty range draws nothing, so a collapsed selection cannot leave a
    /// sliver of highlight behind.
    #[test]
    fn an_empty_range_draws_no_band() {
        let laid = lay(&[Message::assistant("text")]);
        assert!(block_bands(&laid[0].blocks[0], (3, 3), SCALE).is_empty());
    }
    /// The click ladder: caret, word, line, then back to a caret so someone
    /// leaning on the button is not stuck selecting whole lines.
    #[test]
    fn the_click_ladder_cycles_caret_word_line() {
        let expected = [
            Granularity::Character,
            Granularity::Word,
            Granularity::Line,
            Granularity::Character,
            Granularity::Word,
            Granularity::Line,
        ];
        for (index, want) in expected.iter().enumerate() {
            assert_eq!(
                Granularity::for_click(index + 1),
                *want,
                "click {}",
                index + 1
            );
        }
    }

    /// Each of these rules is a bug when it is missing, so each is asserted
    /// separately rather than through one happy-path double click.
    #[test]
    fn a_streak_continues_only_when_it_is_soon_enough_and_close_enough() {
        let window = std::time::Duration::from_millis(400);
        let start = std::time::Instant::now();

        // Same spot, straight away: a double click.
        let mut streak = ClickStreak::default();
        assert_eq!(
            streak.press_at(start, 10.0, 10.0, window),
            Granularity::Character
        );
        assert_eq!(
            streak.press_at(start, 10.0, 10.0, window),
            Granularity::Word,
            "a second click at the same spot was not a double click"
        );

        // Too slow: a fresh click.
        let mut streak = ClickStreak::default();
        streak.press_at(start, 10.0, 10.0, window);
        assert_eq!(
            streak.press_at(start + window * 2, 10.0, 10.0, window),
            Granularity::Character,
            "a slow second click was treated as a double click"
        );

        // Too far: a fresh click.
        let mut streak = ClickStreak::default();
        streak.press_at(start, 10.0, 10.0, window);
        assert_eq!(
            streak.press_at(start, 400.0, 10.0, window),
            Granularity::Character,
            "clicks at two different spots were treated as a double click"
        );

        // Interrupted by a drag: a fresh click.
        let mut streak = ClickStreak::default();
        streak.press_at(start, 10.0, 10.0, window);
        streak.interrupt();
        assert_eq!(
            streak.press_at(start, 10.0, 10.0, window),
            Granularity::Character,
            "a click after a drag continued the streak"
        );
    }

    /// A trackpad finger always drifts, so a tiny move must not break a double
    /// click. Without this, double click is unreliable on exactly the input
    /// device most users have.
    #[test]
    fn a_small_drift_between_clicks_still_counts_as_a_double() {
        let window = std::time::Duration::from_millis(400);
        let start = std::time::Instant::now();
        let mut streak = ClickStreak::default();
        streak.press_at(start, 10.0, 10.0, window);
        assert_eq!(
            streak.press_at(start, 10.0 + CLICK_SLOP / 2.0, 10.0, window),
            Granularity::Word,
            "a trackpad's drift broke the double click"
        );
    }
}
