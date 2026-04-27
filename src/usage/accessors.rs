use super::provider_fetch::fetch_openai_usage_report;
use super::*;

static USAGE: tokio::sync::OnceCell<Arc<RwLock<UsageData>>> = tokio::sync::OnceCell::const_new();
static REFRESH_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

pub(super) async fn get_usage() -> Arc<RwLock<UsageData>> {
    USAGE
        .get_or_init(|| async { Arc::new(RwLock::new(UsageData::default())) })
        .await
        .clone()
}

/// Fetch usage data from the API
async fn fetch_usage() -> Result<UsageData> {
    let creds = auth::claude::load_credentials().context("Failed to load Claude credentials")?;

    let now = chrono::Utc::now().timestamp_millis();
    let active_label =
        auth::claude::active_account_label().unwrap_or_else(auth::claude::primary_account_label);
    let access_token = if creds.expires_at < now + 300_000 && !creds.refresh_token.is_empty() {
        match auth::oauth::refresh_claude_tokens_for_account(&creds.refresh_token, &active_label)
            .await
        {
            Ok(refreshed) => refreshed.access_token,
            Err(_) => creds.access_token,
        }
    } else {
        creds.access_token
    };

    let cache_key = anthropic_usage_cache_key(&access_token, Some(&active_label));
    fetch_anthropic_usage_data(access_token, cache_key).await
}

async fn refresh_usage(usage: Arc<RwLock<UsageData>>) {
    match fetch_usage().await {
        Ok(new_data) => {
            *usage.write().await = new_data;
        }
        Err(e) => {
            let err_msg = e.to_string();
            let mut data = usage.write().await;
            let is_new_error = data.last_error.as_deref() != Some(&err_msg);
            data.last_error = Some(err_msg.clone());
            data.fetched_at = Some(Instant::now());
            if is_new_error {
                crate::logging::error(&format!("Usage fetch error: {}", err_msg));
            }
        }
    }
}

fn try_spawn_refresh(usage: Arc<RwLock<UsageData>>) {
    if REFRESH_IN_FLIGHT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    tokio::spawn(async move {
        refresh_usage(usage).await;
        REFRESH_IN_FLIGHT.store(false, Ordering::SeqCst);
    });
}

/// Get current usage data, refreshing if stale
pub async fn get() -> UsageData {
    let usage = get_usage().await;

    // Check if we need to refresh
    let (should_refresh, current_data) = {
        let data = usage.read().await;
        (data.is_stale(), data.clone())
    };

    if should_refresh {
        try_spawn_refresh(usage.clone());
    }

    current_data.display_snapshot()
}

static OPENAI_USAGE: tokio::sync::OnceCell<Arc<RwLock<OpenAIUsageData>>> =
    tokio::sync::OnceCell::const_new();
static OPENAI_REFRESH_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

pub(super) async fn get_openai_usage_cell() -> Arc<RwLock<OpenAIUsageData>> {
    OPENAI_USAGE
        .get_or_init(|| async { Arc::new(RwLock::new(OpenAIUsageData::default())) })
        .await
        .clone()
}

async fn fetch_openai_usage_data() -> OpenAIUsageData {
    match fetch_openai_usage_report().await {
        Some(report) => openai_usage_data_from_provider_report(&report),
        None => OpenAIUsageData {
            fetched_at: Some(Instant::now()),
            last_error: Some("No OpenAI/Codex OAuth credentials found".to_string()),
            ..Default::default()
        },
    }
}

async fn refresh_openai_usage(usage: Arc<RwLock<OpenAIUsageData>>) {
    let new_data = fetch_openai_usage_data().await;
    *usage.write().await = new_data;
}

fn try_spawn_openai_refresh(usage: Arc<RwLock<OpenAIUsageData>>) {
    if OPENAI_REFRESH_IN_FLIGHT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    tokio::spawn(async move {
        refresh_openai_usage(usage).await;
        OPENAI_REFRESH_IN_FLIGHT.store(false, Ordering::SeqCst);
    });
}

pub async fn get_openai_usage() -> OpenAIUsageData {
    let usage = get_openai_usage_cell().await;

    let (should_refresh, current_data) = {
        let data = usage.read().await;
        (data.is_stale(), data.clone())
    };

    if should_refresh {
        try_spawn_openai_refresh(usage.clone());
    }

    current_data.display_snapshot()
}

pub fn get_openai_usage_sync() -> OpenAIUsageData {
    if let Some(usage) = OPENAI_USAGE.get()
        && let Ok(data) = usage.try_read()
    {
        if data.is_stale() {
            try_spawn_openai_refresh(usage.clone());
        }
        return data.display_snapshot();
    }

    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::spawn(async {
            let _ = get_openai_usage().await;
        });
    }

    OpenAIUsageData::default()
}

/// Check if extra usage (1M context, etc.) is enabled for the account.
/// Returns false if unknown/not yet fetched.
pub fn has_extra_usage() -> bool {
    if let Some(usage) = USAGE.get()
        && let Ok(data) = usage.try_read()
    {
        return data.extra_usage_enabled;
    }
    false
}

/// Fetch usage data for a specific Anthropic account token (blocking).
/// Used for account rotation - checks if a particular account is exhausted.
/// Returns an error if the fetch fails (network, auth, etc.).
/// Results are cached per-account to avoid hammering the API.
pub fn fetch_usage_for_account_sync(
    access_token: &str,
    refresh_token: &str,
    expires_at: i64,
) -> Result<UsageData> {
    let cache_key = anthropic_usage_cache_key(access_token, None);

    if let Some(cached) = cached_anthropic_usage(&cache_key) {
        return Ok(cached);
    }

    if tokio::runtime::Handle::try_current().is_err() {
        anyhow::bail!("Anthropic usage refresh requires a Tokio runtime")
    }

    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(fetch_usage_for_account(
            access_token.to_string(),
            refresh_token.to_string(),
            expires_at,
        ))
    });

    if let Ok(ref data) = result {
        store_anthropic_usage(cache_key, data.clone());
    }

    result
}

pub fn fetch_openai_usage_for_account_sync(
    label: &str,
    email: Option<String>,
    creds: auth::codex::CodexCredentials,
) -> Result<AccountUsageSnapshot> {
    let cache_key = openai_usage_cache_key(&creds.access_token, Some(label));
    if let Some(cached) = cached_openai_usage(&cache_key) {
        return Ok(openai_snapshot_from_usage(
            label.to_string(),
            email,
            &cached,
        ));
    }

    if tokio::runtime::Handle::try_current().is_err() {
        anyhow::bail!("OpenAI usage refresh requires a Tokio runtime")
    }

    let report = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(fetch_openai_usage_for_account(
            openai_provider_display_name(label, email.as_deref(), 2, false),
            creds,
            Some(label),
        ))
    });
    let data = openai_usage_data_from_provider_report(&report);
    store_openai_usage(cache_key, data.clone());
    Ok(openai_snapshot_from_usage(label.to_string(), email, &data))
}

pub fn account_usage_probe_sync(provider: MultiAccountProviderKind) -> Option<AccountUsageProbe> {
    match provider {
        MultiAccountProviderKind::Anthropic => anthropic_account_usage_probe_sync(),
        MultiAccountProviderKind::OpenAI => openai_account_usage_probe_sync(),
    }
}

fn anthropic_account_usage_probe_sync() -> Option<AccountUsageProbe> {
    let accounts = auth::claude::list_accounts().ok()?;
    if accounts.is_empty() {
        return None;
    }

    let current_label = auth::claude::active_account_label()
        .or_else(|| accounts.first().map(|account| account.label.clone()))?;
    let active_cached = get_sync();

    let mut snapshots = Vec::with_capacity(accounts.len());
    for account in &accounts {
        let usage = if account.label == current_label && active_cached.fetched_at.is_some() {
            Ok(active_cached.clone())
        } else {
            fetch_usage_for_account_sync(&account.access, &account.refresh, account.expires)
        };

        match usage {
            Ok(usage) => snapshots.push(anthropic_snapshot_from_usage(
                account.label.clone(),
                account.email.clone(),
                &usage,
            )),
            Err(err) => snapshots.push(AccountUsageSnapshot {
                label: account.label.clone(),
                email: account.email.clone(),
                exhausted: false,
                five_hour_ratio: None,
                seven_day_ratio: None,
                resets_at: None,
                error: Some(err.to_string()),
            }),
        }
    }

    Some(AccountUsageProbe {
        provider: MultiAccountProviderKind::Anthropic,
        current_label,
        accounts: snapshots,
    })
}

fn openai_account_usage_probe_sync() -> Option<AccountUsageProbe> {
    let accounts = auth::codex::list_accounts().ok()?;
    if accounts.is_empty() {
        return None;
    }

    let current_label = auth::codex::active_account_label()
        .or_else(|| accounts.first().map(|account| account.label.clone()))?;
    let active_cached = get_openai_usage_sync();

    let mut snapshots = Vec::with_capacity(accounts.len());
    for account in &accounts {
        let usage = if account.label == current_label && active_cached.fetched_at.is_some() {
            Ok(openai_snapshot_from_usage(
                account.label.clone(),
                account.email.clone(),
                &active_cached,
            ))
        } else {
            fetch_openai_usage_for_account_sync(
                &account.label,
                account.email.clone(),
                auth::codex::CodexCredentials {
                    access_token: account.access_token.clone(),
                    refresh_token: account.refresh_token.clone(),
                    id_token: account.id_token.clone(),
                    account_id: account.account_id.clone(),
                    expires_at: account.expires_at,
                },
            )
        };

        match usage {
            Ok(snapshot) => snapshots.push(snapshot),
            Err(err) => snapshots.push(AccountUsageSnapshot {
                label: account.label.clone(),
                email: account.email.clone(),
                exhausted: false,
                five_hour_ratio: None,
                seven_day_ratio: None,
                resets_at: None,
                error: Some(err.to_string()),
            }),
        }
    }

    Some(AccountUsageProbe {
        provider: MultiAccountProviderKind::OpenAI,
        current_label,
        accounts: snapshots,
    })
}

async fn fetch_usage_for_account(
    access_token: String,
    _refresh_token: String,
    expires_at: i64,
) -> Result<UsageData> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    if expires_at < now_ms {
        anyhow::bail!("OAuth token expired");
    }

    let cache_key = anthropic_usage_cache_key(&access_token, None);
    fetch_anthropic_usage_data(access_token, cache_key).await
}

/// Get usage data synchronously (returns cached data, triggers refresh if stale)
pub fn get_sync() -> UsageData {
    // Try to get cached data
    if let Some(usage) = USAGE.get() {
        // Return current cached value (blocking read)
        if let Ok(data) = usage.try_read() {
            if data.is_stale() {
                try_spawn_refresh(usage.clone());
            }
            return data.display_snapshot();
        }
    }

    // Not initialized yet - trigger initialization
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::spawn(async {
            let _ = get().await;
        });
    }

    UsageData::default()
}
