//! Recorded ChatGPT usage, deliberately separate from the API-key spend ledger.
//! A stable sidecar lock serializes read/modify/atomic-replace across processes.
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs::OpenOptions, path::Path};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Totals {
    requests: u64,
    input: u64,
    output: u64,
    cached: u64,
    known_usd: f64,
    unpriced_requests: u64,
    incomplete_requests: u64,
}

impl Totals {
    fn add(&mut self, other: &Self) {
        self.requests = self.requests.saturating_add(other.requests);
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cached = self.cached.saturating_add(other.cached);
        self.known_usd += other.known_usd;
        self.unpriced_requests = self
            .unpriced_requests
            .saturating_add(other.unpriced_requests);
        self.incomplete_requests = self
            .incomplete_requests
            .saturating_add(other.incomplete_requests);
    }

    fn display(&self) -> String {
        if self.requests == 0 {
            return "No recorded usage (API-equivalent estimate, not a bill)".into();
        }
        let cost = if self.unpriced_requests == self.requests {
            "cost unknown (pricing unavailable)".into()
        } else if self.unpriced_requests > 0 {
            format!(
                "${:.4} known + unknown cost ({} unpriced responses)",
                self.known_usd, self.unpriced_requests
            )
        } else {
            format!("${:.4}", self.known_usd)
        };
        let partial = if self.incomplete_requests > 0 {
            "; partial token counts"
        } else {
            ""
        };
        format!(
            "{} input / {} output tokens ({} cached input), {} API-equivalent estimate, not a bill; recorded only{}",
            self.input, self.output, self.cached, cost, partial
        )
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct AccountUsage {
    first_recorded: Option<i64>,
    lifetime: Totals,
    // Local calendar dates, not fixed 24-hour windows, so DST is respected.
    days: BTreeMap<String, Totals>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Store {
    accounts: BTreeMap<String, AccountUsage>,
}

fn path() -> anyhow::Result<std::path::PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("openai_oauth_usage.json"))
}

fn load(path: &Path) -> anyhow::Result<Store> {
    match std::fs::metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Store::default()),
        Err(error) => return Err(error.into()),
    }
    // Never replace an unreadable ledger with an empty one.
    crate::storage::read_json(path)
}

fn update(path: &Path, label: &str, totals: &Totals, now: DateTime<Local>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    lock.lock()?;
    let mut store = load(path)?;
    let account = store.accounts.entry(label.to_string()).or_default();
    account.first_recorded = Some(
        account
            .first_recorded
            .map_or(now.timestamp(), |first| first.min(now.timestamp())),
    );
    account.lifetime.add(totals);
    account
        .days
        .entry(now.format("%Y-%m-%d").to_string())
        .or_default()
        .add(totals);
    crate::storage::write_json(path, &store)?;
    Ok(()) // Dropping the stable lock file releases the OS lock.
}

fn priced_usage(
    model: &str,
    tier: Option<&str>,
    input: Option<u64>,
    output: Option<u64>,
    cached: Option<u64>,
) -> Totals {
    let cached = cached.unwrap_or(0);
    let complete = input.is_some() && output.is_some() && cached <= input.unwrap_or(0);
    let cost =
        jcode_provider_core::pricing::openai_api_pricing_with_tier(model, tier).and_then(|price| {
            if !complete {
                return None;
            }
            let input_price = price.input_price_per_mtok_micros?;
            let output_price = price.output_price_per_mtok_micros?;
            let cache_price = if cached > 0 {
                price.cache_read_price_per_mtok_micros?
            } else {
                0
            };
            // Published >272K-input surcharges cover the full request, including
            // cached input. GPT-5.5 documents this for standard/batch/flex only.
            // https://developers.openai.com/api/docs/models/gpt-6-astra
            // https://developers.openai.com/api/docs/models/gpt-5.5 (2026-09-07)
            let base_model = model.strip_suffix("[1m]").unwrap_or(model);
            let has_context_surcharge = base_model == "gpt-6-astra"
                || (base_model == "gpt-5.5"
                    && !tier.is_some_and(|tier| tier.trim().eq_ignore_ascii_case("priority")));
            let long_context = has_context_surcharge && input.unwrap() > 272_000;
            let input_multiplier = if long_context { 2.0 } else { 1.0 };
            let output_multiplier = if long_context { 1.5 } else { 1.0 };
            // OpenAI input_tokens INCLUDES cached input, output includes reasoning.
            Some(
                (((input.unwrap() - cached) as f64 * input_price as f64
                    + cached as f64 * cache_price as f64)
                    * input_multiplier
                    + output.unwrap() as f64 * output_price as f64 * output_multiplier)
                    / 1_000_000_000_000.0,
            )
        });
    Totals {
        requests: 1,
        input: input.unwrap_or(0),
        output: output.unwrap_or(0),
        cached,
        known_usd: cost.unwrap_or(0.0),
        unpriced_requests: u64::from(cost.is_none()),
        incomplete_requests: u64::from(!complete),
    }
}

/// Record an authoritative Responses API terminal usage event. `input` includes
/// cached input and `output` includes reasoning. This is not subscription spend.
/// The caller must capture the credential label before sending the request and
/// call once per response, not from a UI or from cumulative session usage.
pub fn record_openai_oauth_usage(
    label: &str,
    model: &str,
    tier: Option<&str>,
    input: Option<u64>,
    output: Option<u64>,
    cached: Option<u64>,
) {
    if label.trim().is_empty() || (input.is_none() && output.is_none() && cached.is_none()) {
        return;
    }
    let totals = priced_usage(model, tier, input, output, cached);
    let result = path().and_then(|path| update(&path, label, &totals, Local::now()));
    if let Err(error) = result {
        crate::logging::warn(&format!(
            "Could not persist recorded ChatGPT usage: {error}"
        ));
    }
}

/// Today means since local midnight. Lifetime means recorded usage since this
/// tracker began, not the account's lifetime subscription usage or billed cost.
/// Unknown prices are explicitly reported, never silently presented as zero.
pub fn openai_oauth_usage_summary(label: &str) -> Vec<(String, String)> {
    let store = match path().and_then(|path| load(&path)) {
        Ok(store) => store,
        Err(_) => {
            return ["Today", "Lifetime"]
                .into_iter()
                .map(|key| {
                    (
                        key.into(),
                        "Recorded usage unavailable (ledger could not be read)".into(),
                    )
                })
                .collect();
        }
    };
    summary(&store, label, Local::now())
}

fn summary(store: &Store, label: &str, now: DateTime<Local>) -> Vec<(String, String)> {
    let empty = AccountUsage::default();
    let account = store.accounts.get(label).unwrap_or(&empty);
    let today = account
        .days
        .get(&now.format("%Y-%m-%d").to_string())
        .cloned()
        .unwrap_or_default();
    let mut lifetime = account.lifetime.display();
    if let Some(first) = account
        .first_recorded
        .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0))
    {
        lifetime.push_str(&format!(
            "; since {}",
            first.with_timezone(&Local).format("%Y-%m-%d %H:%M %:z")
        ));
    }
    vec![
        (
            "Today".into(),
            format!("{}; since local midnight", today.display()),
        ),
        ("Lifetime".into(), lifetime),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn cached_input_discount_and_output_semantics() {
        let usage = priced_usage(
            "gpt-5.5",
            None,
            Some(1_000_000),
            Some(100_000),
            Some(400_000),
        );
        assert!((usage.known_usd - 10.9).abs() < 1e-9);
        assert_eq!(usage.input, 1_000_000);
        assert_eq!(usage.output, 100_000);
        let priority = priced_usage(
            "gpt-5.5",
            Some("priority"),
            Some(1_000_000),
            Some(0),
            Some(0),
        );
        assert!((priority.known_usd - 12.5).abs() < 1e-9);
    }

    #[test]
    fn astra_equivalent_cost_uses_actual_context_length_and_tier() {
        let at_limit = priced_usage(
            "gpt-6-astra",
            None,
            Some(272_000),
            Some(1_000),
            Some(100_000),
        );
        assert!((at_limit.known_usd - 1.87).abs() < 1e-9);
        assert_eq!(at_limit.unpriced_requests, 0);
        let above = priced_usage(
            "gpt-6-astra[1m]",
            None,
            Some(272_001),
            Some(1_000),
            Some(100_000),
        );
        assert!((above.known_usd - 3.71502).abs() < 1e-9);
        for (tier, multiplier) in [("flex", 0.5), ("priority", 2.0)] {
            let usage = priced_usage(
                "gpt-6-astra",
                Some(tier),
                Some(272_001),
                Some(1_000),
                Some(100_000),
            );
            assert!((usage.known_usd - 3.71502 * multiplier).abs() < 1e-9);
        }
    }

    #[test]
    fn unknown_and_partial_are_not_zero_cost() {
        let unknown = priced_usage("future-model", None, Some(25), Some(10), None);
        assert_eq!(unknown.unpriced_requests, 1);
        assert!(unknown.display().contains("cost unknown"));
        assert!(!unknown.display().contains("$0"));
        let mut mixed = priced_usage("gpt-5.5", None, Some(10), Some(5), None);
        mixed.add(&unknown);
        assert!(mixed.display().contains("+ unknown cost"));
        for usage in [
            priced_usage("gpt-5.5", None, None, Some(1), None),
            priced_usage("gpt-5.5", None, Some(2), Some(1), Some(3)),
        ] {
            assert_eq!(usage.unpriced_requests, 1);
            assert!(usage.display().contains("partial token counts"));
        }
    }

    #[test]
    fn local_midnight_roundtrip_account_isolation_and_no_spend() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let before = Local.with_ymd_and_hms(2026, 9, 5, 23, 59, 59).unwrap();
        let after = Local.with_ymd_and_hms(2026, 9, 6, 0, 0, 0).unwrap();
        let usage = priced_usage("gpt-5.5", None, Some(50), Some(20), Some(10));
        update(&path, "one", &usage, before).unwrap();
        update(&path, "one", &usage, after).unwrap();
        update(&path, "two", &usage, after).unwrap();
        let store = load(&path).unwrap();
        let rows = summary(&store, "one", after);
        assert!(rows[0].1.contains("50 input / 20 output"));
        assert!(rows[1].1.contains("100 input / 40 output"));
        assert!(rows[1].1.contains("since 2026-09-05"));
        assert_eq!(store.accounts["two"].lifetime.input, 50);
        assert!(
            summary(&store, "absent", after)[0]
                .1
                .contains("No recorded usage")
        );
        assert!(!std::fs::read_to_string(path).unwrap().contains("spend"));
    }

    #[test]
    fn concurrent_writers_preserve_every_increment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let path = &path;
                scope.spawn(move || {
                    for _ in 0..10 {
                        update(
                            path,
                            "one",
                            &priced_usage("gpt-5.5", None, Some(1), Some(2), None),
                            Local::now(),
                        )
                        .unwrap();
                    }
                });
            }
        });
        let store = load(&path).unwrap();
        assert_eq!(store.accounts["one"].lifetime.requests, 80);
        assert_eq!(store.accounts["one"].lifetime.output, 160);
    }

    #[test]
    fn process_writer_child() {
        let Some(path) = std::env::var_os("JCODE_OAUTH_TEST_LEDGER") else {
            return;
        };
        for _ in 0..10 {
            update(
                Path::new(&path),
                "one",
                &priced_usage("gpt-5.5", None, Some(1), Some(2), None),
                Local::now(),
            )
            .unwrap();
        }
    }

    #[test]
    fn concurrent_processes_preserve_every_increment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let executable = std::env::current_exe().unwrap();
        let mut children = Vec::new();
        for _ in 0..4 {
            children.push(
                std::process::Command::new(&executable)
                    .args([
                        "--exact",
                        "provider_activity::oauth_usage::tests::process_writer_child",
                    ])
                    .env("JCODE_OAUTH_TEST_LEDGER", &path)
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap(),
            );
        }
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        assert_eq!(load(&path).unwrap().accounts["one"].lifetime.requests, 40);
    }

    #[test]
    fn corrupt_ledger_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        std::fs::write(&path, "broken").unwrap();
        assert!(update(&path, "one", &Totals::default(), Local::now()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken");
    }
}
