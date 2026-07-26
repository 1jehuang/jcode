//! Streaming reveal animation for transcript text.
//!
//! Model tokens arrive in bursts of wildly varying size: a chunk can be one
//! character or a whole paragraph. Painting each chunk instantly makes the
//! transcript flicker and jump, so every byte instead fades and rises into
//! place on its own short schedule, staggered across the chunk it arrived in.
//! The result reads like typing rather than like blocks of text appearing.
//!
//! The animation is a pure function of (arrival marks, now), so a frame can be
//! rendered deterministically by freezing the clock ([`Reveal::frozen`]) and
//! nothing needs a timer thread.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long one byte takes to fade in.
pub const FADE: Duration = Duration::from_millis(240);
/// Extra delay per byte within one arriving chunk, so a burst types itself in
/// instead of flashing all at once.
pub const STAGGER_PER_BYTE: Duration = Duration::from_millis(7);
/// Cap on the staggered delay inside a chunk. Without this, one large chunk
/// would take minutes to finish revealing.
pub const MAX_STAGGER: Duration = Duration::from_millis(360);
/// How far the reveal queue may fall behind real arrivals before it compresses.
pub const MAX_LAG: Duration = Duration::from_millis(500);
/// Distance (logical px) that freshly revealed text rises as it fades in.
pub const RISE: f64 = 5.0;
/// Frame budget while an animation is running.
pub const FRAME: Duration = Duration::from_millis(16);

/// One arrival of streamed text: the byte range it added, when its first byte
/// starts fading, and how long the sweep across the range takes.
#[derive(Clone, Copy, Debug)]
struct Chunk {
    start: usize,
    end: usize,
    begin: Instant,
    span: Duration,
}

impl Chunk {
    fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// When `byte` starts fading in.
    fn start_of(&self, byte: usize) -> Instant {
        let last = self.len().saturating_sub(1);
        if last == 0 {
            return self.begin;
        }
        let index = byte.saturating_sub(self.start).min(last);
        self.begin + self.span.mul_f64(index as f64 / last as f64)
    }

    /// When every byte in the chunk has finished fading in.
    fn done_at(&self) -> Instant {
        self.begin + self.span + FADE
    }
}

/// Reveal state for a transcript.
///
/// Modelled as a typing queue: bytes reveal strictly in order at roughly
/// [`STAGGER_PER_BYTE`], and a chunk that arrives while the previous one is
/// still revealing queues behind it rather than overtaking it. Without that
/// ordering, a large chunk followed by a small one would show the small one
/// first, and the transcript would appear to fill in out of order. The queue is
/// only allowed to fall [`MAX_LAG`] behind real arrivals, after which it
/// compresses, so the animation always stays close to live output.
///
/// Only the still-animating tail is retained, so memory stays bounded no matter
/// how long a session runs.
#[derive(Clone, Debug, Default)]
pub struct Reveal {
    chunks: VecDeque<Chunk>,
    /// Total transcript length in bytes, i.e. where the next chunk starts.
    len: usize,
    /// When the next byte to reveal may begin fading.
    cursor: Option<Instant>,
    /// Pinned clock for deterministic rendering (state-space nodes, tests).
    frozen: Option<Instant>,
}

impl Reveal {
    /// A reveal whose clock is pinned, so frames are reproducible.
    pub fn frozen(at: Instant) -> Self {
        Self {
            frozen: Some(at),
            ..Self::default()
        }
    }

    /// The clock this reveal animates against.
    pub fn now(&self) -> Instant {
        self.frozen.unwrap_or_else(Instant::now)
    }

    /// Record that `added` bytes were appended to the transcript at `at`.
    pub fn push_at(&mut self, added: usize, at: Instant) {
        if added == 0 {
            return;
        }
        // Queue behind whatever is still revealing, but never lag further than
        // MAX_LAG behind the real arrival.
        let begin = self
            .cursor
            .map(|cursor| cursor.max(at).min(at + MAX_LAG))
            .unwrap_or(at);
        let natural = STAGGER_PER_BYTE * (added.saturating_sub(1)) as u32;
        // Compress the sweep when the queue is already behind, so a backlog
        // catches up instead of compounding.
        let backlog = begin.saturating_duration_since(at);
        let budget = MAX_STAGGER.saturating_sub(backlog);
        let span = natural.min(budget);
        let chunk = Chunk {
            start: self.len,
            end: self.len + added,
            begin,
            span,
        };
        self.len = chunk.end;
        self.cursor = Some(begin + span);
        self.chunks.push_back(chunk);
        // Drop chunks that have finished: their bytes are simply opaque.
        while self
            .chunks
            .front()
            .is_some_and(|front| front.done_at() <= at)
        {
            self.chunks.pop_front();
        }
    }

    /// Record appended text using the wall clock.
    pub fn push(&mut self, added: usize) {
        self.push_at(added, self.now());
    }

    /// Pin the clock to `at`, making subsequent queries deterministic. Used by
    /// state-space nodes to capture a frame mid-animation.
    pub fn freeze_at(&mut self, at: Instant) {
        self.frozen = Some(at);
    }

    /// Forget all history, e.g. when the transcript is replaced.
    pub fn reset(&mut self) {
        self.chunks.clear();
        self.len = 0;
        self.cursor = None;
    }

    /// Opacity of the byte at `offset` at time `now`, in 0.0..=1.0.
    /// Bytes older than the animating window, and bytes past the end, are
    /// fully opaque so callers never have to special-case them.
    pub fn alpha_at(&self, offset: usize, now: Instant) -> f32 {
        let Some(chunk) = self
            .chunks
            .iter()
            .find(|chunk| offset >= chunk.start && offset < chunk.end)
        else {
            return 1.0;
        };
        let start = chunk.start_of(offset);
        if now <= start {
            return 0.0;
        }
        let t = (now.duration_since(start).as_secs_f32() / FADE.as_secs_f32()).clamp(0.0, 1.0);
        // Ease out cubic: quick to appear, gentle to settle.
        1.0 - (1.0 - t).powi(3)
    }

    /// Opacity at the reveal's own clock.
    pub fn alpha(&self, offset: usize) -> f32 {
        self.alpha_at(offset, self.now())
    }

    /// Vertical offset (logical px, positive = below its final position) of
    /// the byte at `offset`, so fresh text rises into place.
    pub fn rise_at(&self, offset: usize, now: Instant) -> f64 {
        f64::from(1.0 - self.alpha_at(offset, now)) * RISE
    }

    /// Whether anything is still animating at `now`.
    pub fn animating_at(&self, now: Instant) -> bool {
        self.chunks.iter().any(|chunk| chunk.done_at() > now)
    }

    pub fn animating(&self) -> bool {
        self.frozen.is_none() && self.animating_at(self.now())
    }

    /// When the next animation frame should be drawn, if any.
    pub fn next_frame_at(&self, now: Instant) -> Option<Instant> {
        (self.frozen.is_none() && self.animating_at(now)).then(|| now + FRAME)
    }

    /// Number of retained arrival marks. Only used to prove pruning works.
    #[cfg(test)]
    fn tracked(&self) -> usize {
        self.chunks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reveal_with(chunks: &[usize], start: Instant) -> Reveal {
        let mut reveal = Reveal::frozen(start);
        for (index, &len) in chunks.iter().enumerate() {
            reveal.push_at(len, start + FADE.mul_f32(index as f32 * 0.25));
        }
        reveal
    }

    #[test]
    fn fresh_text_fades_in_and_then_stays_opaque() {
        let start = Instant::now();
        let mut reveal = Reveal::frozen(start);
        reveal.push_at(4, start);
        assert_eq!(reveal.alpha_at(0, start), 0.0, "byte was visible instantly");
        let mid = reveal.alpha_at(0, start + FADE / 2);
        assert!(
            (0.1..0.99).contains(&mid),
            "midpoint alpha was not partial: {mid}"
        );
        assert_eq!(reveal.alpha_at(0, start + FADE), 1.0);
        assert_eq!(reveal.alpha_at(0, start + FADE * 4), 1.0);
    }

    #[test]
    fn alpha_never_decreases_over_time() {
        let start = Instant::now();
        let reveal = reveal_with(&[12, 30, 5], start);
        for offset in [0, 5, 11, 12, 30, 41, 46] {
            let mut previous = -1.0;
            for step in 0..40 {
                let alpha = reveal.alpha_at(offset, start + FADE.mul_f32(step as f32 * 0.1));
                assert!(
                    alpha + 1e-6 >= previous,
                    "alpha went backwards at offset {offset}: {previous} -> {alpha}"
                );
                previous = alpha;
            }
            assert_eq!(previous, 1.0, "offset {offset} never became opaque");
        }
    }

    #[test]
    fn earlier_bytes_in_a_chunk_lead_later_ones() {
        let start = Instant::now();
        let mut reveal = Reveal::frozen(start);
        reveal.push_at(40, start);
        let now = start + FADE / 2;
        let head = reveal.alpha_at(0, now);
        let tail = reveal.alpha_at(30, now);
        assert!(
            head > tail,
            "chunk did not type in left to right: {head} vs {tail}"
        );
    }

    /// A large chunk must reveal smoothly across its whole length, not type in
    /// a prefix and then dump the remainder.
    #[test]
    fn a_large_chunk_reveals_evenly_across_its_length() {
        let start = Instant::now();
        let mut reveal = Reveal::frozen(start);
        let len = 4_000;
        reveal.push_at(len, start);
        let now = start + MAX_STAGGER / 2;
        let revealed = (0..len).filter(|&i| reveal.alpha_at(i, now) > 0.0).count();
        let fraction = revealed as f64 / len as f64;
        assert!(
            (0.3..0.8).contains(&fraction),
            "halfway through the sweep, {:.0}% of a large chunk was revealed",
            fraction * 100.0
        );
    }

    #[test]
    fn a_huge_chunk_still_finishes_promptly() {
        // Without a stagger cap, a 100kB chunk would animate for minutes.
        let start = Instant::now();
        let mut reveal = Reveal::frozen(start);
        reveal.push_at(100_000, start);
        let bound = start + MAX_STAGGER + FADE;
        assert!(
            !reveal.animating_at(bound + Duration::from_millis(1)),
            "reveal outlived the stagger cap"
        );
        assert_eq!(reveal.alpha_at(99_999, bound), 1.0);
    }

    #[test]
    fn finished_chunks_are_pruned_so_memory_stays_bounded() {
        let start = Instant::now();
        let mut reveal = Reveal::frozen(start);
        for step in 0..500 {
            reveal.push_at(8, start + FADE * (step + 1));
        }
        assert!(
            reveal.tracked() <= 4,
            "kept {} arrival marks; pruning is broken",
            reveal.tracked()
        );
        // Pruned history must still read as fully revealed.
        assert_eq!(reveal.alpha_at(0, start + FADE * 500), 1.0);
    }

    #[test]
    fn text_rises_as_it_fades_in() {
        let start = Instant::now();
        let mut reveal = Reveal::frozen(start);
        reveal.push_at(1, start);
        assert!((reveal.rise_at(0, start) - RISE).abs() < 1e-6);
        assert_eq!(reveal.rise_at(0, start + FADE), 0.0);
    }

    #[test]
    fn animation_stops_and_schedules_no_more_frames() {
        let start = Instant::now();
        let mut reveal = Reveal::frozen(start);
        reveal.push_at(10, start);
        assert!(reveal.animating_at(start + FADE / 2));
        let done = start + MAX_STAGGER + FADE + Duration::from_millis(1);
        assert!(!reveal.animating_at(done));
        // A frozen reveal must never ask the event loop to wake: pinned frames
        // are for deterministic captures.
        assert_eq!(reveal.next_frame_at(start), None);
    }

    #[test]
    fn a_live_reveal_requests_frames_while_animating() {
        let mut reveal = Reveal::default();
        reveal.push(16);
        let now = Instant::now();
        assert!(
            reveal.animating_at(now),
            "no animation after appending text"
        );
        let next = reveal.next_frame_at(now).expect("no frame scheduled");
        assert!(next > now && next <= now + FRAME * 2);
    }

    /// The bug this queue exists to prevent: a big chunk followed by a small
    /// one must not reveal the small one first, or the transcript appears to
    /// fill in out of order.
    #[test]
    fn text_always_reveals_in_reading_order() {
        let start = Instant::now();
        let mut reveal = Reveal::frozen(start);
        reveal.push_at(300, start);
        reveal.push_at(5, start + Duration::from_millis(30));
        for step in 0..80 {
            let now = start + Duration::from_millis(step * 25);
            let mut previous = 1.0;
            for offset in (0..305).step_by(5) {
                let alpha = reveal.alpha_at(offset, now);
                assert!(
                    alpha <= previous + 1e-6,
                    "byte {offset} led the text before it at {}ms ({alpha} > {previous})",
                    step * 25
                );
                previous = alpha;
            }
        }
    }

    /// A long backlog must not push the animation minutes behind live output.
    #[test]
    fn the_queue_never_falls_far_behind_live_output() {
        let start = Instant::now();
        let mut reveal = Reveal::frozen(start);
        for step in 0..200 {
            let at = start + Duration::from_millis(step * 10);
            reveal.push_at(200, at);
            let lag = reveal
                .chunks
                .back()
                .expect("chunk")
                .done_at()
                .saturating_duration_since(at);
            assert!(
                lag <= MAX_LAG + MAX_STAGGER + FADE,
                "reveal fell {lag:?} behind live output"
            );
        }
    }

    #[test]
    fn reset_clears_history() {
        let start = Instant::now();
        let mut reveal = Reveal::frozen(start);
        reveal.push_at(10, start);
        reveal.reset();
        assert!(!reveal.animating_at(start));
        assert_eq!(reveal.alpha_at(0, start), 1.0);
    }

    #[test]
    fn empty_appends_are_ignored() {
        let start = Instant::now();
        let mut reveal = Reveal::frozen(start);
        reveal.push_at(0, start);
        assert!(!reveal.animating_at(start));
    }
}
