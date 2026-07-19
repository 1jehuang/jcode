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
//! *reveal*. Incoming content accumulates in an ordered backlog, and a
//! time-paced proportional controller drips it out: the reveal rate rises with
//! the backlog so we never fall far behind a fast model, yet a lone burst is
//! spread over several frames instead of dumped in one. The elapsed-time step
//! is clamped so an idle gap (connect latency, tool pauses) cannot bank budget
//! that would instantly dump the next burst.
//!
//! The backlog is *segment-aware*: reasoning text and normal answer text are
//! queued as ordered segments of one stream (plus zero-width
//! "close reasoning region" markers), so both kinds share the same smoothing
//! controller and reveal strictly in arrival order. Historically only answer
//! text was paced while reasoning deltas were appended raw, which made
//! reasoning pop in provider-sized clumps and forced ordering flushes that
//! defeated the answer-text pacing too.

use serde::Serialize;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Steady-state reveal rate (chars/sec) when the backlog is empty. This sets the
/// floor cadence and how the trailing characters of a burst drain out.
const BASE_REVEAL_CPS: f32 = 180.0;

/// Additional reveal rate per buffered character. The controller speeds up as the
/// backlog grows so we track fast models with bounded latency: at steady incoming
/// rate `R`, the backlog settles near `(R - BASE_REVEAL_CPS) / REVEAL_BACKLOG_GAIN`.
const REVEAL_BACKLOG_GAIN: f32 = 3.0;

/// Hard ceiling for paced output, in characters per second. The proportional
/// controller above may ask to catch up much faster when a provider delivers a
/// whole response in one burst. Without a ceiling, a 3k-character backlog at a
/// 50ms redraw cadence reveals more than 500 characters in one frame, which is
/// several terminal rows appearing at once. Capping by elapsed time (rather
/// than by a fixed chars-per-frame value) keeps 16ms and 50ms redraw loops at
/// the same visual rate while still draining a large burst in a few seconds.
const MAX_REVEAL_CPS: f32 = 960.0;

/// Maximum elapsed time credited to a single reveal step. Without this, a long
/// idle gap before the first/next burst would bank a huge budget and dump the
/// whole burst at once, reintroducing the choppiness we are trying to remove.
const MAX_REVEAL_STEP: Duration = Duration::from_millis(50);

/// Maximum jitter-recorder events retained per series (arrivals / reveals).
const JITTER_EVENT_CAP: usize = 4096;

/// Kind of streamed content moving through the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StreamKind {
    /// Normal assistant answer text.
    Text,
    /// Reasoning ("thinking") text, rendered dim+italic by the app.
    Reasoning,
}

/// A revealed operation, in arrival order. Callers apply these to the UI:
/// `Text` appends answer text, `Reasoning` appends reasoning text, and
/// `CloseReasoning` ends the live reasoning region (exactly after the final
/// buffered reasoning character it followed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamOp {
    Text(String),
    Reasoning(String),
    CloseReasoning,
}

/// One queued backlog entry.
#[derive(Debug)]
enum QueuedOp {
    Chunk { kind: StreamKind, text: String },
    CloseReasoning,
}

/// Buffer that accumulates streaming content and reveals it at a smooth, paced
/// rate, preserving the arrival order of answer text, reasoning text, and
/// reasoning-region boundaries.
pub struct StreamBuffer {
    queue: VecDeque<QueuedOp>,
    /// Cached total chars across queued chunks (markers cost nothing).
    backlog_chars: usize,
    last_reveal: Instant,
    /// Fractional reveal budget carried between steps so slow rates still make
    /// progress instead of rounding down to zero forever.
    carry: f32,
    /// Independent hard-ceiling budget. Keeping this separate from `carry`
    /// prevents a large proportional-controller backlog from bypassing the
    /// wall-clock cap when many provider deltas arrive between redraw ticks.
    ceiling_carry: f32,
    /// Whether reasoning pushed through this buffer is still "open" (no close
    /// marker queued since the last reasoning chunk).
    reasoning_open: bool,
    base_cps: f32,
    backlog_gain: f32,
    max_step: Duration,
    jitter: JitterRecorder,
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
            queue: VecDeque::new(),
            backlog_chars: 0,
            last_reveal: Instant::now(),
            carry: 0.0,
            ceiling_carry: 0.0,
            reasoning_open: false,
            base_cps: BASE_REVEAL_CPS,
            backlog_gain: REVEAL_BACKLOG_GAIN,
            max_step: MAX_REVEAL_STEP,
            jitter: JitterRecorder::default(),
        }
    }

    /// Push answer text into the buffer, returning any paced ops ready to apply
    /// now. If a reasoning region is open in the backlog and this text contains
    /// non-whitespace, a `CloseReasoning` marker is queued first so the region
    /// closes (in order) before the answer text reveals.
    pub fn push_text(&mut self, text: &str) -> Vec<StreamOp> {
        if text.is_empty() {
            return self.reveal_now(Instant::now());
        }
        if self.reasoning_open && !text.trim().is_empty() {
            self.queue.push_back(QueuedOp::CloseReasoning);
            self.reasoning_open = false;
        }
        self.push_chunk(StreamKind::Text, text);
        self.reveal_now(Instant::now())
    }

    /// Push reasoning text into the buffer, returning any paced ops ready to
    /// apply now. Marks the reasoning region open until a close marker is queued.
    pub fn push_reasoning(&mut self, text: &str) -> Vec<StreamOp> {
        if text.is_empty() {
            return self.reveal_now(Instant::now());
        }
        self.reasoning_open = true;
        self.push_chunk(StreamKind::Reasoning, text);
        self.reveal_now(Instant::now())
    }

    /// Queue a reasoning-region close marker (no-op when no reasoning is open in
    /// the backlog), returning any paced ops ready to apply now. The marker
    /// reveals exactly after the final buffered reasoning character.
    pub fn push_close_reasoning(&mut self) -> Vec<StreamOp> {
        if self.reasoning_open {
            self.queue.push_back(QueuedOp::CloseReasoning);
            self.reasoning_open = false;
        }
        self.reveal_now(Instant::now())
    }

    /// Force flush the entire backlog (call on message end, commit, or
    /// interrupt). Returns every remaining op in order.
    pub fn flush(&mut self) -> Vec<StreamOp> {
        self.carry = 0.0;
        self.ceiling_carry = 0.0;
        self.last_reveal = Instant::now();
        let ops = self.drain_ops(self.backlog_chars, true);
        debug_assert!(self.queue.is_empty());
        self.backlog_chars = 0;
        self.reasoning_open = false;
        ops
    }

    /// Reveal one paced frame worth of buffered content. Called from the
    /// periodic redraw tick so the backlog drains smoothly even when no new
    /// delta arrived this frame. Finalization paths should still call [`flush`]
    /// to avoid leaving content buffered at message boundaries.
    pub fn flush_smooth_frame(&mut self) -> Vec<StreamOp> {
        self.reveal_now(Instant::now())
    }

    /// Check if the backlog is empty (no chunks and no markers).
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Clear the backlog without returning content.
    pub fn clear(&mut self) {
        self.queue.clear();
        self.backlog_chars = 0;
        self.carry = 0.0;
        self.ceiling_carry = 0.0;
        self.reasoning_open = false;
        self.last_reveal = Instant::now();
    }

    pub fn debug_memory_profile(&self) -> StreamBufferMemoryProfile {
        let buffered_text_bytes = self
            .queue
            .iter()
            .map(|op| match op {
                QueuedOp::Chunk { text, .. } => text.len(),
                QueuedOp::CloseReasoning => 0,
            })
            .sum();
        StreamBufferMemoryProfile {
            buffered_text_bytes,
            base_reveal_cps: self.base_cps as u32,
        }
    }

    /// Arrival-vs-reveal smoothness statistics. See [`StreamJitterProfile`].
    pub fn jitter_profile(&self) -> StreamJitterProfile {
        self.jitter.profile()
    }

    /// Reset the jitter recorder (e.g. to measure one turn in isolation).
    pub fn reset_jitter(&mut self) {
        self.jitter = JitterRecorder::default();
    }

    /// Append a chunk, coalescing with the previous queue entry when it has the
    /// same kind so the queue stays short under token-level feeds.
    fn push_chunk(&mut self, kind: StreamKind, text: &str) {
        self.backlog_chars += text.chars().count();
        self.jitter.record_arrival(kind, text.chars().count());
        if let Some(QueuedOp::Chunk {
            kind: last_kind,
            text: last_text,
        }) = self.queue.back_mut()
            && *last_kind == kind
        {
            last_text.push_str(text);
            return;
        }
        self.queue.push_back(QueuedOp::Chunk {
            kind,
            text: text.to_string(),
        });
    }

    /// Proportional, time-paced reveal. Advances the budget by the (clamped)
    /// elapsed time times a backlog-scaled rate, then drains that many chars
    /// (and any zero-cost markers reached along the way).
    fn reveal_now(&mut self, now: Instant) -> Vec<StreamOp> {
        if self.backlog_chars == 0 {
            // No chunk backlog: reset so an idle gap cannot bank reveal budget.
            self.carry = 0.0;
            self.ceiling_carry = 0.0;
            self.last_reveal = now;
            // Any queued entries are markers only; emit them immediately.
            return self.drain_ops(0, true);
        }

        let dt = now
            .saturating_duration_since(self.last_reveal)
            .min(self.max_step)
            .as_secs_f32();
        self.last_reveal = now;

        let cps = self.base_cps + self.backlog_chars as f32 * self.backlog_gain;
        self.carry += dt * cps;
        self.ceiling_carry += dt * MAX_REVEAL_CPS;

        let controller_budget = self.carry.floor() as usize;
        let ceiling_budget = self.ceiling_carry.floor() as usize;
        let mut reveal = controller_budget.min(ceiling_budget);
        if reveal == 0 {
            // Budget hasn't reached a whole char yet; keep accumulating. Leading
            // markers (if any) still emit so region boundaries are not delayed.
            return self.drain_ops(0, false);
        }

        // The backlog-scaled controller is intentionally aggressive about
        // catching up, but it must never turn one provider burst into a visible
        // wall of text. Bound each step by the same wall-clock rate regardless
        // of redraw cadence: 16ms permits at most 16 chars, while the clamped
        // 50ms step permits at most 48. The independent token bucket also means
        // repeated push calls with near-zero elapsed time cannot mint one free
        // character each and exceed the wall-clock ceiling.
        reveal = reveal.min(self.backlog_chars);
        self.carry -= reveal as f32;
        self.ceiling_carry -= reveal as f32;
        self.drain_ops(reveal, false)
    }

    /// Drain up to `char_count` chunk characters from the front of the queue (on
    /// UTF-8 boundaries), emitting markers whenever they reach the front. When
    /// `drain_all_markers` is set, trailing markers behind the final drained
    /// chunk are emitted even if the char budget is exhausted (used by flush).
    fn drain_ops(&mut self, mut char_count: usize, drain_all_markers: bool) -> Vec<StreamOp> {
        let mut ops: Vec<StreamOp> = Vec::new();
        loop {
            match self.queue.front_mut() {
                None => break,
                Some(QueuedOp::CloseReasoning) => {
                    self.queue.pop_front();
                    ops.push(StreamOp::CloseReasoning);
                }
                Some(QueuedOp::Chunk { kind, text }) => {
                    if char_count == 0 {
                        if drain_all_markers {
                            // flush() always passes the full backlog as budget,
                            // so a chunk here means budget accounting drifted.
                            debug_assert!(false, "flush budget must cover the backlog");
                        }
                        break;
                    }
                    let kind = *kind;
                    let available = text.chars().count();
                    let take = char_count.min(available);
                    let chunk = if take == available {
                        let QueuedOp::Chunk { text, .. } = self.queue.pop_front().expect("front")
                        else {
                            unreachable!()
                        };
                        text
                    } else {
                        let end = text
                            .char_indices()
                            .nth(take)
                            .map(|(idx, _)| idx)
                            .unwrap_or(text.len());
                        let chunk = text[..end].to_string();
                        text.replace_range(..end, "");
                        chunk
                    };
                    char_count -= take;
                    self.backlog_chars = self.backlog_chars.saturating_sub(take);
                    self.jitter.record_reveal(kind, take);
                    match kind {
                        StreamKind::Text => ops.push(StreamOp::Text(chunk)),
                        StreamKind::Reasoning => ops.push(StreamOp::Reasoning(chunk)),
                    }
                }
            }
        }
        ops
    }
}

// ---------------------------------------------------------------------------
// Jitter metrics
// ---------------------------------------------------------------------------

/// One recorded event: when it happened and how many chars moved.
#[derive(Debug, Clone, Copy)]
struct JitterEvent {
    at: Instant,
    chars: usize,
    kind: StreamKind,
}

/// Records arrival (provider burst) and reveal (paced UI) events so choppiness
/// can be quantified: a smooth reveal stream has low variance in chars-per-time
/// regardless of how bursty the arrivals were.
#[derive(Debug, Default)]
struct JitterRecorder {
    arrivals: VecDeque<JitterEvent>,
    reveals: VecDeque<JitterEvent>,
}

impl JitterRecorder {
    fn record_arrival(&mut self, kind: StreamKind, chars: usize) {
        Self::record(&mut self.arrivals, kind, chars);
    }

    fn record_reveal(&mut self, kind: StreamKind, chars: usize) {
        Self::record(&mut self.reveals, kind, chars);
    }

    fn record(series: &mut VecDeque<JitterEvent>, kind: StreamKind, chars: usize) {
        if chars == 0 {
            return;
        }
        if series.len() >= JITTER_EVENT_CAP {
            series.pop_front();
        }
        series.push_back(JitterEvent {
            at: Instant::now(),
            chars,
            kind,
        });
    }

    fn profile(&self) -> StreamJitterProfile {
        StreamJitterProfile {
            arrivals: SeriesStats::compute(&self.arrivals, None),
            reveals: SeriesStats::compute(&self.reveals, None),
            reasoning_arrivals: SeriesStats::compute(&self.arrivals, Some(StreamKind::Reasoning)),
            reasoning_reveals: SeriesStats::compute(&self.reveals, Some(StreamKind::Reasoning)),
            text_arrivals: SeriesStats::compute(&self.arrivals, Some(StreamKind::Text)),
            text_reveals: SeriesStats::compute(&self.reveals, Some(StreamKind::Text)),
        }
    }
}

/// Smoothness statistics for one event series. The headline number is
/// `bucket_100ms_cv`: the coefficient of variation of chars revealed per 100ms
/// bucket over the active span. Bursty (choppy) streams have a high CV; a
/// perfectly smooth drip approaches 0. Comparing `arrivals` vs `reveals` shows
/// how much smoothing the buffer added.
#[derive(Debug, Clone, Serialize)]
pub struct SeriesStats {
    pub events: usize,
    pub total_chars: usize,
    pub mean_chunk: f64,
    pub max_chunk: usize,
    pub p95_chunk: usize,
    /// Coefficient of variation (stddev/mean) of per-event chunk sizes.
    pub chunk_cv: f64,
    pub mean_gap_ms: f64,
    pub p95_gap_ms: f64,
    pub max_gap_ms: f64,
    /// Coefficient of variation of chars-per-100ms buckets across the span.
    pub bucket_100ms_cv: f64,
    pub bucket_100ms_max_chars: usize,
    pub span_ms: f64,
}

impl SeriesStats {
    fn compute(series: &VecDeque<JitterEvent>, kind: Option<StreamKind>) -> Self {
        let events: Vec<&JitterEvent> = series
            .iter()
            .filter(|e| kind.is_none_or(|k| e.kind == k))
            .collect();
        let mut stats = SeriesStats {
            events: events.len(),
            total_chars: events.iter().map(|e| e.chars).sum(),
            mean_chunk: 0.0,
            max_chunk: events.iter().map(|e| e.chars).max().unwrap_or(0),
            p95_chunk: 0,
            chunk_cv: 0.0,
            mean_gap_ms: 0.0,
            p95_gap_ms: 0.0,
            max_gap_ms: 0.0,
            bucket_100ms_cv: 0.0,
            bucket_100ms_max_chars: 0,
            span_ms: 0.0,
        };
        if events.is_empty() {
            return stats;
        }

        let chunks: Vec<f64> = events.iter().map(|e| e.chars as f64).collect();
        stats.mean_chunk = mean(&chunks);
        stats.p95_chunk = percentile_usize(events.iter().map(|e| e.chars), 0.95);
        stats.chunk_cv = coefficient_of_variation(&chunks);

        if events.len() >= 2 {
            let gaps: Vec<f64> = events
                .windows(2)
                .map(|w| w[1].at.duration_since(w[0].at).as_secs_f64() * 1000.0)
                .collect();
            stats.mean_gap_ms = mean(&gaps);
            stats.p95_gap_ms = percentile_f64(&gaps, 0.95);
            stats.max_gap_ms = gaps.iter().copied().fold(0.0_f64, f64::max);

            let start = events.first().expect("non-empty").at;
            let span = events.last().expect("non-empty").at.duration_since(start);
            stats.span_ms = span.as_secs_f64() * 1000.0;
            let bucket_count = (span.as_millis() as usize / 100).max(1) + 1;
            let mut buckets = vec![0.0_f64; bucket_count];
            for e in &events {
                let idx = (e.at.duration_since(start).as_millis() as usize / 100)
                    .min(bucket_count.saturating_sub(1));
                buckets[idx] += e.chars as f64;
            }
            stats.bucket_100ms_cv = coefficient_of_variation(&buckets);
            stats.bucket_100ms_max_chars = buckets.iter().copied().fold(0.0_f64, f64::max) as usize;
        }
        stats
    }
}

/// Arrival-vs-reveal smoothness report. `reveals.bucket_100ms_cv` should be
/// substantially lower than `arrivals.bucket_100ms_cv` when pacing is working.
#[derive(Debug, Clone, Serialize)]
pub struct StreamJitterProfile {
    pub arrivals: SeriesStats,
    pub reveals: SeriesStats,
    pub reasoning_arrivals: SeriesStats,
    pub reasoning_reveals: SeriesStats,
    pub text_arrivals: SeriesStats,
    pub text_reveals: SeriesStats,
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn coefficient_of_variation(values: &[f64]) -> f64 {
    let m = mean(values);
    if m == 0.0 || values.len() < 2 {
        return 0.0;
    }
    let var = values.iter().map(|v| (v - m).powi(2)).sum::<f64>() / values.len() as f64;
    var.sqrt() / m
}

fn percentile_f64(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn percentile_usize(values: impl Iterator<Item = usize>, p: f64) -> usize {
    let mut sorted: Vec<usize> = values.collect();
    if sorted.is_empty() {
        return 0;
    }
    sorted.sort_unstable();
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sum chars of text+reasoning chunks in a batch of ops.
    fn op_chars(ops: &[StreamOp]) -> usize {
        ops.iter()
            .map(|op| match op {
                StreamOp::Text(t) | StreamOp::Reasoning(t) => t.chars().count(),
                StreamOp::CloseReasoning => 0,
            })
            .sum()
    }

    /// Drain the buffer to empty using fixed-cadence redraw frames, returning the
    /// per-frame reveal sizes (in chars).
    fn drain_frames(buf: &mut StreamBuffer, start: Instant, frame: Duration) -> Vec<usize> {
        let mut sizes = Vec::new();
        let mut t = start;
        let mut guard = 0;
        while !buf.is_empty() {
            t += frame;
            let ops = buf.reveal_now(t);
            let chars = op_chars(&ops);
            if chars > 0 {
                sizes.push(chars);
            }
            guard += 1;
            assert!(guard < 100_000, "drain did not converge");
        }
        sizes
    }

    /// Concatenate ops into a flat (kind, text) trace for ordering assertions,
    /// merging adjacent same-kind chunks.
    fn flatten(ops: impl IntoIterator<Item = StreamOp>) -> Vec<(char, String)> {
        let mut out: Vec<(char, String)> = Vec::new();
        for op in ops {
            let (tag, text) = match op {
                StreamOp::Text(t) => ('t', t),
                StreamOp::Reasoning(t) => ('r', t),
                StreamOp::CloseReasoning => ('c', String::new()),
            };
            if tag != 'c'
                && let Some((last_tag, last_text)) = out.last_mut()
                && *last_tag == tag
            {
                last_text.push_str(&text);
                continue;
            }
            out.push((tag, text));
        }
        out
    }

    #[test]
    fn flush_drains_everything() {
        let mut buf = StreamBuffer::new();
        buf.push_chunk(StreamKind::Text, "remaining content");
        let ops = buf.flush();
        assert_eq!(ops, vec![StreamOp::Text("remaining content".to_string())]);
        assert!(buf.is_empty());
    }

    #[test]
    fn empty_push_reveals_nothing() {
        let mut buf = StreamBuffer::new();
        assert!(buf.push_text("").is_empty());
        assert!(buf.push_reasoning("").is_empty());
        assert!(buf.is_empty());
    }

    #[test]
    fn paced_reveal_spreads_a_burst_over_multiple_frames() {
        let start = Instant::now();
        let mut buf = StreamBuffer::new();
        buf.last_reveal = start;
        buf.push_chunk(StreamKind::Text, &"a".repeat(40));

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
    fn large_single_burst_is_bounded_by_wall_clock_reveal_rate() {
        let start = Instant::now();
        let mut buf = StreamBuffer::new();
        buf.last_reveal = start;
        buf.push_chunk(StreamKind::Text, &"a".repeat(3_356));

        let sizes = drain_frames(&mut buf, start, Duration::from_millis(50));
        assert_eq!(sizes.iter().sum::<usize>(), 3_356);
        assert!(
            sizes.iter().all(|&n| n <= 48),
            "a 50ms paced frame must reveal at most 48 chars: {sizes:?}"
        );
        assert!(
            sizes.len() >= 70,
            "the burst should drain smoothly over several seconds: {} frames",
            sizes.len()
        );
    }

    #[test]
    fn reveal_ceiling_is_independent_of_redraw_cadence() {
        let start = Instant::now();
        let mut buf = StreamBuffer::new();
        buf.last_reveal = start;
        buf.push_chunk(StreamKind::Text, &"b".repeat(1_000));

        let sizes = drain_frames(&mut buf, start, Duration::from_millis(16));
        assert_eq!(sizes.iter().sum::<usize>(), 1_000);
        assert!(
            sizes.iter().all(|&n| n <= 16),
            "a 16ms paced frame must reveal at most 16 chars: {sizes:?}"
        );
    }

    #[test]
    fn frequent_push_calls_cannot_bypass_the_wall_clock_ceiling() {
        let start = Instant::now();
        let mut buf = StreamBuffer::new();
        buf.last_reveal = start;
        buf.push_chunk(StreamKind::Text, &"c".repeat(1_000));

        let first_at = start + Duration::from_millis(50);
        let mut revealed = op_chars(&buf.reveal_now(first_at));
        for _ in 0..100 {
            // Simulate many provider callbacks before the clock advances to the
            // next redraw frame. They must not each receive a free character.
            revealed += op_chars(&buf.reveal_now(first_at));
        }
        assert_eq!(revealed, 48);

        let second = op_chars(&buf.reveal_now(first_at + Duration::from_millis(50)));
        assert_eq!(second, 48);
    }

    #[test]
    fn reasoning_burst_is_paced_like_text() {
        let start = Instant::now();
        let mut buf = StreamBuffer::new();
        buf.last_reveal = start;
        buf.push_chunk(StreamKind::Reasoning, &"r".repeat(40));

        let sizes = drain_frames(&mut buf, start, Duration::from_millis(16));
        assert_eq!(sizes.iter().sum::<usize>(), 40);
        assert!(
            sizes.len() >= 3 && sizes.iter().all(|&n| n < 40),
            "reasoning bursts must be paced, got {sizes:?}"
        );
    }

    #[test]
    fn idle_gap_does_not_dump_the_next_burst() {
        let start = Instant::now();
        let mut buf = StreamBuffer::new();
        buf.last_reveal = start;
        // Simulate a long connect/tool pause, then a burst arrives.
        let arrival = start + Duration::from_secs(5);
        buf.push_chunk(StreamKind::Text, &"b".repeat(30));
        let first = op_chars(&buf.reveal_now(arrival));
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
            steady.push_chunk(StreamKind::Text, "abcd");
            let chars = op_chars(&steady.reveal_now(t));
            if chars > 0 {
                steady_sizes.push(chars);
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
                bursty.push_chunk(StreamKind::Text, &"x".repeat(24));
            }
            let chars = op_chars(&bursty.reveal_now(t));
            if chars > 0 {
                bursty_sizes.push(chars);
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
    fn bursty_reasoning_feed_is_smoothed() {
        let start = Instant::now();
        let frame = Duration::from_millis(16);
        let mut buf = StreamBuffer::new();
        buf.last_reveal = start;
        let mut sizes = Vec::new();
        let mut t = start;
        for i in 0..60 {
            t += frame;
            if i % 6 == 0 {
                buf.push_chunk(StreamKind::Reasoning, &"y".repeat(24));
            }
            let chars = op_chars(&buf.reveal_now(t));
            if chars > 0 {
                sizes.push(chars);
            }
        }
        sizes.extend(drain_frames(&mut buf, t, frame));
        let max_burst = *sizes.iter().max().unwrap();
        assert!(
            max_burst < 24,
            "bursty reasoning should be smoothed, max frame reveal was {max_burst} ({sizes:?})"
        );
    }

    #[test]
    fn ordering_is_preserved_across_kinds_and_markers() {
        let mut buf = StreamBuffer::new();
        let mut ops = Vec::new();
        ops.extend(buf.push_reasoning("think think"));
        ops.extend(buf.push_close_reasoning());
        ops.extend(buf.push_text("answer one"));
        ops.extend(buf.push_reasoning("more thinking"));
        ops.extend(buf.push_text("answer two"));
        ops.extend(buf.flush());

        let trace = flatten(ops);
        assert_eq!(
            trace,
            vec![
                ('r', "think think".to_string()),
                ('c', String::new()),
                ('t', "answer one".to_string()),
                ('r', "more thinking".to_string()),
                // push_text auto-queued the close marker for the reopened region.
                ('c', String::new()),
                ('t', "answer two".to_string()),
            ]
        );
        assert!(buf.is_empty());
    }

    #[test]
    fn close_marker_waits_for_buffered_reasoning() {
        let start = Instant::now();
        let mut buf = StreamBuffer::new();
        buf.last_reveal = start;
        buf.push_chunk(StreamKind::Reasoning, &"z".repeat(60));
        buf.reasoning_open = true;
        // Marker queued behind a large backlog: nothing closes yet.
        let immediate = buf.push_close_reasoning();
        assert!(
            !immediate.contains(&StreamOp::CloseReasoning),
            "close must not jump ahead of buffered reasoning"
        );
        // Drain everything; the close marker must come after all reasoning chars.
        let mut all = immediate;
        let mut t = start;
        let mut guard = 0;
        while !buf.is_empty() {
            t += Duration::from_millis(16);
            all.extend(buf.reveal_now(t));
            guard += 1;
            assert!(guard < 100_000);
        }
        let close_idx = all
            .iter()
            .position(|op| matches!(op, StreamOp::CloseReasoning))
            .expect("close marker must drain");
        assert_eq!(close_idx, all.len() - 1);
        let reasoning_chars: usize = all
            .iter()
            .map(|op| match op {
                StreamOp::Reasoning(t) => t.chars().count(),
                _ => 0,
            })
            .sum();
        assert_eq!(reasoning_chars, 60);
    }

    #[test]
    fn whitespace_text_does_not_close_reasoning() {
        let mut buf = StreamBuffer::new();
        let mut ops = Vec::new();
        ops.extend(buf.push_reasoning("thinking"));
        ops.extend(buf.push_text("\n"));
        ops.extend(buf.push_reasoning(" still thinking"));
        ops.extend(buf.flush());
        assert!(
            !ops.iter().any(|op| matches!(op, StreamOp::CloseReasoning)),
            "whitespace-only text must not close the reasoning region: {ops:?}"
        );
    }

    #[test]
    fn marker_only_queue_emits_immediately() {
        let mut buf = StreamBuffer::new();
        buf.reasoning_open = true;
        let ops = buf.push_close_reasoning();
        assert_eq!(ops, vec![StreamOp::CloseReasoning]);
        assert!(buf.is_empty());
    }

    #[test]
    fn reveal_respects_utf8_boundaries() {
        let start = Instant::now();
        let mut buf = StreamBuffer::new();
        buf.last_reveal = start;
        buf.push_chunk(StreamKind::Text, &"é".repeat(40));

        let sizes = drain_frames(&mut buf, start, Duration::from_millis(16));
        assert_eq!(sizes.iter().sum::<usize>(), 40);
    }

    #[test]
    fn small_trailing_text_eventually_drains() {
        let start = Instant::now();
        let mut buf = StreamBuffer::new();
        buf.last_reveal = start;
        buf.push_chunk(StreamKind::Text, "hi");
        let sizes = drain_frames(&mut buf, start, Duration::from_millis(16));
        assert_eq!(sizes.iter().sum::<usize>(), 2);
    }

    #[test]
    fn jitter_profile_shows_reveals_smoother_than_arrivals() {
        let start = Instant::now();
        let frame = Duration::from_millis(16);
        let mut buf = StreamBuffer::new();
        buf.last_reveal = start;
        let mut t = start;
        for i in 0..120 {
            t += frame;
            if i % 6 == 0 {
                buf.push_chunk(StreamKind::Reasoning, &"j".repeat(24));
            }
            buf.reveal_now(t);
        }
        drain_frames(&mut buf, t, frame);
        let profile = buf.jitter_profile();
        assert!(profile.reasoning_arrivals.events > 0);
        assert!(profile.reasoning_reveals.events > 0);
        assert_eq!(
            profile.reasoning_arrivals.total_chars,
            profile.reasoning_reveals.total_chars
        );
        // Reveals must be meaningfully smoother (per-event max far below the
        // arrival burst size).
        assert!(
            profile.reasoning_reveals.max_chunk < profile.reasoning_arrivals.max_chunk,
            "reveal max chunk {} should be below arrival burst {}",
            profile.reasoning_reveals.max_chunk,
            profile.reasoning_arrivals.max_chunk
        );
    }
}
