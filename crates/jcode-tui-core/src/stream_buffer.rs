//! Semantic stream buffer - paces streaming text reveal at a smooth rate.
//!
//! Providers feed text deltas with wildly different cadences. OpenAI emits many
//! tiny token-level deltas (a few chars every ~10-15ms), which already looks
//! smooth. Anthropic coalesces `content_block_delta` events into larger chunks
//! that arrive in bursts with gaps (e.g. 20-40 chars every ~80-100ms). If we
//! reveal each burst the instant it arrives, the UI stair-steps: a clump of
//! text pops in, then nothing for several frames, then another clump.
//!
//! To make every provider look the same, this buffer decouples *arrival* from
//! *reveal*. Incoming text accumulates in a backlog, and a time-paced
//! proportional controller drips it out: the reveal rate rises with the backlog
//! so we never fall far behind a fast model, yet a lone burst is spread over
//! several frames instead of dumped in one. The elapsed-time step is clamped so
//! an idle gap (connect latency, tool pauses) cannot bank budget that would
//! instantly dump the next burst.

use serde::Serialize;
use std::time::{Duration, Instant};

/// Steady-state reveal rate (chars/sec) when the backlog is empty. This sets the
/// floor cadence and how the trailing characters of a burst drain out.
const BASE_REVEAL_CPS: f32 = 180.0;

/// Additional reveal rate per buffered character. The controller speeds up as the
/// backlog grows so we track fast models with bounded latency: at steady incoming
/// rate `R`, the backlog settles near `(R - BASE_REVEAL_CPS) / REVEAL_BACKLOG_GAIN`.
const REVEAL_BACKLOG_GAIN: f32 = 3.0;

/// Maximum elapsed time credited to a single reveal step. Without this, a long
/// idle gap before the first/next burst would bank a huge budget and dump the
/// whole burst at once, reintroducing the choppiness we are trying to remove.
const MAX_REVEAL_STEP: Duration = Duration::from_millis(50);

/// Buffer that accumulates streaming text and reveals it at a smooth, paced rate.
pub struct StreamBuffer {
    buffer: String,
    last_reveal: Instant,
    /// Fractional reveal budget carried between steps so slow rates still make
    /// progress instead of rounding down to zero forever.
    carry: f32,
    base_cps: f32,
    backlog_gain: f32,
    max_step: Duration,
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamBufferMemoryProfile {
    pub buffered_text_bytes: usize,
    pub base_reveal_cps: u32,
}

impl Default for StreamBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamBuffer {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            last_reveal: Instant::now(),
            carry: 0.0,
            base_cps: BASE_REVEAL_CPS,
            backlog_gain: REVEAL_BACKLOG_GAIN,
            max_step: MAX_REVEAL_STEP,
        }
    }

    /// Push text into the buffer, returning any paced chunk ready to display now.
    pub fn push(&mut self, text: &str) -> Option<String> {
        self.buffer.push_str(text);
        self.reveal_now(Instant::now())
    }

    /// Force flush the entire buffer (call on message end, commit, or interrupt).
    pub fn flush(&mut self) -> Option<String> {
        self.carry = 0.0;
        self.last_reveal = Instant::now();
        if self.buffer.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.buffer))
        }
    }

    /// Reveal one paced frame worth of buffered text. Called from the periodic
    /// redraw tick so the backlog drains smoothly even when no new delta arrived
    /// this frame. Finalization paths should still call [`flush`] to avoid
    /// leaving text buffered at message boundaries.
    pub fn flush_smooth_frame(&mut self) -> Option<String> {
        self.reveal_now(Instant::now())
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Clear the buffer without returning content
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.carry = 0.0;
        self.last_reveal = Instant::now();
    }

    pub fn debug_memory_profile(&self) -> StreamBufferMemoryProfile {
        StreamBufferMemoryProfile {
            buffered_text_bytes: self.buffer.len(),
            base_reveal_cps: self.base_cps as u32,
        }
    }

    /// Proportional, time-paced reveal. Advances the budget by the (clamped)
    /// elapsed time times a backlog-scaled rate, then drains that many chars.
    fn reveal_now(&mut self, now: Instant) -> Option<String> {
        let backlog = self.buffer.chars().count();
        if backlog == 0 {
            // No backlog: reset so an idle gap cannot bank reveal budget.
            self.carry = 0.0;
            self.last_reveal = now;
            return None;
        }

        let dt = now
            .saturating_duration_since(self.last_reveal)
            .min(self.max_step)
            .as_secs_f32();
        self.last_reveal = now;

        let cps = self.base_cps + backlog as f32 * self.backlog_gain;
        self.carry += dt * cps;

        let mut reveal = self.carry.floor() as usize;
        if reveal == 0 {
            // Budget hasn't reached a whole char yet; keep accumulating.
            return None;
        }
        reveal = reveal.min(backlog);
        self.carry -= reveal as f32;
        Some(self.drain_chars(reveal))
    }

    /// Drain `char_count` characters from the front of the buffer on a UTF-8
    /// boundary.
    fn drain_chars(&mut self, char_count: usize) -> String {
        if char_count == 0 {
            return String::new();
        }
        let end = self
            .buffer
            .char_indices()
            .nth(char_count)
            .map(|(idx, _)| idx)
            .unwrap_or(self.buffer.len());
        let chunk = self.buffer[..end].to_string();
        self.buffer.replace_range(..end, "");
        chunk
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drain the buffer to empty using fixed-cadence redraw frames, returning the
    /// per-frame reveal sizes (in chars).
    fn drain_frames(buf: &mut StreamBuffer, start: Instant, frame: Duration) -> Vec<usize> {
        let mut sizes = Vec::new();
        let mut t = start;
        let mut guard = 0;
        while !buf.is_empty() {
            t += frame;
            if let Some(chunk) = buf.reveal_now(t) {
                sizes.push(chunk.chars().count());
            }
            guard += 1;
            assert!(guard < 100_000, "drain did not converge");
        }
        sizes
    }

    #[test]
    fn flush_drains_everything() {
        let mut buf = StreamBuffer::new();
        buf.buffer.push_str("remaining content");
        let result = buf.flush();
        assert_eq!(result, Some("remaining content".to_string()));
        assert!(buf.is_empty());
    }

    #[test]
    fn empty_push_reveals_nothing() {
        let mut buf = StreamBuffer::new();
        assert_eq!(buf.push(""), None);
        assert!(buf.is_empty());
    }

    #[test]
    fn paced_reveal_spreads_a_burst_over_multiple_frames() {
        let start = Instant::now();
        let mut buf = StreamBuffer::new();
        buf.last_reveal = start;
        buf.buffer.push_str(&"a".repeat(40));

        let sizes = drain_frames(&mut buf, start, Duration::from_millis(16));
        let total: usize = sizes.iter().sum();
        assert_eq!(total, 40);
        assert!(
            sizes.len() >= 3,
            "a 40-char burst should reveal across multiple frames, got {sizes:?}"
        );
        // No single 16ms frame should dump the whole burst.
        assert!(
            sizes.iter().all(|&n| n < 40),
            "no frame should reveal the entire burst, got {sizes:?}"
        );
    }

    #[test]
    fn idle_gap_does_not_dump_the_next_burst() {
        let start = Instant::now();
        let mut buf = StreamBuffer::new();
        buf.last_reveal = start;
        // Simulate a long connect/tool pause, then a burst arrives.
        let arrival = start + Duration::from_secs(5);
        buf.buffer.push_str(&"b".repeat(30));
        let first = buf
            .reveal_now(arrival)
            .map(|c| c.chars().count())
            .unwrap_or(0);
        assert!(
            first < 30,
            "the idle gap must not bank budget that dumps the burst, revealed {first}"
        );
        // The remainder still drains over subsequent frames.
        let sizes = drain_frames(&mut buf, arrival, Duration::from_millis(16));
        assert_eq!(first + sizes.iter().sum::<usize>(), 30);
    }

    #[test]
    fn bursty_and_steady_feeds_reveal_at_similar_smoothness() {
        // Steady (OpenAI-like): 4 chars every frame.
        let start = Instant::now();
        let frame = Duration::from_millis(16);
        let mut steady = StreamBuffer::new();
        steady.last_reveal = start;
        let mut steady_sizes = Vec::new();
        let mut t = start;
        for _ in 0..40 {
            t += frame;
            steady.buffer.push_str("abcd");
            if let Some(c) = steady.reveal_now(t) {
                steady_sizes.push(c.chars().count());
            }
        }
        steady_sizes.extend(drain_frames(&mut steady, t, frame));

        // Bursty (Anthropic-like): 24 chars every 6th frame.
        let mut bursty = StreamBuffer::new();
        bursty.last_reveal = start;
        let mut bursty_sizes = Vec::new();
        let mut t = start;
        for i in 0..60 {
            t += frame;
            if i % 6 == 0 {
                bursty.buffer.push_str(&"x".repeat(24));
            }
            if let Some(c) = bursty.reveal_now(t) {
                bursty_sizes.push(c.chars().count());
            }
        }
        bursty_sizes.extend(drain_frames(&mut bursty, t, frame));

        let max_burst = *bursty_sizes.iter().max().unwrap();
        // The whole 24-char clump must never appear in a single frame; pacing
        // should break it into smaller per-frame reveals like the steady feed.
        assert!(
            max_burst < 24,
            "bursty feed should be smoothed, max frame reveal was {max_burst} ({bursty_sizes:?})"
        );
    }

    #[test]
    fn reveal_respects_utf8_boundaries() {
        let start = Instant::now();
        let mut buf = StreamBuffer::new();
        buf.last_reveal = start;
        buf.buffer.push_str(&"é".repeat(40));

        let sizes = drain_frames(&mut buf, start, Duration::from_millis(16));
        assert_eq!(sizes.iter().sum::<usize>(), 40);
    }

    #[test]
    fn small_trailing_text_eventually_drains() {
        let start = Instant::now();
        let mut buf = StreamBuffer::new();
        buf.last_reveal = start;
        buf.buffer.push_str("hi");
        let sizes = drain_frames(&mut buf, start, Duration::from_millis(16));
        assert_eq!(sizes.iter().sum::<usize>(), 2);
    }
}
