//! Managed provider-account storage and safe import from OpenCodeX.
//!
//! This module deliberately stores credentials separately from provider runtime
//! state. Selection is process-local, while the account file is stored as a
//! plaintext owner-protected secret file. It is not encrypted at rest. Never
//! include its contents in logs, quota snapshots, or diagnostics.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

const MAX_ACCOUNTS: usize = 100;
const MAX_QUOTA_MODELS_PER_ACCOUNT: usize = 200;
const QUOTA_SNAPSHOT_TTL_SECS: i64 = 6 * 60 * 60;
const MAX_QUOTA_HISTORY: usize = 8;

/// Process-local health state. Credential files remain durable, while cooldowns
/// are intentionally ephemeral and cannot strand an account after a restart.
static ACCOUNT_COOLDOWNS: LazyLock<Mutex<HashMap<(String, String), Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static ACCOUNT_LEASES: LazyLock<Mutex<HashMap<(String, String), (Instant, u64)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_ACCOUNT_LEASE_ID: AtomicU64 = AtomicU64::new(1);
static ACCOUNT_REQUEST_GATES: LazyLock<Mutex<HashMap<String, Arc<AccountRequestGate>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
// Account refresh, import, and switching all use a read-modify-write cycle.
// Keep those mutations serialized within the daemon so concurrent refreshes do
// not overwrite an account imported or updated by another request.
static ACCOUNT_STORE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
// Quota and cooldown updates are performed by concurrent provider requests.
// Serialize the read-modify-write cycle so one account's update cannot erase
// another account's freshly recorded state. HealthFileLock extends this
// protection across independent processes sharing the same JCODE_HOME.
static HEALTH_STATE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// A short-lived admission lease prevents concurrent failover attempts from
/// stampeding the same alternate account. The lease is held by the returned
/// value and released when the request stream is dropped or completes.
#[derive(Debug)]
pub struct AccountLease {
    key: (String, String),
    id: u64,
}

impl Drop for AccountLease {
    fn drop(&mut self) {
        if let Ok(mut leases) = ACCOUNT_LEASES.lock()
            && leases.get(&self.key).is_some_and(|(_, id)| *id == self.id)
        {
            leases.remove(&self.key);
        }
    }
}

pub fn try_acquire_account_lease(
    provider: &str,
    label: &str,
    duration: Duration,
) -> Option<AccountLease> {
    let key = (provider.to_string(), label.to_string());
    let now = Instant::now();
    let id = NEXT_ACCOUNT_LEASE_ID.fetch_add(1, AtomicOrdering::Relaxed);
    let mut leases = ACCOUNT_LEASES.lock().ok()?;
    if leases.get(&key).is_some_and(|(until, _)| *until > now) {
        return None;
    }
    leases.insert(key.clone(), (now + duration, id));
    Some(AccountLease { key, id })
}

/// Providers whose credentials are selected through the process-local account
/// override need an exclusive request scope. The downstream runtimes read that
/// override while constructing a request and some of them may rotate it while
/// a stream is still alive. Keeping this gate for the complete stream lifetime
/// prevents two requests from observing each other's account.
fn uses_runtime_account_override(provider: &str) -> bool {
    matches!(provider, "claude" | "openai" | "antigravity" | "cursor")
}

struct AccountRequestGate {
    held: std::sync::atomic::AtomicBool,
    released: Notify,
}

impl AccountRequestGate {
    fn new() -> Self {
        Self {
            held: std::sync::atomic::AtomicBool::new(false),
            released: Notify::new(),
        }
    }

    fn try_acquire(self: &Arc<Self>) -> Option<AccountRequestLease> {
        self.held
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::Acquire,
                std::sync::atomic::Ordering::Relaxed,
            )
            .is_ok()
            .then(|| AccountRequestLease {
                _inner: Arc::new(AccountRequestLeaseInner {
                    gate: Arc::clone(self),
                }),
            })
    }

    async fn acquire(self: Arc<Self>) -> AccountRequestLease {
        loop {
            let notified = self.released.notified();
            if let Some(lease) = self.try_acquire() {
                return lease;
            }
            notified.await;
        }
    }
}

struct AccountRequestLeaseInner {
    gate: Arc<AccountRequestGate>,
}

impl Drop for AccountRequestLeaseInner {
    fn drop(&mut self) {
        self.gate
            .held
            .store(false, std::sync::atomic::Ordering::Release);
        self.gate.released.notify_one();
    }
}

/// Request-scoped ownership of a provider's account override. It is cloneable
/// so a caller can retain the scope while passing one ownership reference to a
/// returned event stream. The gate is released only after the final clone is
/// dropped.
#[derive(Clone)]
pub struct AccountRequestLease {
    _inner: Arc<AccountRequestLeaseInner>,
}

fn account_request_gate(provider: &str) -> Option<Arc<AccountRequestGate>> {
    if !uses_runtime_account_override(provider) {
        return None;
    }
    let mut gates = ACCOUNT_REQUEST_GATES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Some(
        gates
            .entry(provider.to_string())
            .or_insert_with(|| Arc::new(AccountRequestGate::new()))
            .clone(),
    )
}

/// Acquire the request scope asynchronously. Non-account providers return
/// `None` and retain their existing concurrent behavior.
pub async fn acquire_account_request_lease(provider: &str) -> Option<AccountRequestLease> {
    match account_request_gate(provider) {
        Some(gate) => Some(gate.acquire().await),
        None => None,
    }
}

/// Try to acquire the request scope from synchronous account-management paths.
/// A switch is rejected while an active stream owns the scope instead of
/// mutating the global override underneath that request.
pub fn try_acquire_account_request_lease(provider: &str) -> Option<AccountRequestLease> {
    account_request_gate(provider).and_then(|gate| gate.try_acquire())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PoolHealthFile {
    /// Provider health is deliberately stored separately from credential files.
    /// This file must never contain access or refresh tokens.
    #[serde(default)]
    cooldowns: HashMap<String, HashMap<String, i64>>,
    #[serde(default)]
    quotas: HashMap<String, HashMap<String, HashMap<String, AccountQuotaSnapshot>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountQuotaSnapshot {
    pub remaining_fraction_milli: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_time: Option<String>,
    pub observed_at_unix_secs: i64,
    /// Recent observations make durable quota state inspectable without allowing
    /// an unbounded provider response stream to grow the state file. This is
    /// additive so snapshots written before history was introduced still load.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<QuotaObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuotaObservation {
    pub remaining_fraction_milli: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_time: Option<String>,
    pub observed_at_unix_secs: i64,
}

impl AccountQuotaSnapshot {
    fn observations(&self) -> impl Iterator<Item = QuotaObservation> + '_ {
        self.history
            .iter()
            .cloned()
            .chain(std::iter::once(QuotaObservation {
                remaining_fraction_milli: self.remaining_fraction_milli,
                reset_time: self.reset_time.clone(),
                observed_at_unix_secs: self.observed_at_unix_secs,
            }))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedProviderAccount {
    pub id: String,
    pub label: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AccountFile {
    #[serde(default)]
    active_account: Option<String>,
    #[serde(default)]
    accounts: Vec<ManagedProviderAccount>,
}

pub fn accounts_path(provider: &str) -> Result<PathBuf> {
    let filename = match provider {
        "antigravity" => "antigravity_accounts.json",
        "cursor" => "cursor_accounts.json",
        _ => anyhow::bail!("unsupported managed account provider: {provider}"),
    };
    Ok(crate::storage::jcode_dir()?.join(filename))
}

fn read(provider: &str) -> Result<AccountFile> {
    let _file_lock = AccountFileLock::acquire(provider, true)
        .ok_or_else(|| anyhow::anyhow!("could not lock managed {provider} account store"))?;
    read_unlocked(provider)
}

/// Coordinate managed-account reads and read-modify-write mutations across
/// independent processes sharing the same JCODE_HOME. The account JSON remains
/// plaintext owner-protected, but concurrent refreshes and imports cannot
/// overwrite one another's updates.
struct AccountFileLock {
    file: File,
}

impl AccountFileLock {
    fn acquire(provider: &str, shared: bool) -> Option<Self> {
        let path = accounts_path(provider).ok().map(|path| {
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("accounts");
            path.with_file_name(format!("{file_name}.lock"))
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)
            .ok()?;

        #[cfg(unix)]
        {
            let operation = if shared { libc::LOCK_SH } else { libc::LOCK_EX };
            if unsafe { libc::flock(file.as_raw_fd(), operation) } != 0 {
                return None;
            }
        }

        Some(Self { file })
    }
}

impl Drop for AccountFileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

fn read_unlocked(provider: &str) -> Result<AccountFile> {
    let file_path = accounts_path(provider)?;
    if !file_path.exists() {
        return Ok(AccountFile::default());
    }
    crate::storage::harden_secret_file_permissions(&file_path);
    let file: AccountFile = crate::storage::read_json(&file_path)
        .with_context(|| format!("failed to read managed {provider} account store"))?;
    if file.accounts.len() > MAX_ACCOUNTS {
        anyhow::bail!("managed {provider} account store exceeds the {MAX_ACCOUNTS} account limit");
    }
    if file.accounts.iter().any(|account| {
        account.id.trim().is_empty()
            || account.label.trim().is_empty()
            || account.access_token.trim().is_empty()
            || account.refresh_token.trim().is_empty()
    }) {
        anyhow::bail!("managed {provider} account store contains an incomplete account");
    }
    Ok(file)
}

#[cfg(test)]
fn write(provider: &str, file: &AccountFile) -> Result<()> {
    let _file_lock = AccountFileLock::acquire(provider, false)
        .ok_or_else(|| anyhow::anyhow!("could not lock managed {provider} account store"))?;
    write_unlocked(provider, file)
}

fn write_unlocked(provider: &str, file: &AccountFile) -> Result<()> {
    let file_path = accounts_path(provider)?;
    crate::storage::write_json_secret(&file_path, file)
}

pub fn list_accounts(provider: &str) -> Result<Vec<ManagedProviderAccount>> {
    Ok(read(provider)?.accounts)
}

pub fn active_account(provider: &str) -> Result<Option<ManagedProviderAccount>> {
    let file = read(provider)?;
    let label = crate::auth::account_store::active_account_label(
        crate::auth::account_store::runtime_active_override(provider),
        file.active_account,
        &file.accounts,
        |account| account.label.as_str(),
    );
    Ok(label.and_then(|label| {
        file.accounts
            .into_iter()
            .find(|account| account.label == label)
    }))
}

pub fn set_active_account(provider: &str, label: &str) -> Result<()> {
    let _request_lease = try_acquire_account_request_lease(provider).ok_or_else(|| {
        anyhow::anyhow!("Cannot switch {provider} accounts while a request is active")
    })?;
    let _guard = ACCOUNT_STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _file_lock = AccountFileLock::acquire(provider, false)
        .ok_or_else(|| anyhow::anyhow!("could not lock managed {provider} account store"))?;
    let mut file = read_unlocked(provider)?;
    crate::auth::account_store::set_active_account(
        label,
        &file.accounts,
        &mut file.active_account,
        &format!("No managed {provider} account named '{{}}'"),
        |account| account.label.as_str(),
    )?;
    write_unlocked(provider, &file)?;
    set_runtime_active_override_for_provider(provider, Some(label.to_string()));
    Ok(())
}

pub fn set_runtime_active_override(provider: &'static str, label: Option<String>) {
    crate::auth::account_store::set_runtime_active_override(provider, label);
}

fn set_runtime_active_override_for_provider(provider: &str, label: Option<String>) {
    let provider: &'static str = match provider {
        "antigravity" => "antigravity",
        "cursor" => "cursor",
        _ => return,
    };
    set_runtime_active_override(provider, label);
}

fn health_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("provider_pool_state.json"))
}

fn health_lock_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("provider_pool_state.json.lock"))
}

/// Coordinate health read-modify-write cycles across independent jcode
/// processes. The in-process mutex remains necessary for threads, while this
/// advisory lock protects the durable JSON file when more than one daemon or
/// CLI process shares the same JCODE_HOME.
struct HealthFileLock {
    file: File,
}

impl HealthFileLock {
    fn acquire(shared: bool) -> Option<Self> {
        let path = health_lock_path().ok()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)
            .ok()?;

        #[cfg(unix)]
        {
            let operation = if shared { libc::LOCK_SH } else { libc::LOCK_EX };
            if unsafe { libc::flock(file.as_raw_fd(), operation) } != 0 {
                return None;
            }
        }

        Some(Self { file })
    }
}

impl Drop for HealthFileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn read_health_unlocked() -> PoolHealthFile {
    let Ok(path) = health_path() else {
        return PoolHealthFile::default();
    };
    if !path.exists() {
        return PoolHealthFile::default();
    }
    crate::storage::read_json(&path).unwrap_or_default()
}

fn write_health_unlocked(health: &PoolHealthFile) {
    let Ok(path) = health_path() else {
        return;
    };
    if let Err(error) = jcode_storage::write_json(&path, health) {
        crate::logging::warn(&format!(
            "Could not persist provider pool health state at {}: {}",
            path.display(),
            error
        ));
    }
}

fn read_health() -> PoolHealthFile {
    let Ok(_guard) = HEALTH_STATE_LOCK.lock() else {
        return PoolHealthFile::default();
    };
    let Some(_file_lock) = HealthFileLock::acquire(true) else {
        return PoolHealthFile::default();
    };
    read_health_unlocked()
}

fn update_health(update: impl FnOnce(&mut PoolHealthFile)) {
    let Ok(_guard) = HEALTH_STATE_LOCK.lock() else {
        return;
    };
    let Some(_file_lock) = HealthFileLock::acquire(false) else {
        crate::logging::warn("Could not acquire provider pool health state lock");
        return;
    };
    let mut health = read_health_unlocked();
    update(&mut health);
    write_health_unlocked(&health);
}

fn persisted_cooldown_until(provider: &str, label: &str) -> Option<i64> {
    read_health()
        .cooldowns
        .get(provider)
        .and_then(|accounts| accounts.get(label).copied())
}

fn persist_cooldown(provider: &str, label: &str, until: Option<i64>) {
    update_health(|health| {
        let accounts = health.cooldowns.entry(provider.to_string()).or_default();
        match until {
            Some(until) => {
                accounts.insert(label.to_string(), until);
            }
            None => {
                accounts.remove(label);
                if accounts.is_empty() {
                    health.cooldowns.remove(provider);
                }
            }
        }
    });
}

/// Record model-scoped quota metadata without ever persisting credentials.
/// Providers may omit either quota value when their response has no usable
/// quota signal. Old snapshots are ignored by the ranking helper below.
pub fn record_account_quota(
    provider: &str,
    label: &str,
    model: &str,
    remaining_fraction_milli: Option<u16>,
    reset_time: Option<String>,
) {
    record_account_quotas(
        provider,
        label,
        &[(model.to_string(), remaining_fraction_milli, reset_time)],
    );
}

/// Record several model-scoped quota values in one atomic state-file update.
pub fn record_account_quotas(
    provider: &str,
    label: &str,
    quotas: &[(String, Option<u16>, Option<String>)],
) {
    update_health(|health| {
        let models = health
            .quotas
            .entry(provider.to_string())
            .or_default()
            .entry(label.to_string())
            .or_default();
        let observed_at_unix_secs = unix_now();
        for (model, remaining_fraction_milli, reset_time) in quotas {
            let model = model.trim();
            if model.is_empty() {
                continue;
            }
            if models.len() >= MAX_QUOTA_MODELS_PER_ACCOUNT && !models.contains_key(model) {
                if let Some(oldest) = models
                    .iter()
                    .min_by_key(|(_, snapshot)| snapshot.observed_at_unix_secs)
                    .map(|(model, _)| model.clone())
                {
                    models.remove(&oldest);
                }
            }
            let observation = QuotaObservation {
                remaining_fraction_milli: *remaining_fraction_milli,
                reset_time: reset_time.clone(),
                observed_at_unix_secs,
            };
            let history = models
                .get(model)
                .map(|previous| {
                    previous
                        .observations()
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .take(MAX_QUOTA_HISTORY - 1)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            models.insert(
                model.to_string(),
                AccountQuotaSnapshot {
                    remaining_fraction_milli: observation.remaining_fraction_milli,
                    reset_time: observation.reset_time.clone(),
                    observed_at_unix_secs,
                    history,
                },
            );
        }
    });
}

/// Return the best recent remaining quota signal for an account. `None` means
/// the provider has not supplied a usable quota signal recently.
pub fn account_quota_score(provider: &str, label: &str) -> Option<u16> {
    let now = unix_now();
    read_health()
        .quotas
        .get(provider)
        .and_then(|accounts| accounts.get(label))
        .and_then(|models| {
            models
                .values()
                .filter(|snapshot| {
                    now.saturating_sub(snapshot.observed_at_unix_secs) <= QUOTA_SNAPSHOT_TTL_SECS
                })
                .filter_map(|snapshot| snapshot.remaining_fraction_milli)
                .max()
        })
}

/// Return the recent remaining quota signal for one model on an account.
/// Callers should use this when selecting an account for a concrete request;
/// the provider-wide score remains useful when no model is known yet.
pub fn account_quota_score_for_model(provider: &str, label: &str, model: &str) -> Option<u16> {
    let model = model.trim();
    if model.is_empty() {
        return account_quota_score(provider, label);
    }
    let now = unix_now();
    read_health()
        .quotas
        .get(provider)
        .and_then(|accounts| accounts.get(label))
        .and_then(|models| models.get(model))
        .filter(|snapshot| {
            now.saturating_sub(snapshot.observed_at_unix_secs) <= QUOTA_SNAPSHOT_TTL_SECS
        })
        .and_then(|snapshot| snapshot.remaining_fraction_milli)
}

/// Return recent quota snapshots without exposing credentials. This is used by
/// usage reporters to keep the last known-good display when a provider briefly
/// returns 401/429 or malformed data.
pub fn account_quota_snapshots(provider: &str, label: &str) -> Vec<(String, AccountQuotaSnapshot)> {
    let now = unix_now();
    read_health()
        .quotas
        .get(provider)
        .and_then(|accounts| accounts.get(label))
        .map(|models| {
            models
                .iter()
                .filter(|(_, snapshot)| {
                    now.saturating_sub(snapshot.observed_at_unix_secs) <= QUOTA_SNAPSHOT_TTL_SECS
                })
                .map(|(model, snapshot)| (model.clone(), snapshot.clone()))
                .collect()
        })
        .unwrap_or_default()
}

pub fn account_on_cooldown(provider: &str, label: &str) -> bool {
    let key = (provider.to_string(), label.to_string());
    if let Ok(mut cooldowns) = ACCOUNT_COOLDOWNS.lock() {
        match cooldowns.get(&key).copied() {
            Some(until) if until > Instant::now() => return true,
            Some(_) => {
                cooldowns.remove(&key);
            }
            None => {}
        }
    }

    let now = unix_now();
    match persisted_cooldown_until(provider, label) {
        Some(until) if until > now => {
            if let Ok(mut cooldowns) = ACCOUNT_COOLDOWNS.lock() {
                let seconds = (until - now).try_into().unwrap_or(u64::MAX);
                cooldowns.insert(key, Instant::now() + Duration::from_secs(seconds));
            }
            true
        }
        Some(_) => {
            persist_cooldown(provider, label, None);
            false
        }
        None => false,
    }
}

pub fn mark_account_cooldown(provider: &str, label: &str, duration: Duration) {
    if let Ok(mut cooldowns) = ACCOUNT_COOLDOWNS.lock() {
        cooldowns.insert(
            (provider.to_string(), label.to_string()),
            Instant::now() + duration,
        );
    }
    persist_cooldown(
        provider,
        label,
        Some(unix_now().saturating_add(duration.as_secs().try_into().unwrap_or(i64::MAX))),
    );
}

pub fn clear_account_cooldown(provider: &str, label: &str) {
    if let Ok(mut cooldowns) = ACCOUNT_COOLDOWNS.lock() {
        cooldowns.remove(&(provider.to_string(), label.to_string()));
    }
    persist_cooldown(provider, label, None);
}

/// Choose a durable account cooldown from an upstream error. Provider retry
/// loops already honor bounded Retry-After hints; using the same hint here
/// prevents failover from immediately retrying an account the server asked us
/// to park. A short floor avoids hot-looping on a zero or near-zero hint.
pub fn cooldown_for_error(error: &anyhow::Error, default: Duration) -> Duration {
    jcode_provider_core::retry_after::retry_after_from_error(error)
        .map(|hint| hint.max(Duration::from_secs(30)))
        .unwrap_or(default)
}

pub fn upsert_account(provider: &str, account: ManagedProviderAccount) -> Result<String> {
    let _guard = ACCOUNT_STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _file_lock = AccountFileLock::acquire(provider, false)
        .ok_or_else(|| anyhow::anyhow!("could not lock managed {provider} account store"))?;
    let mut file = read_unlocked(provider)?;
    let id = account.id.clone();
    if let Some(existing) = file.accounts.iter_mut().find(|existing| existing.id == id) {
        let label = existing.label.clone();
        *existing = ManagedProviderAccount { label, ..account };
    } else {
        if file.accounts.len() >= MAX_ACCOUNTS {
            anyhow::bail!("managed {provider} account store is full");
        }
        file.accounts.push(account);
    }
    if file.active_account.is_none() {
        file.active_account = file.accounts.first().map(|account| account.label.clone());
    }
    let label = file
        .accounts
        .iter()
        .find(|account| account.id == id)
        .map(|account| account.label.clone())
        .context("managed account disappeared while importing")?;
    write_unlocked(provider, &file)?;
    Ok(label)
}

pub fn update_tokens_for_refresh(
    provider: &str,
    previous_refresh_token: &str,
    access_token: String,
    refresh_token: String,
    expires_at: i64,
    email: Option<String>,
    project_id: Option<String>,
) -> Result<bool> {
    let _guard = ACCOUNT_STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _file_lock = AccountFileLock::acquire(provider, false)
        .ok_or_else(|| anyhow::anyhow!("could not lock managed {provider} account store"))?;
    let mut file = read_unlocked(provider)?;
    let Some(account) = file
        .accounts
        .iter_mut()
        .find(|account| account.refresh_token == previous_refresh_token)
    else {
        return Ok(false);
    };
    account.access_token = access_token;
    account.refresh_token = refresh_token;
    account.expires_at = expires_at;
    if email.is_some() {
        account.email = email;
    }
    if project_id.is_some() {
        account.project_id = project_id;
    }
    write_unlocked(provider, &file)?;
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportSummary {
    pub cursor_imported: usize,
    pub antigravity_imported: usize,
}

#[derive(Debug, Deserialize)]
struct OpenCodeXAuth {
    #[serde(rename = "cursor", default)]
    cursor: Option<OpenCodeXProvider>,
    #[serde(rename = "google-antigravity", default)]
    antigravity: Option<OpenCodeXProvider>,
}

#[derive(Debug, Deserialize)]
struct OpenCodeXProvider {
    #[serde(default)]
    accounts: Vec<OpenCodeXAccount>,
    #[serde(default, rename = "activeAccountId")]
    active_account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenCodeXAccount {
    id: String,
    #[serde(default)]
    alias: Option<String>,
    credential: OpenCodeXCredential,
}

#[derive(Debug, Deserialize)]
struct OpenCodeXCredential {
    access: String,
    refresh: String,
    expires: i64,
    #[serde(default)]
    email: Option<String>,
    #[serde(default, rename = "projectId")]
    project_id: Option<String>,
}

fn opencodex_path() -> Result<PathBuf> {
    crate::storage::user_home_path(".opencodex/auth.json")
        .context("could not locate the user home directory")
}

fn label(provider: &str, index: usize, alias: Option<&str>, id: &str) -> String {
    let alias = alias.map(str::trim).filter(|v| !v.is_empty());
    let suffix = alias.unwrap_or(id);
    let safe = suffix
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    let safe = safe.trim_matches('-');
    if safe.is_empty() {
        crate::auth::account_store::canonical_account_label(provider, index + 1)
    } else {
        format!("{provider}-{safe}")
    }
}

pub fn import_opencodex() -> Result<ImportSummary> {
    let source = opencodex_path()?;
    if !source.exists() {
        anyhow::bail!("OpenCodeX auth file not found at {}", source.display());
    }
    let safe_source = crate::storage::validate_external_auth_file(&source)
        .context("refusing to import an unsafe OpenCodeX auth file")?;
    let raw =
        std::fs::read_to_string(&safe_source).context("failed to read OpenCodeX auth file")?;
    if raw.len() > 2 * 1024 * 1024 {
        anyhow::bail!("OpenCodeX auth file is unexpectedly large");
    }
    let parsed: OpenCodeXAuth =
        serde_json::from_str(&raw).context("failed to parse OpenCodeX auth file")?;
    let mut summary = ImportSummary {
        cursor_imported: 0,
        antigravity_imported: 0,
    };

    if let Some(provider) = parsed.cursor {
        for (index, account) in provider.accounts.into_iter().enumerate() {
            let account_id = account.id.trim();
            let access = account.credential.access.trim();
            let refresh = account.credential.refresh.trim();
            if account_id.is_empty() || access.is_empty() || refresh.is_empty() {
                anyhow::bail!("OpenCodeX cursor account contains incomplete credentials");
            }
            let account = ManagedProviderAccount {
                id: account_id.to_string(),
                label: label("cursor", index, account.alias.as_deref(), account_id),
                access_token: access.to_string(),
                refresh_token: refresh.to_string(),
                expires_at: account.credential.expires,
                email: account.credential.email,
                project_id: account.credential.project_id,
            };
            upsert_account("cursor", account)?;
            summary.cursor_imported += 1;
        }
        if let Some(active) = provider.active_account_id {
            if let Some(account) = list_accounts("cursor")?
                .into_iter()
                .find(|a| a.id == active)
            {
                set_active_account("cursor", &account.label)?;
            }
        }
    }

    if let Some(provider) = parsed.antigravity {
        for (index, account) in provider.accounts.into_iter().enumerate() {
            let account_id = account.id.trim();
            let access = account.credential.access.trim();
            let refresh = account.credential.refresh.trim();
            if account_id.is_empty() || access.is_empty() || refresh.is_empty() {
                anyhow::bail!("OpenCodeX Antigravity account contains incomplete credentials");
            }
            let account = ManagedProviderAccount {
                id: account_id.to_string(),
                label: label("antigravity", index, account.alias.as_deref(), account_id),
                access_token: access.to_string(),
                refresh_token: refresh.to_string(),
                expires_at: account.credential.expires,
                email: account.credential.email,
                project_id: account.credential.project_id,
            };
            upsert_account("antigravity", account)?;
            summary.antigravity_imported += 1;
        }
        if let Some(active) = provider.active_account_id {
            if let Some(account) = list_accounts("antigravity")?
                .into_iter()
                .find(|a| a.id == active)
            {
                set_active_account("antigravity", &account.label)?;
            }
        }
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_stable_and_safe() {
        assert_eq!(
            label("cursor", 0, Some("Work Email"), "id"),
            "cursor-Work-Email"
        );
        assert_eq!(label("cursor", 0, None, "id/unsafe"), "cursor-id-unsafe");
    }

    #[test]
    fn persisted_managed_switch_updates_runtime_override() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
        let previous_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home.path());
        set_runtime_active_override("cursor", Some("cursor-one".to_string()));

        let account = |label: &str| ManagedProviderAccount {
            id: label.to_string(),
            label: label.to_string(),
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: 0,
            email: None,
            project_id: None,
        };
        write(
            "cursor",
            &AccountFile {
                active_account: Some("cursor-one".to_string()),
                accounts: vec![account("cursor-one"), account("cursor-two")],
            },
        )
        .expect("write managed accounts");

        set_active_account("cursor", "cursor-two").expect("switch managed account");
        assert_eq!(
            crate::auth::account_store::runtime_active_override("cursor").as_deref(),
            Some("cursor-two")
        );
        assert_eq!(
            active_account("cursor")
                .expect("read active managed account")
                .expect("active account")
                .label,
            "cursor-two"
        );

        set_runtime_active_override("cursor", None);
        match previous_home {
            Some(previous) => crate::env::set_var("JCODE_HOME", previous),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[test]
    fn cooldown_state_survives_process_local_cache_reset_without_secrets() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
        let previous_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home.path());
        let provider = "cursor";
        let account = "cursor-persistence-test";

        clear_account_cooldown(provider, account);
        mark_account_cooldown(provider, account, Duration::from_secs(60));
        assert!(account_on_cooldown(provider, account));

        let state_path = health_path().expect("resolve health path");
        let state = std::fs::read_to_string(&state_path).expect("read persisted health state");
        assert!(state.contains(account));
        assert!(!state.contains("access_token"));
        assert!(!state.contains("refresh_token"));

        ACCOUNT_COOLDOWNS
            .lock()
            .expect("lock cooldown cache")
            .clear();
        assert!(account_on_cooldown(provider, account));

        clear_account_cooldown(provider, account);
        assert!(!account_on_cooldown(provider, account));

        match previous_home {
            Some(previous) => crate::env::set_var("JCODE_HOME", previous),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[test]
    fn quota_score_prefers_recent_high_remaining_account() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
        let previous_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home.path());

        record_account_quotas(
            "antigravity",
            "antigravity-low",
            &[("gemini-3-flash".to_string(), Some(100), None)],
        );
        record_account_quotas(
            "antigravity",
            "antigravity-high",
            &[(
                "gemini-3-flash".to_string(),
                Some(900),
                Some("reset".to_string()),
            )],
        );

        assert_eq!(
            account_quota_score("antigravity", "antigravity-low"),
            Some(100)
        );
        assert_eq!(
            account_quota_score("antigravity", "antigravity-high"),
            Some(900)
        );
        let state = std::fs::read_to_string(health_path().expect("resolve health path"))
            .expect("read quota state");
        assert!(state.contains("gemini-3-flash"));
        assert!(state.contains("reset"));
        assert!(!state.contains("access_token"));
        assert!(!state.contains("refresh_token"));

        match previous_home {
            Some(previous) => crate::env::set_var("JCODE_HOME", previous),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[test]
    fn model_quota_score_does_not_use_another_model_as_a_proxy() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
        let previous_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home.path());

        record_account_quotas(
            "antigravity",
            "antigravity-model-scoped",
            &[
                ("gemini-3-pro".to_string(), Some(900), None),
                ("gemini-3-flash".to_string(), Some(100), None),
            ],
        );

        assert_eq!(
            account_quota_score_for_model(
                "antigravity",
                "antigravity-model-scoped",
                "gemini-3-pro",
            ),
            Some(900)
        );
        assert_eq!(
            account_quota_score_for_model(
                "antigravity",
                "antigravity-model-scoped",
                "gemini-3-flash",
            ),
            Some(100)
        );
        assert_eq!(
            account_quota_score_for_model(
                "antigravity",
                "antigravity-model-scoped",
                "unknown-model",
            ),
            None
        );

        match previous_home {
            Some(previous) => crate::env::set_var("JCODE_HOME", previous),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[test]
    fn quota_history_is_bounded_and_legacy_snapshots_still_deserialize() {
        let legacy: AccountQuotaSnapshot = serde_json::from_str(
            r#"{"remaining_fraction_milli":321,"reset_time":"reset","observed_at_unix_secs":1}"#,
        )
        .expect("legacy quota snapshot should deserialize");
        assert!(legacy.history.is_empty());

        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
        let previous_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home.path());
        for remaining in 0..(MAX_QUOTA_HISTORY as u16 + 3) {
            record_account_quota(
                "antigravity",
                "history-test",
                "gemini-3-flash",
                Some(remaining),
                None,
            );
        }
        let health = read_health();
        let snapshot = &health.quotas["antigravity"]["history-test"]["gemini-3-flash"];
        assert_eq!(snapshot.history.len(), MAX_QUOTA_HISTORY - 1);
        assert_eq!(
            account_quota_score_for_model("antigravity", "history-test", "gemini-3-flash"),
            Some(MAX_QUOTA_HISTORY as u16 + 2)
        );

        match previous_home {
            Some(previous) => crate::env::set_var("JCODE_HOME", previous),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[test]
    fn concurrent_health_updates_preserve_each_account_snapshot() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
        let previous_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home.path());

        let workers = (0..12)
            .map(|index| {
                std::thread::spawn(move || {
                    let label = format!("antigravity-concurrent-{index}");
                    record_account_quota(
                        "antigravity",
                        &label,
                        "gemini-3-flash",
                        Some((index as u16 + 1) * 50),
                        None,
                    );
                    label
                })
            })
            .collect::<Vec<_>>();

        for worker in workers {
            let label = worker.join().expect("health update worker should finish");
            assert!(
                account_quota_score_for_model("antigravity", &label, "gemini-3-flash").is_some(),
                "concurrent update for {label} was lost"
            );
        }

        match previous_home {
            Some(previous) => crate::env::set_var("JCODE_HOME", previous),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn account_request_scopes_serialize_same_provider_and_isolate_other_providers() {
        let first = acquire_account_request_lease("openai")
            .await
            .expect("OpenAI should use an account request scope");
        assert!(
            try_acquire_account_request_lease("openai").is_none(),
            "a synchronous account switch must not mutate an active request"
        );

        let second_started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let second_started_task = std::sync::Arc::clone(&second_started);
        let second = tokio::spawn(async move {
            let lease = acquire_account_request_lease("openai")
                .await
                .expect("OpenAI should use an account request scope");
            second_started_task.store(true, std::sync::atomic::Ordering::Release);
            lease
        });

        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        assert!(
            !second_started.load(std::sync::atomic::Ordering::Acquire),
            "same-provider requests must wait for the prior request scope"
        );

        let other_provider = acquire_account_request_lease("claude").await;
        assert!(
            other_provider.is_some(),
            "different account providers must retain independent request scopes"
        );
        drop(other_provider);
        drop(first);

        let second = second.await.expect("waiting request should finish");
        assert!(second_started.load(std::sync::atomic::Ordering::Acquire));
        drop(second);
        assert!(
            try_acquire_account_request_lease("openai").is_some(),
            "the request scope must be reusable after the stream owner drops it"
        );
    }

    #[test]
    fn concurrent_account_upserts_preserve_each_account() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
        let previous_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home.path());

        let workers = (0..12)
            .map(|index| {
                std::thread::spawn(move || {
                    upsert_account(
                        "cursor",
                        ManagedProviderAccount {
                            id: format!("cursor-id-{index}"),
                            label: format!("cursor-imported-{index}"),
                            access_token: format!("access-{index}"),
                            refresh_token: format!("refresh-{index}"),
                            expires_at: 1,
                            email: None,
                            project_id: None,
                        },
                    )
                    .expect("account upsert should succeed");
                })
            })
            .collect::<Vec<_>>();

        for worker in workers {
            worker.join().expect("account upsert worker should finish");
        }

        let accounts = list_accounts("cursor").expect("list accounts");
        assert_eq!(accounts.len(), 12);
        for index in 0..12 {
            assert!(
                accounts
                    .iter()
                    .any(|account| account.id == format!("cursor-id-{index}")),
                "concurrent account {index} was lost"
            );
        }

        match previous_home {
            Some(previous) => crate::env::set_var("JCODE_HOME", previous),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }

    #[test]
    fn account_lease_excludes_concurrent_acquisition_and_releases_on_drop() {
        let label = format!("lease-test-{}", std::process::id());
        let first = try_acquire_account_lease("cursor", &label, Duration::from_secs(60))
            .expect("first lease should be available");
        assert!(try_acquire_account_lease("cursor", &label, Duration::from_secs(60)).is_none());

        drop(first);
        assert!(try_acquire_account_lease("cursor", &label, Duration::from_secs(60)).is_some());
    }
}
