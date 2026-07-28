//! Burst detection and attribution for session snapshot loads.
//!
//! A process with a large `~/.jcode/sessions` directory (100k files is real)
//! can be pushed into loading *every* session snapshot by a single unbounded
//! `read_dir` + `Session::load` loop. When that happens two bad things follow:
//!
//! 1. Tens of thousands of blocking file reads land on whatever thread ran the
//!    scan, which is felt as input lag and overlay flicker in the TUI.
//! 2. Every load emits two INFO lines, so the burst also floods the log and
//!    buries the very event that caused it.
//!
//! This module makes bursts self-reporting: individual loads stay quiet once a
//! burst is recognised, and each burst emits one summary line (with a captured
//! backtrace for the first burst in the process) naming the caller responsible.
//! That turns "the logs are full of session_load" into an actionable pointer at
//! the unbounded scan.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Loads within [`BURST_WINDOW`] before per-load logging is suppressed and the
/// run is treated as a burst worth attributing.
const BURST_THRESHOLD: usize = 32;

/// Loads closer together than this continue the current burst. Anything slower
/// is ordinary demand loading and keeps per-load logging.
const BURST_WINDOW: Duration = Duration::from_millis(250);

/// Whether this process has already captured a backtrace for a burst. Capturing
/// is comparatively expensive, and one attribution per process is enough to
/// identify the offending scan.
static BACKTRACE_CAPTURED: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct BurstState {
    /// Loads observed in the current run of closely-spaced loads.
    count: usize,
    /// Wall time spent inside those loads.
    elapsed: Duration,
    /// Snapshot plus journal bytes read by those loads.
    bytes: u64,
    /// When the run started, for reporting burst duration.
    started: Option<Instant>,
    /// Most recent load, for deciding whether the run continues.
    last: Option<Instant>,
    /// Whether the "burst started" attribution has been emitted for this run.
    announced: bool,
}

impl BurstState {
    fn reset(&mut self) {
        self.count = 0;
        self.elapsed = Duration::ZERO;
        self.bytes = 0;
        self.started = None;
        self.last = None;
        self.announced = false;
    }
}

thread_local! {
    static BURST: std::cell::RefCell<BurstState> =
        std::cell::RefCell::new(BurstState::default());
}

/// Outcome of recording one session load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoadLogDecision {
    /// Ordinary load: keep the detailed per-load log lines.
    Log,
    /// Part of a recognised burst: stay quiet, the burst summary covers it.
    Suppress,
}

impl LoadLogDecision {
    pub(super) fn should_log(self) -> bool {
        matches!(self, Self::Log)
    }
}

/// Record one completed session load and report whether it should be logged
/// individually. Also emits the burst summary when a burst ends.
pub(super) fn note_load(elapsed: Duration, bytes: u64) -> LoadLogDecision {
    BURST.with(|cell| {
        let mut state = cell.borrow_mut();
        let now = Instant::now();
        let continues = state
            .last
            .is_some_and(|last| now.duration_since(last) <= BURST_WINDOW);
        if !continues {
            // The previous run (if any) ended without crossing the threshold,
            // or ended and was already reported; start a fresh run.
            flush_locked(&mut state);
            state.started = Some(now);
        }
        state.count += 1;
        state.elapsed += elapsed;
        state.bytes += bytes;
        state.last = Some(now);
        if state.count > BURST_THRESHOLD {
            // Announce as soon as the burst is recognised rather than only when
            // it ends: a burst that hangs the UI for seconds should be visible
            // in the log while it is still happening, and the process may exit
            // (or the thread may park forever) before any later load flushes
            // the summary.
            if !state.announced {
                state.announced = true;
                announce_burst(&state);
            }
            LoadLogDecision::Suppress
        } else {
            LoadLogDecision::Log
        }
    })
}

/// Emit the pending burst summary, if a burst is in progress, and reset state.
#[cfg(test)]
fn flush() {
    BURST.with(|cell| flush_locked(&mut cell.borrow_mut()));
}

/// One warning at the moment a burst is recognised, carrying the caller that
/// started it. This is the line that turns a wall of `session_load` entries
/// into a pointer at the unbounded directory scan responsible.
fn announce_burst(state: &BurstState) {
    let mut fields = vec![
        ("phase", "load_burst_started".to_string()),
        ("loads", state.count.to_string()),
        ("load_ms", state.elapsed.as_millis().to_string()),
        ("bytes", state.bytes.to_string()),
        ("thread", thread_label()),
    ];
    // Capturing a backtrace is comparatively expensive, and one attribution per
    // process is enough to identify the offending scan.
    if !BACKTRACE_CAPTURED.swap(true, Ordering::Relaxed) {
        fields.push(("backtrace", compact_backtrace()));
    }
    crate::logging::event_warn("SESSION_LOAD_BURST", fields);
}

fn flush_locked(state: &mut BurstState) {
    if state.count > BURST_THRESHOLD {
        let span_ms = state
            .started
            .and_then(|start| state.last.map(|last| last.duration_since(start)))
            .unwrap_or_default()
            .as_millis();
        let fields = vec![
            ("phase", "load_burst_done".to_string()),
            ("loads", state.count.to_string()),
            ("load_ms", state.elapsed.as_millis().to_string()),
            ("span_ms", span_ms.to_string()),
            ("bytes", state.bytes.to_string()),
            ("thread", thread_label()),
        ];
        crate::logging::event_warn("SESSION_LOAD_BURST", fields);
    }
    state.reset();
}

fn thread_label() -> String {
    std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_string()
}

/// A single-line, frame-limited backtrace suitable for a log field.
fn compact_backtrace() -> String {
    let backtrace = std::backtrace::Backtrace::force_capture().to_string();
    let frames: Vec<&str> = backtrace
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("jcode"))
        .take(12)
        .collect();
    if frames.is_empty() {
        "unavailable".to_string()
    } else {
        frames.join(" <- ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contiguous_loads_are_suppressed_after_the_threshold() {
        // Run on a dedicated thread so the thread-local burst state is clean
        // regardless of what other tests in this binary have loaded.
        std::thread::spawn(|| {
            for _ in 0..BURST_THRESHOLD {
                assert_eq!(
                    note_load(Duration::from_micros(10), 1024),
                    LoadLogDecision::Log,
                    "loads up to the threshold stay individually logged"
                );
            }
            assert_eq!(
                note_load(Duration::from_micros(10), 1024),
                LoadLogDecision::Suppress,
                "loads past the threshold are covered by the burst summary"
            );
            flush();
            assert_eq!(
                note_load(Duration::from_micros(10), 1024),
                LoadLogDecision::Log,
                "state resets after the burst is reported"
            );
        })
        .join()
        .expect("burst test thread");
    }

    #[test]
    fn a_gap_longer_than_the_window_starts_a_new_run() {
        std::thread::spawn(|| {
            for _ in 0..BURST_THRESHOLD {
                assert!(note_load(Duration::ZERO, 0).should_log());
            }
            std::thread::sleep(BURST_WINDOW + Duration::from_millis(20));
            assert!(
                note_load(Duration::ZERO, 0).should_log(),
                "a slow demand load is not part of the earlier run"
            );
        })
        .join()
        .expect("gap test thread");
    }
}
