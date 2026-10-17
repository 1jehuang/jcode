//! Background account-scoped OAuth probes. Never called from a renderer.
use super::*;
use futures::future::join_all;

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy(label: &str) -> AccountUsageSnapshot {
        anthropic_snapshot_from_usage(
            label.into(),
            None,
            &UsageData {
                five_hour: 0.25,
                fetched_at: Some(Instant::now()),
                ..Default::default()
            },
        )
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_accounts_do_not_discard_healthy_usage_or_serialize_deadlines() {
        let started = tokio::time::Instant::now();
        let snapshots = join_all((0..3).map(|index| {
            bounded_probe(
                format!("account-{index}"),
                Some(format!("{index}@example.test")),
                async move {
                    if index != 1 {
                        std::future::pending::<()>().await;
                    }
                    healthy("account-1")
                },
            )
        }))
        .await;
        assert_eq!(tokio::time::Instant::now() - started, ACCOUNT_PROBE_TIMEOUT);
        assert_eq!(snapshots[1].five_hour_ratio, Some(0.25));
        assert!(snapshots[1].error.is_none());
        for index in [0, 2] {
            assert_eq!(snapshots[index].label, format!("account-{index}"));
            assert_eq!(
                snapshots[index].email,
                Some(format!("{index}@example.test"))
            );
            assert_eq!(
                snapshots[index].error.as_deref(),
                Some("Usage probe timed out")
            );
            assert!(snapshots[index].five_hour_ratio.is_none());
            assert!(!snapshots[index].exhausted);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn successful_probes_do_not_wait_for_the_deadline() {
        let started = tokio::time::Instant::now();
        let result = bounded_probe("healthy".into(), None, async { healthy("healthy") }).await;
        assert_eq!(tokio::time::Instant::now(), started);
        assert_eq!(result.five_hour_ratio, Some(0.25));
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn cached_usage_does_not_clear_runtime_quota_failures() {
        let label = "usage-recovery-cache-hit-test";
        let key = format!("label:{label}");
        let data = UsageData {
            fetched_at: Some(Instant::now()),
            ..Default::default()
        };
        store_anthropic_usage(key.clone(), data);
        store_openai_usage(
            key,
            OpenAIUsageData {
                fetched_at: Some(Instant::now()),
                ..Default::default()
            },
        );
        for provider in ["claude", "openai"] {
            crate::provider::record_provider_unavailable_for_account_label(
                provider,
                Some(label),
                "usage_limit_reached",
            );
        }
        assert!(
            fetch_anthropic_usage_data("unused-token".into(), format!("label:{label}"))
                .await
                .is_ok()
        );
        let report = fetch_openai_usage_for_account(
            "OpenAI".into(),
            auth::codex::CodexCredentials {
                access_token: "unused-token".into(),
                refresh_token: "unused-refresh".into(),
                id_token: None,
                account_id: None,
                expires_at: None,
            },
            Some(label),
        )
        .await;
        assert!(report.error.is_none());
        for provider in ["claude", "openai"] {
            assert!(
                crate::provider::provider_unavailability_detail_for_account_label(
                    provider,
                    Some(label)
                )
                .is_some()
            );
        }
        crate::provider::clear_claude_provider_unavailability_for_account_label(Some(label));
        crate::provider::clear_openai_provider_unavailability_for_account_label(Some(label));
    }
}

// Bound credential refresh and body reads too: one stalled login must not
// consume the daemon worker deadline and discard healthy accounts' usage.
const ACCOUNT_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

async fn bounded_probe<F>(label: String, email: Option<String>, probe: F) -> AccountUsageSnapshot
where
    F: std::future::Future<Output = AccountUsageSnapshot>,
{
    tokio::time::timeout(ACCOUNT_PROBE_TIMEOUT, probe)
        .await
        .unwrap_or_else(|_| AccountUsageSnapshot {
            label,
            email,
            exhausted: false,
            primary_label: None,
            five_hour_ratio: None,
            secondary_label: None,
            seven_day_ratio: None,
            resets_at: None,
            error: Some("Usage probe timed out".into()),
        })
}

/// Fetch saved OAuth accounts only. API keys and external credential fallbacks
/// are deliberately excluded. The existing per-account caches enforce freshness.
pub async fn fetch_oauth_account_usage(
    provider: MultiAccountProviderKind,
) -> Vec<AccountUsageSnapshot> {
    match provider {
        MultiAccountProviderKind::Anthropic => {
            let accounts = auth::claude::list_accounts().unwrap_or_default();
            join_all(accounts.into_iter().map(|account| {
                bounded_probe(account.label.clone(), account.email.clone(), async move {
                    let cache_key =
                        anthropic_usage_cache_key(&account.access, Some(&account.label));
                    let usage = if let Some(cached) = cached_anthropic_usage(&cache_key) {
                        Ok(cached)
                    } else {
                        let now = chrono::Utc::now().timestamp_millis();
                        let token =
                            if account.expires < now + 300_000 && !account.refresh.is_empty() {
                                auth::oauth::refresh_claude_tokens_for_account(
                                    &account.refresh,
                                    &account.label,
                                )
                                .await
                                .map(|tokens| tokens.access_token)
                            } else if account.expires < now {
                                Err(anyhow::anyhow!("OAuth token expired"))
                            } else {
                                Ok(account.access)
                            };
                        match token {
                            Ok(token) => {
                                let key = anthropic_usage_cache_key(&token, Some(&account.label));
                                fetch_anthropic_usage_data(token, key).await
                            }
                            Err(error) => Err(error),
                        }
                    };
                    let data = usage.unwrap_or_else(|error| {
                        let data = UsageData {
                            fetched_at: Some(Instant::now()),
                            last_error: Some(error.to_string()),
                            ..Default::default()
                        };
                        // Refresh and expired-token failures happen before the HTTP
                        // fetcher's cache. Give them the same per-account retry floor.
                        store_anthropic_usage(cache_key, data.clone());
                        data
                    });
                    anthropic_snapshot_from_usage(account.label, account.email, &data)
                })
            }))
            .await
        }
        MultiAccountProviderKind::OpenAI => {
            let accounts = auth::codex::list_accounts().unwrap_or_default();
            join_all(
                accounts
                    .into_iter()
                    .filter(|account| {
                        // Saved API-key accounts must not acquire fictional quota bars.
                        !account.refresh_token.is_empty() || account.id_token.is_some()
                    })
                    .map(|account| {
                        bounded_probe(account.label.clone(), account.email.clone(), async move {
                            let report = fetch_openai_usage_for_account(
                                "OpenAI (ChatGPT)".into(),
                                auth::codex::CodexCredentials {
                                    access_token: account.access_token,
                                    refresh_token: account.refresh_token,
                                    id_token: account.id_token,
                                    account_id: account.account_id,
                                    expires_at: account.expires_at,
                                },
                                Some(&account.label),
                            )
                            .await;
                            openai_snapshot_from_usage(
                                account.label,
                                account.email,
                                &openai_usage_data_from_provider_report(&report),
                            )
                        })
                    }),
            )
            .await
        }
    }
}
