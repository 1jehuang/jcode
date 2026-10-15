//! Process-wide "stored credentials changed" signal.
//!
//! Provider retry loops sleep between attempts (up to the Retry-After cap).
//! When the user swaps accounts during that sleep, the retry should wake right
//! away and use the new credential instead of waiting out a delay that belongs
//! to the previous account. Every auth-change path bumps this generation (via
//! `AuthStatus::invalidate_cache`), and so does a provider that notices the
//! stored credential changed under its cache.

use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Notify;

static GENERATION: AtomicU64 = AtomicU64::new(0);
static NOTIFY: Notify = Notify::const_new();

/// Current credential generation. Compare with a later value, or pass it to
/// [`changed_since`], to learn whether credentials changed in between.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// Record that stored credentials changed and wake every waiter.
pub fn bump() {
    GENERATION.fetch_add(1, Ordering::AcqRel);
    NOTIFY.notify_waiters();
}

/// Resolve once the generation differs from `since`. Returns immediately if
/// it already does, so a bump between reading the generation and waiting is
/// never missed.
pub async fn changed_since(since: u64) {
    loop {
        let notified = NOTIFY.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if generation() != since {
            return;
        }
        notified.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn bump_wakes_a_waiter_and_a_stale_generation_returns_at_once() {
        let start = generation();
        let waiter = tokio::spawn(changed_since(start));
        tokio::time::sleep(Duration::from_millis(20)).await;
        bump();
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("bump must wake the waiter")
            .unwrap();
        tokio::time::timeout(Duration::from_millis(50), changed_since(start))
            .await
            .expect("an already-changed generation must not wait");
    }
}
