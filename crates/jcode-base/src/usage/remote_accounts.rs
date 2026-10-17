//! TUI-only, credential-free mirror of daemon account usage.
use super::{AccountUsageSnapshot, MultiAccountProviderKind};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

#[derive(Default)]
struct RemoteAccounts {
    received: Option<Instant>,
    anthropic: Vec<jcode_protocol::AccountUsageSnapshot>,
    openai: Vec<jcode_protocol::AccountUsageSnapshot>,
}

static REMOTE: OnceLock<Mutex<RemoteAccounts>> = OnceLock::new();

/// Enable daemon-only usage reads, even when the daemon reports no accounts.
/// Call with empty vectors on connection changes to discard another host's data.
pub fn set_remote_account_usage(
    anthropic: Vec<jcode_protocol::AccountUsageSnapshot>,
    openai: Vec<jcode_protocol::AccountUsageSnapshot>,
) {
    if let Ok(mut state) = REMOTE.get_or_init(Mutex::default).lock() {
        *state = RemoteAccounts { received: Some(Instant::now()), anthropic, openai };
    }
}

pub(super) fn cached(
    provider: MultiAccountProviderKind,
    label: &str,
) -> Option<Option<AccountUsageSnapshot>> {
    let remote = REMOTE.get()?;
    // Once enabled, contention or expired data must never fall back to local credentials.
    let Ok(state) = remote.try_lock() else { return Some(None) };
    Some(state.lookup(provider, label, Instant::now()))
}

impl RemoteAccounts {
    fn lookup(&self, provider: MultiAccountProviderKind, label: &str, now: Instant) -> Option<AccountUsageSnapshot> {
        if now.duration_since(self.received?) > super::CACHE_DURATION + std::time::Duration::from_secs(60) {
            return None;
        }
        let accounts = match provider {
            MultiAccountProviderKind::Anthropic => &self.anthropic,
            MultiAccountProviderKind::OpenAI => &self.openai,
        };
        let data = accounts.iter().find(|data| data.label == label.trim())?;
        Some(AccountUsageSnapshot {
            label: data.label.clone(), email: data.email.clone(), exhausted: data.exhausted,
            primary_label: data.primary_label.clone(), five_hour_ratio: data.five_hour_ratio,
            secondary_label: data.secondary_label.clone(), seven_day_ratio: data.seven_day_ratio,
            resets_at: data.resets_at.clone(), error: data.error.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_remote_and_expired_remote_are_unknown() {
        let now = Instant::now();
        let mut state = RemoteAccounts { received: Some(now), ..Default::default() };
        assert!(state.lookup(MultiAccountProviderKind::OpenAI, "local-only", now).is_none());
        state.openai.push(jcode_protocol::AccountUsageSnapshot {
            label: "remote".into(), email: None, exhausted: false,
            primary_label: Some("5h".into()), five_hour_ratio: Some(0.4),
            secondary_label: None, seven_day_ratio: None, resets_at: None, error: None,
        });
        assert_eq!(state.lookup(MultiAccountProviderKind::OpenAI, "remote", now).unwrap().five_hour_ratio, Some(0.4));
        assert!(state.lookup(MultiAccountProviderKind::Anthropic, "remote", now).is_none());
        assert!(state.lookup(MultiAccountProviderKind::OpenAI, "remote", now + std::time::Duration::from_secs(361)).is_none());
    }
}
