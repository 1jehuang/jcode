//! Cadence for the one-cell status spinner, kept off the async runtime's timer.
//!
//! The client event loop runs inside `block_on` on the main thread of a
//! multi-thread runtime, so the timer driver lives on a worker thread. Every
//! runtime timer that fires therefore costs a chain of thread wakeups instead of
//! one: measured on an unfocused client running a tool, the 80ms spinner timer
//! (12.5 ticks/s) cost about 39 wakeups/s on a worker plus 14 on the main
//! thread. A session that is only thinking or running a tool has nothing else to
//! do, so that chain was most of what a working window cost, in every window
//! showing the session, focused or not.
//!
//! A plain thread that sleeps from frame to frame and nudges the loop through a
//! channel costs one wakeup per frame on each side and paints the same frames.
//!
//! The thread free-runs while its ticks are being collected, so the loop never
//! has to wake it to ask for the next one. Once two ticks in a row find the
//! previous one still uncollected it parks, and the next `tick` call restarts
//! it. An idle client therefore leaves the thread asleep indefinitely.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Uncollected ticks in a row before the thread stops producing them.
const PARK_AFTER_MISSED: u8 = 2;

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    /// Mirrors `State::next.is_none()` so `tick` can skip the lock.
    parked: AtomicBool,
}

struct State {
    /// When the thread should send the next tick. `None` parks it.
    next: Option<Instant>,
    closed: bool,
}

pub(crate) struct SpinnerTicker {
    period: Duration,
    shared: Arc<Shared>,
    ticks: tokio::sync::mpsc::Receiver<()>,
    /// Delay before the first tick after the thread is next started.
    restart_delay: Duration,
    /// Deadline used only when the thread could not be started.
    fallback_next: Instant,
    has_thread: bool,
}

impl SpinnerTicker {
    pub(crate) fn new(period: Duration, first_delay: Duration) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                next: None,
                closed: false,
            }),
            wake: Condvar::new(),
            parked: AtomicBool::new(true),
        });
        let (sender, ticks) = tokio::sync::mpsc::channel(1);
        let thread_shared = Arc::clone(&shared);
        let has_thread = std::thread::Builder::new()
            .name("jcode-spinner".to_string())
            .stack_size(64 * 1024)
            .spawn(move || run(&thread_shared, &sender, period))
            .map_err(|error| {
                crate::logging::warn(&format!(
                    "Spinner thread failed to start, using the runtime timer: {error}"
                ));
            })
            .is_ok();
        Self {
            period,
            shared,
            ticks,
            restart_delay: first_delay,
            fallback_next: Instant::now() + first_delay,
            has_thread,
        }
    }

    /// Wait for the next spinner frame. Cancel safe: dropping the wait leaves
    /// the thread running and the next call picks up where this one stopped.
    pub(crate) async fn tick(&mut self) {
        if self.has_thread {
            if self.shared.parked.load(Ordering::Acquire) {
                // Ticks left over from before the thread parked are stale.
                while self.ticks.try_recv().is_ok() {}
                self.schedule(Instant::now() + self.restart_delay);
                self.restart_delay = self.period;
            }
            if self.ticks.recv().await.is_some() {
                return;
            }
            // The thread is gone. Do not spin: use the runtime timer from now on.
            self.has_thread = false;
            self.fallback_next = Instant::now() + self.period;
        }
        tokio::time::sleep_until(tokio::time::Instant::from_std(self.fallback_next)).await;
        self.fallback_next = next_on_grid(self.fallback_next, self.period, Instant::now());
    }

    /// Restart the cadence so the next tick lands `delay` from now. Called
    /// after a full frame, which already painted the current spinner glyph.
    pub(crate) fn reset_after(&mut self, delay: Duration) {
        self.fallback_next = Instant::now() + delay;
        if !self.has_thread {
            return;
        }
        // A tick already queued belongs to the old cadence.
        while self.ticks.try_recv().is_ok() {}
        if self.shared.parked.load(Ordering::Acquire) {
            // Nobody is waiting for ticks. Remember the phase without waking
            // the thread: an idle client repaints full frames too.
            self.restart_delay = delay;
        } else {
            self.schedule(self.fallback_next);
        }
    }

    fn schedule(&self, next: Instant) {
        let mut state = lock(&self.shared.state);
        state.next = Some(next);
        self.shared.parked.store(false, Ordering::Release);
        drop(state);
        self.shared.wake.notify_one();
    }
}

impl Drop for SpinnerTicker {
    fn drop(&mut self) {
        lock(&self.shared.state).closed = true;
        self.shared.wake.notify_one();
    }
}

fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// First point after `now` on the grid `previous + n * period`. Missed frames
/// are skipped rather than replayed: the spinner is liveness, not simulated time.
fn next_on_grid(previous: Instant, period: Duration, now: Instant) -> Instant {
    let next = previous + period;
    if next > now {
        return next;
    }
    let behind = now.duration_since(next).as_nanos();
    let skipped = u32::try_from(behind / period.as_nanos().max(1) + 1).unwrap_or(u32::MAX);
    next + period.saturating_mul(skipped)
}

fn run(shared: &Shared, sender: &tokio::sync::mpsc::Sender<()>, period: Duration) {
    use tokio::sync::mpsc::error::TrySendError;

    let mut missed = 0u8;
    let mut state = lock(&shared.state);
    loop {
        if state.closed {
            return;
        }
        let Some(deadline) = state.next else {
            missed = 0;
            state = shared
                .wake
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            continue;
        };
        let now = Instant::now();
        if deadline > now {
            state = shared
                .wake
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
            continue;
        }
        match sender.try_send(()) {
            Ok(()) => missed = 0,
            Err(TrySendError::Full(())) => missed = missed.saturating_add(1),
            Err(TrySendError::Closed(())) => return,
        }
        if missed >= PARK_AFTER_MISSED {
            state.next = None;
            shared.parked.store(true, Ordering::Release);
        } else {
            state.next = Some(next_on_grid(deadline, period, now));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ticks_keep_the_requested_cadence() {
        let period = Duration::from_millis(20);
        let mut ticker = SpinnerTicker::new(period, period);
        let start = Instant::now();
        for _ in 0..5 {
            ticker.tick().await;
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(95) && elapsed < Duration::from_millis(400),
            "five 20ms ticks took {elapsed:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn uncollected_ticks_park_the_thread() {
        let period = Duration::from_millis(10);
        let mut ticker = SpinnerTicker::new(period, period);
        assert!(
            ticker.shared.parked.load(Ordering::Acquire),
            "a ticker nobody has awaited must not run"
        );
        ticker.tick().await;
        assert!(!ticker.shared.parked.load(Ordering::Acquire));

        // Stop collecting, as the loop does once the spinner leaves the screen.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            ticker.shared.parked.load(Ordering::Acquire),
            "the thread must park once its ticks go uncollected"
        );
        assert!(lock(&ticker.shared.state).next.is_none());

        // The stale tick is dropped and the cadence restarts from now.
        let start = Instant::now();
        ticker.tick().await;
        assert!(
            start.elapsed() >= Duration::from_millis(8),
            "a tick queued before parking must not be replayed: {:?}",
            start.elapsed()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_wait_keeps_the_cadence() {
        let period = Duration::from_millis(40);
        let mut ticker = SpinnerTicker::new(period, period);
        let start = Instant::now();
        // Drop a wait before its deadline, as a select! branch losing a race does.
        let cancelled = tokio::time::timeout(Duration::from_millis(5), ticker.tick()).await;
        assert!(cancelled.is_err());
        ticker.tick().await;
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(35) && elapsed < Duration::from_millis(75),
            "the cancelled wait must not restart or skip the frame: {elapsed:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reset_moves_the_next_tick() {
        let period = Duration::from_millis(300);
        let mut ticker = SpinnerTicker::new(period, period);

        // While parked, the reset only records the phase for the next start.
        ticker.reset_after(Duration::from_millis(10));
        assert!(ticker.shared.parked.load(Ordering::Acquire));
        let start = Instant::now();
        ticker.tick().await;
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "reset should set the first tick: {:?}",
            start.elapsed()
        );

        // While running, it reschedules the thread.
        ticker.reset_after(Duration::from_millis(10));
        let start = Instant::now();
        ticker.tick().await;
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "reset should pull a running tick in: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn late_grid_points_are_skipped_not_replayed() {
        let period = Duration::from_millis(80);
        let base = Instant::now();
        assert_eq!(next_on_grid(base, period, base), base + period);
        // 250ms late: the next point is 320ms, not three queued frames.
        assert_eq!(
            next_on_grid(base, period, base + Duration::from_millis(250)),
            base + Duration::from_millis(320)
        );
        // Exactly on a grid point counts as due, so step past it.
        assert_eq!(
            next_on_grid(base, period, base + Duration::from_millis(160)),
            base + Duration::from_millis(240)
        );
    }
}
