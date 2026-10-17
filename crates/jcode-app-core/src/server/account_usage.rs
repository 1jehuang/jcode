//! Daemon-owned OAuth usage polling, shared by every connected client.
//! The agent/session lock and the socket reader are never held during probes.
use crate::protocol::{AccountUsageSnapshot, ServerEvent};
use std::future::Future;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc};

// Re-enumerate accounts promptly. Individual endpoint caches own their five
// minute freshness and fifteen minute 429 backoff, independently per account.
const REFRESH: Duration = Duration::from_secs(30);
const WORKER_BACKOFF: Duration = Duration::from_secs(300);

#[derive(Clone, Default)]
struct Snapshot {
    anthropic: Vec<AccountUsageSnapshot>,
    openai: Vec<AccountUsageSnapshot>,
}

#[derive(Default)]
struct State {
    snapshot: Snapshot,
    next_fetch: Option<Instant>,
}

impl State {
    fn due(&self, now: Instant) -> bool {
        self.next_fetch.is_none_or(|deadline| now >= deadline)
    }
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

/// Queue an asynchronous response. The shared mutex covers the entire worker,
/// so simultaneous clients coalesce into one fetch and errors get a retry floor.
pub(super) fn request(id: u64, tx: mpsc::UnboundedSender<ServerEvent>) {
    tokio::spawn(async move {
        let snapshot = snapshot_with(STATE.get_or_init(Mutex::default), fetch).await;
        let _ = tx.send(ServerEvent::AccountUsage {
            id,
            anthropic: snapshot.anthropic,
            openai: snapshot.openai,
        });
    });
}

async fn snapshot_with<F, Fut>(state: &Mutex<State>, fetch: F) -> Snapshot
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Snapshot>,
{
    let mut state = state.lock().await;
    if state.due(Instant::now()) {
        match tokio::time::timeout(Duration::from_secs(120), fetch()).await {
            Ok(snapshot) => {
                state.snapshot = snapshot;
                state.next_fetch = Some(Instant::now() + REFRESH);
            }
            Err(error) => {
                crate::logging::warn(&format!("Account usage worker failed: {error}"));
                state.next_fetch = Some(Instant::now() + WORKER_BACKOFF);
                // A worker failure is unknown usage, never an old quota presented as fresh.
                state.snapshot = Snapshot::default();
            }
        }
    }
    state.snapshot.clone()
}

async fn fetch() -> Snapshot {
    use crate::usage::MultiAccountProviderKind;
    let (anthropic, openai) = tokio::join!(
        crate::usage::fetch_oauth_account_usage(MultiAccountProviderKind::Anthropic),
        crate::usage::fetch_oauth_account_usage(MultiAccountProviderKind::OpenAI),
    );
    Snapshot {
        anthropic: anthropic.into_iter().map(wire_snapshot).collect(),
        openai: openai.into_iter().map(wire_snapshot).collect(),
    }
}

fn wire_snapshot(value: crate::usage::AccountUsageSnapshot) -> AccountUsageSnapshot {
    let error = value.error.map(|error| {
        let lower = error.to_ascii_lowercase();
        if lower.contains("429")
            || lower.contains("rate limit")
            || lower.contains("too many requests")
        {
            "Usage temporarily rate limited (429)".to_string()
        } else if lower.contains("401") || lower.contains("unauthorized") {
            "Usage authentication failed (401)".to_string()
        } else {
            "Usage unavailable".to_string()
        }
    });
    let failed = error.is_some();
    AccountUsageSnapshot {
        label: value.label,
        email: value.email,
        exhausted: !failed && value.exhausted,
        primary_label: value.primary_label,
        five_hour_ratio: (!failed).then_some(value.five_hour_ratio).flatten(),
        secondary_label: value.secondary_label,
        seven_day_ratio: (!failed).then_some(value.seven_day_ratio).flatten(),
        resets_at: value.resets_at,
        // Raw endpoint failures can contain request diagnostics. Only send a safe status.
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn account(error: Option<&str>) -> crate::usage::AccountUsageSnapshot {
        crate::usage::AccountUsageSnapshot {
            label: "test".into(),
            email: None,
            exhausted: true,
            primary_label: None,
            five_hour_ratio: Some(1.0),
            secondary_label: None,
            seven_day_ratio: None,
            resets_at: None,
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn refresh_deadline_and_safe_error_statuses() {
        let now = Instant::now();
        let mut state = State::default();
        assert!(state.due(now));
        state.next_fetch = Some(now + REFRESH);
        assert!(!state.due(now + Duration::from_secs(29)));
        assert!(state.due(now + REFRESH));
        for (raw, safe) in [
            (
                "HTTP 429 secret diagnostic",
                "Usage temporarily rate limited (429)",
            ),
            (
                "HTTP 401 secret diagnostic",
                "Usage authentication failed (401)",
            ),
            ("secret transport diagnostic", "Usage unavailable"),
        ] {
            let account = wire_snapshot(account(Some(raw)));
            assert_eq!(account.error.as_deref(), Some(safe));
            assert!(!account.exhausted);
            assert!(account.five_hour_ratio.is_none());
        }
        assert_eq!(wire_snapshot(account(None)).five_hour_ratio, Some(1.0));
    }

    #[tokio::test]
    async fn simultaneous_clients_coalesce_and_receive_same_snapshot() {
        let state = Mutex::new(State::default());
        let calls = AtomicUsize::new(0);
        let worker = || async {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            Snapshot {
                openai: vec![wire_snapshot(account(None))],
                ..Default::default()
            }
        };
        let (first, second) =
            tokio::join!(snapshot_with(&state, worker), snapshot_with(&state, worker));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(first.openai, second.openai);
        assert_eq!(first.openai[0].label, "test");
        state.lock().await.next_fetch = Some(Instant::now() - Duration::from_secs(1));
        let refreshed = snapshot_with(&state, worker).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(refreshed.openai, first.openai);
    }
}
