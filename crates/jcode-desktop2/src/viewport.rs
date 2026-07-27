//! Transcript viewport: which pixels of the conversation are on screen.
//!
//! Separated from [`crate::transcript`] because the two answer different
//! questions. That module answers "how tall is this message"; this one answers
//! "given a region this tall and a scroll position, what is visible and where
//! does it sit". Keeping them apart is what makes the geometry testable
//! without a GPU: a viewport is arithmetic over measured heights.
//!
//! The invariant this module exists to enforce: **nothing is ever placed
//! outside the region**. The old renderer bottom-aligned a paragraph whose
//! height it had under-measured, so tall content drew over the composer. Here
//! the region is the input, placements are derived from it, and the renderer
//! additionally clips to it, so an arithmetic mistake degrades to cropped text
//! rather than text painted across the input box.

use crate::transcript::{LaidMessage, MESSAGE_GAP};

/// One message placed in the region's coordinate space.
pub struct Placed<'a> {
    /// Index of this message within the laid slice. Carried so a pointer hit
    /// can name a position that outlives the frame: which messages are visible
    /// changes with every scroll, but the index does not.
    pub index: usize,
    pub message: &'a LaidMessage,
    /// Top of the message in logical units, in the region's own coordinates
    /// (that is, relative to `region_top`). May be negative when a tall
    /// message is partly scrolled off the top.
    pub top: f64,
}

/// The laid-out, positioned transcript for one frame.
pub struct Viewport<'a> {
    /// Messages that intersect the region, in order.
    pub visible: Vec<Placed<'a>>,
    /// Total height of the whole conversation in logical units.
    pub content_height: f64,
    /// Height of the region the transcript may occupy.
    pub region_height: f64,
    /// Top of the first message in region coordinates, whether or not it is
    /// visible. This is the conversation's absolute position on screen, and
    /// the only stable thing to reason about while scrolling: which message
    /// happens to be last on screen changes as the view moves.
    #[cfg_attr(not(test), allow(dead_code))]
    pub content_top: f64,
}

impl<'a> Viewport<'a> {
    /// Place already-laid messages in a region of `region_height`, scrolled up
    /// by `scroll` logical pixels from the tail.
    ///
    /// Layout is *not* done here. Measuring a message is the most expensive
    /// thing the app does and the transcript barely changes between frames, so
    /// the caller owns a [`crate::paint::TranscriptCache`] and this module is
    /// left as pure arithmetic over measured heights: testable without a GPU,
    /// and free to run on every frame.
    ///
    /// Scrolling is in pixels, not lines, because the screen is in pixels: a
    /// line-based scroll disagrees with the display the moment anything wraps,
    /// which is most of the time.
    pub fn new(laid: &'a [LaidMessage], region_height: f64, scroll: f64) -> Self {
        let content_height = total_height(laid);
        let scroll = clamp_scroll(scroll, content_height, region_height);

        // Bottom-align: the newest message sits against the bottom of the
        // region, so a reply streams in just above the composer like a
        // terminal, and a short conversation does not float mid-page.
        let bottom = region_height + scroll;
        let mut top = bottom - content_height;
        let content_top = top;

        let mut visible = Vec::new();
        for (index, message) in laid.iter().enumerate() {
            let height = message.height;
            if top + height > 0.0 && top < region_height {
                visible.push(Placed {
                    index,
                    message,
                    top,
                });
            }
            top += height + MESSAGE_GAP;
        }

        Self {
            visible,
            content_height,
            region_height,
            content_top,
        }
    }

    /// Largest useful scroll offset in logical pixels: scrolling further would
    /// only reveal blank space above the conversation.
    pub fn max_scroll(&self) -> f64 {
        (self.content_height - self.region_height).max(0.0)
    }
}

/// Total height of a laid-out conversation, gaps included.
fn total_height(messages: &[LaidMessage]) -> f64 {
    if messages.is_empty() {
        return 0.0;
    }
    messages.iter().map(|m| m.height).sum::<f64>() + MESSAGE_GAP * (messages.len() - 1) as f64
}

/// Clamp a scroll offset to the range that shows content.
pub fn clamp_scroll(scroll: f64, content_height: f64, region_height: f64) -> f64 {
    let max = (content_height - region_height).max(0.0);
    scroll.clamp(0.0, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::TranscriptCache;
    use crate::text::{ParagraphStyle, TextSystem};
    use crate::theme::Theme;
    use crate::transcript::{Message, Transcript};

    const WIDTH: f64 = 600.0;
    const REGION: f64 = 240.0;

    fn base() -> ParagraphStyle {
        ParagraphStyle {
            font_size: crate::layout::BODY_SIZE,
            line_height: crate::layout::BODY_LEADING as f32,
            ..Default::default()
        }
    }

    fn long_conversation(turns: usize) -> Transcript {
        let mut transcript = Transcript::default();
        for n in 0..turns {
            transcript.push(Message::user(format!("question {n}")));
            transcript.push(Message::assistant(format!(
                "answer {n}. {}",
                "some prose that will wrap at this width ".repeat(3)
            )));
        }
        transcript
    }

    /// Run `check` against a viewport over `transcript` at `scroll`, keeping
    /// the cache alive for the borrow.
    fn with_viewport(transcript: &Transcript, scroll: f64, check: impl FnOnce(&Viewport<'_>)) {
        with_viewport_sized(transcript, scroll, REGION, check);
    }

    fn with_viewport_sized(
        transcript: &Transcript,
        scroll: f64,
        region: f64,
        check: impl FnOnce(&Viewport<'_>),
    ) {
        let mut cache = TranscriptCache::default();
        let mut text = TextSystem::default();
        let laid = cache.lay_out(
            &mut text,
            transcript,
            WIDTH,
            &Theme::print_light(),
            base(),
            1.75,
        );
        check(&Viewport::new(laid, region, scroll));
    }

    /// The invariant the old renderer broke: no placed message may start below
    /// the region, and the visible set must never extend past its bottom.
    /// This is what stopped the transcript painting over the composer.
    #[test]
    fn nothing_is_placed_outside_the_region() {
        for turns in [0, 1, 2, 10, 40] {
            for scroll in [0.0, 50.0, 400.0, 10_000.0] {
                with_viewport(&long_conversation(turns), scroll, |view| {
                    for placed in &view.visible {
                        assert!(
                            placed.top < view.region_height,
                            "{turns} turns at scroll {scroll}: message placed below the region"
                        );
                        assert!(
                            placed.top + placed.message.height > 0.0,
                            "{turns} turns at scroll {scroll}: message placed above the region"
                        );
                    }
                });
            }
        }
    }

    /// A short conversation sits against the bottom of the region, so replies
    /// appear just above the composer rather than floating mid-page.
    #[test]
    fn a_short_conversation_is_bottom_aligned() {
        let mut transcript = Transcript::default();
        transcript.push(Message::assistant("short"));
        with_viewport(&transcript, 0.0, |view| {
            let placed = view.visible.first().expect("one visible message");
            let bottom = placed.top + placed.message.height;
            assert!(
                (bottom - view.region_height).abs() < 0.5,
                "not bottom-aligned: bottom {bottom:.1} vs region {:.1}",
                view.region_height
            );
        });
    }

    /// Scrolling is clamped at both ends: past the tail is the tail, and past
    /// the head is the head. Without this the view can scroll into blank space
    /// and look like the conversation was lost.
    #[test]
    fn scrolling_clamps_at_both_ends() {
        let transcript = long_conversation(20);
        let mut tail_len = 0;
        let mut max = 0.0;
        with_viewport(&transcript, 0.0, |view| {
            tail_len = view.visible.len();
            max = view.max_scroll();
        });
        with_viewport(&transcript, -500.0, |view| {
            assert_eq!(
                tail_len,
                view.visible.len(),
                "negative scroll moved the view"
            );
        });
        let mut head_len = 0;
        with_viewport(&transcript, max, |view| head_len = view.visible.len());
        with_viewport(&transcript, max + 5_000.0, |view| {
            assert_eq!(
                head_len,
                view.visible.len(),
                "scrolling past the head kept moving"
            );
        });
    }

    /// Scrolling up moves content down the screen, monotonically. If this
    /// drifts, the wheel and the display disagree.
    #[test]
    fn scrolling_up_moves_content_down_monotonically() {
        let transcript = long_conversation(20);
        let mut previous = f64::NEG_INFINITY;
        let mut max = 0.0;
        with_viewport(&transcript, 0.0, |view| max = view.max_scroll());
        assert!(max > 0.0, "test conversation does not overflow the region");
        for step in 0..10 {
            let scroll = max * f64::from(step) / 10.0;
            with_viewport(&transcript, scroll, |view| {
                assert!(
                    view.content_top >= previous - 0.5,
                    "content moved up while scrolling up at {scroll:.1}"
                );
                previous = view.content_top;
            });
        }
    }

    /// Only what fits is placed for drawing: a huge conversation must not
    /// place thousands of messages, or every frame pays for history nobody can
    /// see.
    #[test]
    fn a_huge_conversation_only_places_what_fits() {
        with_viewport(&long_conversation(400), 0.0, |view| {
            assert!(
                view.visible.len() < 20,
                "{} messages placed for a {REGION}pt region",
                view.visible.len()
            );
            assert!(!view.visible.is_empty(), "nothing was placed");
        });
    }

    /// A degenerate region must not panic or place anything absurd.
    #[test]
    fn a_degenerate_region_is_safe() {
        for region in [0.0, 1.0, 5.0] {
            with_viewport_sized(&long_conversation(5), 0.0, region, |view| {
                assert!(view.max_scroll() >= 0.0);
            });
        }
    }

    /// An empty transcript places nothing, leaving the hero its space.
    #[test]
    fn an_empty_transcript_places_nothing() {
        with_viewport(&Transcript::default(), 0.0, |view| {
            assert!(view.visible.is_empty());
            assert_eq!(view.content_height, 0.0);
        });
    }
}
