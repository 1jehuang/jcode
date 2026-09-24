//! Keep the long-lived bridge process small.
//!
//! The bridge is a thin JSON translator, yet glibc's default of one malloc
//! arena per core let every Tokio worker and session-scan thread retain its
//! own 64 MiB-aligned arena. Measured on a 16-core machine, one idle desktop
//! connection held ~94 MB of anonymous memory, almost all freed-but-retained
//! arena pages from listing and attaching sessions.
//!
//! Two arenas are plenty for a handful of connections, and trimming after the
//! bursty list/attach work returns those pages to the OS.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Arena cap applied at startup. `MALLOC_ARENA_MAX` still wins when set.
const DEFAULT_ARENA_MAX: i32 = 2;
/// Never trim more often than this; `malloc_trim` walks every arena.
const MIN_TRIM_INTERVAL_MS: u64 = 2_000;

static LAST_TRIM_MS: AtomicU64 = AtomicU64::new(0);

/// Configure the system allocator. Call once, before spawning threads.
pub fn configure() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        if std::env::var_os("MALLOC_ARENA_MAX").is_none() {
            // SAFETY: mallopt only adjusts allocator parameters.
            unsafe {
                libc::mallopt(libc::M_ARENA_MAX, DEFAULT_ARENA_MAX);
            }
        }
    }
}

/// Return freed heap pages to the OS, at most once per interval.
pub fn trim() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    let last = LAST_TRIM_MS.load(Ordering::Relaxed);
    if now.saturating_sub(last) < MIN_TRIM_INTERVAL_MS
        || LAST_TRIM_MS
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        return;
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim is thread-safe and only releases free pages.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// Whether a request typically allocates large transient buffers.
pub fn request_is_heavy(request: &serde_json::Value) -> bool {
    matches!(
        request["req"].as_str(),
        Some(
            "list_sessions"
                | "attach_session"
                | "create_session"
                | "get_history"
                | "peek_session"
                | "read_file"
                | "find_files"
                | "search_text"
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn heavy_requests_are_classified() {
        assert!(request_is_heavy(&json!({"req": "list_sessions"})));
        assert!(request_is_heavy(&json!({"req": "attach_session"})));
        assert!(!request_is_heavy(&json!({"req": "ping"})));
        assert!(!request_is_heavy(&json!({})));
    }

    #[test]
    fn streaming_events_do_not_enter_block_in_place() {
        use crate::translate::legacy_event_may_block;
        for kind in ["text_delta", "reasoning_delta", "tool_start", "tool_exec", "done"] {
            assert!(!legacy_event_may_block(&json!({"type": kind})), "{kind}");
        }
        for kind in ["state", "history", "split_response", "side_panel_state"] {
            assert!(legacy_event_may_block(&json!({"type": kind})), "{kind}");
        }
    }

    #[test]
    fn trim_is_rate_limited_and_safe() {
        trim();
        let first = LAST_TRIM_MS.load(Ordering::Relaxed);
        trim();
        assert_eq!(LAST_TRIM_MS.load(Ordering::Relaxed), first);
    }
}
