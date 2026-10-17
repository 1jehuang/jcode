//! Daemon-owned OAuth usage polling, shared by every connected client.
//! The agent/session lock and the socket reader are never held during probes.
use crate::protocol::{AccountUsageSnapshot, ServerEvent};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc};

const REFRESH: Duration = Duration::from_secs(300);
const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(900);

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

    fn install(&mut self, snapshot: Snapshot, now: Instant) {
        let rate_limited = snapshot.anthropic.iter().chain(&snapshot.openai).any(|account| {
            account.error.as_deref().is_some_and(|error| {
                let error = error.to_ascii_lowercase();
                error.contains("429") || error.contains("rate limit") || error.contains("too many requests")
            })
        });
        self.next_fetch = Some(now + if rate_limited { RATE_LIMIT_BACKOFF } else { REFRESH });
        self.snapshot = snapshot;
    }
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

/// Queue an asynchronous response. The shared mutex covers the entire worker,
/// so simultaneous clients coalesce into one fetch and errors get a retry floor.
pub(super) fn request(id: u64, tx: mpsc::UnboundedSender<ServerEvent>) {
    tokio::spawn(async move {
        let mut state = STATE.get_or_init(Mutex::default).lock().await;
        if state.due(Instant::now()) {
            let result = tokio::time::timeout(Duration::from_secs(120), fetch()).await;
            match result {
                Ok(snapshot) => state.install(snapshot, Instant::now()),
                Err(error) => {
                    crate::logging::warn(&format!("Account usage worker failed: {error}"));
                    state.next_fetch = Some(Instant::now() + REFRESH);
                    // A worker failure is unknown usage, never an old quota presented as fresh.
                    state.snapshot = Snapshot::default();
                }
            }
        }
        let _ = tx.send(ServerEvent::AccountUsage {
            id,
            anthropic: state.snapshot.anthropic.clone(),
            openai: state.snapshot.openai.clone(),
        });
    });
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
    AccountUsageSnapshot {
        label: value.label, email: value.email, exhausted: value.exhausted,
        primary_label: value.primary_label, five_hour_ratio: value.five_hour_ratio,
        secondary_label: value.secondary_label, seven_day_ratio: value.seven_day_ratio,
        resets_at: value.resets_at,
        // Raw endpoint failures can contain request diagnostics. Only send a safe status.
        error: value.error.map(|error| {
            let lower = error.to_ascii_lowercase();
            if lower.contains("429") || lower.contains("rate limit") || lower.contains("too many requests") {
                "Usage temporarily rate limited (429)".to_string()
            } else {
                "Usage unavailable".to_string()
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_deadline_and_rate_limit_backoff() {
        let now = Instant::now();
        let mut state = State::default();
        assert!(state.due(now));
        state.install(Snapshot::default(), now);
        assert!(!state.due(now + Duration::from_secs(299)));
        assert!(state.due(now + REFRESH));
        let account = wire_snapshot(crate::usage::AccountUsageSnapshot {
            label: "test".into(), email: None, exhausted: false, primary_label: None,
            five_hour_ratio: None, secondary_label: None, seven_day_ratio: None,
            resets_at: None, error: Some("HTTP 429 secret diagnostic".into()),
        });
        assert_eq!(account.error.as_deref(), Some("Usage temporarily rate limited (429)"));
        state.install(Snapshot { anthropic: vec![account], ..Default::default() }, now);
        assert!(!state.due(now + REFRESH));
        assert!(state.due(now + RATE_LIMIT_BACKOFF));
    }
}
