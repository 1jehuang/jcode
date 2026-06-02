use super::*;

pub(super) fn usage_percent_from_used_limit(used: f64, limit: f64) -> f32 {
    if !used.is_finite() || !limit.is_finite() || limit <= 0.0 {
        return 0.0;
    }
    ((used.max(0.0) / limit) * 100.0) as f32
}

pub(super) fn usage_percent_from_remaining_limit(remaining: f64, limit: f64) -> f32 {
    if !remaining.is_finite() || !limit.is_finite() || limit <= 0.0 {
        return 0.0;
    }
    usage_percent_from_used_limit((limit - remaining).max(0.0), limit)
}

pub(super) async fn fetch_anthropic_usage_for_token(
    display_name: String,
    access_token: String,
    refresh_token: String,
    account_label: String,
    expires_at: i64,
) -> ProviderUsage {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let access_token = if expires_at < now_ms + 300_000 && !refresh_token.is_empty() {
        match crate::auth::oauth::refresh_claude_tokens_for_account(&refresh_token, &account_label)
            .await
        {
            Ok(refreshed) => refreshed.access_token,
            Err(_) => {
                if expires_at < now_ms {
                    return ProviderUsage {
                        provider_name: display_name,
                        error: Some(
                            "OAuth token expired - use `/login claude` to re-authenticate"
                                .to_string(),
                        ),
                        ..Default::default()
                    };
                }
                access_token
            }
        }
    } else {
        access_token
    };

    let cache_key = anthropic_usage_cache_key(&access_token, Some(&account_label));
    match fetch_anthropic_usage_data(access_token, cache_key).await {
        Ok(data) => provider_report_from_usage_data(display_name, &data),
        Err(e) => ProviderUsage {
            provider_name: display_name,
            error: Some(e.to_string()),
            ..Default::default()
        },
    }
}

pub(super) async fn fetch_all_openai_usage_reports() -> Vec<ProviderUsage> {
    let accounts = auth::codex::list_accounts().unwrap_or_default();
    if !accounts.is_empty() {
        let active_label = auth::codex::active_account_label();
        let mut reports = Vec::with_capacity(accounts.len());
        for account in &accounts {
            let display_name = openai_provider_display_name(
                &account.label,
                account.email.as_deref(),
                accounts.len(),
                active_label.as_deref() == Some(&account.label),
            );
            reports.push(
                fetch_openai_usage_for_account(
                    display_name,
                    auth::codex::CodexCredentials {
                        access_token: account.access_token.clone(),
                        refresh_token: account.refresh_token.clone(),
                        id_token: account.id_token.clone(),
                        account_id: account.account_id.clone(),
                        expires_at: account.expires_at,
                    },
                    Some(account.label.as_str()),
                )
                .await,
            );
        }
        return reports;
    }

    let creds = match auth::codex::load_credentials() {
        Ok(creds) => creds,
        Err(_) => return Vec::new(),
    };
    let is_chatgpt = !creds.refresh_token.is_empty() || creds.id_token.is_some();
    if !is_chatgpt || creds.access_token.is_empty() {
        return Vec::new();
    }

    vec![
        fetch_openai_usage_for_account(
            openai_provider_display_name("default", None, 1, true),
            creds,
            None,
        )
        .await,
    ]
}

pub(super) async fn fetch_openai_usage_report() -> Option<ProviderUsage> {
    let reports = fetch_all_openai_usage_reports().await;
    active_openai_usage_report(&reports)
        .cloned()
        .or_else(|| reports.into_iter().next())
}

pub(super) async fn fetch_openai_usage_for_account(
    display_name: String,
    mut creds: auth::codex::CodexCredentials,
    account_label: Option<&str>,
) -> ProviderUsage {
    let is_chatgpt = !creds.refresh_token.is_empty() || creds.id_token.is_some();
    if creds.access_token.is_empty() || !is_chatgpt {
        return ProviderUsage {
            provider_name: display_name,
            error: Some("No OpenAI/Codex OAuth credentials found".to_string()),
            ..Default::default()
        };
    }

    let initial_cache_key = openai_usage_cache_key(&creds.access_token, account_label);
    if let Some(cached) = cached_openai_usage(&initial_cache_key) {
        return provider_report_from_openai_usage_data(display_name, &cached);
    }

    if let Some(expires_at) = creds.expires_at {
        let now = chrono::Utc::now().timestamp_millis();
        if expires_at < now + 300_000 && !creds.refresh_token.is_empty() {
            let refreshed = match account_label {
                Some(label) => {
                    crate::auth::oauth::refresh_openai_tokens_for_account(
                        &creds.refresh_token,
                        label,
                    )
                    .await
                }
                None => crate::auth::oauth::refresh_openai_tokens(&creds.refresh_token).await,
            };
            match refreshed {
                Ok(refreshed) => {
                    creds.access_token = refreshed.access_token;
                    creds.refresh_token = refreshed.refresh_token;
                    creds.id_token = refreshed.id_token.or(creds.id_token);
                    creds.account_id = creds.account_id.clone().or_else(|| {
                        creds
                            .id_token
                            .as_deref()
                            .and_then(auth::codex::extract_account_id)
                    });
                    creds.expires_at = Some(refreshed.expires_at);
                }
                Err(e) => {
                    let report = ProviderUsage {
                        provider_name: display_name,
                        error: Some(format!(
                            "Token refresh failed: {} - use `/login openai` to re-authenticate",
                            e
                        )),
                        ..Default::default()
                    };
                    store_openai_usage(
                        initial_cache_key,
                        openai_usage_data_from_provider_report(&report),
                    );
                    return report;
                }
            }
        }
    }

    let cache_key = openai_usage_cache_key(&creds.access_token, account_label);
    if cache_key != initial_cache_key
        && let Some(cached) = cached_openai_usage(&cache_key)
    {
        return provider_report_from_openai_usage_data(display_name, &cached);
    }

    let client = crate::provider::shared_http_client();
    let mut builder = client
        .get(OPENAI_USAGE_URL)
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {}", creds.access_token));

    if let Some(ref account_id) = creds.account_id {
        builder = builder.header("chatgpt-account-id", account_id);
    }

    let response = match builder.send().await {
        Ok(response) => response,
        Err(e) => {
            let report = ProviderUsage {
                provider_name: display_name,
                error: Some(format!("Failed to fetch: {}", e)),
                ..Default::default()
            };
            store_openai_usage(cache_key, openai_usage_data_from_provider_report(&report));
            return report;
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let report = ProviderUsage {
            provider_name: display_name,
            error: Some(format!("API error ({}): {}", status, body)),
            ..Default::default()
        };
        store_openai_usage(cache_key, openai_usage_data_from_provider_report(&report));
        return report;
    }

    let body_text = match response.text().await {
        Ok(text) => text,
        Err(e) => {
            let report = ProviderUsage {
                provider_name: display_name,
                error: Some(format!("Failed to read response: {}", e)),
                ..Default::default()
            };
            store_openai_usage(cache_key, openai_usage_data_from_provider_report(&report));
            return report;
        }
    };

    let json: serde_json::Value = match serde_json::from_str(&body_text) {
        Ok(value) => value,
        Err(e) => {
            let report = ProviderUsage {
                provider_name: display_name,
                error: Some(format!("Failed to parse response: {}", e)),
                ..Default::default()
            };
            store_openai_usage(cache_key, openai_usage_data_from_provider_report(&report));
            return report;
        }
    };

    let parsed = parse_openai_usage_payload(&json);

    let report = ProviderUsage {
        provider_name: display_name,
        limits: parsed.limits,
        extra_info: parsed.extra_info,
        hard_limit_reached: parsed.hard_limit_reached,
        error: None,
    };
    store_openai_usage(cache_key, openai_usage_data_from_provider_report(&report));
    report
}

pub(super) async fn fetch_openrouter_usage_report() -> Option<ProviderUsage> {
    let api_key = openrouter_api_key()?;

    let client = crate::provider::shared_http_client();

    let (key_resp, credits_resp) = tokio::join!(
        client
            .get("https://openrouter.ai/api/v1/key")
            .header("Authorization", format!("Bearer {}", api_key))
            .send(),
        client
            .get("https://openrouter.ai/api/v1/credits")
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
    );

    let mut limits = Vec::new();
    let mut extra_info = Vec::new();

    if let Ok(resp) = credits_resp
        && resp.status().is_success()
        && let Ok(json) = resp.json::<serde_json::Value>().await
        && let Some(data) = json.get("data")
    {
        let total_credits = data
            .get("total_credits")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let total_usage = data
            .get("total_usage")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let balance = total_credits - total_usage;

        if total_credits > 0.0 {
            let usage_pct = usage_percent_from_used_limit(total_usage, total_credits);
            limits.push(UsageLimit {
                name: "Credits".to_string(),
                usage_percent: usage_pct,
                resets_at: None,
            });
        }

        extra_info.push((
            "Balance".to_string(),
            format!("${:.2} / ${:.2}", balance, total_credits),
        ));
    }

    if let Ok(resp) = key_resp
        && resp.status().is_success()
        && let Ok(json) = resp.json::<serde_json::Value>().await
        && let Some(data) = json.get("data")
    {
        let usage_daily = data
            .get("usage_daily")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let usage_weekly = data
            .get("usage_weekly")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let usage_monthly = data
            .get("usage_monthly")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        extra_info.push(("Today".to_string(), format!("${:.2}", usage_daily)));
        extra_info.push(("This week".to_string(), format!("${:.2}", usage_weekly)));
        extra_info.push(("This month".to_string(), format!("${:.2}", usage_monthly)));

        if let Some(limit) = data.get("limit").and_then(|v| v.as_f64()) {
            let remaining = data
                .get("limit_remaining")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let pct = usage_percent_from_remaining_limit(remaining, limit);
            limits.push(UsageLimit {
                name: "Key limit".to_string(),
                usage_percent: pct,
                resets_at: None,
            });
            extra_info.push((
                "Key limit".to_string(),
                format!("${:.2} / ${:.2}", remaining, limit),
            ));
        }
    }

    if limits.is_empty() && extra_info.is_empty() {
        return None;
    }

    Some(ProviderUsage {
        provider_name: "OpenRouter".to_string(),
        limits,
        extra_info,
        hard_limit_reached: false,
        error: None,
    })
}

pub(super) fn openrouter_api_key() -> Option<String> {
    std::env::var("OPENROUTER_API_KEY")
        .ok()
        .or_else(|| {
            let config_path = crate::storage::app_config_dir()
                .ok()?
                .join("openrouter.env");
            crate::storage::harden_secret_file_permissions(&config_path);
            let content = std::fs::read_to_string(config_path).ok()?;
            content
                .lines()
                .find_map(|line| line.strip_prefix("OPENROUTER_API_KEY="))
                .map(|k| k.trim().to_string())
        })
        .filter(|k| !k.is_empty())
}

pub(super) async fn fetch_copilot_usage_report() -> Option<ProviderUsage> {
    if !auth::copilot::has_copilot_credentials() {
        return None;
    }

    let github_token = auth::copilot::load_github_token().ok()?;

    let mut limits = Vec::new();
    let mut extra_info = Vec::new();

    // Fetch plan/quota info from the token endpoint
    let client = crate::provider::shared_http_client();
    let api_result = client
        .get(auth::copilot::COPILOT_TOKEN_URL)
        .header("Authorization", format!("token {}", github_token))
        .header("User-Agent", auth::copilot::EDITOR_VERSION)
        .header("Editor-Version", auth::copilot::EDITOR_VERSION)
        .header(
            "Editor-Plugin-Version",
            auth::copilot::EDITOR_PLUGIN_VERSION,
        )
        .header("Accept", "application/json")
        .send()
        .await;

    if let Ok(resp) = api_result
        && resp.status().is_success()
        && let Ok(json) = resp.json::<serde_json::Value>().await
    {
        if let Some(sku) = json.get("sku").and_then(|v| v.as_str()) {
            extra_info.push(("Plan".to_string(), sku.to_string()));
        }

        let reset_date = json
            .get("limited_user_reset_date")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if let Some(quotas) = json.get("limited_user_quotas").and_then(|v| v.as_object()) {
            for (name, value) in quotas {
                if let Some(obj) = value.as_object() {
                    let used = obj.get("used").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let limit = obj.get("limit").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    if limit > 0.0 {
                        let pct = usage_percent_from_used_limit(used, limit);
                        limits.push(UsageLimit {
                            name: format!("{} (remote)", humanize_key(name)),
                            usage_percent: pct,
                            resets_at: reset_date.clone(),
                        });
                        extra_info.push((
                            humanize_key(name),
                            format!("{} / {} used", used as u64, limit as u64),
                        ));
                    }
                }
            }
        }

        if let Some(ref rd) = reset_date {
            let relative = crate::usage::format_reset_time(rd);
            extra_info.push(("Resets in".to_string(), relative));
        }
    }

    // Local usage tracking
    let usage = crate::copilot_usage::get_usage();

    extra_info.push((
        "Today".to_string(),
        format!(
            "{} premium + {} agent = {} total ({} in + {} out)",
            usage.today.premium_requests,
            usage
                .today
                .requests
                .saturating_sub(usage.today.premium_requests),
            usage.today.requests,
            format_token_count(usage.today.input_tokens),
            format_token_count(usage.today.output_tokens),
        ),
    ));
    extra_info.push((
        "This month".to_string(),
        format!(
            "{} premium + {} agent = {} total ({} in + {} out)",
            usage.month.premium_requests,
            usage
                .month
                .requests
                .saturating_sub(usage.month.premium_requests),
            usage.month.requests,
            format_token_count(usage.month.input_tokens),
            format_token_count(usage.month.output_tokens),
        ),
    ));
    extra_info.push((
        "All time".to_string(),
        format!(
            "{} premium + {} agent = {} total ({} in + {} out)",
            usage.all_time.premium_requests,
            usage
                .all_time
                .requests
                .saturating_sub(usage.all_time.premium_requests),
            usage.all_time.requests,
            format_token_count(usage.all_time.input_tokens),
            format_token_count(usage.all_time.output_tokens),
        ),
    ));

    Some(ProviderUsage {
        provider_name: "GitHub Copilot".to_string(),
        limits,
        extra_info,
        hard_limit_reached: false,
        error: None,
    })
}
