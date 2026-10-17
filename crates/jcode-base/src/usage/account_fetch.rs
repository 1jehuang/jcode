//! Background account-scoped OAuth probes. Never called from a renderer.
use super::*;

/// Fetch saved OAuth accounts only. API keys and external credential fallbacks
/// are deliberately excluded. The existing per-account caches enforce freshness.
pub async fn fetch_oauth_account_usage(
    provider: MultiAccountProviderKind,
) -> Vec<AccountUsageSnapshot> {
    match provider {
        MultiAccountProviderKind::Anthropic => {
            let accounts = auth::claude::list_accounts().unwrap_or_default();
            let mut result = Vec::with_capacity(accounts.len());
            for account in accounts {
                let cache_key = anthropic_usage_cache_key(&account.access, Some(&account.label));
                let usage = if let Some(cached) = cached_anthropic_usage(&cache_key) {
                    Ok(cached)
                } else {
                    let now = chrono::Utc::now().timestamp_millis();
                    let token = if account.expires < now + 300_000 && !account.refresh.is_empty() {
                        auth::oauth::refresh_claude_tokens_for_account(&account.refresh, &account.label)
                            .await.map(|tokens| tokens.access_token)
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
                let data = usage.unwrap_or_else(|error| UsageData {
                    fetched_at: Some(Instant::now()), last_error: Some(error.to_string()),
                    ..Default::default()
                });
                result.push(anthropic_snapshot_from_usage(account.label, account.email, &data));
            }
            result
        }
        MultiAccountProviderKind::OpenAI => {
            let accounts = auth::codex::list_accounts().unwrap_or_default();
            let mut result = Vec::with_capacity(accounts.len());
            for account in accounts {
                // Saved API-key accounts must not acquire fictional quota bars.
                if account.refresh_token.is_empty() && account.id_token.is_none() {
                    continue;
                }
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
                ).await;
                result.push(openai_snapshot_from_usage(
                    account.label, account.email, &openai_usage_data_from_provider_report(&report),
                ));
            }
            result
        }
    }
}
