//! Per-frame painting resources: fonts, layout contexts, and the transcript
//! layout cache.
//!
//! The cache is the reason this module exists. Laying a message out means
//! parsing its markdown and running Parley over every block, which is the most
//! expensive thing the app does. The transcript is also almost entirely
//! *unchanged* between frames: a caret blink, a donut tick, a wheel event, or
//! one streamed delta re-lays the whole conversation only because nothing
//! remembers the previous answer. On a long session that is tens of
//! milliseconds of work per frame for a blinking cursor, which is exactly what
//! made the window feel laggy.
//!
//! [`TranscriptCache`] keeps one [`LaidMessage`] per message, keyed by the
//! inputs its layout actually depends on (source text, wrap width, scale,
//! theme, base style). A frame re-lays only the messages whose key changed, so
//! streaming costs one message and everything else costs a lookup.

use crate::text::{ParagraphStyle, TextSystem};
use crate::theme::Theme;
use crate::transcript::{LaidMessage, Role, Transcript, lay_out_message_reusing};

/// Everything a frame needs to turn a model into a scene: the font and layout
/// contexts, and the transcript layout cache. Bundled so the cache travels with
/// the text system that built its layouts and cannot be accidentally dropped
/// (which would silently restore the per-frame relayout this module removes).
#[derive(Default)]
pub struct Painter {
    pub text: TextSystem,
    pub transcript: TranscriptCache,
}

/// The layout inputs a cached message must still match to be reusable.
/// Anything that changes wrapping, metrics, or colour belongs here; anything
/// else (scroll, selection, caret) must *not*, or the cache would miss every
/// frame for a change that does not affect layout.
#[derive(Clone, Copy, PartialEq)]
struct Geometry {
    width: f64,
    scale: f64,
    theme: Theme,
    font_size: f32,
    line_height: f32,
}

/// Laid-out messages, reused across frames.
#[derive(Default)]
pub struct TranscriptCache {
    geometry: Option<Geometry>,
    /// Identity of each cached message: what it must still be to be reused.
    keys: Vec<(Role, String)>,
    /// The layouts themselves, index-aligned with `keys`. Kept in their own
    /// vector so callers can borrow them as one contiguous slice.
    laid: Vec<LaidMessage>,
    hits: usize,
    misses: usize,
    /// Misses accumulated over the cache's whole life. The per-call `misses`
    /// resets every call, and a real frame calls `lay_out` several times (the
    /// frame measurement, the glide observation, the scene build), so "how
    /// much layout work did this *frame* do" needs a counter that only ever
    /// goes up: the bench reads it before and after and subtracts.
    total_misses: usize,
}

impl TranscriptCache {
    /// Lay out `transcript`, reusing every message whose source and geometry
    /// are unchanged since the last call. Blank messages are skipped, matching
    /// what the viewport draws.
    pub fn lay_out(
        &mut self,
        text: &mut TextSystem,
        transcript: &Transcript,
        width: f64,
        theme: &Theme,
        base: ParagraphStyle,
        scale: f64,
    ) -> &[LaidMessage] {
        let geometry = Geometry {
            width,
            scale,
            theme: *theme,
            font_size: base.font_size,
            line_height: base.line_height,
        };
        // A geometry change (resize, DPI move, theme switch) invalidates every
        // measurement at once, so there is nothing to salvage.
        if self.geometry != Some(geometry) {
            self.geometry = Some(geometry);
            self.keys.clear();
            self.laid.clear();
        }
        self.hits = 0;
        self.misses = 0;

        let messages: Vec<_> = transcript
            .messages()
            .iter()
            .filter(|message| !message.source.trim().is_empty())
            .collect();

        // Truncate first: a cleared or rewound transcript must not leave stale
        // tail entries behind.
        self.keys.truncate(messages.len());
        self.laid.truncate(messages.len());

        for (index, message) in messages.iter().enumerate() {
            let reusable = self
                .keys
                .get(index)
                .is_some_and(|(role, source)| *role == message.role && source == &message.source);
            if reusable {
                // Delivery is drawn, not laid out: an ack moves a dot and
                // offsets the card, and neither changes where the text wraps.
                // Refreshing it on a hit is what lets the acknowledgement land
                // without re-laying the message that was acknowledged.
                if let Some(entry) = self.laid.get_mut(index) {
                    entry.delivery = message.delivery;
                }
                self.hits += 1;
                continue;
            }
            self.misses += 1;
            self.total_misses += 1;
            // A changed message is re-laid *reusing* its own previous blocks:
            // a streamed delta only appends, so everything before the changed
            // tail block is byte-identical and keeps its layout. This is what
            // stops a delta's cost growing with the reply's length. The
            // matcher inside compares text, kind, and inset, so a genuinely
            // different message reuses nothing; geometry changes cleared the
            // whole cache above, so stale widths can never leak through here.
            let previous = self
                .laid
                .get_mut(index)
                .map(|entry| std::mem::take(&mut entry.blocks))
                .unwrap_or_default();
            let (laid, _) =
                lay_out_message_reusing(text, message, previous, width, theme, base, scale);
            let key = (message.role, message.source.clone());
            match (self.keys.get_mut(index), self.laid.get_mut(index)) {
                (Some(slot), Some(entry)) => {
                    *slot = key;
                    *entry = laid;
                }
                _ => {
                    self.keys.push(key);
                    self.laid.push(laid);
                }
            }
        }

        &self.laid
    }

    /// Reused and re-laid message counts from the last call. Exposed so the
    /// tests and the state-space profiler can assert the cache actually
    /// caches: a cache whose hit rate is never checked silently degrades into
    /// no cache at all.
    ///
    /// This is the *deterministic* half of the perf signal. Wall-clock frame
    /// cost varies with the machine and the scheduler, so a timing gate has to
    /// leave slack and can only catch a large regression. "How many messages
    /// did this frame lay out" is exact, reproducible, and catches the same
    /// regression the moment it appears.
    pub fn stats(&self) -> (usize, usize) {
        (self.hits, self.misses)
    }

    /// Messages laid out since the cache was created. Monotonic, so a caller
    /// can meter the layout work of any span of calls (a frame, a whole
    /// streaming replay) by differencing two readings.
    pub fn total_relayouts(&self) -> usize {
        self.total_misses
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Message;

    fn base() -> ParagraphStyle {
        ParagraphStyle {
            font_size: crate::layout::BODY_SIZE,
            line_height: crate::layout::BODY_LEADING as f32,
            ..Default::default()
        }
    }

    fn lay(
        cache: &mut TranscriptCache,
        text: &mut TextSystem,
        transcript: &Transcript,
        width: f64,
    ) -> usize {
        cache
            .lay_out(text, transcript, width, &Theme::print_light(), base(), 1.75)
            .len()
    }

    fn conversation(turns: usize) -> Transcript {
        let mut transcript = Transcript::default();
        for n in 0..turns {
            transcript.push(Message::user(format!("question {n}")));
            transcript.push(Message::assistant(format!("answer {n} with some prose")));
        }
        transcript
    }

    /// The point of the cache: an unchanged transcript costs no layout work.
    /// This is the frame a blinking caret or a donut tick asks for, which used
    /// to re-lay the whole conversation.
    #[test]
    fn an_unchanged_transcript_is_laid_out_once() {
        let mut cache = TranscriptCache::default();
        let mut text = TextSystem::default();
        let transcript = conversation(10);
        lay(&mut cache, &mut text, &transcript, 600.0);
        assert_eq!(cache.stats(), (0, 20), "first frame should lay everything");
        for _ in 0..5 {
            lay(&mut cache, &mut text, &transcript, 600.0);
            assert_eq!(cache.stats(), (20, 0), "a repeat frame re-laid messages");
        }
    }

    /// Streaming must cost one message, not the whole history: the tail grows
    /// on every delta and everything before it is untouched.
    #[test]
    fn streaming_relays_only_the_tail() {
        let mut cache = TranscriptCache::default();
        let mut text = TextSystem::default();
        let mut transcript = conversation(10);
        lay(&mut cache, &mut text, &transcript, 600.0);
        for _ in 0..5 {
            transcript.append_assistant(" more");
            lay(&mut cache, &mut text, &transcript, 600.0);
            assert_eq!(cache.stats(), (19, 1), "a delta re-laid more than the tail");
        }
    }

    /// Geometry is part of the key: a resize must not reuse layouts wrapped to
    /// the old width, or the text would wrap off the new one.
    #[test]
    fn a_resize_invalidates_every_layout() {
        let mut cache = TranscriptCache::default();
        let mut text = TextSystem::default();
        let transcript = conversation(4);
        lay(&mut cache, &mut text, &transcript, 600.0);
        lay(&mut cache, &mut text, &transcript, 420.0);
        assert_eq!(cache.stats(), (0, 8), "a resize reused stale layouts");
    }

    /// A shorter transcript (a cleared session, or `/new`) must not leave the
    /// old tail cached and drawable.
    #[test]
    fn a_shrinking_transcript_drops_its_tail() {
        let mut cache = TranscriptCache::default();
        let mut text = TextSystem::default();
        assert_eq!(lay(&mut cache, &mut text, &conversation(6), 600.0), 12);
        assert_eq!(lay(&mut cache, &mut text, &conversation(2), 600.0), 4);
        assert_eq!(lay(&mut cache, &mut text, &Transcript::default(), 600.0), 0);
    }

    /// Blank messages are filtered, and the filter must agree with the keys:
    /// an off-by-one here would draw one message's text with another's layout.
    #[test]
    fn blank_messages_do_not_shift_the_cache() {
        let mut cache = TranscriptCache::default();
        let mut text = TextSystem::default();
        let mut transcript = Transcript::default();
        transcript.push(Message::user("first"));
        transcript.push(Message::assistant("   "));
        transcript.push(Message::user("second"));
        assert_eq!(lay(&mut cache, &mut text, &transcript, 600.0), 2);
        assert_eq!(lay(&mut cache, &mut text, &transcript, 600.0), 2);
        assert_eq!(cache.stats(), (2, 0));
    }

    /// The cache has to be worth its complexity: a repeat frame over a long
    /// conversation must be orders of magnitude cheaper than laying it out.
    /// This is the lag fix stated as a measurement rather than an impression.
    #[test]
    fn a_repeat_frame_is_far_cheaper_than_the_first() {
        let mut cache = TranscriptCache::default();
        let mut text = TextSystem::default();
        let transcript = conversation(60);

        let start = std::time::Instant::now();
        lay(&mut cache, &mut text, &transcript, 600.0);
        let cold = start.elapsed();

        let start = std::time::Instant::now();
        for _ in 0..10 {
            lay(&mut cache, &mut text, &transcript, 600.0);
        }
        let warm = start.elapsed() / 10;

        eprintln!("60-turn transcript: cold {cold:?}, warm {warm:?}");
        assert!(
            warm * 20 < cold,
            "cache saved almost nothing: cold {cold:?} vs warm {warm:?}"
        );
    }
}
