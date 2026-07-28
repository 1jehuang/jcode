use std::sync::Arc;

/// A soft interrupt message queued for injection at the next safe point.
#[derive(Debug, Clone)]
pub struct SoftInterruptMessage {
    pub content: String,
    pub images: Vec<(String, String)>,
    /// If true, can skip remaining tools when injected at point C.
    pub urgent: bool,
    pub source: SoftInterruptSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftInterruptSource {
    User,
    System,
    BackgroundTask,
}

/// Thread-safe soft interrupt queue that can be accessed without holding the agent lock.
pub type SoftInterruptQueue = Arc<std::sync::Mutex<Vec<SoftInterruptMessage>>>;

/// Signal to move the currently executing tool to background.
/// Uses std::sync so it can be set without async from outside the agent lock.
pub type BackgroundToolSignal = Arc<std::sync::atomic::AtomicBool>;

/// Signal to gracefully stop generation.
pub type GracefulShutdownSignal = Arc<std::sync::atomic::AtomicBool>;

/// Async-aware interrupt signal that combines AtomicBool (sync read) with
/// tokio::Notify (async wake). Eliminates spin-loops during tool execution.
#[derive(Clone)]
pub struct InterruptSignal {
    flag: Arc<std::sync::atomic::AtomicBool>,
    /// Monotonic fire counter. Lets owners of a timed/deferred reset detect
    /// that a *newer* fire landed in the meantime and skip the reset instead
    /// of erasing a cancel the target has not observed yet (issue #428).
    epoch: Arc<std::sync::atomic::AtomicU64>,
    notify: Arc<tokio::sync::Notify>,
}

impl InterruptSignal {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    pub fn fire(&self) {
        self.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.flag.store(true, std::sync::atomic::Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_set(&self) -> bool {
        self.flag.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn reset(&self) {
        self.flag.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Current fire epoch. Capture this right after a [`fire`](Self::fire) to
    /// later reset only that specific fire via
    /// [`reset_if_epoch`](Self::reset_if_epoch).
    pub fn epoch(&self) -> u64 {
        self.epoch.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Reset the signal only if no newer [`fire`](Self::fire) happened since
    /// `epoch` was captured. Returns `true` when the reset was applied.
    ///
    /// If a racing fire lands between the epoch check and the reset, the
    /// fire is restored (flag re-set and waiters re-notified) so no cancel
    /// is ever silently erased.
    pub fn reset_if_epoch(&self, epoch: u64) -> bool {
        if self.epoch.load(std::sync::atomic::Ordering::SeqCst) != epoch {
            return false;
        }
        self.flag.store(false, std::sync::atomic::Ordering::SeqCst);
        if self.epoch.load(std::sync::atomic::Ordering::SeqCst) != epoch {
            // A newer fire raced with the reset; restore it.
            self.flag.store(true, std::sync::atomic::Ordering::SeqCst);
            self.notify.notify_waiters();
            return false;
        }
        true
    }

    pub async fn notified(&self) {
        let mut notified = std::pin::pin!(self.notify.notified());
        // Explicitly register this waiter with the Notify before checking the
        // flag. `notify_waiters()` (used by `fire()`) wakes only registered
        // waiters; current tokio registers a `notified()` future at creation,
        // but `enable()` makes the registration explicit rather than relying
        // on that version-specific guarantee, since a lost wakeup here parks
        // the cancel path (agent stream loop, tool-wait select) until an
        // unrelated event arrives (issue #428).
        notified.as_mut().enable();
        if self.is_set() {
            return;
        }
        notified.await;
    }

    pub fn as_atomic(&self) -> Arc<std::sync::atomic::AtomicBool> {
        Arc::clone(&self.flag)
    }

    /// True when `other` is a clone of this signal (shares the same state).
    /// Used by cancel fan-out to avoid double-firing the same signal and by
    /// diagnostics that need to detect stale signal instances (issue #428).
    pub fn same_instance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.flag, &other.flag)
    }
}

impl Default for InterruptSignal {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct StreamError {
    pub message: String,
    pub retry_after_secs: Option<u64>,
}

impl StreamError {
    pub fn new(message: String, retry_after_secs: Option<u64>) -> Self {
        Self {
            message,
            retry_after_secs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Documents the tokio semantics `InterruptSignal::notified()` relies on:
    /// current tokio guarantees a `notified()` future receives wakeups from
    /// `notify_waiters()` from the moment it is *created*, even before its
    /// first poll. The explicit `enable()` in `notified()` makes that
    /// registration explicit instead of relying on the version-specific
    /// creation-time guarantee (hardening for issue #428).
    #[tokio::test]
    async fn notified_future_receives_notify_waiters_from_creation() {
        let notify = tokio::sync::Notify::new();

        // Created before the notification, not yet polled: must be woken.
        let created_before = notify.notified();
        notify.notify_waiters();
        tokio::time::timeout(Duration::from_millis(100), created_before)
            .await
            .expect("a notified() future created before notify_waiters() must be woken");

        // Created after the notification: must NOT be woken (notify_waiters
        // stores no permit). This is why fire() also sets the atomic flag.
        let created_after = notify.notified();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), created_after)
                .await
                .is_err(),
            "notify_waiters() must not store a permit for future waiters"
        );
    }

    /// Probabilistic race hammer for issue #428: `fire()` must never be lost
    /// regardless of where the waiter is between creating the `notified()`
    /// future and its first poll. The agent stream loop recreates this future
    /// per stream event, so under fast token streams the pre-fix race made
    /// Esc/Ctrl+C cancels appear to be ignored.
    #[test]
    fn fire_never_loses_wakeup_while_notified_races() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_time()
            .build()
            .expect("runtime");
        rt.block_on(async {
            for i in 0..2000 {
                let signal = InterruptSignal::new();
                let waiter = {
                    let signal = signal.clone();
                    tokio::spawn(async move { signal.notified().await })
                };
                // Fire concurrently: the waiter may be anywhere between
                // future creation and first poll.
                signal.fire();
                tokio::time::timeout(Duration::from_secs(2), waiter)
                    .await
                    .unwrap_or_else(|_| {
                        panic!(
                            "lost wakeup on iteration {i}: notified() missed fire() (issue #428)"
                        )
                    })
                    .expect("waiter task must not panic");
            }
        });
    }

    /// A fire() that happened before notified() is observed immediately.
    #[tokio::test]
    async fn notified_returns_immediately_when_already_fired() {
        let signal = InterruptSignal::new();
        signal.fire();
        tokio::time::timeout(Duration::from_millis(100), signal.notified())
            .await
            .expect("pre-fired signal must resolve notified() immediately");
    }

    /// reset() clears the flag so subsequent notified() calls wait again.
    #[tokio::test]
    async fn reset_clears_fired_state() {
        let signal = InterruptSignal::new();
        signal.fire();
        assert!(signal.is_set());
        signal.reset();
        assert!(!signal.is_set());
        let waited = tokio::time::timeout(Duration::from_millis(50), signal.notified()).await;
        assert!(waited.is_err(), "reset signal must park notified() again");
    }

    /// reset_if_epoch() clears the flag only for the fire that captured the
    /// epoch. A deferred reset (e.g. the server's 500ms timer for detached
    /// turns) must not erase a newer cancel fired in the meantime, otherwise
    /// rapid repeated Esc presses cancel each other out (issue #428).
    #[test]
    fn reset_if_epoch_skips_when_newer_fire_landed() {
        let signal = InterruptSignal::new();
        signal.fire();
        let first_epoch = signal.epoch();

        // A second cancel (repeated Esc) fires before the deferred reset runs.
        signal.fire();
        assert!(
            !signal.reset_if_epoch(first_epoch),
            "stale deferred reset must be skipped"
        );
        assert!(
            signal.is_set(),
            "newer cancel must survive the stale deferred reset"
        );

        // The reset scheduled for the latest fire still works.
        let second_epoch = signal.epoch();
        assert!(signal.reset_if_epoch(second_epoch));
        assert!(!signal.is_set());

        // And a reset for an already-consumed epoch stays a no-op.
        assert!(!signal.reset_if_epoch(first_epoch));
    }

    /// A fire() racing the flag-clear inside reset_if_epoch() is restored
    /// rather than silently erased.
    #[test]
    fn reset_if_epoch_never_erases_concurrent_fire() {
        for _ in 0..2000 {
            let signal = InterruptSignal::new();
            signal.fire();
            let epoch = signal.epoch();

            let firer = {
                let signal = signal.clone();
                std::thread::spawn(move || signal.fire())
            };
            let _ = signal.reset_if_epoch(epoch);
            firer.join().expect("firer thread");

            assert!(
                signal.is_set(),
                "a concurrent fire() must never be erased by reset_if_epoch()"
            );
        }
    }
}
