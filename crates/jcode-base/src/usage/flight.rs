//! One Anthropic probe per account, shared by all callers and processes.

use super::*;
use std::sync::{Mutex, OnceLock, Weak};

#[cfg(test)]
#[path = "flight_tests.rs"]
mod tests;

type Flights = HashMap<String, Weak<tokio::sync::Mutex<()>>>;
static FLIGHTS: OnceLock<Mutex<Flights>> = OnceLock::new();

fn flight(key: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut flights = FLIGHTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    flights.retain(|_, flight| flight.strong_count() > 0);
    if let Some(flight) = flights.get(key).and_then(Weak::upgrade) {
        return flight;
    }
    let flight = Arc::new(tokio::sync::Mutex::new(()));
    flights.insert(key.into(), Arc::downgrade(&flight));
    flight
}

struct CancelledProbe(Option<String>);

impl Drop for CancelledProbe {
    fn drop(&mut self) {
        if let Some(key) = self.0.take() {
            // A dropped timeout future must not leave an uncached miss that
            // immediately starts the same request again. The file lock is still
            // held here, so waiters observe this backoff before making a request.
            let _ = cache_anthropic_usage_failure(
                key,
                "Usage probe cancelled before completion".into(),
            );
        }
    }
}

pub(super) async fn fetch<F, Fut>(key: String, probe: F) -> Result<UsageData>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<UsageData>>,
{
    if let Some(data) = cached_anthropic_usage(&key) {
        return Ok(data);
    }
    let flight = flight(&key);
    let _local = flight.lock().await;
    if let Some(data) = cached_anthropic_usage(&key) {
        return Ok(data);
    }
    let _shared = match disk_cache::lock_probe(&key).await {
        Ok(lock) => lock,
        Err(_) => {
            return cache_anthropic_usage_failure(
                key,
                "Cannot coordinate Anthropic usage cache".into(),
            );
        }
    };
    // Another process may have fetched or entered backoff while we waited.
    if let Some(data) = disk_cache::fresh(&key) {
        store_anthropic_usage(key, data.clone());
        return Ok(data);
    }
    let mut cancellation = CancelledProbe(Some(key.clone()));
    let result = match tokio::time::timeout(Duration::from_secs(15), probe()).await {
        Ok(result) => result,
        Err(_) => cache_anthropic_usage_failure(key, "Usage probe timed out".into()),
    };
    cancellation.0 = None;
    result
}

/// Only an opaque scope hash and response status are logged, never tokens or
/// response bodies. Drop also records cancellations and body-read timeouts.
pub(super) struct RequestLog {
    scope: String,
    started: Instant,
    pub status: Option<u16>,
}

impl RequestLog {
    pub fn new(key: &str) -> Self {
        use sha2::{Digest, Sha256};
        let scope = hex::encode(Sha256::digest(key.as_bytes()))[..16].to_string();
        crate::logging::info(&format!("Anthropic usage probe start scope={scope}"));
        Self {
            scope,
            started: Instant::now(),
            status: None,
        }
    }
}

impl Drop for RequestLog {
    fn drop(&mut self) {
        crate::logging::info(&format!(
            "Anthropic usage probe finish scope={} status={:?} duration_ms={}",
            self.scope,
            self.status,
            self.started.elapsed().as_millis()
        ));
    }
}
