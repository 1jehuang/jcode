//! Scroll feel, measured instead of felt.
//!
//! "The scrollwheel feels wrong" is the least actionable bug report in this
//! codebase, and the one that keeps coming back: the constants in
//! [`crate::scroll`] were each tuned against somebody's impression of one
//! gesture on one machine, so tightening one of them silently loosens another.
//! [`crate::stream_bench`] fixed the same problem for streaming by replaying a
//! scripted stream through the real per-frame work; this does it for input.
//!
//! The replay is faithful to the event loop: wheel and trackpad events are fed
//! through the same `App` handlers the window uses, then each simulated frame
//! runs the `RedrawRequested` body (advance the smoothing, apply momentum,
//! measure the frame, build the scene) and records where the view was actually
//! drawn. Time is simulated, so every *quality* number below is exact on any
//! machine; only the microsecond costs vary.
//!
//! What the numbers are for, in the order they matter to the hand:
//!
//! - **latency**: frames between the event and the first pixel of movement. A
//!   scroll that answers late reads as a heavy page no matter how pretty the
//!   ease is.
//! - **tracking error**: during a trackpad drag, how far the drawn view is
//!   from the finger's own travel. A trackpad is a position input: anything
//!   other than 1:1 while the fingers are down is the page fighting the hand.
//! - **travel ratio**: total drawn travel over total input travel. Above 1 for
//!   a *drag* means momentum is being integrated on top of the finger's own
//!   deltas, which is the double-count bug that keeps reappearing.
//! - **reversals**: frames that moved against the gesture. Any is a visible
//!   stutter, and the exponential lag plus a fling can produce them.
//! - **jerk**: the third derivative of position, peak over the gesture. This
//!   is what separates a glide from a sequence of teleports, and it is the
//!   number a notch-per-line wheel fails.
//! - **wasted frames**: frames that repainted without moving the view, and
//!   frames that re-laid out text while merely scrolling. Both are cost with
//!   nothing on screen to show for it.

use crate::transcript::Message;
use crate::{App, Model, build_scene};
use std::time::{Duration, Instant};
use vello::Scene;

/// Surface the replay runs at. Fixed for the same reason [`crate::profile`]
/// fixes it: a scroll is only comparable against one wrap width.
const WIDTH: u32 = 2200;
const HEIGHT: u32 = 1440;
const SCALE: f64 = 2.0;

/// Simulated frame cadences, in milliseconds per frame.
///
/// The replay used to run only at 8ms, matching [`crate::scroll::FRAME`], on
/// the assumption that a faster cadence is a strictly harder test. It is not,
/// and that assumption is what let a real problem hide: the ease and the
/// friction here are exponential decays evaluated per frame, so *how much of
/// the gesture each frame gets* changes with the display. A 60Hz panel (which
/// is what most laptops, including the one this was tuned on, actually run)
/// gets half as many frames to resolve the same glide, so it sees twice the lag
/// per frame and coarser motion. Anything that is frame-rate dependent rather
/// than time dependent shows up as a difference between these rows, and a
/// single-cadence bench cannot show it at all.
const CADENCES: [u64; 2] = [8, 16];

/// Cadence the quality gates are enforced at. 16ms, not 8: gating the faster
/// display and merely reporting the slower one would pass a scroll that only
/// feels right on hardware the user does not have.
const GATE_MS: u64 = 16;

/// Hard cap on replayed frames, so a fling that never comes to rest fails
/// loudly rather than hanging the bench.
const MAX_FRAMES: usize = 20_000;

/// Movement below this many logical pixels in a frame is not visible motion.
const MOVED_EPSILON: f64 = 0.05;

/// One scripted input gesture, in the terms the *device* produces rather than
/// the terms the model uses: this is what the event loop is handed.
#[derive(Clone, Debug, PartialEq)]
pub enum Gesture {
    /// Discrete wheel notches, `count` of them one every `every_ms`.
    /// Positive travels toward older content, like a scroll up.
    Notches { count: usize, every_ms: u64 },
    /// A trackpad drag: `steps` pixel-delta events of `pixels` each, one every
    /// `every_ms`, bracketed by the backend's gesture phase.
    Drag {
        steps: usize,
        pixels: f64,
        every_ms: u64,
        /// Whether the backend reports the phase (Wayland/X11/macOS do). False
        /// exercises the timeout fallback.
        phased: bool,
    },
    /// Hold still with the fingers down for `ms`. Only meaningful inside a
    /// phased drag, and it is the case that used to fling under the hand.
    Hold { ms: u64 },
    /// Nothing at all for `ms`: where a fling coasts and the view settles.
    Coast { ms: u64 },
}

/// A named script of gestures, plus how much room the page has to move in.
pub struct Script {
    pub name: &'static str,
    pub history_turns: usize,
    pub gestures: Vec<Gesture>,
    /// Where the view starts, in logical pixels from the tail. `None` starts
    /// at the tail, which is where a real session sits.
    pub start_scroll: Option<f64>,
}

impl Script {
    /// The gestures a hand actually makes, each chosen because it has broken
    /// before: a single notch (the smallest thing the wheel can say), a
    /// notch burst (where per-notch eases pile up), a steady drag (1:1
    /// tracking), a drag with a pause in it (the fling-under-the-finger bug),
    /// a flick (momentum), and a flick into the top edge (grinding).
    pub fn suite() -> Vec<Script> {
        vec![
            Script {
                name: "one notch",
                history_turns: 40,
                gestures: vec![
                    Gesture::Notches {
                        count: 1,
                        every_ms: 0,
                    },
                    Gesture::Coast { ms: 400 },
                ],
                start_scroll: Some(2_000.0),
            },
            Script {
                name: "notch burst",
                history_turns: 40,
                gestures: vec![
                    Gesture::Notches {
                        count: 10,
                        every_ms: 24,
                    },
                    Gesture::Coast { ms: 500 },
                ],
                start_scroll: Some(2_000.0),
            },
            Script {
                name: "steady drag",
                history_turns: 40,
                gestures: vec![
                    Gesture::Drag {
                        steps: 40,
                        pixels: 12.0,
                        every_ms: 8,
                        phased: true,
                    },
                    Gesture::Coast { ms: 600 },
                ],
                start_scroll: Some(2_000.0),
            },
            Script {
                name: "drag with a pause",
                history_turns: 40,
                gestures: vec![
                    Gesture::Drag {
                        steps: 20,
                        pixels: 14.0,
                        every_ms: 8,
                        phased: true,
                    },
                    Gesture::Hold { ms: 300 },
                    Gesture::Drag {
                        steps: 20,
                        pixels: 14.0,
                        every_ms: 8,
                        phased: true,
                    },
                    Gesture::Coast { ms: 600 },
                ],
                start_scroll: Some(2_000.0),
            },
            Script {
                name: "flick",
                history_turns: 60,
                gestures: vec![
                    Gesture::Drag {
                        steps: 8,
                        pixels: 40.0,
                        every_ms: 8,
                        phased: true,
                    },
                    Gesture::Coast { ms: 1_200 },
                ],
                start_scroll: Some(2_000.0),
            },
            Script {
                name: "flick into the top",
                history_turns: 8,
                gestures: vec![
                    Gesture::Drag {
                        steps: 8,
                        pixels: 60.0,
                        every_ms: 8,
                        phased: true,
                    },
                    Gesture::Coast { ms: 1_200 },
                ],
                start_scroll: None,
            },
            Script {
                name: "phaseless drag",
                history_turns: 40,
                gestures: vec![
                    Gesture::Drag {
                        steps: 20,
                        pixels: 20.0,
                        every_ms: 8,
                        phased: false,
                    },
                    Gesture::Coast { ms: 1_000 },
                ],
                start_scroll: Some(2_000.0),
            },
        ]
    }
}

/// One replayed frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameSample {
    /// Simulated milliseconds since the script started.
    pub at_ms: u64,
    /// Where the frame was drawn, in logical pixels from the tail.
    pub view: f64,
    /// The logical scroll the model holds, which selection and hit-testing use.
    pub logical: f64,
    /// Input travel delivered so far, in logical pixels, signed.
    pub input: f64,
    /// Whether the user's fingers were still on the surface.
    pub gesturing: bool,
    /// CPU cost of the frame's work, in microseconds.
    pub us: u64,
    /// Messages laid out from scratch this frame. Scrolling moves an already
    /// laid-out document, so this must be zero.
    pub relayouts: usize,
}

/// Everything one script says about the scroll's feel.
pub struct Report {
    pub name: &'static str,
    /// Cadence this replay ran at, in milliseconds per frame.
    pub frame_ms: u64,
    pub samples: Vec<FrameSample>,
    /// Simulated milliseconds from the first input event to the first frame
    /// that drew the view somewhere new.
    pub latency_ms: Option<u64>,
    /// Simulated milliseconds from the last input event to the last frame that
    /// moved, i.e. how long the gesture keeps going after the hand stops.
    pub settle_ms: Option<u64>,
}

impl Report {
    fn moved(a: &FrameSample, b: &FrameSample) -> f64 {
        b.view - a.view
    }

    /// Per-frame drawn movement, in logical pixels.
    fn steps(&self) -> Vec<f64> {
        self.samples
            .windows(2)
            .map(|pair| Self::moved(&pair[0], &pair[1]))
            .collect()
    }

    /// Total drawn travel over total input travel. A drag should be 1.0 (the
    /// page goes exactly as far as the fingers), a flick above 1.0 by the
    /// momentum, and *never* below 1.0 unless an edge swallowed the travel.
    pub fn travel_ratio(&self) -> Option<f64> {
        let first = self.samples.first()?;
        let last = self.samples.last()?;
        let input = last.input - first.input;
        (input.abs() > f64::EPSILON).then(|| (last.view - first.view) / input)
    }

    /// Drawn travel over input travel *while the fingers were down*. This is
    /// the number that catches momentum being integrated on top of the user's
    /// own deltas: during a drag the page must go exactly as far as the hand
    /// did, so anything above 1 here is travel the user did not ask for. The
    /// coast after release is deliberately excluded, because there the extra
    /// travel is the feature.
    pub fn drag_travel_ratio(&self) -> Option<f64> {
        let mut first = None;
        let mut last = None;
        for sample in self.samples.iter().filter(|sample| sample.gesturing) {
            first.get_or_insert(sample);
            last = Some(sample);
        }
        let (first, last) = (first?, last?);
        let input = last.input - first.input;
        (input.abs() > f64::EPSILON).then(|| (last.view - first.view) / input)
    }

    /// The worst frame during a fingers-down gesture where the drawn view
    /// disagreed with where the fingers had asked it to be. A trackpad is a
    /// position input, so this is the number that decides whether the page is
    /// stuck to the hand.
    pub fn peak_tracking_error(&self) -> f64 {
        let Some(first) = self.samples.first() else {
            return 0.0;
        };
        self.samples
            .iter()
            .filter(|sample| sample.gesturing)
            .map(|sample| {
                let asked = sample.input - first.input;
                let got = sample.view - first.view;
                (got - asked).abs()
            })
            .fold(0.0, f64::max)
    }

    /// Frames that moved against the gesture's direction. A one-way gesture
    /// that draws a backwards frame is a visible stutter.
    pub fn reversals(&self) -> usize {
        let steps = self.steps();
        let net: f64 = steps.iter().sum();
        if net == 0.0 {
            return 0;
        }
        let forward = net.signum();
        steps
            .iter()
            .filter(|step| step.abs() > MOVED_EPSILON && step.signum() != forward)
            .count()
    }

    /// Peak per-frame *change* in speed, in logical pixels per frame squared.
    /// A glide changes speed gently; a teleport spikes here, which is exactly
    /// what a coarse wheel notch applied raw looks like to the eye.
    pub fn peak_jerk(&self) -> f64 {
        self.steps()
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f64::max)
    }

    /// Frames that repainted without moving the view. Some are legitimate (the
    /// scrollbar is fading), so this is reported rather than gated: what must
    /// not happen is *many* of them while nothing is visibly changing.
    pub fn still_frames(&self) -> usize {
        self.steps()
            .iter()
            .filter(|step| step.abs() <= MOVED_EPSILON)
            .count()
    }

    /// Frames that laid text out again merely to scroll. Must be zero: the
    /// document did not change, only the offset it is drawn at.
    pub fn relayout_frames(&self) -> usize {
        self.samples
            .iter()
            .filter(|sample| sample.relayouts > 0)
            .count()
    }

    /// Worst frame cost, in microseconds. At 8'333 a 120Hz frame is missed,
    /// and a missed frame during a scroll is the one place the eye is
    /// guaranteed to be watching motion.
    pub fn max_us(&self) -> u64 {
        self.samples
            .iter()
            .map(|sample| sample.us)
            .max()
            .unwrap_or(0)
    }

    /// Which frame the worst cost landed on, and the typical frame's cost.
    ///
    /// Reported together with [`Self::max_us`] because on its own a worst-frame
    /// number is unreadable: a 7ms max looks like a dropped frame every time,
    /// but if it lands on frame 0 while the median frame is a tenth of that, it
    /// is the replay's own warm-up (first allocation of the scene buffers, the
    /// glyph atlas filling) and not something a hand can feel. A max that lands
    /// mid-gesture with a median near it is the real thing.
    pub fn worst_frame(&self) -> usize {
        self.samples
            .iter()
            .enumerate()
            .max_by_key(|(_, sample)| sample.us)
            .map_or(0, |(index, _)| index)
    }

    /// Median frame cost, in microseconds: what a scroll frame usually costs.
    pub fn median_us(&self) -> u64 {
        let mut costs: Vec<u64> = self.samples.iter().map(|sample| sample.us).collect();
        if costs.is_empty() {
            return 0;
        }
        costs.sort_unstable();
        costs[costs.len() / 2]
    }

    /// How far the view ended up from the logical position. The model's
    /// `scroll` is what selection and hit-testing read, so a scroll that
    /// finishes with the two apart means a click lands on the wrong glyph.
    pub fn final_lag(&self) -> f64 {
        self.samples
            .last()
            .map_or(0.0, |sample| (sample.view - sample.logical).abs())
    }

    pub fn line(&self) -> String {
        format!(
            "{:<22} latency {:>5} settle {:>6} ratio {:>6} track {:>6.1}px \
             drag {:>6} rev {:>2} jerk {:>6.1} still {:>4} relayout {:>3} \
             mid {:>5}us max {:>6}us@f{:<4} lag {:>5.2}",
            self.name,
            self.latency_ms
                .map_or_else(|| "-".into(), |ms| format!("{ms}ms")),
            self.settle_ms
                .map_or_else(|| "-".into(), |ms| format!("{ms}ms")),
            self.travel_ratio()
                .map_or_else(|| "-".into(), |r| format!("{r:.2}")),
            self.peak_tracking_error(),
            self.drag_travel_ratio()
                .map_or_else(|| "-".into(), |r| format!("{r:.2}")),
            self.reversals(),
            self.peak_jerk(),
            self.still_frames(),
            self.relayout_frames(),
            self.median_us(),
            self.max_us(),
            self.worst_frame(),
            self.final_lag(),
        )
    }
}

/// A flattened script: one entry per simulated millisecond boundary at which
/// something is delivered to the app.
enum Event {
    Notch,
    Pixels {
        pixels: f64,
        phased: bool,
    },
    /// End of a phased gesture: the axis stop the compositor sends.
    Release,
}

fn schedule(gestures: &[Gesture]) -> (Vec<(u64, Event)>, u64) {
    let mut out: Vec<(u64, Event)> = Vec::new();
    let mut at = 0u64;
    let mut open_phase = false;
    for gesture in gestures {
        match *gesture {
            Gesture::Notches { count, every_ms } => {
                for _ in 0..count {
                    out.push((at, Event::Notch));
                    at += every_ms;
                }
            }
            Gesture::Drag {
                steps,
                pixels,
                every_ms,
                phased,
            } => {
                for _ in 0..steps {
                    out.push((at, Event::Pixels { pixels, phased }));
                    at += every_ms;
                }
                open_phase = phased;
            }
            // A hold is the fingers staying down: the phase is not released,
            // and no deltas arrive.
            Gesture::Hold { ms } => at += ms,
            Gesture::Coast { ms } => {
                // The hand leaving the pad is an event in its own right, and
                // on a phased backend it is what starts the fling.
                if open_phase {
                    out.push((at, Event::Release));
                    open_phase = false;
                }
                at += ms;
            }
        }
    }
    (out, at)
}

/// Replay one script at [`GATE_MS`], the cadence the gates apply to.
#[cfg(test)]
pub fn run(script: &Script) -> Report {
    run_at(script, GATE_MS)
}

/// Replay one script at a given frame cadence and measure every frame.
pub fn run_at(script: &Script, frame_ms: u64) -> Report {
    let mut app = App {
        model: Model {
            session_id: Some("session_scroll_bench".into()),
            ..Model::default()
        },
        ..App::default()
    };
    for n in 0..script.history_turns {
        app.model
            .transcript
            .push(Message::user(format!("question {n} about the transport")));
        app.model.transcript.push(Message::assistant(format!(
            "answer {n}. the client opens the socket and sends a hello frame \
             carrying its supported version range, then waits for the \
             server's ack before streaming any payload.\n\n- validate the \
             header\n- check the version overlap\n"
        )));
    }

    // Warm the caches and record the geometry, the way a window that has been
    // showing this conversation already has. Charging the first scroll frame
    // for a cold layout would measure startup, not scrolling.
    app.frame = App::frame_for_model_with((WIDTH, HEIGHT), SCALE, &app.model, &mut app.painter);
    let mut scene = Scene::new();
    build_scene(
        &mut scene,
        &mut app.painter,
        &app.model,
        (WIDTH, HEIGHT),
        SCALE,
    );
    let max = app.max_scroll();
    if let Some(start) = script.start_scroll {
        app.model.scroll = start.min(max);
    }
    // Whatever the start was, the view begins settled: a bench that opened
    // mid-ease would attribute the previous jump's motion to this script.
    app.model.smooth.settle();

    let (events, script_ms) = schedule(&script.gestures);
    let line = app.frame.body_line_height() * crate::WHEEL_LINES;

    let epoch = Instant::now();
    let mut samples: Vec<FrameSample> = Vec::new();
    let mut cursor = 0usize;
    let mut input = 0.0f64;
    let mut gesturing = false;
    let mut first_event_ms: Option<u64> = None;
    let mut last_event_ms: Option<u64> = None;
    let mut sim_ms = 0u64;

    loop {
        if samples.len() >= MAX_FRAMES {
            break;
        }
        let now = epoch + Duration::from_millis(sim_ms);

        // Deliver everything due, through the same calls the window event
        // handler makes for a `MouseWheel`.
        while let Some((at, event)) = events.get(cursor) {
            if *at > sim_ms {
                break;
            }
            first_event_ms.get_or_insert(*at);
            last_event_ms = Some(*at);
            match event {
                Event::Notch => {
                    let max = app.max_scroll();
                    app.model.scroll_up(line, max);
                    input += line;
                }
                Event::Pixels { pixels, phased } => {
                    if *phased {
                        app.model.smooth.gesture_held(true);
                        gesturing = true;
                    }
                    let max = app.max_scroll();
                    if !app.model.scroll_gesture(*pixels, max, now) {
                        app.model.smooth.stop();
                    }
                    input += pixels;
                }
                Event::Release => {
                    app.model.smooth.gesture_held(false);
                    gesturing = false;
                }
            }
            cursor += 1;
        }

        let relayouts_before = app.painter.transcript.total_relayouts();
        let start = Instant::now();

        // The RedrawRequested body, minus the GPU.
        app.model.smooth.advance(now);
        if app.model.has_momentum() {
            let max = app.max_scroll();
            app.model.apply_momentum(max);
        }
        app.frame = App::frame_for_model_with((WIDTH, HEIGHT), SCALE, &app.model, &mut app.painter);
        let mut scene = Scene::new();
        build_scene(
            &mut scene,
            &mut app.painter,
            &app.model,
            (WIDTH, HEIGHT),
            SCALE,
        );

        samples.push(FrameSample {
            at_ms: sim_ms,
            view: app.model.view_scroll(),
            logical: app.model.scroll,
            input,
            gesturing,
            us: start.elapsed().as_micros() as u64,
            relayouts: app.painter.transcript.total_relayouts() - relayouts_before,
        });

        let done =
            cursor >= events.len() && sim_ms >= script_ms && !app.model.smooth.is_animating();
        if done {
            break;
        }
        sim_ms += frame_ms;
    }

    let latency_ms = first_event_ms.and_then(|from| {
        let base = samples
            .iter()
            .find(|sample| sample.at_ms >= from)
            .map(|sample| sample.view)?;
        samples
            .iter()
            .find(|sample| sample.at_ms >= from && (sample.view - base).abs() > MOVED_EPSILON)
            .map(|sample| sample.at_ms.saturating_sub(from))
    });
    let settle_ms = last_event_ms.and_then(|from| {
        samples
            .windows(2)
            .filter(|pair| (pair[1].view - pair[0].view).abs() > MOVED_EPSILON)
            .map(|pair| pair[1].at_ms)
            .next_back()
            .map(|at| at.saturating_sub(from))
    });

    Report {
        name: script.name,
        frame_ms,
        samples,
        latency_ms,
        settle_ms,
    }
}

/// Replay the whole suite at every cadence in [`CADENCES`].
pub fn sweep() -> Vec<Report> {
    CADENCES
        .into_iter()
        .flat_map(|frame_ms| {
            Script::suite()
                .into_iter()
                .map(move |script| run_at(&script, frame_ms))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Print the sweep and say whether the scroll is behaving. Returns false when
/// a gate failed, so a caller can exit non-zero.
pub fn report(reports: &[Report]) -> bool {
    println!("scroll feel ({WIDTH}x{HEIGHT} @{SCALE})\n");
    let mut ok = true;
    let mut cadence = None;
    for report in reports {
        if cadence != Some(report.frame_ms) {
            cadence = Some(report.frame_ms);
            let hz = 1000.0 / report.frame_ms as f64;
            let gated = if report.frame_ms == GATE_MS {
                " (gated)"
            } else {
                " (reported)"
            };
            println!("  at {}ms/frame, {hz:.0}Hz{gated}", report.frame_ms);
        }
        println!("    {}", report.line());
        // Only the gated cadence fails the run. The other is printed so a
        // frame-rate dependence is visible as a difference between the blocks,
        // without making the suite's verdict depend on which display the
        // reader happens to own.
        if report.frame_ms != GATE_MS {
            continue;
        }
        for failure in gate(report) {
            ok = false;
            println!("      FAIL {failure}");
        }
    }
    // The gates above judge each cadence alone, which is not enough: the
    // scroll's *behaviour* must not depend on the display, and the way that
    // dependence appeared was a gesture that measured 1.97x travel at 8ms and
    // 3.39x at 16ms, from a velocity estimate that divided one delta by the
    // microseconds between two batched events. Nothing about either row on its
    // own was obviously wrong; only the disagreement was. So the disagreement
    // is the gate.
    for pair in pairs(reports) {
        let (fast, slow) = pair;
        if let (Some(a), Some(b)) = (fast.travel_ratio(), slow.travel_ratio()) {
            // Loose, because a coarser cadence legitimately quantises the tail
            // of a coast by up to a frame's worth of travel.
            if (a - b).abs() > 0.25 * a.abs().max(1.0) {
                ok = false;
                println!(
                    "  FAIL {}: travelled {a:.2}x at {}ms/frame but {b:.2}x at {}ms/frame; \
                     the scroll must not depend on the display",
                    fast.name, fast.frame_ms, slow.frame_ms,
                );
            }
        }
    }

    println!(
        "\n  latency: to first drawn movement. settle: motion after the last event.\n  \
         ratio: drawn travel / input travel, whole script; drag: the same while\n  \
         the fingers were down, which must be 1.00. track: worst fingers-down\n  \
         disagreement. rev: frames moving backwards. jerk: peak per-frame speed change.\n  \
         mid: the frame cost a hand actually feels. max@fN: worst frame and where it\n  \
         landed; on frame 0 it is the replay warming its own buffers, not a stutter."
    );
    ok
}

/// Pair each script's report across cadences, so the same gesture measured on
/// two displays can be compared.
fn pairs(reports: &[Report]) -> Vec<(&Report, &Report)> {
    let mut out = Vec::new();
    for (index, fast) in reports.iter().enumerate() {
        if fast.frame_ms != CADENCES[0] {
            continue;
        }
        if let Some(slow) = reports[index + 1..]
            .iter()
            .find(|other| other.name == fast.name && other.frame_ms != fast.frame_ms)
        {
            out.push((fast, slow));
        }
    }
    out
}

/// The rules a good scroll obeys, as assertions rather than prose. These are
/// deterministic: they read simulated time and exact pixel positions, so a
/// failure here is a real regression and not a busy machine.
pub fn gate(report: &Report) -> Vec<String> {
    let mut failures = Vec::new();
    // Answering an event late is the one thing no amount of easing can hide.
    // One frame of latency is the floor (the event arrives, the next frame
    // draws it); two is the budget.
    if report.latency_ms.is_none_or(|ms| ms > 2 * report.frame_ms) {
        failures.push(format!(
            "slow to answer: {:?} to first movement",
            report.latency_ms
        ));
    }
    // While the fingers are down the page is a position input, full stop. Any
    // divergence here is momentum leaking under the hand or the ease holding
    // the page back, and both read as the page fighting the user.
    if report
        .drag_travel_ratio()
        .is_some_and(|ratio| (ratio - 1.0).abs() > 0.05)
    {
        failures.push(format!(
            "the page went {:.2}x the fingers' own travel while they were down",
            report.drag_travel_ratio().unwrap_or_default()
        ));
    }
    if report.reversals() > 0 {
        failures.push(format!(
            "{} frames moved against the gesture",
            report.reversals()
        ));
    }
    if report.relayout_frames() > 0 {
        failures.push(format!(
            "{} frames re-laid out text just to scroll",
            report.relayout_frames()
        ));
    }
    // A scroll that ends with the drawn view away from the logical position
    // puts every click on the wrong glyph.
    if report.final_lag() > 0.5 {
        failures.push(format!(
            "ended {:.2}px from the logical scroll",
            report.final_lag()
        ));
    }
    // Momentum must not keep the window awake forever, but the bound has to be
    // a browser's, not a stricter guess: Chrome and Firefox both carry a real
    // flick past a second, and the earlier 1s ceiling here was what made this
    // scroll feel like it stopped the moment you let go. Past ~1.6s the view is
    // sliding long after the hand has moved on, which reads as the page
    // ignoring the user rather than as momentum.
    if report.settle_ms.is_some_and(|ms| ms > 1_600) {
        failures.push(format!(
            "still moving {}ms after the last event",
            report.settle_ms.unwrap()
        ));
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report_for(name: &str) -> Report {
        let script = Script::suite()
            .into_iter()
            .find(|script| script.name == name)
            .expect("no such script");
        run(&script)
    }

    /// The replay must actually move the page. A bench measuring a stuck view
    /// would report the scroll as perfect forever.
    #[test]
    fn the_replay_scrolls() {
        for report in sweep() {
            let first = report.samples.first().expect("no frames");
            let last = report.samples.last().expect("no frames");
            assert!(
                (last.view - first.view).abs() > 1.0,
                "{}: the view never moved",
                report.name
            );
        }
    }

    /// Every script obeys the rules. This is the gate: it is the whole point
    /// of the module, and it is exact on any machine.
    #[test]
    fn the_scroll_behaves_on_every_gesture() {
        for report in sweep() {
            let failures = gate(&report);
            assert!(
                failures.is_empty(),
                "{}: {}\n  {}",
                report.name,
                failures.join("; "),
                report.line()
            );
        }
    }

    /// While the fingers are down, a trackpad is a position input: the page
    /// goes exactly as far as the hand did. The exponential lag is allowed to
    /// hold the view back briefly, but not by more than a couple of lines, or
    /// the page reads as detached from the fingers.
    #[test]
    fn a_drag_tracks_the_fingers() {
        let report = report_for("steady drag");
        assert!(
            report.peak_tracking_error() < 60.0,
            "the page lagged the fingers by {:.1}px\n  {}",
            report.peak_tracking_error(),
            report.line()
        );
    }

    /// A pause mid-drag must not add travel of its own: the finger is still
    /// down, so the only thing moving the page is the finger. This is the
    /// double-count bug that made trackpad scrolling feel like a fight.
    #[test]
    fn a_pause_mid_drag_adds_no_travel() {
        let report = report_for("drag with a pause");
        let ratio = report.drag_travel_ratio().expect("no fingers-down travel");
        assert!(
            ratio < 1.05,
            "a paused drag travelled {ratio:.2}x the fingers' own distance\n  {}",
            report.line()
        );
    }

    /// A flick coasts: the page carries on well past where the fingers let go,
    /// and then stops. Without this the transcript feels like it is bolted down.
    ///
    /// The floor is 4x rather than a token 1.2x because 1.2x *passed* while the
    /// scroll still felt like it had no momentum at all: a bound that weak
    /// cannot tell a browser-like fling from a stroke that merely finishes.
    #[test]
    fn a_flick_carries_past_the_fingers() {
        let report = report_for("flick");
        let ratio = report.travel_ratio().expect("no input travel");
        assert!(
            ratio > 4.0,
            "a flick barely coasted: {ratio:.2}x the input travel\n  {}",
            report.line()
        );
    }

    /// Coasting into the top of the history stops there instead of grinding
    /// against the clamp for the rest of the fling's life.
    #[test]
    fn a_flick_into_the_edge_stops_grinding() {
        let report = report_for("flick into the top");
        let settled = report.settle_ms.unwrap_or(0);
        assert!(
            settled < 400,
            "the fling ground against the top edge for {settled}ms\n  {}",
            report.line()
        );
    }

    /// A wheel notch is the coarsest thing the user can say, and applied raw
    /// it teleports the page by a chunk. The ease exists to turn that into a
    /// glide, so no single frame may carry most of the notch.
    #[test]
    fn a_notch_glides_rather_than_teleports() {
        let report = report_for("one notch");
        let first = report.samples.first().expect("no frames");
        let last = report.samples.last().expect("no frames");
        let total = (last.view - first.view).abs();
        let biggest = report
            .steps()
            .iter()
            .map(|step| step.abs())
            .fold(0.0, f64::max);
        assert!(
            biggest < total * 0.5,
            "one frame carried {biggest:.1}px of a {total:.1}px notch\n  {}",
            report.line()
        );
    }

    /// A burst of notches is one continuous movement to the eye, so the eases
    /// must compose into a glide rather than a staircase. Ten notches at 24ms
    /// is a normal hand on a normal wheel.
    #[test]
    fn a_notch_burst_stays_smooth() {
        let report = report_for("notch burst");
        let steps = report.steps();
        let moving: Vec<f64> = steps
            .iter()
            .copied()
            .filter(|step| step.abs() > MOVED_EPSILON)
            .collect();
        assert!(moving.len() > 20, "the burst was not a glide at all");
        // Peak jerk against mean speed: a staircase spikes, a glide does not.
        let mean = moving.iter().map(|step| step.abs()).sum::<f64>() / moving.len() as f64;
        assert!(
            report.peak_jerk() < mean * 6.0,
            "the burst stepped rather than glided: peak jerk {:.1} vs mean step {mean:.1}\n  {}",
            report.peak_jerk(),
            report.line()
        );
    }

    /// Scrolling moves an unchanged document, so it must never cost a layout.
    /// This is the cheap-frame invariant that keeps a fling at full rate.
    #[test]
    fn scrolling_never_relays_out_text() {
        for report in sweep() {
            assert_eq!(
                report.relayout_frames(),
                0,
                "{}: scrolling re-laid out text\n  {}",
                report.name,
                report.line()
            );
        }
    }

    /// A backend that reports no phase still coasts, via the timeout fallback.
    #[test]
    fn a_phaseless_backend_still_coasts() {
        let report = report_for("phaseless drag");
        let ratio = report.travel_ratio().expect("no input travel");
        assert!(
            ratio > 1.0,
            "a phase-less drag did not coast at all: {ratio:.2}x\n  {}",
            report.line()
        );
    }
}
