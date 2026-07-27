//! Stall watchdog: detects when the process stops making progress and writes
//! actionable diagnostics to the log instead of leaving a silent freeze.
//!
//! Long-running sessions occasionally "just hang". Because the hang is silent,
//! the log ends abruptly with no clue about what the process was doing. This
//! module fixes that in three ways:
//!
//! 1. **Breadcrumbs.** Hot loops call [`beat`] with a short phase label. The
//!    most recent label plus its timestamp are kept in a lock-free slot.
//! 2. **Liveness heartbeat.** A monitor thread logs a periodic `watchdog.alive`
//!    event with RSS, thread count, and open-fd count, so a log that stops
//!    growing pins the freeze to a bounded time window and shows resource
//!    trends leading up to it.
//! 3. **Stall dumps.** When no beat arrives within the stall threshold, the
//!    monitor logs `watchdog.stall` including a per-thread state snapshot
//!    (name, run state, kernel wait channel) on Linux. That distinguishes a
//!    deadlock (threads in futex wait) from a spin (threads running) from a
//!    blocked syscall (threads in a network/IO wchan).
//!
//! The monitor thread is independent of the async runtime and the UI loop, so
//! it still reports when either is wedged.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Default time without a beat before the process is considered stalled.
const DEFAULT_STALL_SECS: u64 = 90;
/// Default interval between liveness heartbeats.
const DEFAULT_HEARTBEAT_SECS: u64 = 300;
/// How often the monitor thread wakes to evaluate stall state.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Cap on threads included in a stall dump so one dump cannot flood the log.
const MAX_THREADS_IN_DUMP: usize = 48;

static STARTED: AtomicBool = AtomicBool::new(false);
static LAST_BEAT_MS: AtomicU64 = AtomicU64::new(0);
static BEAT_COUNT: AtomicU64 = AtomicU64::new(0);
static PHASE: AtomicUsize = AtomicUsize::new(0);
static PROCESS_START: OnceLock<Instant> = OnceLock::new();
/// Number of in-flight units of work. A process that is simply waiting for
/// user input is idle, not stalled, so stall reporting is gated on this.
static BUSY: AtomicUsize = AtomicUsize::new(0);
/// Free-form detail about the current operation (e.g. the running tool). Kept
/// separate from the interned phase because it is dynamic and low-frequency.
static DETAIL: OnceLock<std::sync::Mutex<String>> = OnceLock::new();

fn detail_slot() -> &'static std::sync::Mutex<String> {
    DETAIL.get_or_init(|| std::sync::Mutex::new(String::new()))
}

/// RAII guard marking a unit of work in flight. While any guard is alive the
/// process is expected to make progress, so missing beats mean a real hang
/// rather than an idle wait for input.
#[must_use = "the guard must stay alive for the duration of the work"]
pub struct WorkGuard(());

impl WorkGuard {
    fn new(phase: &'static str) -> Self {
        BUSY.fetch_add(1, Ordering::SeqCst);
        beat(phase);
        Self(())
    }
}

impl Drop for WorkGuard {
    fn drop(&mut self) {
        BUSY.fetch_sub(1, Ordering::SeqCst);
        beat("idle");
    }
}

/// Mark the start of a unit of work that is expected to make steady progress.
pub fn begin_work(phase: &'static str) -> WorkGuard {
    WorkGuard::new(phase)
}

/// Whether any work is currently in flight.
pub fn is_busy() -> bool {
    BUSY.load(Ordering::SeqCst) > 0
}

/// Describe the operation currently in flight. Shown in stall dumps so a hang
/// inside a specific tool or request is immediately identifiable.
pub fn set_detail(detail: impl Into<String>) {
    if let Ok(mut slot) = detail_slot().lock() {
        *slot = detail.into();
    }
}

fn current_detail() -> String {
    detail_slot()
        .lock()
        .map(|slot| slot.clone())
        .unwrap_or_default()
}

/// Phase labels are interned so [`beat`] stays allocation-free on hot paths.
/// The set of labels is small, fixed, and compile-time known in practice.
static PHASES: OnceLock<std::sync::Mutex<Vec<&'static str>>> = OnceLock::new();

fn phases() -> &'static std::sync::Mutex<Vec<&'static str>> {
    PHASES.get_or_init(|| std::sync::Mutex::new(vec!["startup"]))
}

fn process_start() -> Instant {
    *PROCESS_START.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    process_start().elapsed().as_millis() as u64
}

fn env_secs(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

/// Record that the given phase is making progress. Cheap enough for per-frame
/// and per-event use: two relaxed atomic stores in the common case.
pub fn beat(phase: &'static str) {
    LAST_BEAT_MS.store(now_ms(), Ordering::Relaxed);
    BEAT_COUNT.fetch_add(1, Ordering::Relaxed);

    let current = PHASE.load(Ordering::Relaxed);
    let table = phases();
    if table
        .lock()
        .is_ok_and(|guard| guard.get(current).is_some_and(|name| *name == phase))
    {
        return;
    }
    let Ok(mut guard) = table.lock() else {
        return;
    };
    let index = match guard.iter().position(|name| *name == phase) {
        Some(index) => index,
        None => {
            guard.push(phase);
            guard.len() - 1
        }
    };
    PHASE.store(index, Ordering::Relaxed);
}

fn current_phase() -> &'static str {
    let index = PHASE.load(Ordering::Relaxed);
    phases()
        .lock()
        .ok()
        .and_then(|guard| guard.get(index).copied())
        .unwrap_or("unknown")
}

/// Snapshot of cheap process-level resource counters.
#[derive(Debug, Default, Clone, Copy)]
pub struct ResourceSnapshot {
    pub rss_mb: u64,
    pub threads: usize,
    pub open_fds: usize,
}

pub fn resource_snapshot() -> ResourceSnapshot {
    ResourceSnapshot {
        rss_mb: read_rss_mb(),
        threads: count_entries("/proc/self/task"),
        open_fds: count_entries("/proc/self/fd"),
    }
}

fn read_rss_mb() -> u64 {
    let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    let Some(rss_pages) = statm.split_whitespace().nth(1) else {
        return 0;
    };
    let pages: u64 = rss_pages.parse().unwrap_or(0);
    pages.saturating_mul(4096) / (1024 * 1024)
}

fn count_entries(path: &str) -> usize {
    std::fs::read_dir(path)
        .map(|entries| entries.flatten().count())
        .unwrap_or(0)
}

/// Per-thread state, used to tell a deadlock apart from a spin or blocked IO.
pub fn thread_states() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/proc/self/task") else {
        return Vec::new();
    };
    let mut states: Vec<String> = entries
        .flatten()
        .take(MAX_THREADS_IN_DUMP)
        .map(|entry| {
            let dir = entry.path();
            let tid = entry.file_name().to_string_lossy().to_string();
            let name = read_trimmed(dir.join("comm")).unwrap_or_else(|| "?".to_string());
            let wchan = read_trimmed(dir.join("wchan")).unwrap_or_else(|| "?".to_string());
            let state = read_trimmed(dir.join("stat"))
                .and_then(|stat| {
                    // `stat` is "<pid> (<comm>) <state> ...": comm may contain
                    // spaces, so split after the final ')'.
                    let rest = stat.rsplit_once(')')?.1.trim().to_string();
                    rest.split_whitespace().next().map(str::to_string)
                })
                .unwrap_or_else(|| "?".to_string());
            format!("{tid}:{name}:{state}:{wchan}")
        })
        .collect();
    states.sort();
    states
}

fn read_trimmed(path: std::path::PathBuf) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Start the watchdog monitor thread. Idempotent; extra calls are no-ops.
///
/// Disable entirely with `JCODE_WATCHDOG=0`. Tune with
/// `JCODE_WATCHDOG_STALL_SECS` and `JCODE_WATCHDOG_HEARTBEAT_SECS`.
pub fn start() {
    if std::env::var("JCODE_WATCHDOG").is_ok_and(|value| value == "0" || value == "off") {
        return;
    }
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = process_start();
    beat("startup");

    let stall = Duration::from_secs(env_secs("JCODE_WATCHDOG_STALL_SECS", DEFAULT_STALL_SECS));
    let heartbeat = Duration::from_secs(env_secs(
        "JCODE_WATCHDOG_HEARTBEAT_SECS",
        DEFAULT_HEARTBEAT_SECS,
    ));

    std::thread::Builder::new()
        .name("jcode-watchdog".to_string())
        .spawn(move || monitor_loop(stall, heartbeat))
        .ok();
}

fn monitor_loop(stall: Duration, heartbeat: Duration) {
    let mut last_heartbeat = Instant::now();
    let mut stall_reported_at: Option<Duration> = None;
    let mut next_stall_report = stall;

    loop {
        std::thread::sleep(POLL_INTERVAL);

        let last_beat_ms = LAST_BEAT_MS.load(Ordering::Relaxed);
        let since_beat = Duration::from_millis(now_ms().saturating_sub(last_beat_ms));
        let resources = resource_snapshot();

        // An idle client waiting for input is not stalled; only work that
        // claimed to be in flight can hang.
        if !is_busy() {
            if stall_reported_at.take().is_some() {
                next_stall_report = stall;
            }
            emit_heartbeat_if_due(&mut last_heartbeat, heartbeat, resources);
            continue;
        }

        if since_beat >= next_stall_report {
            let states = thread_states();
            super::event_warn(
                "watchdog.stall",
                [
                    ("phase", current_phase().to_string()),
                    ("detail", current_detail()),
                    ("stalled_secs", since_beat.as_secs().to_string()),
                    ("beats", BEAT_COUNT.load(Ordering::Relaxed).to_string()),
                    ("rss_mb", resources.rss_mb.to_string()),
                    ("threads", resources.threads.to_string()),
                    ("open_fds", resources.open_fds.to_string()),
                    ("thread_states", states.join(" | ")),
                ],
            );
            stall_reported_at = Some(since_beat);
            // Back off so a long hang produces a few informative dumps rather
            // than a log full of identical ones.
            next_stall_report = since_beat.saturating_mul(2).max(stall);
            continue;
        }

        if let Some(reported) = stall_reported_at
            && since_beat < stall
        {
            super::event_info(
                "watchdog.recovered",
                [
                    ("phase", current_phase().to_string()),
                    ("stalled_secs", reported.as_secs().to_string()),
                ],
            );
            stall_reported_at = None;
            next_stall_report = stall;
        }

        emit_heartbeat_if_due(&mut last_heartbeat, heartbeat, resources);
    }
}

/// The heartbeat runs whether or not work is in flight: a log that stops
/// growing entirely is itself the signal that the process died or froze hard.
fn emit_heartbeat_if_due(
    last_heartbeat: &mut Instant,
    heartbeat: Duration,
    resources: ResourceSnapshot,
) {
    if heartbeat.is_zero() || last_heartbeat.elapsed() < heartbeat {
        return;
    }
    *last_heartbeat = Instant::now();
    super::event_info(
        "watchdog.alive",
        [
            ("phase", current_phase().to_string()),
            ("detail", current_detail()),
            ("busy", is_busy().to_string()),
            (
                "uptime_secs",
                process_start().elapsed().as_secs().to_string(),
            ),
            ("beats", BEAT_COUNT.load(Ordering::Relaxed).to_string()),
            ("rss_mb", resources.rss_mb.to_string()),
            ("threads", resources.threads.to_string()),
            ("open_fds", resources.open_fds.to_string()),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beat_records_phase_and_advances_counter() {
        let before = BEAT_COUNT.load(Ordering::Relaxed);
        beat("test-phase");
        assert_eq!(current_phase(), "test-phase");
        assert!(BEAT_COUNT.load(Ordering::Relaxed) > before);
        beat("test-phase-2");
        assert_eq!(current_phase(), "test-phase-2");
    }

    #[test]
    fn work_guard_tracks_busy_state() {
        assert!(!is_busy(), "no work should be in flight at test start");
        {
            let _guard = begin_work("test-work");
            assert!(is_busy());
            assert_eq!(current_phase(), "test-work");
        }
        assert!(!is_busy(), "guard drop must clear busy state");
        assert_eq!(current_phase(), "idle");
    }

    #[test]
    fn resource_snapshot_reports_live_process_state() {
        let snapshot = resource_snapshot();
        if cfg!(target_os = "linux") {
            assert!(snapshot.threads > 0, "expected at least one thread");
            assert!(snapshot.open_fds > 0, "expected at least one open fd");
        }
    }

    #[test]
    fn thread_states_are_parsed_into_compact_records() {
        let states = thread_states();
        if cfg!(target_os = "linux") {
            assert!(!states.is_empty());
            let first = &states[0];
            assert_eq!(
                first.split(':').count(),
                4,
                "expected tid:name:state:wchan, got {first}"
            );
        }
    }
}
