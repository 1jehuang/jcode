use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DEFAULT_RATE_LIMIT_COOLDOWN_MS: u64 = 60_000;
pub const MAX_RATE_LIMIT_COOLDOWN_MS: u64 = 15 * 60_000;
pub const DEFAULT_BILLING_COOLDOWN_MS: u64 = 24 * 60 * 60_000;
pub const STICK_WAIT_MAX_SECS: u64 = 5;

static RUNTIME_COOLDOWNS: LazyLock<RwLock<HashMap<String, CooldownRecord>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CooldownRecord {
    pub cooldown_until_ms: u64,
    pub reason: String,
    pub retry_after_secs: Option<u64>,
}

impl CooldownRecord {
    pub fn is_expired(&self, now_ms: u64) -> bool {
        self.cooldown_until_ms <= now_ms
    }

    pub fn is_stick_wait(&self, now_ms: u64) -> bool {
        if self.is_expired(now_ms) {
            return false;
        }
        let remaining_secs = (self.cooldown_until_ms.saturating_sub(now_ms) + 999) / 1000;
        remaining_secs <= STICK_WAIT_MAX_SECS
    }
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn cooldown_key(pool_key: &str, account_id: &str) -> String {
    format!("{}:{}", pool_key.trim(), account_id.trim())
}

pub fn set_account_cooldown(
    pool_key: &str,
    account_id: &str,
    reason: &str,
    retry_after: Option<Duration>,
) -> CooldownRecord {
    let now_ms = now_unix_ms();
    let retry_after_secs = retry_after.map(|d| d.as_secs());
    let delay_ms = match retry_after {
        Some(d) => (d.as_millis() as u64).clamp(1_000, MAX_RATE_LIMIT_COOLDOWN_MS),
        None => {
            if reason.eq_ignore_ascii_case("billing") || reason.eq_ignore_ascii_case("quota") {
                DEFAULT_BILLING_COOLDOWN_MS
            } else {
                DEFAULT_RATE_LIMIT_COOLDOWN_MS
            }
        }
    };

    let record = CooldownRecord {
        cooldown_until_ms: now_ms + delay_ms,
        reason: reason.to_string(),
        retry_after_secs,
    };

    let key = cooldown_key(pool_key, account_id);
    if let Ok(mut lock) = RUNTIME_COOLDOWNS.write() {
        lock.insert(key, record.clone());
    }
    record
}

pub fn get_account_cooldown(pool_key: &str, account_id: &str) -> Option<CooldownRecord> {
    let now_ms = now_unix_ms();
    let key = cooldown_key(pool_key, account_id);
    let mut lock = RUNTIME_COOLDOWNS.write().ok()?;
    if let Some(record) = lock.get(&key) {
        if record.is_expired(now_ms) {
            lock.remove(&key);
            None
        } else {
            Some(record.clone())
        }
    } else {
        None
    }
}

pub fn is_account_in_cooldown(pool_key: &str, account_id: &str) -> bool {
    get_account_cooldown(pool_key, account_id).is_some()
}

pub fn is_rate_limit_stick_wait(pool_key: &str, account_id: &str) -> bool {
    let now_ms = now_unix_ms();
    match get_account_cooldown(pool_key, account_id) {
        Some(record) => record.is_stick_wait(now_ms),
        None => false,
    }
}

pub fn clear_account_cooldown(pool_key: &str, account_id: &str) {
    let key = cooldown_key(pool_key, account_id);
    if let Ok(mut lock) = RUNTIME_COOLDOWNS.write() {
        lock.remove(&key);
    }
}

pub fn clear_all_cooldowns() {
    if let Ok(mut lock) = RUNTIME_COOLDOWNS.write() {
        lock.clear();
    }
}

pub fn prune_expired_cooldowns(records: &mut HashMap<String, CooldownRecord>, now_ms: u64) -> usize {
    let before = records.len();
    records.retain(|_, record| !record.is_expired(now_ms));
    before.saturating_sub(records.len())
}

pub fn load_durable_cooldowns(path: &Path) -> Result<HashMap<String, CooldownRecord>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let content = std::fs::read_to_string(path)?;
    let mut records: HashMap<String, CooldownRecord> = serde_json::from_str(&content)?;
    prune_expired_cooldowns(&mut records, now_unix_ms());
    if let Ok(mut lock) = RUNTIME_COOLDOWNS.write() {
        for (k, v) in &records {
            lock.insert(k.clone(), v.clone());
        }
    }
    Ok(records)
}

pub fn save_durable_cooldowns(path: &Path, records: &HashMap<String, CooldownRecord>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let pid = std::process::id();
    let nonce: u64 = rand::random();
    let tmp_path = path.with_extension(format!("tmp.{}.{}", pid, nonce));

    let bytes = serde_json::to_vec(records)?;
    std::fs::write(&tmp_path, bytes)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

pub fn sync_runtime_to_durable(path: &Path) -> Result<()> {
    let records = {
        let mut lock = RUNTIME_COOLDOWNS
            .write()
            .map_err(|e| anyhow::anyhow!("cooldown lock poisoned: {}", e))?;
        prune_expired_cooldowns(&mut lock, now_unix_ms());
        lock.clone()
    };
    save_durable_cooldowns(path, &records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cooldown_setting_and_expiration() {
        clear_all_cooldowns();
        let record = set_account_cooldown(
            "openai",
            "acc_1",
            "rate_limit",
            Some(Duration::from_secs(3)),
        );
        assert_eq!(record.retry_after_secs, Some(3));
        assert!(is_account_in_cooldown("openai", "acc_1"));
        assert!(is_rate_limit_stick_wait("openai", "acc_1"));

        // Long cooldown is not a stick-wait
        set_account_cooldown(
            "openai",
            "acc_2",
            "rate_limit",
            Some(Duration::from_secs(60)),
        );
        assert!(is_account_in_cooldown("openai", "acc_2"));
        assert!(!is_rate_limit_stick_wait("openai", "acc_2"));

        clear_account_cooldown("openai", "acc_1");
        assert!(!is_account_in_cooldown("openai", "acc_1"));
    }

    #[test]
    fn test_prune_expired_cooldowns() {
        let mut records = HashMap::new();
        let now_ms = 100_000;
        records.insert(
            "pool:expired".to_string(),
            CooldownRecord {
                cooldown_until_ms: 90_000,
                reason: "expired".to_string(),
                retry_after_secs: None,
            },
        );
        records.insert(
            "pool:valid".to_string(),
            CooldownRecord {
                cooldown_until_ms: 110_000,
                reason: "active".to_string(),
                retry_after_secs: Some(10),
            },
        );

        let pruned = prune_expired_cooldowns(&mut records, now_ms);
        assert_eq!(pruned, 1);
        assert_eq!(records.len(), 1);
        assert!(records.contains_key("pool:valid"));
    }

    #[test]
    fn test_durable_save_and_load() {
        let temp = std::env::temp_dir().join(format!("test_cooldown_{}.json", rand::random::<u64>()));
        let mut records = HashMap::new();
        let now_ms = now_unix_ms();
        records.insert(
            "anthropic:acc_a".to_string(),
            CooldownRecord {
                cooldown_until_ms: now_ms + 60_000,
                reason: "rate_limit".to_string(),
                retry_after_secs: Some(60),
            },
        );
        records.insert(
            "anthropic:acc_expired".to_string(),
            CooldownRecord {
                cooldown_until_ms: now_ms.saturating_sub(1_000),
                reason: "expired".to_string(),
                retry_after_secs: None,
            },
        );

        save_durable_cooldowns(&temp, &records).expect("save succeeds");
        let loaded = load_durable_cooldowns(&temp).expect("load succeeds");
        let _ = std::fs::remove_file(&temp);

        assert_eq!(loaded.len(), 1);
        assert!(loaded.contains_key("anthropic:acc_a"));
        assert!(!loaded.contains_key("anthropic:acc_expired"));
    }
}
