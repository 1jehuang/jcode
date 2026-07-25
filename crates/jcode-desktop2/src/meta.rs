//! Build identity shown in the masthead: version, update state, and the
//! signed-in account.
//!
//! The TUI shows these on its idle screen, so the desktop shows the same
//! three facts. All decisions are pure functions of plain inputs
//! ([`update_state`], [`account_label`]) with thin IO wrappers around them, so
//! the header can be tested without a home directory, a network, or a window.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Whether a newer binary is waiting to be run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateState {
    /// The running binary is the newest one installed.
    Current,
    /// A newer binary exists on disk; restarting picks it up.
    Available,
    /// Could not tell (no install channel found, unreadable metadata).
    Unknown,
}

impl UpdateState {
    /// Caption text, or `None` when there is nothing worth saying.
    pub fn caption(self) -> Option<&'static str> {
        match self {
            Self::Current => Some("up to date"),
            Self::Available => Some("update ready: restart to apply"),
            Self::Unknown => None,
        }
    }
}

/// Build identity for one frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    /// Human-readable version, e.g. `v0.58.51-dev (b25eaa7de)`.
    pub version: String,
    pub update: UpdateState,
    /// Signed-in account, e.g. `jeremy@example.dev (anthropic)`.
    pub account: Option<String>,
}

impl Default for Meta {
    fn default() -> Self {
        Self {
            version: jcode_build_meta::version().to_string(),
            update: UpdateState::Unknown,
            account: None,
        }
    }
}

impl Meta {
    /// Read the live values from disk. Cheap (two small file stats and one
    /// small JSON read), but still done once at startup rather than per frame.
    pub fn detect() -> Self {
        Self {
            version: jcode_build_meta::version().to_string(),
            update: detect_update_state(),
            account: detect_account(),
        }
    }

    /// The one thing about this build worth interrupting the user for, or
    /// `None` when everything is normal.
    ///
    /// The app used to print version, update state, and account on every
    /// frame. Nearly always that row said "up to date, signed in", which is
    /// exactly what the user assumes, so it was pure clutter. Only the two
    /// states that need an action survive here.
    pub fn alert(&self) -> Option<String> {
        if self.account.is_none() {
            return Some("not signed in: run `jcode` and use /login".into());
        }
        if self.update == UpdateState::Available {
            return Some("update ready: restart to apply".into());
        }
        None
    }

    /// The full caption row, kept for `--capture`-style diagnostics and any
    /// future about panel. Not drawn on the main surface.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn caption(&self) -> String {
        let mut parts = vec![self.version.clone()];
        if let Some(update) = self.update.caption() {
            parts.push(update.to_string());
        }
        match &self.account {
            Some(account) => parts.push(account.clone()),
            None => parts.push("not signed in".to_string()),
        }
        parts.join("  ·  ")
    }
}

/// Decide the update state from the running binary's mtime and the mtimes of
/// the installed channels. Pure so the comparison is testable.
pub fn update_state(running: Option<SystemTime>, channels: &[Option<SystemTime>]) -> UpdateState {
    let Some(running) = running else {
        return UpdateState::Unknown;
    };
    let mut saw_channel = false;
    for channel in channels.iter().flatten() {
        saw_channel = true;
        // A channel newer than the running payload means a build finished
        // after this process started.
        if *channel > running {
            return UpdateState::Available;
        }
    }
    if saw_channel {
        UpdateState::Current
    } else {
        UpdateState::Unknown
    }
}

/// Format the account label from an auth store's active provider and email.
/// Pure so the display rules are tested without touching `~/.jcode/auth.json`.
pub fn account_label(provider: Option<&str>, email: Option<&str>) -> Option<String> {
    match (provider, email) {
        (Some(provider), Some(email)) => Some(format!("{email} ({provider})")),
        (Some(provider), None) => Some(provider.to_string()),
        (None, Some(email)) => Some(email.to_string()),
        (None, None) => None,
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn detect_update_state() -> UpdateState {
    let running = std::env::current_exe().ok().as_deref().and_then(mtime);
    let Some(home) = home() else {
        return UpdateState::Unknown;
    };
    let channels: Vec<Option<SystemTime>> = ["current", "stable"]
        .iter()
        .map(|channel| {
            mtime(
                &home
                    .join(".jcode/builds")
                    .join(channel)
                    .join("jcode-desktop2"),
            )
            .or_else(|| mtime(&home.join(".jcode/builds").join(channel).join("jcode")))
        })
        .collect();
    update_state(running, &channels)
}

/// Read the active account out of `~/.jcode/auth.json`. Only the account
/// identity is read; tokens are never touched.
fn detect_account() -> Option<String> {
    let path = home()?.join(".jcode/auth.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let object = value.as_object()?;
    // The store keys accounts per provider: `<provider>_accounts` plus
    // `active_<provider>_account`. Find whichever provider is active.
    for (key, active) in object {
        let Some(provider) = key
            .strip_prefix("active_")
            .and_then(|rest| rest.strip_suffix("_account"))
        else {
            continue;
        };
        let label = active.as_str();
        let email = object
            .get(&format!("{provider}_accounts"))
            .and_then(|accounts| accounts.as_array())
            .and_then(|accounts| {
                accounts
                    .iter()
                    .find(|account| match label {
                        Some(label) => account.get("label").and_then(|l| l.as_str()) == Some(label),
                        None => true,
                    })
                    .or_else(|| accounts.first())
            })
            .and_then(|account| account.get("email"))
            .and_then(|email| email.as_str());
        if let Some(label) = account_label(Some(provider), email) {
            return Some(label);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn a_newer_channel_means_an_update_is_ready() {
        assert_eq!(
            update_state(Some(at(100)), &[Some(at(200))]),
            UpdateState::Available
        );
    }

    #[test]
    fn an_older_or_equal_channel_is_current() {
        assert_eq!(
            update_state(Some(at(200)), &[Some(at(100)), Some(at(200))]),
            UpdateState::Current
        );
    }

    #[test]
    fn a_missing_binary_or_channel_is_unknown() {
        assert_eq!(update_state(None, &[Some(at(1))]), UpdateState::Unknown);
        assert_eq!(
            update_state(Some(at(1)), &[None, None]),
            UpdateState::Unknown
        );
        assert_eq!(update_state(Some(at(1)), &[]), UpdateState::Unknown);
    }

    #[test]
    fn account_label_degrades_gracefully() {
        assert_eq!(
            account_label(Some("anthropic"), Some("a@b.dev")).as_deref(),
            Some("a@b.dev (anthropic)")
        );
        assert_eq!(
            account_label(Some("anthropic"), None).as_deref(),
            Some("anthropic")
        );
        assert_eq!(
            account_label(None, Some("a@b.dev")).as_deref(),
            Some("a@b.dev")
        );
        assert_eq!(account_label(None, None), None);
    }

    #[test]
    fn the_caption_lists_version_update_and_account() {
        let meta = Meta {
            version: "v1.2.3".into(),
            update: UpdateState::Available,
            account: Some("a@b.dev (anthropic)".into()),
        };
        assert_eq!(
            meta.caption(),
            "v1.2.3  ·  update ready: restart to apply  ·  a@b.dev (anthropic)"
        );
    }

    #[test]
    fn the_caption_never_shows_a_dangling_separator() {
        let meta = Meta {
            version: "v1.2.3".into(),
            update: UpdateState::Unknown,
            account: None,
        };
        assert_eq!(meta.caption(), "v1.2.3  ·  not signed in");
        assert!(!meta.caption().ends_with('·'));
    }

    #[test]
    fn detect_never_panics_and_always_reports_a_version() {
        let meta = Meta::detect();
        assert!(!meta.version.is_empty());
    }
}

#[cfg(test)]
mod alert_tests {
    use super::*;

    /// The normal case must be silent, or the alert is just the old
    /// always-on clutter with a new name.
    #[test]
    fn a_healthy_current_build_says_nothing() {
        let meta = Meta {
            version: "v1.2.3".into(),
            update: UpdateState::Current,
            account: Some("someone@example.dev (anthropic)".into()),
        };
        assert_eq!(meta.alert(), None);
    }

    /// An unknown update state is not actionable either: the app could not
    /// tell, and guessing would nag the user for nothing.
    #[test]
    fn an_unknown_update_state_is_not_an_alert() {
        let meta = Meta {
            version: "v1.2.3".into(),
            update: UpdateState::Unknown,
            account: Some("someone@example.dev (anthropic)".into()),
        };
        assert_eq!(meta.alert(), None);
    }

    #[test]
    fn a_pending_update_is_reported() {
        let meta = Meta {
            version: "v1.2.3".into(),
            update: UpdateState::Available,
            account: Some("someone@example.dev (anthropic)".into()),
        };
        assert!(meta.alert().is_some_and(|line| line.contains("update")));
    }

    /// Being signed out blocks every turn, so it outranks a pending update.
    #[test]
    fn being_signed_out_outranks_a_pending_update() {
        let meta = Meta {
            version: "v1.2.3".into(),
            update: UpdateState::Available,
            account: None,
        };
        assert!(
            meta.alert().is_some_and(|line| line.contains("signed in")),
            "a signed-out app reported the update instead of the blocker"
        );
    }
}
