//! Local usage must not inherit the quota endpoint's polling/backoff TTL.
use super::*;

pub(super) fn refresh_cached_oauth_activity(reports: &mut [ProviderUsage]) {
    for report in reports {
        let prefix = if report.provider_name.starts_with("Anthropic") {
            "claude:oauth:"
        } else if report.provider_name.starts_with("OpenAI") {
            "openai:oauth:"
        } else {
            continue;
        };
        // attach_activity supplies the stable account identity. Never infer it
        // from a masked email or display label, and leave API-key reports alone.
        let Some((_, label)) = report
            .extra_info
            .iter()
            .find(|(key, _)| key == "Account label")
        else {
            continue;
        };
        let source_key = format!("{prefix}{label}");
        attach_activity(report, &source_key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_oauth_reports_refresh_local_totals_without_refetching_quota() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let cache = PROVIDER_USAGE_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
        struct Restore {
            home: Option<std::ffi::OsString>,
            cache: HashMap<String, (Instant, ProviderUsage)>,
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                match self.home.take() {
                    Some(home) => crate::env::set_var("JCODE_HOME", home),
                    None => crate::env::remove_var("JCODE_HOME"),
                }
                *PROVIDER_USAGE_CACHE.get().unwrap().lock().unwrap() =
                    std::mem::take(&mut self.cache);
            }
        }
        let _restore = Restore {
            home: std::env::var_os("JCODE_HOME"),
            cache: std::mem::take(&mut *cache.lock().unwrap()),
        };
        crate::env::set_var("JCODE_HOME", home.path());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for rate_limited in [false, true] {
            let label = if rate_limited {
                "backoff-account"
            } else {
                "fresh-account"
            };
            let error = rate_limited.then(|| "Usage API error (429)".to_string());
            let mut reports = vec![
                ProviderUsage {
                    provider_name: "Anthropic - account".into(),
                    error: error.clone(),
                    ..Default::default()
                },
                ProviderUsage {
                    provider_name: "OpenAI - account".into(),
                    error: error.clone(),
                    ..Default::default()
                },
                ProviderUsage {
                    provider_name: "Anthropic API key".into(),
                    extra_info: vec![("Local spend (this machine)".into(), "$12.00".into())],
                    error: error.clone(),
                    ..Default::default()
                },
            ];
            attach_activity(&mut reports[0], &format!("claude:oauth:{label}"));
            attach_activity(&mut reports[1], &format!("openai:oauth:{label}"));
            // A rate-limited report stays cached beyond the normal two-minute
            // TTL. Local totals must still refresh during that longer backoff.
            let fetched_at = Instant::now()
                - if rate_limited {
                    Duration::from_secs(180)
                } else {
                    Duration::ZERO
                };
            *cache.lock().unwrap() = reports
                .into_iter()
                .map(|report| (report.provider_name.clone(), (fetched_at, report)))
                .collect();
            let initial = runtime.block_on(fetch_all_provider_usage());
            assert!(
                initial[0]
                    .extra_info
                    .iter()
                    .any(|(key, value)| key == "Today" && value == "No recorded usage")
            );

            crate::provider_activity::record_anthropic_oauth_usage(
                label,
                "claude-sonnet-4-6",
                Some(100),
                Some(20),
                Some(40),
                Some(30),
                None,
                Some(10),
                Some(20),
            );
            crate::provider_activity::record_openai_oauth_usage(
                label,
                "gpt-5.5",
                None,
                Some(100),
                Some(20),
                Some(40),
            );
            crate::provider_activity::record_use(&format!("claude:oauth:{label}"));
            for _ in 0..2 {
                let mut updates = Vec::new();
                let refreshed = runtime.block_on(fetch_all_provider_usage_progressive(|update| {
                    updates.push(update)
                }));
                assert_eq!(updates.len(), 1);
                assert!(updates[0].from_cache && updates[0].done);
                assert_eq!(refreshed.len(), 3);
                for report in &refreshed {
                    assert_eq!(report.error, error);
                    if report.provider_name == "Anthropic API key" {
                        assert_eq!(
                            report.extra_info,
                            vec![("Local spend (this machine)".into(), "$12.00".into())]
                        );
                        continue;
                    }
                    let expected = if report.provider_name.starts_with("Anthropic") {
                        crate::provider_activity::anthropic_oauth_usage_summary(label)
                    } else {
                        crate::provider_activity::openai_oauth_usage_summary(label)
                    };
                    for row in expected {
                        assert_ne!(row.1, "No recorded usage");
                        assert!(report.extra_info.contains(&row));
                        assert_eq!(
                            report
                                .extra_info
                                .iter()
                                .filter(|(key, _)| key == &row.0)
                                .count(),
                            1
                        );
                    }
                }
                assert_eq!(refreshed[0].provider_name, "Anthropic - account");
                assert_eq!(
                    refreshed[0]
                        .extra_info
                        .iter()
                        .filter(|(key, _)| key == "Last used")
                        .count(),
                    1
                );
            }
            // Refresh does not advance quota-cache timestamps or persist a
            // local snapshot into the cache: the next read reloads the ledger.
            assert!(
                cache
                    .lock()
                    .unwrap()
                    .values()
                    .all(|(at, _)| *at == fetched_at)
            );
        }
    }
}
