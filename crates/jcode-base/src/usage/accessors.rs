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
    let active_label = auth::claude::active_account_label();
    let initial_key = anthropic_usage_cache_key(&creds.access_token, active_label.as_deref());
    if let Some(data) = cached_or_shared_anthropic_usage(&initial_key) {
        return Ok(data);
    }
    let refresh_label = active_label
        .clone()
        .unwrap_or_else(auth::claude::primary_account_label);
    let access_token = if creds.expires_at < now + 300_000 && !creds.refresh_token.is_empty() {
        match auth::oauth::refresh_claude_tokens_for_account(&creds.refresh_token, &refresh_label)
            .await
        {
            Ok(refreshed) => refreshed.access_token,
            Err(_) => creds.access_token,
        }
    } else {
        creds.access_token
    };

    let cache_key = anthropic_usage_cache_key(&access_token, active_label.as_deref());
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
    let generation = openai_usage_generation();
    let new_data = fetch_openai_usage_data().await;
    let mut cached = usage.write().await;
    if generation == openai_usage_generation() {
        *cached = new_data;
    }
}

fn try_spawn_openai_refresh(usage: Arc<RwLock<OpenAIUsageData>>) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if OPENAI_REFRESH_IN_FLIGHT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    runtime.spawn(async move {
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

static OPENAI_LABEL_REFRESH_IN_FLIGHT: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashSet<String>>,
> = std::sync::OnceLock::new();

/// Usage for one saved OpenAI account, by label. Never blocks: reads the
/// label-scoped cache and, when stale, refreshes that account in the
/// background. Use this for the account a request actually authenticates
/// as, not `get_openai_usage_sync`, which follows the global active login.
pub fn get_openai_usage_for_label_sync(label: &str) -> OpenAIUsageData {
    let label = label.trim().to_string();
    if label.is_empty() {
        return OpenAIUsageData::default();
    }
    let key = openai_usage_cache_key("", Some(&label));
    let snapshot = openai_usage_cache()
        .try_lock()
        .ok()
        .and_then(|map| map.get(&key).cloned());
    let stale = snapshot.as_ref().is_none_or(OpenAIUsageData::is_stale);
    if stale && let Ok(runtime) = tokio::runtime::Handle::try_current() {
        let in_flight = OPENAI_LABEL_REFRESH_IN_FLIGHT.get_or_init(Default::default);
        if in_flight
            .lock()
            .is_ok_and(|mut labels| labels.insert(label.clone()))
        {
            runtime.spawn(async move {
                if let Some(account) = auth::codex::list_accounts()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|account| account.label == label)
                {
                    let creds = auth::codex::CodexCredentials {
                        access_token: account.access_token,
                        refresh_token: account.refresh_token,
                        id_token: account.id_token,
                        account_id: account.account_id,
                        expires_at: account.expires_at,
                    };
                    let _ = fetch_openai_usage_for_account(
                        "OpenAI (ChatGPT)".into(),
                        creds,
                        Some(&label),
                    )
                    .await;
                }
                if let Ok(mut labels) = in_flight.lock() {
                    labels.remove(&label);
                }
            });
        }
    }
    snapshot
        .map(|data| data.display_snapshot())
        .unwrap_or_default()
}

#[derive(Default)]
struct EmailScopedOpenAiUsage {
    generation: u64,
    data: OpenAIUsageData,
    refreshing: bool,
}

impl EmailScopedOpenAiUsage {
    fn current(&mut self, generation: u64) -> &mut Self {
        if self.generation != generation {
            *self = Self {
                generation,
                ..Default::default()
            };
        }
        self
    }
}

static OPENAI_EMAIL_USAGE: std::sync::OnceLock<
    std::sync::Mutex<HashMap<String, EmailScopedOpenAiUsage>>,
> = std::sync::OnceLock::new();

fn normalized_usage_email(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

fn unique_openai_account_for_email<'a>(
    accounts: &'a [auth::codex::OpenAiAccount],
    email: &str,
) -> Option<&'a auth::codex::OpenAiAccount> {
    let email = normalized_usage_email(email);
    if email.is_empty() {
        return None;
    }
    let mut matches = accounts.iter().filter(|account| {
        account
            .email
            .as_deref()
            .is_some_and(|candidate| normalized_usage_email(candidate) == email)
    });
    let account = matches.next()?;
    // An email may identify multiple workspace logins with different quotas.
    matches.next().is_none().then_some(account)
}

async fn fetch_openai_usage_for_email(email: &str) -> OpenAIUsageData {
    let accounts = auth::codex::list_accounts().unwrap_or_default();
    let Some(account) = unique_openai_account_for_email(&accounts, email) else {
        return OpenAIUsageData {
            fetched_at: Some(Instant::now()),
            last_error: Some("No unique OpenAI OAuth account matches this email".into()),
            ..Default::default()
        };
    };
    let credentials = auth::codex::CodexCredentials {
        access_token: account.access_token.clone(),
        refresh_token: account.refresh_token.clone(),
        id_token: account.id_token.clone(),
        account_id: account.account_id.clone(),
        expires_at: account.expires_at,
    };
    let report = fetch_openai_usage_for_account(
        "OpenAI (ChatGPT)".into(),
        credentials,
        Some(&account.label),
    )
    .await;
    openai_usage_data_from_provider_report(&report)
}

/// Nonblocking usage for the session's OAuth email, never the global login.
///
/// Reads only an in-memory, normalized exact-email cache on the render thread.
/// Account lookup and refresh run in the background and reuse the label-scoped
/// usage cache. Unknown or ambiguous emails return unavailable data. With no
/// runtime, cached data is still readable but no refresh is started.
pub fn get_openai_usage_for_email_sync(email: &str) -> OpenAIUsageData {
    let email = normalized_usage_email(email);
    if email.is_empty() {
        return OpenAIUsageData::default();
    }
    let cache = OPENAI_EMAIL_USAGE.get_or_init(Default::default);
    let Ok(mut entries) = cache.try_lock() else {
        return OpenAIUsageData::default();
    };
    let generation = openai_usage_generation();
    let entry = entries
        .entry(email.clone())
        .or_default()
        .current(generation);
    let snapshot = entry.data.display_snapshot();
    if !entry.refreshing
        && entry.data.is_stale()
        && let Ok(runtime) = tokio::runtime::Handle::try_current()
    {
        entry.refreshing = true;
        runtime.spawn(async move {
            let data = fetch_openai_usage_for_email(&email).await;
            if let Ok(mut entries) = cache.lock()
                && generation == openai_usage_generation()
                && let Some(entry) = entries.get_mut(&email)
                && entry.generation == generation
            {
                entry.data = data;
                entry.refreshing = false;
            }
        });
    }
    snapshot
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

    if let Some(cached) = cached_or_shared_anthropic_usage(&cache_key) {
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

    let generation = openai_usage_generation();
    let report = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(fetch_openai_usage_for_account(
            openai_provider_display_name(label, email.as_deref(), 2, false),
            creds,
            Some(label),
        ))
    });
    let data = openai_usage_data_from_provider_report(&report);
    anyhow::ensure!(
        generation == openai_usage_generation(),
        "OpenAI usage changed during a banked reset. Retry the usage check."
    );
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
                primary_label: None,
                five_hour_ratio: None,
                secondary_label: None,
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
                primary_label: None,
                five_hour_ratio: None,
                secondary_label: None,
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

/// Fetch the current Anthropic OAuth usage for an already-resolved access
/// token. This is used on the request path when model-scoped quota affects
/// routing. Unlike [`get`], it waits for the first fetch instead of returning
/// an empty snapshot while a background refresh starts.
pub async fn fetch_usage_for_access_token(access_token: &str) -> Result<UsageData> {
    let cache_key = anthropic_usage_cache_key(access_token, None);
    fetch_anthropic_usage_data(access_token.to_string(), cache_key).await
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

#[cfg(test)]
mod email_usage_tests {
    use super::*;

    fn account(label: &str, email: Option<&str>) -> auth::codex::OpenAiAccount {
        auth::codex::OpenAiAccount {
            label: label.into(),
            email: email.map(str::to_string),
            access_token: String::new(),
            refresh_token: String::new(),
            id_token: None,
            account_id: None,
            expires_at: None,
        }
    }

    fn usage(ratio: f32) -> OpenAIUsageData {
        OpenAIUsageData {
            five_hour: Some(OpenAIUsageWindow {
                name: "5-hour window".into(),
                usage_ratio: ratio,
                resets_at: None,
            }),
            fetched_at: Some(Instant::now()),
            ..Default::default()
        }
    }

    #[test]
    fn email_resolution_is_exact_normalized_and_not_first_account() {
        let accounts = vec![
            account("global", Some("global@example.test")),
            account("session", Some(" Session@Example.Test ")),
            account("unknown-profile", None),
        ];
        assert_eq!(
            unique_openai_account_for_email(&accounts, " session@EXAMPLE.test ")
                .map(|account| account.label.as_str()),
            Some("session")
        );
        for email in ["", " ", "missing@example.test", "example.test", "session"] {
            assert!(unique_openai_account_for_email(&accounts, email).is_none());
        }
    }

    #[test]
    fn email_resolution_rejects_multiple_workspace_accounts() {
        let accounts = vec![
            account("personal", Some("shared@example.test")),
            account("workspace", Some("SHARED@example.test")),
        ];
        assert!(unique_openai_account_for_email(&accounts, "shared@example.test").is_none());
    }

    #[test]
    fn email_cache_keeps_account_snapshots_separate() {
        let mut entries = HashMap::<String, EmailScopedOpenAiUsage>::new();
        for (email, ratio) in [("healthy@example.test", 0.18), ("full@example.test", 1.0)] {
            entries.insert(
                normalized_usage_email(email),
                EmailScopedOpenAiUsage {
                    generation: 7,
                    data: usage(ratio),
                    refreshing: false,
                },
            );
        }
        let healthy = entries
            .get_mut(&normalized_usage_email(" HEALTHY@example.test "))
            .unwrap()
            .current(7)
            .data
            .display_snapshot();
        assert_eq!(healthy.five_hour.unwrap().usage_ratio, 0.18);
        let full = entries
            .get_mut("full@example.test")
            .unwrap()
            .current(7)
            .data
            .display_snapshot();
        assert!(full.exhausted());
        let unknown = entries
            .entry("unknown@example.test".into())
            .or_default()
            .current(7);
        assert!(!unknown.data.has_limits());
        assert!(unknown.data.is_stale());
    }

    #[test]
    fn email_cache_generation_change_discards_snapshot_and_inflight_marker() {
        let mut entry = EmailScopedOpenAiUsage {
            generation: 7,
            data: usage(1.0),
            refreshing: true,
        };
        assert!(entry.current(7).data.exhausted());
        let invalidated = entry.current(8);
        assert!(!invalidated.data.has_limits());
        assert!(invalidated.data.is_stale());
        assert!(!invalidated.refreshing);
        assert_eq!(invalidated.generation, 8);
    }

    #[test]
    fn email_accessor_without_runtime_is_unavailable_for_unknown_identity() {
        assert!(!get_openai_usage_for_email_sync("").has_limits());
        assert!(!get_openai_usage_for_email_sync("unknown-accessor@example.test").has_limits());
    }
}
