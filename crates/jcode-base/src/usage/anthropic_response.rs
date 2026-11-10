//! Quota telemetry already returned by successful subscription model requests.
//! This must not clear the usage endpoint's independent polling backoff.
use super::*;

fn window(headers: &reqwest::header::HeaderMap, name: &str) -> Option<(f32, String)> {
    let prefix = format!("anthropic-ratelimit-unified-{name}");
    let (Ok(ratio), Ok(epoch)) = (
        headers.get(format!("{prefix}-utilization"))?.to_str(),
        headers.get(format!("{prefix}-reset"))?.to_str(),
    ) else {
        return None;
    };
    let (Ok(ratio), Ok(epoch)) = (ratio.parse::<f32>(), epoch.parse::<i64>()) else {
        return None;
    };
    if !ratio.is_finite() || !(0.0..=1.0).contains(&ratio) {
        return None;
    }
    let reset = chrono::DateTime::from_timestamp(epoch, 0)?;
    (reset > chrono::Utc::now()).then(|| (ratio, reset.to_rfc3339()))
}

/// Record real quota ratios, not token-count estimates, from a successful
/// official Anthropic OAuth response. Call on a blocking worker, never from a
/// renderer. Incomplete, malformed, or expired header sets are ignored rather
/// than manufacturing a zero-usage window. API-key responses must not call this.
pub fn record_anthropic_response_limits(
    access_token: &str,
    account_label: Option<&str>,
    headers: &reqwest::header::HeaderMap,
) {
    let (Some(five), Some(seven)) = (window(headers, "5h"), window(headers, "7d")) else {
        return;
    };
    let key = anthropic_usage_cache_key(access_token, account_label);
    let data = disk_cache::store_response_limits(&key, five, seven);
    store_anthropic_usage(key, data);
}

/// Refresh rejections must reach the accounts UI as an authentication problem,
/// not an opaque usage-endpoint failure. Transient refresh errors stay retryable.
pub(super) fn credential_error(error: anyhow::Error) -> anyhow::Error {
    let message = error.to_string();
    if auth::refresh_state::error_is_permanent_rejection(&message)
        || message.contains("previously rejected")
        || message == "OAuth token expired"
    {
        anyhow::anyhow!("Usage authentication failed (401): sign in again with /login claude")
    } else {
        error
    }
}

/// A newly saved credential can repair authentication immediately, but must
/// never bypass a usage lookup's 429/Retry-After deadline.
pub(crate) fn invalidate_auth_failure(label: &str) {
    let key = format!("label:{label}");
    let prefix = format!("{key}:");
    disk_cache::clear_auth_failure(&key);
    if let Ok(mut cache) = cache::anthropic_usage_cache().lock() {
        cache.retain(|name, data| {
            !(name == &key || name.starts_with(&prefix))
                || !data.last_error.as_deref().is_some_and(is_auth_failure)
        });
    }
}

pub(super) fn is_auth_failure(error: &str) -> bool {
    error.contains("401")
        || error.contains("OAuth token expired")
        || error.contains("previously rejected")
        || auth::refresh_state::error_is_permanent_rejection(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_home(f: impl FnOnce(tokio::runtime::Runtime)) {
        let _guard = crate::storage::lock_test_env();
        let dir = tempfile::tempdir().unwrap();
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(value) => crate::env::set_var("JCODE_HOME", value),
                    None => crate::env::remove_var("JCODE_HOME"),
                }
            }
        }
        let _restore = Restore(std::env::var_os("JCODE_HOME"));
        crate::env::set_var("JCODE_HOME", dir.path());
        f(tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap());
    }

    fn headers() -> reqwest::header::HeaderMap {
        let reset = (chrono::Utc::now().timestamp() + 7200).to_string();
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in [
            ("anthropic-ratelimit-unified-5h-utilization", "0.37"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.62"),
            ("anthropic-ratelimit-unified-5h-reset", reset.as_str()),
            ("anthropic-ratelimit-unified-7d-reset", reset.as_str()),
        ] {
            headers.insert(name, value.parse().unwrap());
        }
        headers
    }

    #[test]
    fn response_limits_recover_bars_without_bypassing_endpoint_backoff() {
        with_home(|runtime| {
            runtime.block_on(async {
                let key = anthropic_usage_cache_key("fixture", Some("passive-recovery"));
                let before =
                    disk_cache::store_error_with_hint(&key, "Usage API error (429)", Some("3600"));
                let deadline = before.refresh_after.unwrap();
                record_anthropic_response_limits("fixture", Some("passive-recovery"), &headers());
                let data = disk_cache::fresh(&key).unwrap();
                assert_eq!((data.five_hour, data.seven_day), (0.37, 0.62));
                assert!(data.last_error.is_none());
                assert!(data.last_success_at.is_some());
                assert!(data.refresh_after.unwrap() + Duration::from_millis(10) >= deadline);
                let snapshot = cached_account_usage_snapshot(
                    MultiAccountProviderKind::Anthropic,
                    "passive-recovery",
                )
                .unwrap();
                assert_eq!(snapshot.five_hour_ratio, Some(0.37));
                assert_eq!(snapshot.seven_day_ratio, Some(0.62));
                assert!(snapshot.error.is_none());
                flight::fetch(key.clone(), || async {
                    panic!("passive telemetry must not cause a usage request during Retry-After")
                })
                .await
                .unwrap();
                // Relogin must not erase a known provider cooldown either.
                invalidate_auth_failure("passive-recovery");
                assert!(
                    disk_cache::fresh(&key).unwrap().refresh_after.unwrap()
                        + Duration::from_millis(10)
                        >= deadline
                );
            })
        });
    }

    #[test]
    fn partial_invalid_and_expired_headers_never_create_zero_usage() {
        with_home(|_| {
            let key = anthropic_usage_cache_key("fixture", Some("passive-invalid"));
            let name = "anthropic-ratelimit-unified-5h-utilization";
            for value in ["NaN", "inf", "-0.1", "1.01", "garbage"] {
                let mut headers = headers();
                headers.insert(name, value.parse().unwrap());
                record_anthropic_response_limits("fixture", Some("passive-invalid"), &headers);
                assert!(disk_cache::fresh(&key).is_none());
            }
            for name in [
                "anthropic-ratelimit-unified-5h-utilization",
                "anthropic-ratelimit-unified-7d-utilization",
                "anthropic-ratelimit-unified-5h-reset",
                "anthropic-ratelimit-unified-7d-reset",
            ] {
                let mut headers = headers();
                headers.remove(name);
                record_anthropic_response_limits("fixture", Some("passive-invalid"), &headers);
                assert!(disk_cache::fresh(&key).is_none());
            }
            let mut headers = headers();
            headers.insert("anthropic-ratelimit-unified-7d-reset", "1".parse().unwrap());
            record_anthropic_response_limits("fixture", Some("passive-invalid"), &headers);
            assert!(disk_cache::fresh(&key).is_none());
        });
    }

    #[test]
    fn first_failed_account_probe_keeps_the_shared_retry_after() {
        with_home(|_| {
            let key = anthropic_usage_cache_key("fixture", Some("first-failure-policy"));
            let original =
                disk_cache::store_error_with_hint(&key, "Usage API error (429)", Some("7200"));
            let data = account_fetch::cache_anthropic_account_failure(
                key.clone(),
                anyhow::anyhow!("Usage API error (429)"),
            );
            assert!(
                data.refresh_after.unwrap() + Duration::from_millis(10)
                    >= original.refresh_after.unwrap()
            );
            assert!(data.last_success_at.is_none());
            let snapshot =
                anthropic_snapshot_from_usage("first-failure-policy".into(), None, &data);
            assert!(snapshot.error.unwrap().contains("retry at"));
            assert!(snapshot.five_hour_ratio.is_none());
            assert!(snapshot.seven_day_ratio.is_none());
        });
    }

    #[test]
    fn permanent_refresh_errors_are_actionable_but_network_errors_are_not_auth_failures() {
        for message in [
            "OAuth token expired",
            "invalid_grant: Refresh token not found or invalid",
            "Claude OAuth refresh token was previously rejected by Anthropic",
        ] {
            let error = credential_error(anyhow::anyhow!(message)).to_string();
            assert!(error.contains("401"), "{error}");
            assert!(error.contains("/login claude"), "{error}");
            assert!(!error.contains("invalid_grant"));
        }
        for message in ["connection timed out", "HTTP 503", "HTTP 429"] {
            assert_eq!(
                credential_error(anyhow::anyhow!(message)).to_string(),
                message
            );
        }
    }

    #[test]
    fn expired_rejected_account_stops_refreshing_and_relogin_clears_auth_cache() {
        with_home(|runtime| {
            runtime.block_on(async {
                let label = auth::claude::primary_account_label();
                let tokens = auth::oauth::OAuthTokens {
                    access_token: "fixture-rejected-access".into(),
                    refresh_token: "fixture-rejected-refresh".into(),
                    expires_at: 1,
                    id_token: None,
                    scopes: vec![],
                };
                auth::oauth::save_claude_tokens_for_account(&tokens, &label).unwrap();
                auth::refresh_state::record_permanent_rejection(
                    "claude",
                    &tokens.refresh_token,
                    "invalid_grant",
                )
                .unwrap();
                // This call must return the local guard, not try the real token URL.
                let error =
                    auth::oauth::refresh_claude_tokens_for_account(&tokens.refresh_token, &label)
                        .await
                        .unwrap_err();
                assert!(error.to_string().contains("previously rejected"));
                for _ in 0..2 {
                    let accounts =
                        fetch_oauth_account_usage(MultiAccountProviderKind::Anthropic).await;
                    assert_eq!(accounts.len(), 1);
                    assert!(accounts[0].error.as_deref().unwrap().contains("401"));
                    assert!(accounts[0].five_hour_ratio.is_none());
                }
                let key = anthropic_usage_cache_key(&tokens.access_token, Some(&label));
                disk_cache::store_error(&key, "Usage API error (401 Unauthorized)");
                assert!(cached_anthropic_usage(&key).is_some());
                let new_tokens = auth::oauth::OAuthTokens {
                    access_token: "fixture-new-access".into(),
                    refresh_token: "fixture-new-refresh".into(),
                    expires_at: chrono::Utc::now().timestamp_millis() + 7_200_000,
                    ..tokens
                };
                auth::oauth::save_claude_tokens_for_account(&new_tokens, &label).unwrap();
                assert!(cached_anthropic_usage(&key).is_none());
                assert!(disk_cache::fresh(&key).is_none());
                // A separate login process cannot clear the daemon's RAM. The
                // next read must notice that the shared auth failure was removed.
                store_anthropic_usage(
                    key.clone(),
                    UsageData {
                        fetched_at: Some(Instant::now()),
                        last_error: Some("Usage API error (401)".into()),
                        ..Default::default()
                    },
                );
                assert!(cached_or_shared_anthropic_usage(&key).is_none());
                assert!(!auth::refresh_state::refresh_token_is_known_rejected(
                    "claude",
                    &new_tokens.refresh_token
                ));
                let restored = auth::oauth::refresh_claude_tokens_for_account(
                    "fixture-rejected-refresh",
                    &label,
                )
                .await
                .unwrap();
                assert_eq!(restored.access_token, new_tokens.access_token);
            })
        });
    }
}
