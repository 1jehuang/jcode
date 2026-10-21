//! Account-keyed subscription usage. Never reads or changes the active login.
use crate::usage::ProviderUsage;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

type Key = (String, String);
#[derive(Default)]
struct Snapshot {
    credential_fingerprint: u64,
    report: Option<ProviderUsage>,
    fetched: Option<Instant>,
    loading: bool,
    error: Option<String>,
}
impl Snapshot {
    fn complete(&mut self, fingerprint: u64, report: ProviderUsage) -> bool {
        if self.credential_fingerprint != fingerprint {
            return false;
        }
        self.loading = false;
        self.fetched = Some(Instant::now());
        self.error = report.error.clone();
        if report.error.is_none() || self.report.is_none() {
            self.report = Some(report);
        }
        true
    }
}
#[derive(Default)]
struct Cache {
    accounts: HashMap<Key, Snapshot>,
    changed: bool,
    scanned: Option<Instant>,
}
fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
const TTL: Duration = Duration::from_secs(60);

fn credential_fingerprint(access: &str, refresh: &str) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    access.hash(&mut hash);
    refresh.hash(&mut hash);
    hash.finish()
}

/// Start only missing/stale requests. Safe to call repeatedly while the overlay is open.
pub(crate) fn start_account_usage_refresh() {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    {
        let mut state = cache().lock().unwrap_or_else(|p| p.into_inner());
        if state
            .scanned
            .is_some_and(|at| at.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        state.scanned = Some(Instant::now());
    }
    let mut keys = Vec::new();
    if let Ok(accounts) = crate::auth::claude::list_accounts() {
        keys.extend(accounts.into_iter().map(|a| {
            (
                ("claude".to_string(), a.label),
                credential_fingerprint(&a.access, &a.refresh),
            )
        }));
    }
    if let Ok(accounts) = crate::auth::codex::list_accounts() {
        keys.extend(accounts.into_iter().map(|a| {
            (
                ("openai".to_string(), a.label),
                credential_fingerprint(&a.access_token, &a.refresh_token),
            )
        }));
    }
    let mut state = cache().lock().unwrap_or_else(|p| p.into_inner());
    state
        .accounts
        .retain(|key, _| keys.iter().any(|(saved, _)| saved == key));
    for (key, fingerprint) in keys {
        let entry = state.accounts.entry(key.clone()).or_default();
        if entry.credential_fingerprint != fingerprint {
            *entry = Snapshot {
                credential_fingerprint: fingerprint,
                ..Default::default()
            };
        }
        if entry.loading || entry.fetched.is_some_and(|at| at.elapsed() < TTL) {
            continue;
        }
        entry.loading = true;
        state.changed = true;
        runtime.spawn(async move {
            let report = match tokio::time::timeout(
                Duration::from_secs(45),
                crate::usage::fetch_saved_account_usage(&key.0, &key.1),
            )
            .await
            {
                Ok(report) => report,
                Err(_) => ProviderUsage {
                    error: Some("Usage request timed out".into()),
                    ..Default::default()
                },
            };
            let mut state = cache().lock().unwrap_or_else(|p| p.into_inner());
            // An account removed while its request was in flight must stay removed.
            if let Some(entry) = state.accounts.get_mut(&key) {
                // Preserve old successful limits on errors, but never across credentials.
                let changed = entry.complete(fingerprint, report);
                state.changed |= changed;
            }
        });
    }
}

/// Poll from the UI tick while the account center is visible; true requests an in-place update.
pub(crate) fn poll_account_usage_refresh() -> bool {
    start_account_usage_refresh();
    let mut state = cache().lock().unwrap_or_else(|p| p.into_inner());
    std::mem::take(&mut state.changed)
}

fn remaining(percent: f32) -> Option<f32> {
    percent
        .is_finite()
        .then(|| 100.0 - percent.clamp(0.0, 100.0))
}

/// Only abbreviate known window terms. Keep model/special-limit qualifiers intact.
fn compact_window_name(name: &str) -> String {
    let words: Vec<_> = name.split_whitespace().collect();
    let mut compact = Vec::new();
    let mut recognized = false;
    let mut index = 0;
    while index < words.len() {
        let word = words[index].to_ascii_lowercase();
        let next = words.get(index + 1).map(|word| word.to_ascii_lowercase());
        let (replacement, consumed) = match (word.as_str(), next.as_deref()) {
            ("5" | "five", Some("hour" | "hours")) => (Some("5h"), 2),
            ("7" | "seven", Some("day" | "days")) | ("1" | "one", Some("week")) => (Some("7d"), 2),
            ("5-hour" | "5-hours" | "five-hour" | "5h" | "session", _) => (Some("5h"), 1),
            ("7-day" | "7-days" | "seven-day" | "7d" | "1-week" | "1w" | "weekly", _) => {
                (Some("7d"), 1)
            }
            _ => (None, 1),
        };
        if let Some(replacement) = replacement {
            compact.push(replacement);
            recognized = true;
        } else {
            compact.push(words[index]);
        }
        index += consumed;
    }
    if recognized {
        compact.retain(|word| !word.eq_ignore_ascii_case("window"));
    }
    compact.join(" ")
}

fn presentation(entry: Option<&Snapshot>) -> (String, Vec<(String, String)>) {
    let Some(entry) = entry else {
        return (
            "Usage loading".into(),
            vec![(
                "Subscription usage".into(),
                "Loading saved-login limits".into(),
            )],
        );
    };
    let stale = entry
        .report
        .as_ref()
        .is_some_and(|report| !report.limits.is_empty())
        && (entry.error.is_some()
            || entry.loading
            || entry.fetched.is_none_or(|at| at.elapsed() >= TTL));
    let mut details = vec![(
        "Usage status".into(),
        match (&entry.error, entry.loading, stale) {
            (Some(error), _, true) => format!("Stale limits; refresh failed: {error}"),
            (Some(error), _, _) => format!("Unavailable: {error}"),
            (_, true, true) => "Stale limits; refreshing".into(),
            (_, true, _) => "Loading".into(),
            _ => "Loaded".into(),
        },
    )];
    let Some(report) = &entry.report else {
        return ("Usage loading".into(), details);
    };
    let mut capacities = Vec::new();
    for limit in &report.limits {
        let capacity = remaining(limit.usage_percent)
            .map(|n| format!("{n:.0}% left"))
            .unwrap_or_else(|| "remaining unknown".into());
        capacities.push(format!("{} {capacity}", compact_window_name(&limit.name)));
        details.push((
            limit.name.clone(),
            format!(
                "{capacity}; used {}; resets {}",
                if limit.usage_percent.is_finite() {
                    format!("{:.0}%", limit.usage_percent.clamp(0.0, 100.0))
                } else {
                    "unknown".into()
                },
                limit.resets_at.as_deref().unwrap_or("unknown")
            ),
        ));
    }
    details.extend(report.extra_info.clone());
    let mut summary = if !capacities.is_empty() {
        capacities.join(" · ")
    } else if entry.error.is_some() {
        "Usage unavailable".into()
    } else {
        "Usage unsupported / no limits reported".into()
    };
    if stale {
        summary.push_str(" (stale)");
    }
    if entry.loading {
        summary.push_str(" · refreshing");
    }
    (summary, details)
}

pub(crate) fn account_usage_presentation(
    provider: &str,
    label: &str,
) -> (String, Vec<(String, String)>) {
    let state = cache().lock().unwrap_or_else(|p| p.into_inner());
    presentation(state.accounts.get(&(provider.into(), label.into())))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_window_labels_preserve_special_and_model_distinctions() {
        for (name, expected) in [
            ("5-hour window", "5h"),
            ("Session", "5h"),
            ("5 hours", "5h"),
            ("Weekly", "7d"),
            ("7-day window", "7d"),
            ("1 week", "7d"),
            ("7-day Opus window", "7d Opus"),
            ("Codex Spark 5-hour window", "Codex Spark 5h"),
            ("Codex 1w", "Codex 7d"),
            ("Review requests", "Review requests"),
        ] {
            assert_eq!(compact_window_name(name), expected, "{name}");
        }
    }
    #[test]
    fn summaries_are_compact_but_details_keep_full_window_names() {
        let entry = Snapshot {
            report: Some(ProviderUsage {
                limits: vec![
                    crate::usage::UsageLimit {
                        name: "5-hour window".into(),
                        usage_percent: 25.0,
                        resets_at: Some("first-reset".into()),
                    },
                    crate::usage::UsageLimit {
                        name: "7-day window".into(),
                        usage_percent: 75.0,
                        resets_at: Some("second-reset".into()),
                    },
                ],
                ..Default::default()
            }),
            fetched: Some(Instant::now()),
            ..Default::default()
        };
        let (summary, details) = presentation(Some(&entry));
        assert_eq!(summary, "5h 75% left · 7d 25% left");
        assert_eq!(details[1].0, "5-hour window");
        assert!(details[1].1.contains("first-reset"));
        assert_eq!(details[2].0, "7-day window");
        assert!(details[2].1.contains("second-reset"));
    }
    #[test]
    fn failed_refresh_preserves_limits_but_old_credential_response_is_ignored() {
        let mut entry = Snapshot {
            credential_fingerprint: 42,
            loading: true,
            ..Default::default()
        };
        let report = ProviderUsage {
            limits: vec![crate::usage::UsageLimit {
                name: "Weekly".into(),
                usage_percent: 40.0,
                resets_at: Some("reset-one".into()),
            }],
            ..Default::default()
        };
        assert!(!entry.complete(41, report.clone()));
        assert!(entry.loading);
        assert!(entry.report.is_none());
        assert!(entry.complete(42, report));
        assert!(entry.complete(
            42,
            ProviderUsage {
                error: Some("offline".into()),
                ..Default::default()
            }
        ));
        assert_eq!(entry.report.as_ref().unwrap().limits[0].usage_percent, 40.0);
        assert_eq!(entry.error.as_deref(), Some("offline"));
        assert!(presentation(Some(&entry)).0.contains("stale"));
    }
    #[test]
    fn separate_accounts_keep_their_own_remaining_and_resets() {
        let mut state = Cache::default();
        for (label, used, reset) in [
            ("work", 25.0, "2026-10-22T01:00:00Z"),
            ("personal", 90.0, "2026-10-23T02:00:00Z"),
        ] {
            state.accounts.insert(
                ("openai".into(), label.into()),
                Snapshot {
                    report: Some(ProviderUsage {
                        limits: vec![crate::usage::UsageLimit {
                            name: "Weekly".into(),
                            usage_percent: used,
                            resets_at: Some(reset.into()),
                        }],
                        ..Default::default()
                    }),
                    fetched: Some(Instant::now()),
                    ..Default::default()
                },
            );
        }
        let (work, work_details) =
            presentation(state.accounts.get(&("openai".into(), "work".into())));
        let (personal, personal_details) =
            presentation(state.accounts.get(&("openai".into(), "personal".into())));
        assert_eq!(work, "7d 75% left");
        assert_eq!(personal, "7d 10% left");
        assert!(work_details[1].1.contains("2026-10-22T01:00:00Z"));
        assert!(personal_details[1].1.contains("2026-10-23T02:00:00Z"));
    }
    #[test]
    fn credential_replacement_changes_cache_identity_without_storing_tokens() {
        assert_ne!(
            credential_fingerprint("old", "refresh"),
            credential_fingerprint("new", "refresh")
        );
        assert_ne!(
            credential_fingerprint("access", "old"),
            credential_fingerprint("access", "new")
        );
        assert_eq!(
            credential_fingerprint("same", "same"),
            credential_fingerprint("same", "same")
        );
    }
    #[test]
    fn successful_limits_remain_visible_but_marked_stale_after_error() {
        let entry = Snapshot {
            report: Some(ProviderUsage {
                limits: vec![crate::usage::UsageLimit {
                    name: "5h".into(),
                    usage_percent: 40.0,
                    resets_at: Some("2026-10-22T00:00:00Z".into()),
                }],
                ..Default::default()
            }),
            error: Some("offline".into()),
            fetched: Some(Instant::now()),
            ..Default::default()
        };
        let (summary, details) = presentation(Some(&entry));
        assert!(summary.starts_with("5h 60% left"));
        assert!(summary.contains("stale"));
        assert!(details[0].1.contains("refresh failed: offline"));
        assert!(details[1].1.contains("2026-10-22T00:00:00Z"));
    }
    #[test]
    fn empty_success_is_unsupported_and_missing_snapshot_is_loading() {
        let entry = Snapshot {
            report: Some(ProviderUsage::default()),
            fetched: Some(Instant::now()),
            ..Default::default()
        };
        assert!(presentation(Some(&entry)).0.contains("unsupported"));
        assert!(presentation(None).0.contains("loading"));
    }
    #[test]
    fn remaining_capacity_clamps_and_rejects_nonfinite() {
        assert_eq!(remaining(-12.0), Some(100.0));
        assert_eq!(remaining(150.0), Some(0.0));
        assert_eq!(remaining(23.0), Some(77.0));
        assert_eq!(remaining(f32::NAN), None);
        assert_eq!(remaining(f32::INFINITY), None);
    }
    #[test]
    fn errors_are_not_zero_usage_and_stale_limits_stay_explicit() {
        let entry = Snapshot {
            report: Some(ProviderUsage {
                error: Some("offline".into()),
                ..Default::default()
            }),
            error: Some("offline".into()),
            fetched: Some(Instant::now()),
            ..Default::default()
        };
        let (summary, details) = presentation(Some(&entry));
        assert!(summary.contains("unavailable"));
        assert!(!summary.contains("100%"));
        assert!(details[0].1.contains("offline"));
    }
    #[test]
    fn account_keys_isolate_provider_and_label() {
        let mut state = Cache::default();
        state.accounts.insert(
            ("claude".into(), "work".into()),
            Snapshot {
                error: Some("expired".into()),
                ..Default::default()
            },
        );
        assert!(
            state
                .accounts
                .get(&("openai".into(), "work".into()))
                .is_none()
        );
        assert!(
            state
                .accounts
                .get(&("claude".into(), "personal".into()))
                .is_none()
        );
    }
}
