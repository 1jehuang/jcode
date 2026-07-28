//! Streaming smoothness benchmark: replay a scripted token stream through the
//! same per-frame work the event loop does, and measure every frame.
//!
//! "The streaming looks laggy" is an impression; this turns it into numbers.
//! [`crate::profile`] already sweeps *static* states, but streaming is a
//! different animal: the cost that matters is the frame built between two
//! deltas, and how that cost changes as the reply grows. A static sweep can
//! never see either, which is exactly why the lag was felt before it was
//! measured.
//!
//! The replay is faithful to `RedrawRequested`: apply any due delta the way
//! `drain_harness_updates` does, advance the reveal and the scroll smoothing,
//! measure the frame through the shared painter, feed the glide, and build the
//! full scene. Time is simulated (the [`crate::stream::Stream`] API takes
//! `now` as a parameter), so the replay is deterministic in *work*; only the
//! wall-clock timings vary with the machine.
//!
//! Frames are classified so the numbers answer separate questions:
//!
//! - **delta**: a frame that applied newly "received" text. This is where
//!   layout work is legitimate (the tail message changed), and its trend over
//!   the reply's growth is the smoothness curve: today the whole tail message
//!   is re-parsed and re-laid per delta, so this cost grows with reply length.
//! - **reveal**: a frame between deltas while the reveal animation runs. It
//!   must do *zero* layout work: it exists only to fade glyphs in, and any
//!   relayout here is the exact bug class that makes streaming stutter.
//! - **idle**: a frame after the reveal has caught up (still animating the
//!   glide, or the settle tail of the loop).

use crate::paint::Painter;
use crate::transcript::Message;
use crate::{App, Model, build_scene};
use std::time::{Duration, Instant};
use vello::Scene;

/// Surface the replay renders at. Same fixed geometry as [`crate::profile`],
/// for the same reason: costs are only comparable against one wrap width.
const WIDTH: u32 = 2200;
const HEIGHT: u32 = 1440;
const SCALE: f64 = 2.0;

/// Simulated frame cadence in milliseconds, matching [`crate::stream::FRAME`]:
/// the loop asks for ~120Hz while the reveal runs.
const FRAME_MS: u64 = 8;

/// Hard cap on replayed frames, so a bug that keeps the stream animating
/// forever fails loudly instead of hanging the bench.
const MAX_FRAMES: usize = 100_000;

/// One scripted streaming session.
pub struct Config {
    /// Prior turns in the transcript before the stream starts. Streaming into
    /// a long session is the case that lagged; an empty page hides it.
    pub history_turns: usize,
    /// Characters in the streamed reply.
    pub reply_chars: usize,
    /// Simulated milliseconds between delta arrivals. Real deltas arrive in
    /// bursts around this order; one delta every third frame is a typical
    /// steady stream.
    pub delta_interval_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            history_turns: 30,
            reply_chars: 6_000,
            delta_interval_ms: 24,
        }
    }
}

impl Config {
    /// A smaller replay for the test suite: large enough to cross several
    /// hundred frames and every frame kind, small enough to stay well under a
    /// second on a contended machine.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn small() -> Self {
        Self {
            history_turns: 8,
            reply_chars: 1_200,
            delta_interval_ms: 24,
        }
    }
}

/// What a replayed frame was doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// Applied newly received text before drawing.
    Delta,
    /// No new text, but the reveal was still sweeping.
    Reveal,
    /// No new text and the reveal had caught up (glide or settle frames).
    Idle,
}

/// One frame's measurements.
pub struct FrameSample {
    pub kind: FrameKind,
    /// Wall-clock cost of the frame's CPU work (advance, measure, glide,
    /// scene build), in microseconds.
    pub us: u64,
    /// Messages laid out from scratch during this frame. The deterministic
    /// half of the signal: exact on any machine.
    pub relayouts: usize,
    /// Reply characters received when the frame was built, so cost can be
    /// read as a function of reply growth.
    pub received: usize,
}

/// Summary of one frame class.
pub struct Stats {
    pub count: usize,
    pub mean_us: u64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub max_us: u64,
}

fn stats_of(mut samples: Vec<u64>) -> Option<Stats> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    let count = samples.len();
    let at = |q: f64| samples[((count - 1) as f64 * q).round() as usize];
    Some(Stats {
        count,
        mean_us: samples.iter().sum::<u64>() / count as u64,
        p50_us: at(0.50),
        p95_us: at(0.95),
        max_us: samples[count - 1],
    })
}

/// The whole replay's samples, with the questions the bench exists to answer
/// exposed as methods rather than left for every caller to re-derive.
pub struct Report {
    pub samples: Vec<FrameSample>,
}

impl Report {
    pub fn stats(&self, kind: FrameKind) -> Option<Stats> {
        stats_of(
            self.samples
                .iter()
                .filter(|sample| sample.kind == kind)
                .map(|sample| sample.us)
                .collect(),
        )
    }

    /// Reveal/idle frames that did layout work. Must be empty: a frame whose
    /// only job is fading glyphs in has no business re-laying messages, and
    /// this is exact on any machine.
    pub fn wasteful_animation_frames(&self) -> Vec<&FrameSample> {
        self.samples
            .iter()
            .filter(|sample| sample.kind != FrameKind::Delta && sample.relayouts > 0)
            .collect()
    }

    /// Delta frames that laid out more than the tail message. The history is
    /// untouched by a delta, so anything above one relayout means the cache
    /// broke and streaming is paying for the whole conversation again.
    pub fn overworked_delta_frames(&self) -> Vec<&FrameSample> {
        self.samples
            .iter()
            .filter(|sample| sample.kind == FrameKind::Delta && sample.relayouts > 1)
            .collect()
    }

    /// Mean delta-frame cost early (first quarter of the reply) versus late
    /// (last quarter): the smoothness *curve*. A flat pair means streaming
    /// costs the same at the end of a long reply as at its start; a rising
    /// pair is the "it gets choppier as it writes" feeling, quantified.
    pub fn delta_growth(&self) -> Option<(u64, u64)> {
        let total = self.samples.iter().map(|s| s.received).max()?;
        let mean = |lo: usize, hi: usize| {
            stats_of(
                self.samples
                    .iter()
                    .filter(|s| s.kind == FrameKind::Delta && s.received >= lo && s.received < hi)
                    .map(|s| s.us)
                    .collect(),
            )
            .map(|stats| stats.mean_us)
        };
        Some((mean(0, total / 4)?, mean(total * 3 / 4, total + 1)?))
    }

    /// Frames costing more than `budget_us`, which at 8'333us is a missed
    /// 120Hz frame and at 16'600us a dropped 60Hz frame.
    pub fn over(&self, budget_us: u64) -> usize {
        self.samples
            .iter()
            .filter(|sample| sample.us > budget_us)
            .count()
    }
}

/// A deterministic markdown reply of at least `chars` characters: prose,
/// emphasis, lists, and periodic code fences, because a real reply is not one
/// long paragraph and code blocks cost differently from prose.
pub fn reply_text(chars: usize) -> String {
    let mut out = String::new();
    let mut n = 0usize;
    while out.len() < chars {
        match n % 6 {
            0 => out.push_str(
                "The client opens the socket and sends a **hello** frame carrying its \
                 supported version range, then waits for the server's `ack` before \
                 streaming any payload.\n\n",
            ),
            1 => out.push_str(
                "- validate the frame header\n- check the version overlap\n- reply \
                 with the negotiated codec\n\n",
            ),
            2 => out.push_str(&format!(
                "Step {n} narrows the window: each retry halves the budget until the \
                 peer answers or the deadline lands.\n\n"
            )),
            3 => out.push_str(&format!(
                "```rust\nfn negotiate_{n}(frame: &Frame) -> Result<Codec> {{\n    \
                 let overlap = frame.versions.intersect(SUPPORTED);\n    \
                 overlap.newest().ok_or(Error::NoOverlap)\n}}\n```\n\n"
            )),
            4 => out.push_str(
                "Backpressure is the interesting part: the reader owns the window, \
                 and the writer only learns about it one round trip late.\n\n",
            ),
            _ => out.push_str(&format!(
                "Section {n} covers the *slow path*, where the peer is behind a relay \
                 and every frame is re-encoded in flight.\n\n"
            )),
        }
        n += 1;
    }
    out
}

/// Delta sizes, cycled. Real streams are bursty: a few characters of a word,
/// then most of a sentence at once.
const CHUNKS: [usize; 8] = [12, 3, 45, 20, 80, 7, 120, 30];

/// Replay one scripted stream and measure every frame.
pub fn run(config: &Config) -> Report {
    let mut model = Model {
        busy: true,
        session_id: Some("session_bench".into()),
        ..Model::default()
    };
    for n in 0..config.history_turns {
        model
            .transcript
            .push(Message::user(format!("question {n} about the transport")));
        model.transcript.push(Message::assistant(format!(
            "answer {n}. the client opens the socket and sends a hello frame \
             carrying its supported version range. "
        )));
    }
    model.transcript.push(Message::user("stream me the design"));

    let reply: Vec<char> = reply_text(config.reply_chars).chars().collect();

    let mut painter = Painter::default();
    // Warm frame before the stream starts: the window has been drawing this
    // conversation already, so the history's layouts are cached. Charging the
    // first delta for a cold cache would measure startup, not streaming.
    let mut scene = Scene::new();
    build_scene(&mut scene, &mut painter, &model, (WIDTH, HEIGHT), SCALE);

    let epoch = Instant::now();
    let mut sim_ms = 0u64;
    let mut received = 0usize;
    let mut next_delta_at = 0u64;
    let mut chunk = 0usize;
    let mut samples = Vec::new();

    while received < reply.len() || model.stream.is_animating() {
        if samples.len() >= MAX_FRAMES {
            break;
        }
        let now = epoch + Duration::from_millis(sim_ms);

        // Deltas due this frame, folded in exactly as drain_harness_updates
        // folds a HarnessUpdate::Text.
        let mut applied_delta = false;
        if received < reply.len() && sim_ms >= next_delta_at {
            let take = CHUNKS[chunk % CHUNKS.len()].min(reply.len() - received);
            chunk += 1;
            let text: String = reply[received..received + take].iter().collect();
            received += take;
            next_delta_at = sim_ms + config.delta_interval_ms;
            model.transcript.append_assistant(&text);
            model
                .stream
                .extend_to(model.transcript.streaming_len(), now);
            applied_delta = true;
        }

        let relayouts_before = painter.transcript.total_relayouts();
        let start = Instant::now();

        // The RedrawRequested body, minus the GPU: advance the animations,
        // measure the frame through the shared painter, feed the glide, build
        // the scene.
        model.stream.advance(now);
        model.smooth.advance(now);
        let revealing = model.stream.is_revealing();
        let frame = App::frame_for_model_with((WIDTH, HEIGHT), SCALE, &model, &mut painter);
        {
            // observe_stream_growth, inlined: the same warm-cache measurement
            // the event loop makes to feed content growth into the glide.
            let width = (frame.column() - crate::transcript::USER_PAD_X * 2.0).max(1.0);
            let style = crate::scene::transcript_body_style(&model);
            let Painter {
                text,
                transcript: cache,
            } = &mut painter;
            let laid = cache.lay_out(
                text,
                &model.transcript,
                width,
                &model.theme,
                style,
                frame.scale,
            );
            let content = crate::viewport::Viewport::new(laid, 0.0, 0.0).content_height;
            model.stream.observe_content(content, model.scroll == 0.0);
        }
        let mut scene = Scene::new();
        build_scene(&mut scene, &mut painter, &model, (WIDTH, HEIGHT), SCALE);

        let us = start.elapsed().as_micros() as u64;
        let relayouts = painter.transcript.total_relayouts() - relayouts_before;
        let kind = if applied_delta {
            FrameKind::Delta
        } else if revealing {
            FrameKind::Reveal
        } else {
            FrameKind::Idle
        };
        samples.push(FrameSample {
            kind,
            us,
            relayouts,
            received,
        });
        sim_ms += FRAME_MS;
    }

    Report { samples }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> Report {
        run(&Config::small())
    }

    /// The replay must actually stream: deltas applied, reveal frames between
    /// them, and the animation finished. A bench that measured an empty loop
    /// would report the app as smooth forever.
    #[test]
    fn the_replay_streams_and_settles() {
        let report = report();
        assert!(report.stats(FrameKind::Delta).is_some(), "no delta frames");
        assert!(
            report.stats(FrameKind::Reveal).is_some(),
            "no reveal frames: the reveal never ran between deltas"
        );
        let last = report.samples.last().expect("no frames");
        assert!(last.received > 0, "nothing was received");
    }

    /// A frame between deltas exists to fade glyphs in, nothing else. Any
    /// layout work there is pure stutter, is exact on any machine, and is the
    /// regression class that makes streaming feel laggy. This is the gate.
    #[test]
    fn animation_frames_do_no_layout_work() {
        let report = report();
        let wasteful = report.wasteful_animation_frames();
        assert!(
            wasteful.is_empty(),
            "{} reveal/idle frames re-laid messages (first at {} received chars, {} relayouts)",
            wasteful.len(),
            wasteful[0].received,
            wasteful[0].relayouts
        );
    }

    /// A delta touches the tail message and nothing above it, so a delta frame
    /// lays out exactly one message. More means the cache broke and every
    /// delta is paying for the whole conversation.
    #[test]
    fn a_delta_relays_only_the_tail() {
        let report = report();
        let overworked = report.overworked_delta_frames();
        assert!(
            overworked.is_empty(),
            "{} delta frames laid out more than the tail (worst: {} relayouts)",
            overworked.len(),
            overworked
                .iter()
                .map(|sample| sample.relayouts)
                .max()
                .unwrap_or(0)
        );
    }

    /// No replayed frame suffers an order-of-magnitude blow-up. Loose on
    /// purpose, exactly like [`crate::profile::GATE_BUDGET_US`]: this runs on
    /// contended CI cores, and a flaky timing gate gets muted. The exact gates
    /// above are the real teeth.
    #[test]
    fn no_frame_is_grossly_over_budget() {
        let report = report();
        let gross = report.over(crate::profile::GATE_BUDGET_US);
        assert_eq!(
            gross,
            0,
            "{gross} frames over {}ms",
            crate::profile::GATE_BUDGET_US / 1000
        );
    }
}
