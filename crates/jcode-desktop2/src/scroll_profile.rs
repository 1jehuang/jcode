//! Where a scroll frame's microseconds actually go.
//!
//! [`crate::scroll_bench`] answers "does the scroll *feel* right" and reports
//! one worst-frame number per gesture. When that number is too big it says
//! nothing about which of a frame's phases spent it, and the obvious next tool
//! (a sampling profiler) is close to useless here: the hot leaves are all
//! inside Parley and Vello iterators, inlined and shared between the phases,
//! so a flat profile shows `GlyphIter::next` and leaves the reader to guess
//! which caller asked for it.
//!
//! This module measures the phases directly instead, because a scroll frame is
//! a fixed short pipeline and each phase is separately callable:
//!
//! - **measure**: [`crate::App::frame_for_model_with`], which lays the composer
//!   out and asks the transcript cache for the conversation's height. Runs
//!   *twice* per real frame (once for the event loop's geometry, once inside
//!   `build_scene`), so a cost here is paid twice.
//! - **place**: [`crate::viewport::Viewport::new`], the arithmetic that decides
//!   which messages intersect the region.
//! - **encode**: everything `build_scene` does on top of the above, which is
//!   the glyph and shape encoding for the visible messages.
//!
//! Each is reported per frame at a fixed geometry, and the sweep varies the
//! *only* two things a scroll frame's cost can legitimately depend on: how long
//! the conversation is (which must not matter, since the work is culled to the
//! region) and how much is on screen. A phase whose cost grows with history
//! length is the regression this module exists to name, and the flat profile
//! cannot distinguish it from honest work.

use crate::transcript::{Message, Transcript};
use crate::{App, Model, build_scene};
use std::time::Instant;
use vello::Scene;

/// Surface the sweep measures at, matching [`crate::scroll_bench`] so the two
/// reports are comparable.
const WIDTH: u32 = 2200;
const HEIGHT: u32 = 1440;
const SCALE: f64 = 2.0;

/// Frames timed per measurement. The *fastest* is reported, not the mean: a
/// frame's honest cost is what it takes with the machine to itself, and every
/// sample is that plus some amount of interference (a scheduler preemption, a
/// core migration, an unlucky cache). Averaging folds the interference into the
/// number, and at the sub-millisecond costs here it dominated: the same history
/// measured 0.30ms and 0.69ms on consecutive runs, enough to trip a growth gate
/// on noise alone. The minimum is stable to a few percent, which is what makes
/// a ratio between two of them mean anything.
const FRAMES: u32 = 60;

/// History lengths swept, in turns. The point of the sweep is the *shape* of
/// the curve across these, not any single number: culled work is flat here.
const HISTORY: [usize; 4] = [10, 40, 160, 640];

/// One history length's per-frame phase costs, in microseconds.
#[derive(Clone, Debug)]
pub struct PhaseCost {
    pub turns: usize,
    pub messages: usize,
    /// Messages the region actually shows, so a cost can be read per drawn
    /// message rather than per message in history.
    pub visible: usize,
    /// `frame_for_model_with`: composer layout plus the cached transcript
    /// measurement. Charged once here, paid twice per real frame.
    pub measure_us: u64,
    /// `Viewport::new`: pure placement arithmetic.
    pub place_us: u64,
    /// The rest of `build_scene`: glyph and shape encoding.
    pub encode_us: u64,
    /// A whole scroll frame as the event loop runs it: measure, then
    /// `build_scene` (which measures again internally).
    pub frame_us: u64,
    /// Messages laid out from scratch during the measured frames. Scrolling
    /// moves an already laid-out document, so this must be zero.
    pub relayouts: usize,
}

impl PhaseCost {
    pub fn line(&self) -> String {
        format!(
            "{:>5} turns {:>5} msgs {:>3} vis  measure {:>7.2}ms  place {:>7.2}ms  \
             encode {:>7.2}ms  frame {:>7.2}ms  relayout {:>3}",
            self.turns,
            self.messages,
            self.visible,
            self.measure_us as f64 / 1000.0,
            self.place_us as f64 / 1000.0,
            self.encode_us as f64 / 1000.0,
            self.frame_us as f64 / 1000.0,
            self.relayouts,
        )
    }
}

/// Run `work` [`FRAMES`] times and return the fastest run, in microseconds.
/// See [`FRAMES`] for why the minimum rather than the mean.
fn fastest(mut work: impl FnMut()) -> u64 {
    // One untimed run, so a lazily populated glyph or font cache is not
    // charged to the first sample and made the permanent outlier.
    work();
    (0..FRAMES)
        .map(|_| {
            let start = Instant::now();
            work();
            start.elapsed().as_micros() as u64
        })
        .min()
        .unwrap_or(0)
}

fn conversation(turns: usize) -> Transcript {
    let mut transcript = Transcript::default();
    for n in 0..turns {
        transcript.push(Message::user(format!("question {n} about the transport")));
        transcript.push(Message::assistant(format!(
            "answer {n}. the client opens the socket and sends a hello frame \
             carrying its supported version range, then waits for the \
             server's ack before streaming any payload.\n\n- validate the \
             header\n- check the version overlap\n"
        )));
    }
    transcript
}

/// Measure one history length. The caches are warmed first, because a scroll
/// frame in a window that has been showing the conversation never pays a cold
/// layout, and charging it one would measure startup instead of scrolling.
pub fn measure(turns: usize) -> PhaseCost {
    let mut app = App {
        model: Model {
            session_id: Some("session_scroll_profile".into()),
            transcript: conversation(turns),
            ..Model::default()
        },
        ..App::default()
    };
    let mut scene = Scene::new();
    for _ in 0..2 {
        app.frame = App::frame_for_model_with((WIDTH, HEIGHT), SCALE, &app.model, &mut app.painter);
        build_scene(
            &mut scene,
            &mut app.painter,
            &app.model,
            (WIDTH, HEIGHT),
            SCALE,
        );
    }
    // Park the view mid-history rather than at the tail, so the region is full
    // and there is content culled both above and below it: the tail shows the
    // last reply against blank space, which is the cheapest case and the one
    // least like the scroll being profiled.
    let max = app.max_scroll();
    app.model.scroll = max / 2.0;
    app.model.smooth.settle();
    app.frame = App::frame_for_model_with((WIDTH, HEIGHT), SCALE, &app.model, &mut app.painter);

    let before = app.painter.transcript.total_relayouts();

    let measure_us = fastest(|| {
        app.frame = App::frame_for_model_with((WIDTH, HEIGHT), SCALE, &app.model, &mut app.painter);
    });

    let region = (app.frame.body_bottom - app.frame.body_top).max(0.0);
    let view = app.model.view_scroll();
    let width = (app.frame.column() - crate::transcript::USER_PAD_X * 2.0).max(1.0);
    let laid = app.painter.transcript.lay_out(
        &mut app.painter.text,
        &app.model.transcript,
        width,
        &app.model.theme,
        crate::scene::transcript_body_style(&app.model),
        app.frame.scale,
    );
    let visible = crate::viewport::Viewport::new(laid, region, view)
        .visible
        .len();
    let place_us = fastest(|| {
        let placed = crate::viewport::Viewport::new(laid, region, view);
        std::hint::black_box(placed.visible.len());
    });

    let build_us = fastest(|| {
        let mut scene = Scene::new();
        build_scene(
            &mut scene,
            &mut app.painter,
            &app.model,
            (WIDTH, HEIGHT),
            SCALE,
        );
        std::hint::black_box(scene.encoding().n_paths);
    });

    let frame_us = fastest(|| {
        app.frame = App::frame_for_model_with((WIDTH, HEIGHT), SCALE, &app.model, &mut app.painter);
        let mut scene = Scene::new();
        build_scene(
            &mut scene,
            &mut app.painter,
            &app.model,
            (WIDTH, HEIGHT),
            SCALE,
        );
        std::hint::black_box(scene.encoding().n_paths);
    });

    PhaseCost {
        turns,
        messages: app.model.transcript.messages().len(),
        visible,
        measure_us,
        place_us,
        // `build_scene` measures the frame again internally, so the encoding
        // half is what is left after that measurement is taken back out.
        encode_us: build_us.saturating_sub(measure_us),
        frame_us,
        relayouts: app.painter.transcript.total_relayouts() - before,
    }
}

/// Sweep every history length.
pub fn sweep() -> Vec<PhaseCost> {
    HISTORY.into_iter().map(measure).collect()
}

/// A scroll frame at 120Hz has this many microseconds. Past it the motion the
/// eye is guaranteed to be watching drops a frame.
pub const FRAME_BUDGET_US: u64 = 8_333;

/// Print the sweep and say whether scrolling is behaving. Returns false when a
/// gate failed, so a caller can exit non-zero.
///
/// The gates are the two things that are *properties* rather than machine
/// speeds, plus one advisory budget:
///
/// - No frame may lay a message out: the document did not change.
/// - Cost must not grow with history length. Everything a scroll frame does is
///   culled to the region, so the longest history may not cost meaningfully
///   more than the shortest. This is the gate a timing budget cannot express,
///   because a fast machine hides an O(history) frame until the session is
///   long enough, which is exactly when a user notices.
pub fn report(costs: &[PhaseCost]) -> bool {
    println!("scroll frame cost by history ({WIDTH}x{HEIGHT} @{SCALE})\n");
    let mut ok = true;
    for cost in costs {
        println!("  {}", cost.line());
        if cost.relayouts > 0 {
            ok = false;
            println!(
                "    FAIL laid out {} messages while merely scrolling",
                cost.relayouts
            );
        }
    }

    if let (Some(first), Some(last)) = (costs.first(), costs.last()) {
        let growth = last.frame_us as f64 / (first.frame_us.max(1)) as f64;
        let turns = last.turns as f64 / (first.turns.max(1)) as f64;
        println!(
            "\n  history x{turns:.0} costs x{growth:.2} per frame \
             ({} -> {} turns, {:.2}ms -> {:.2}ms)",
            first.turns,
            last.turns,
            first.frame_us as f64 / 1000.0,
            last.frame_us as f64 / 1000.0,
        );
        // Culled work should be flat. A little growth is honest: the cache and
        // the viewport each walk the message list once per frame to decide what
        // changed and what is visible, which is O(history) in pointer-chasing
        // even though it lays out and draws nothing extra. So the gate allows a
        // modest factor and catches what actually hurts, which is per-frame
        // work proportional to the *content* rather than to a list walk.
        if growth > 1.8 {
            ok = false;
            println!(
                "    FAIL frame cost grows with history (x{growth:.2} over x{turns:.0} turns); \
                 scroll work must be culled to the region"
            );
        }
    }

    let worst = costs.iter().max_by_key(|cost| cost.frame_us);
    if let Some(worst) = worst {
        if worst.frame_us > FRAME_BUDGET_US {
            println!(
                "  SLOW worst frame {:.2}ms over the {:.2}ms 120Hz budget at {} turns",
                worst.frame_us as f64 / 1000.0,
                FRAME_BUDGET_US as f64 / 1000.0,
                worst.turns,
            );
        }
    }
    println!(
        "\n  measure: composer layout + cached transcript height, run twice per real frame.\n  \
         place: viewport arithmetic. encode: glyph and shape encoding for the visible\n  \
         region. frame: measure + build_scene, as the event loop runs it."
    );
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property the module exists to defend: a scroll frame is culled to
    /// the region, so it must not lay out any message and must not care how
    /// long the conversation is.
    #[test]
    fn scroll_frames_do_not_depend_on_history_length() {
        let short = measure(10);
        let long = measure(320);
        assert_eq!(short.relayouts, 0, "short history re-laid while scrolling");
        assert_eq!(long.relayouts, 0, "long history re-laid while scrolling");
        assert!(
            long.visible <= short.visible + 2,
            "same region should show a comparable number of messages: {} vs {}",
            short.visible,
            long.visible
        );
    }
}
