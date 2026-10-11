//! Once-per-reason logging for LaTeX fallbacks.
//!
//! Redraws re-read cached render failures, so a last-error-only check re-logged
//! alternating reasons on every frame. Distinct reasons are remembered instead,
//! bounded so a pathological session cannot grow the set without limit.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

static LOG_HOOK: LazyLock<Mutex<fn(&str)>> = LazyLock::new(|| Mutex::new(|_| {}));
static REPORTED_ERRORS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
static REPORTED_ERRORS_FULL: AtomicBool = AtomicBool::new(false);
const REPORTED_ERROR_LIMIT: usize = 256;

pub(crate) fn set_log_hook(hook: fn(&str)) {
    if let Ok(mut current) = LOG_HOOK.lock() {
        *current = hook;
    }
}

/// Log each distinct fallback reason once per process. Redraws re-read cached
/// failures, and alternating reasons defeated a last-error-only check.
/// Once the set is full, new reasons are dropped rather than evicting old
/// ones, so cached failures can never be logged again on redraw. The first
/// dropped reason is logged with a one-time notice so the cap is never silent.
pub(crate) fn report_error(error: &str) {
    let message = match REPORTED_ERRORS.lock() {
        Ok(seen) if seen.contains(error) => return,
        Ok(mut seen) if seen.len() < REPORTED_ERROR_LIMIT => {
            seen.insert(error.to_string());
            error.to_string()
        }
        Ok(_) if !REPORTED_ERRORS_FULL.swap(true, Ordering::Relaxed) => {
            format!(
                "{error} (LaTeX fallback: {REPORTED_ERROR_LIMIT} distinct reasons logged, \
                 suppressing further new reasons)"
            )
        }
        _ => return,
    };
    if let Ok(hook) = LOG_HOOK.lock() {
        hook(&message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    static OVERFLOW_NOTICES: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn reason_cap_logs_one_overflow_notice_instead_of_going_silent() {
        set_log_hook(|message| {
            if message.contains("suppressing further new reasons") {
                OVERFLOW_NOTICES.fetch_add(1, Ordering::SeqCst);
            }
        });
        for _ in 0..2 {
            for i in 0..REPORTED_ERROR_LIMIT + 8 {
                report_error(&format!("overflow probe {i}"));
            }
        }
        assert_eq!(OVERFLOW_NOTICES.load(Ordering::SeqCst), 1);
    }
}
