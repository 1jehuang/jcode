//! Timing fields are closed labels, never request data or error messages.
use crate::{ErrorKind, Result};
use std::time::{Duration, Instant};

pub(super) fn operation(method: &str, path: &str) -> &'static str {
    match (method, path) {
        ("POST", "/auth/email/start") => "email_start",
        ("POST", "/auth/email/verify") => "email_verify",
        ("POST", "/auth/device") => "device_start",
        ("POST", "/auth/token") => "device_poll",
        ("POST", "/cloud/host/connect") => "host_connect",
        ("POST", "/push/devices") => "push_register",
        ("DELETE", "/push/devices") => "push_unregister",
        ("POST", "/push/badge") => "push_badge",
        ("POST", "/push/send") => "push_send",
        _ => "unknown",
    }
}

pub(super) fn record(
    stage: &'static str,
    operation: &'static str,
    outcome: &'static str,
    elapsed: Duration,
) {
    let line = format!(
        "[CLOUD_TIMING] stage={stage} operation={operation} outcome={outcome} duration_ms={}",
        elapsed.as_millis()
    );
    jcode_logging::info(&line);
    #[cfg(test)]
    EVENTS.with(|events| events.borrow_mut().push(line));
}

pub(super) fn http_outcome<T>(result: &std::result::Result<T, ureq::Error>) -> &'static str {
    match result {
        Ok(_) => "success",
        Err(ureq::Error::Timeout(_)) => "timeout",
        Err(_) => "error",
    }
}

pub(super) fn measured<T>(
    stage: &'static str,
    operation: &'static str,
    run: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let start = Instant::now();
    let result = run();
    let outcome = match &result {
        Ok(_) => "success",
        Err(error) => match error.kind {
            ErrorKind::Cancelled => "cancelled",
            ErrorKind::Timeout | ErrorKind::StartupTimeout => "timeout",
            _ => "error",
        },
    };
    record(stage, operation, outcome, start.elapsed());
    result
}

#[cfg(test)]
thread_local! {
    static EVENTS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(super) fn take_events() -> Vec<String> {
    EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
}
