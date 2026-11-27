//! Refresh live catalogs even when a session stays attached for days.

use crate::bus::Bus;
use crate::provider::Provider;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;

const REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);
const REFRESH_TIMEOUT: Duration = Duration::from_secs(2 * 60);

// Pointer identity keeps separate accounts/endpoints independent while repeated
// attaches to the same runtime cannot create duplicate polling loops.
static WATCHED: LazyLock<Mutex<HashMap<usize, Weak<dyn Provider>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) fn ensure_periodic_refresh(provider: &Arc<dyn Provider>) -> bool {
    let key = Arc::as_ptr(provider) as *const () as usize;
    let weak = Arc::downgrade(provider);
    {
        let mut watched = WATCHED.lock().unwrap_or_else(|error| error.into_inner());
        watched.retain(|_, provider| provider.strong_count() > 0);
        if watched.contains_key(&key) {
            return false;
        }
        watched.insert(key, weak.clone());
    }
    tokio::spawn(async move {
        refresh_periodically(weak.clone()).await;
        let mut watched = WATCHED.lock().unwrap_or_else(|error| error.into_inner());
        // An allocator may reuse an expired provider's address before its old
        // timer wakes. Never remove a new runtime's registration in that case.
        if watched
            .get(&key)
            .is_some_and(|entry| Weak::ptr_eq(entry, &weak))
        {
            watched.remove(&key);
        }
    });
    true
}

async fn refresh_periodically(provider: Weak<dyn Provider>) {
    // Attach-time prefetch handles the initial load. Do not immediately issue
    // another full fanout or turn missed ticks after suspend into a request burst.
    let mut ticks = tokio::time::interval_at(
        tokio::time::Instant::now() + REFRESH_INTERVAL,
        REFRESH_INTERVAL,
    );
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticks.tick().await;
        let Some(provider) = provider.upgrade() else {
            break;
        };
        let result = tokio::time::timeout(REFRESH_TIMEOUT, provider.refresh_model_catalog()).await;
        match result {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => crate::logging::debug(&format!(
                "Periodic {} model catalog refresh failed: {error}",
                provider.name()
            )),
            Err(_) => crate::logging::debug(&format!(
                "Periodic {} model catalog refresh timed out",
                provider.name()
            )),
        }
        // Fanout may have refreshed some endpoints before another failed or
        // timed out. Publish partial discoveries as well as complete success.
        Bus::global().publish_models_updated();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Probe {
        calls: AtomicUsize,
        hangs: bool,
    }

    #[async_trait::async_trait]
    impl Provider for Probe {
        async fn complete(
            &self,
            _: &[crate::message::Message],
            _: &[crate::message::ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<crate::provider::EventStream> {
            unreachable!("model discovery must not send a chat prompt")
        }
        fn name(&self) -> &str {
            "periodic-discovery-probe"
        }
        fn model(&self) -> String {
            "offline-fallback".into()
        }
        fn fork(&self) -> Arc<dyn Provider> {
            unreachable!()
        }
        async fn prefetch_models(&self) -> anyhow::Result<()> {
            panic!("periodic refresh must bypass fresh prefetch caches")
        }
        async fn refresh_model_catalog(
            &self,
        ) -> anyhow::Result<jcode_provider_core::ModelCatalogRefreshSummary> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.hangs {
                std::future::pending::<()>().await;
            }
            anyhow::bail!("temporary catalog outage")
        }
    }

    #[tokio::test(start_paused = true)]
    async fn periodic_model_refresh_runs_every_half_hour_and_deduplicates_attaches() {
        let probe = Arc::new(Probe {
            calls: AtomicUsize::new(0),
            hangs: false,
        });
        let provider: Arc<dyn Provider> = probe.clone();
        assert!(ensure_periodic_refresh(&provider));
        assert!(!ensure_periodic_refresh(&provider));
        tokio::task::yield_now().await;
        assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
        for expected in 1..=2 {
            tokio::time::advance(REFRESH_INTERVAL).await;
            tokio::task::yield_now().await;
            assert_eq!(probe.calls.load(Ordering::SeqCst), expected);
        }
        // Repeated failures do not kill the timer, and the timer does not keep
        // a closed session/provider alive just to poll its catalog.
        let weak = Arc::downgrade(&provider);
        drop(provider);
        drop(probe);
        assert_eq!(weak.strong_count(), 0);
        tokio::time::advance(REFRESH_INTERVAL).await;
        tokio::task::yield_now().await;
    }

    #[tokio::test(start_paused = true)]
    async fn periodic_model_refresh_recovers_from_a_hung_catalog_request() {
        let probe = Arc::new(Probe {
            calls: AtomicUsize::new(0),
            hangs: true,
        });
        let provider: Arc<dyn Provider> = probe.clone();
        let task = tokio::spawn(refresh_periodically(Arc::downgrade(&provider)));
        tokio::task::yield_now().await;
        tokio::time::advance(REFRESH_INTERVAL).await;
        tokio::task::yield_now().await;
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(REFRESH_TIMEOUT).await;
        tokio::task::yield_now().await;
        tokio::time::advance(REFRESH_INTERVAL).await;
        tokio::task::yield_now().await;
        assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
        task.abort();
    }
}
