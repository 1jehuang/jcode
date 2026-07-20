//! Retained-heap release for active and idle clients.
//!
//! Long-lived clients accumulate glibc arena retention while rendering remote
//! turns, and a workspace often has many client processes. The periodic growth
//! watchdog therefore runs during active work with a conservative per-client
//! threshold. Idle clients additionally get edge-triggered and periodic trims
//! because heartbeats and remote snapshots can regrow retention without a turn
//! completion event.

use super::*;

/// How long the client must be quiet before an idle trim fires. Matches the
/// deep-idle redraw threshold so trims never race active rendering.
const IDLE_TRIM_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

/// Recheck retained-heap growth while a client remains idle. A client can keep
/// receiving remote snapshots after its once-per-idle trim without becoming
/// "active" again, so the original edge-triggered trim alone can miss later
/// allocator growth for the rest of a long idle period.
const RETENTION_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// A server is one process, while every open TUI has its own allocator. Letting
/// each client use the shared 64 MiB default can retain hundreds of MiB in
/// aggregate across a normal multi-session workspace. Keep the per-client
/// growth budget smaller while still honoring an explicitly lower global
/// threshold or the global disable switch.
const CLIENT_RETENTION_TRIM_THRESHOLD_BYTES: u64 = 16 * 1024 * 1024;

/// Re-trim cadence while a client stays idle. Heartbeats and remote snapshots
/// keep churning small allocations on idle clients, so retention regrows
/// 20-45 MB over long idle stretches: enough to matter across dozens of
/// clients, but below the growth-watchdog threshold, so the once-per-idle
/// trim alone leaves it resident forever. A periodic malloc_trim at idle is
/// nearly free, so sweep it back on a fixed cadence.
const IDLE_RETRIM_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300);

#[derive(Default)]
pub(super) struct IdleHeapRelease {
    /// True once the current idle period has already been trimmed. Reset when
    /// activity resumes so the next idle period trims again.
    trimmed_this_idle_period: bool,
    last_retention_check: Option<std::time::Instant>,
    /// When the most recent idle trim (edge, watchdog, or periodic) ran, used
    /// to schedule the next periodic re-trim within the same idle period.
    last_idle_trim: Option<std::time::Instant>,
}

impl App {
    /// Called from the periodic tick loops (local and remote). Trims retained
    /// heap once per idle period, going quiet until the next busy->idle edge.
    pub(super) fn maybe_release_idle_heap(&mut self) {
        let idle = !crate::tui::TuiState::is_processing(self)
            && self.streaming.streaming_text.is_empty()
            && crate::tui::TuiState::time_since_activity(self)
                .is_none_or(|since| since >= IDLE_TRIM_AFTER);

        let now = std::time::Instant::now();
        if retention_check_due(self.idle_heap_release.last_retention_check, now) {
            self.idle_heap_release.last_retention_check = Some(now);
            let threshold = client_retention_trim_threshold_bytes(
                crate::process_memory::retention_trim_threshold_bytes(),
            );
            if threshold != u64::MAX
                && crate::process_memory::release_retained_heap_if_excessive(
                    "client_retention_watchdog",
                    threshold,
                    RETENTION_CHECK_INTERVAL,
                )
            {
                if idle {
                    self.idle_heap_release.trimmed_this_idle_period = true;
                    self.idle_heap_release.last_idle_trim = Some(now);
                }
                return;
            }
        }

        if !idle {
            self.idle_heap_release.trimmed_this_idle_period = false;
            self.idle_heap_release.last_idle_trim = None;
            return;
        }

        // Below-threshold retention still regrows steadily on idle clients
        // (heartbeats, remote snapshots), so re-trim on a slow cadence even
        // after the once-per-idle trim already ran.
        let periodic_retrim_due = idle_retrim_due(self.idle_heap_release.last_idle_trim, now);
        if self.idle_heap_release.trimmed_this_idle_period && !periodic_retrim_due {
            return;
        }

        // Shared debounce with the turn-completion hook, so a turn that just
        // trimmed does not get an immediate duplicate idle trim.
        if crate::process_memory::release_retained_heap_debounced(
            "client_idle",
            std::time::Duration::from_secs(60),
        ) {
            self.idle_heap_release.trimmed_this_idle_period = true;
            self.idle_heap_release.last_idle_trim = Some(now);
        }
    }
}

fn retention_check_due(last_check: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    last_check.is_none_or(|last| now.saturating_duration_since(last) >= RETENTION_CHECK_INTERVAL)
}

fn client_retention_trim_threshold_bytes(global_threshold: u64) -> u64 {
    if global_threshold == u64::MAX {
        u64::MAX
    } else {
        global_threshold.min(CLIENT_RETENTION_TRIM_THRESHOLD_BYTES)
    }
}

/// True when the periodic idle re-trim should fire: a trim already ran this
/// idle period and at least [`IDLE_RETRIM_INTERVAL`] has elapsed since it.
fn idle_retrim_due(last_trim: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    last_trim.is_some_and(|last| now.saturating_duration_since(last) >= IDLE_RETRIM_INTERVAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_check_runs_immediately_then_obeys_interval() {
        let now = std::time::Instant::now();
        assert!(retention_check_due(None, now));
        assert!(!retention_check_due(Some(now), now));
        assert!(retention_check_due(
            Some(now - RETENTION_CHECK_INTERVAL),
            now
        ));
    }

    #[test]
    fn client_retention_threshold_caps_default_but_honors_lower_and_disabled() {
        assert_eq!(
            client_retention_trim_threshold_bytes(64 * 1024 * 1024),
            CLIENT_RETENTION_TRIM_THRESHOLD_BYTES
        );
        assert_eq!(
            client_retention_trim_threshold_bytes(8 * 1024 * 1024),
            8 * 1024 * 1024
        );
        assert_eq!(client_retention_trim_threshold_bytes(u64::MAX), u64::MAX);
    }

    #[test]
    fn idle_retrim_waits_for_first_trim_then_fires_on_cadence() {
        let now = std::time::Instant::now();
        // No trim yet this idle period: the once-per-idle edge trim owns it.
        assert!(!idle_retrim_due(None, now));
        assert!(!idle_retrim_due(Some(now), now));
        assert!(idle_retrim_due(Some(now - IDLE_RETRIM_INTERVAL), now));
    }
}
