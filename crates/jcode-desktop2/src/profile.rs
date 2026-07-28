//! Deterministic frame-cost profiling over the state space.
//!
//! The way a performance problem used to be found here was that someone used
//! the app, felt it lag, and said so. That only catches what a person happens
//! to touch, only after it ships, and it produces an impression rather than a
//! number.
//!
//! `build_scene` is a pure function of [`Model`], and [`crate::states`] already
//! enumerates the app's visual states for capture. That makes frame cost a
//! function this module can simply *evaluate*, over every state, with no
//! window, no compositor, no GPU, and no clock. The result is one comparable
//! number per state, which is the thing a person watching a window can never
//! produce.
//!
//! Two numbers are reported per node, because they answer different questions:
//!
//! - **cold**: the first frame after a change, with every cache empty. This is
//!   what a resize, a theme switch, or the first paint costs.
//! - **warm**: the steady-state frame, which is what almost every frame really
//!   is (a caret blink, an animation tick, a scroll, one streamed delta). This
//!   is the number that decides whether the window feels alive, and the one
//!   that regressed when the transcript was re-laid on every frame.

use crate::paint::Painter;
use crate::{Model, build_scene, states};
use vello::Scene;

/// Surface the sweep measures at. Fixed, because a cost is only comparable
/// against the same geometry: a different window size wraps differently and
/// lays out a different number of lines.
const WIDTH: u32 = 2200;
const HEIGHT: u32 = 1440;
const SCALE: f64 = 2.0;

/// Frames averaged for the warm measurement. Enough to average out scheduler
/// noise, few enough that the whole sweep stays a second or two.
const WARM_FRAMES: u32 = 20;

/// One state's measured frame cost, in microseconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeCost {
    pub name: String,
    /// First frame with empty caches, in microseconds.
    pub cold_us: u64,
    /// Steady-state frame, in microseconds. The number that decides whether
    /// the window feels responsive.
    pub warm_us: u64,
    /// Messages the steady-state frame had to lay out from scratch.
    ///
    /// The deterministic half of the signal. Timing depends on the machine, so
    /// a timing gate must leave slack and only catches gross regressions. This
    /// counts *work*, so it is exact and reproducible: a steady-state frame
    /// that lays out any message at all is redoing work it already did, which
    /// is precisely the bug that made the window lag. A timing-only gate would
    /// have let that back in on a fast machine.
    pub warm_relayouts: usize,
    /// Messages in this state, so a relayout count can be read in proportion.
    pub messages: usize,
}

/// Measure one model's cold and warm frame cost.
///
/// The painter is created here rather than passed in, so "cold" really is
/// cold: sharing one across nodes would let the first node pay for font
/// loading and report every later node as artificially fast.
pub fn measure(model: &Model) -> (u64, u64, usize) {
    let mut painter = Painter::default();
    let frame = |painter: &mut Painter| {
        let mut scene = Scene::new();
        build_scene(&mut scene, painter, model, (WIDTH, HEIGHT), SCALE);
    };

    let start = std::time::Instant::now();
    frame(&mut painter);
    let cold = start.elapsed();

    // One more frame before timing: the cold pass also loads fonts and grows
    // allocator arenas, which belong to the cold number and would otherwise
    // smear into the first warm sample.
    frame(&mut painter);

    let start = std::time::Instant::now();
    for _ in 0..WARM_FRAMES {
        frame(&mut painter);
    }
    let warm = start.elapsed() / WARM_FRAMES;
    let (_, relayouts) = painter.transcript.stats();

    (cold.as_micros() as u64, warm.as_micros() as u64, relayouts)
}

/// Measure every node in the state space, slowest warm frame first.
pub fn sweep() -> Vec<NodeCost> {
    let mut costs: Vec<NodeCost> = states::names()
        .into_iter()
        .filter_map(|name| {
            let model = states::by_name(name)?;
            let (cold_us, warm_us, warm_relayouts) = measure(&model);
            let messages = model.transcript.messages().len();
            Some(NodeCost {
                name: name.to_string(),
                cold_us,
                warm_us,
                warm_relayouts,
                messages,
            })
        })
        .collect();
    costs.sort_by_key(|cost| std::cmp::Reverse(cost.warm_us));
    costs
}

/// Frame budget for the interactive report, in microseconds of scene building.
///
/// 16ms is one frame at 60Hz, and that budget belongs to the compositor and the
/// GPU as much as to us, so scene building should take a fraction of it.
/// Exceeding this is not a dropped frame by itself; it means there is no
/// headroom left, which is what produces visible lag as soon as anything else
/// contends.
pub const WARM_BUDGET_US: u64 = 4_000;

/// Budget the *automated gate* enforces, as opposed to the advisory one above.
///
/// Deliberately far looser, for two compounding reasons. A wall-clock number
/// measured while the rest of the suite shares the cores is worth several
/// times its idle value, and CI runners are slower and more variable than a
/// developer machine. A tight timing gate would therefore be a flaky gate, and
/// a flaky gate gets muted, which leaves no gate at all.
///
/// So this one only catches an order-of-magnitude blow-up, and the real work
/// is done by [`wasteful`], which counts layout work rather than time and
/// needs no slack at all: it fires identically on the fastest laptop and the
/// slowest shared runner.
pub const GATE_BUDGET_US: u64 = 40_000;

/// Nodes whose steady-state frame exceeds the budget.
pub fn over_budget(costs: &[NodeCost], budget_us: u64) -> Vec<&NodeCost> {
    costs
        .iter()
        .filter(|cost| cost.warm_us > budget_us)
        .collect()
}

/// Nodes that redo transcript layout on a steady-state frame.
///
/// Separate from the timing budget on purpose. This is a *correctness*
/// statement about the render loop ("an unchanged frame must not redo work"),
/// it is exact rather than noisy, and it catches the regression on any machine
/// including one fast enough to stay under the time budget while doing all the
/// work again.
pub fn wasteful(costs: &[NodeCost]) -> Vec<&NodeCost> {
    costs
        .iter()
        .filter(|cost| cost.warm_relayouts > 0)
        .collect()
}

/// Print the sweep as a table, and report whether every node passed.
///
/// Returns false when something failed, so the caller can exit non-zero and a
/// CI gate can fail on it rather than printing a number nobody reads.
pub fn report(costs: &[NodeCost], budget_us: u64) -> bool {
    println!(
        "{:<30} {:>10} {:>10} {:>10}   status",
        "state", "cold", "warm", "relayout"
    );
    let rule = "-".repeat(78);
    println!("{rule}");
    for cost in costs {
        let slow = cost.warm_us > budget_us;
        let wasteful = cost.warm_relayouts > 0;
        let status = match (slow, wasteful) {
            (true, true) => "SLOW+WASTE",
            (true, false) => "SLOW",
            (false, true) => "WASTE",
            (false, false) => "ok",
        };
        println!(
            "{:<30} {:>8.2}ms {:>8.2}ms {:>4}/{:<5} {}",
            cost.name,
            cost.cold_us as f64 / 1000.0,
            cost.warm_us as f64 / 1000.0,
            cost.warm_relayouts,
            cost.messages,
            status
        );
    }

    let slow = over_budget(costs, budget_us);
    let wasteful = wasteful(costs);
    println!();
    println!(
        "{} states | warm budget {:.1}ms | {} over budget | {} redoing layout",
        costs.len(),
        budget_us as f64 / 1000.0,
        slow.len(),
        wasteful.len()
    );

    for cost in &slow {
        println!(
            "SLOW: {} builds a steady-state frame in {:.2}ms",
            cost.name,
            cost.warm_us as f64 / 1000.0
        );
    }
    for cost in &wasteful {
        println!(
            "WASTE: {} lays out {} of {} messages again on an unchanged frame",
            cost.name, cost.warm_relayouts, cost.messages
        );
    }

    // Only exact or gross problems fail the command. The advisory budget is a
    // reading aid: it is wall-clock, so on a machine that is busy compiling
    // (or on an unoptimised build) it flags states that are perfectly healthy.
    // Failing on it would make the command cry wolf, and a tool that cries
    // wolf gets ignored, which is the failure mode this whole harness exists
    // to avoid.
    let gross = over_budget(costs, GATE_BUDGET_US);
    if !slow.is_empty() && gross.is_empty() && wasteful.is_empty() {
        println!();
        println!(
            "note: over the advisory {:.1}ms budget but under the {:.1}ms failure \
             threshold, and no state is redoing layout. On a loaded machine or an \
             unoptimised build this is expected; compare relayout counts, not \
             milliseconds.",
            budget_us as f64 / 1000.0,
            GATE_BUDGET_US as f64 / 1000.0
        );
    }
    gross.is_empty() && wasteful.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sweep must cover the whole state space. A profiler that silently
    /// skips states is worse than none: it reports "all clear" for a space it
    /// never visited.
    #[test]
    fn the_sweep_covers_every_state() {
        let costs = sweep();
        assert_eq!(
            costs.len(),
            states::names().len(),
            "the sweep skipped states"
        );
    }

    /// The state space must reach the slow end, or the sweep is measuring 29
    /// near-empty screens and will report the app as fast while a real session
    /// lags. That blind spot is exactly what let the transcript relayout ship.
    ///
    /// Measured on *cold* cost. Warm cost is deliberately flat when the caches
    /// are doing their job, so asserting a spread there would be asserting the
    /// caches are broken.
    #[test]
    fn the_state_space_reaches_the_slow_end() {
        let mut costs = sweep();
        costs.sort_by_key(|cost| std::cmp::Reverse(cost.cold_us));
        let heaviest = costs.first().expect("no states");
        let median = costs[costs.len() / 2].cold_us.max(1);
        // 3x rather than a bigger multiple: the optimized text/vector stack
        // (the workspace's opt-level pins) compressed cold costs across the
        // board, and the heavy nodes gained the most, so the spread between
        // the heavy end and the median is genuinely narrower than it was at
        // opt-level 0. What this gate owes is that a *heavy end exists*; the
        // exact regression detection is the relayout gate below, which does
        // not depend on timing at all.
        assert!(
            heaviest.cold_us > median * 3,
            "the heaviest state ({}, {}us cold) is barely above the median ({}us): the state \
             space has no heavy end, so this sweep cannot detect the lag that matters",
            heaviest.name,
            heaviest.cold_us,
            median
        );

        // And the heavy end must be the nodes built for it, not an accident of
        // whichever small node happened to run first.
        assert!(
            costs
                .iter()
                .take(3)
                .any(|cost| cost.name.starts_with("heavy_")),
            "no purpose-built heavy state is among the three most expensive: {:?}",
            costs.iter().take(3).map(|c| &c.name).collect::<Vec<_>>()
        );
    }

    /// No state suffers an order-of-magnitude timing blow-up.
    ///
    /// The budget here is much looser than the one the interactive report
    /// prints, and that is deliberate: this runs alongside the rest of the
    /// suite on a contended machine, where a 2.7ms frame measures 4.7ms simply
    /// because other tests are using the cores. A gate tuned to the idle
    /// number would fail at random, and a gate that fails at random gets
    /// muted. This catches a catastrophic regression; the exact one is below.
    #[test]
    fn every_state_builds_a_frame_within_budget() {
        let costs = sweep();
        let over = over_budget(&costs, GATE_BUDGET_US);
        assert!(
            over.is_empty(),
            "states over the {:.1}ms warm frame budget: {}",
            GATE_BUDGET_US as f64 / 1000.0,
            over.iter()
                .map(|cost| format!("{} ({:.2}ms)", cost.name, cost.warm_us as f64 / 1000.0))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    /// No state redoes transcript layout on an unchanged frame.
    ///
    /// This is the gate that would have caught the lag on any machine. It is a
    /// statement about work rather than time, so it does not depend on how
    /// fast the box is and cannot be tuned away by generous slack: a
    /// steady-state frame that lays a message out again is redoing work it
    /// already did, full stop.
    #[test]
    fn no_state_redoes_layout_on_an_unchanged_frame() {
        let costs = sweep();
        let wasteful = wasteful(&costs);
        assert!(
            wasteful.is_empty(),
            "states re-laying the transcript every frame: {}",
            wasteful
                .iter()
                .map(|cost| format!("{} ({}/{})", cost.name, cost.warm_relayouts, cost.messages))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}
