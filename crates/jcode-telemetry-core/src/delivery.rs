//! Telemetry delivery: HTTP posting, retry policy, and background workers.
//!
//! Extracted from `lib.rs` to keep that file within its tracked size. Every
//! failure path degrades to dropping the payload rather than panicking: a
//! host must never crash because an endpoint was unreachable.

use jcode_logging as logging;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;

use super::{
    ASYNC_SEND_TIMEOUT, BACKGROUND_QUEUE_CAPACITY, TELEMETRY_BACKGROUND_SENDER,
    TELEMETRY_ENDPOINT, TELEMETRY_HTTP_CLIENT, TELEMETRY_PERMANENTLY_REJECTED,
    TRANSCRIPT_BACKGROUND_SENDER, TRANSCRIPT_ENDPOINT,
};

/// Lazily initialize and borrow the shared telemetry HTTP client. Returns `None`
/// when the builder fails, keeping telemetry optional rather than crashing.
pub(crate) fn http_client() -> Option<&'static reqwest::blocking::Client> {
    TELEMETRY_HTTP_CLIENT
        .get_or_init(|| {
            let built = reqwest::blocking::Client::builder()
                .user_agent(jcode_provider_core::JCODE_USER_AGENT)
                .build();
            match built {
                Ok(client) => Some(client),
                Err(err) => {
                    logging::warn(&format!("telemetry HTTP client build failed: {err}"));
                    None
                }
            }
        })
        .as_ref()
}

pub(crate) fn post_payload(payload: serde_json::Value, timeout: Duration) -> bool {
    if TELEMETRY_PERMANENTLY_REJECTED.load(Ordering::Relaxed) {
        return false;
    }
    let Some(client) = http_client() else {
        logging::warn("telemetry HTTP client failed to build; dropping payload");
        return false;
    };
    match client
        .post(TELEMETRY_ENDPOINT)
        .timeout(timeout)
        .json(&payload)
        .send()
    {
        Ok(response) if response.status().is_success() => true,
        Ok(response) => {
            let status = response.status();
            if telemetry_status_is_permanent(status.as_u16()) {
                TELEMETRY_PERMANENTLY_REJECTED.store(true, Ordering::Relaxed);
                logging::warn(&format!(
                    "telemetry endpoint permanently rejected payload with HTTP {status}; suppressing telemetry delivery for this process"
                ));
            } else {
                logging::warn(&format!(
                    "telemetry endpoint temporarily rejected payload with HTTP {status}"
                ));
            }
            false
        }
        Err(err) => {
            logging::warn(&format!("telemetry payload send failed: {err}"));
            false
        }
    }
}

pub(crate) fn post_payload_with_retry(payload: serde_json::Value, timeout: Duration) -> bool {
    const RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(200), Duration::from_millis(800)];
    if post_payload(payload.clone(), timeout) {
        return true;
    }
    for delay in RETRY_DELAYS {
        if TELEMETRY_PERMANENTLY_REJECTED.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(delay);
        if post_payload(payload.clone(), timeout) {
            return true;
        }
    }
    false
}

pub(crate) fn post_transcript_payload(payload: serde_json::Value, timeout: Duration) -> bool {
    let Some(client) = http_client() else {
        logging::warn("telemetry HTTP client failed to build; dropping transcript");
        return false;
    };
    match client
        .post(TRANSCRIPT_ENDPOINT)
        .timeout(timeout)
        .json(&payload)
        .send()
    {
        Ok(response) if response.status().is_success() => true,
        Ok(response) => {
            logging::warn(&format!(
                "transcript endpoint rejected upload with HTTP {}",
                response.status()
            ));
            false
        }
        Err(err) => {
            logging::warn(&format!("transcript upload failed: {err}"));
            false
        }
    }
}

pub(crate) fn telemetry_status_is_permanent(status: u16) -> bool {
    (400..500).contains(&status) && !matches!(status, 408 | 425 | 429)
}

pub(crate) fn spawn_background_worker<F>(capacity: usize, mut deliver: F) -> std::io::Result<SyncSender<Value>>
where
    F: FnMut(Value) + Send + 'static,
{
    let (sender, receiver) = sync_channel(capacity);
    std::thread::Builder::new()
        .name("jcode-telemetry".to_string())
        .spawn(move || {
            while let Ok(payload) = receiver.recv() {
                deliver(payload);
            }
        })?;
    Ok(sender)
}

/// Lazily start a telemetry background sender, returning `None` when the
/// worker thread cannot be spawned. The fallback lets callers drop payloads
/// rather than crash the host on thread-creation failure.
pub(crate) fn init_background_sender<F>(capacity: usize, mut deliver: F) -> Option<SyncSender<Value>>
where
    F: FnMut(Value) + Send + 'static,
{
    match spawn_background_worker(capacity, deliver) {
        Ok(sender) => Some(sender),
        Err(_) => None,
    }
}

/// initialize one OnceLock-held worker and borrow it.
pub(crate) fn sender<'a>(
    slot: &'a OnceLock<Option<SyncSender<Value>>>,
    capacity: usize,
    deliver: impl Fn(Value) + Send + 'static,
) -> Option<&'a SyncSender<Value>> {
    slot.get_or_init(|| init_background_sender(capacity, deliver))
        .as_ref()
}

pub(crate) fn background_sender() -> Option<&'static SyncSender<Value>> {
    // A worker thread has no caller to report to: delivery outcome is
    // intentionally discarded, and both callees already log their own failure.
    sender(&TELEMETRY_BACKGROUND_SENDER, BACKGROUND_QUEUE_CAPACITY, |payload| {
        drop(post_payload_with_retry(payload, ASYNC_SEND_TIMEOUT));
    })
}

pub(crate) fn transcript_background_sender() -> Option<&'static SyncSender<Value>> {
    sender(&TRANSCRIPT_BACKGROUND_SENDER, 64, |payload| {
        drop(post_transcript_payload(payload, ASYNC_SEND_TIMEOUT));
    })
}

