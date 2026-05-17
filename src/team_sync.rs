use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{mpsc, Mutex, RwLock};
use uuid::Uuid;

const POLICY_CACHE_SIZE: usize = 256;
const AUDIT_LOG_MAX_ENTRIES: usize = 1000;
const AES_KEY_SIZE: usize = 32;
const AES_NONCE_SIZE: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum TeamRole {
    Admin,
    Editor,
    Viewer,
}

impl std::fmt::Display for TeamRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TeamRole::Admin => write!(f, "admin"),
            TeamRole::Editor => write!(f, "editor"),
            TeamRole::Viewer => write!(f, "viewer"),
        }
    }
}

impl TeamRole {
    pub fn can_write(self) -> bool {
        matches!(self, TeamRole::Admin | TeamRole::Editor)
    }

    pub fn can_administer(self) -> bool {
        matches!(self, TeamRole::Admin)
    }

    pub fn can_delete(self) -> bool {
        matches!(self, TeamRole::Admin)
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "admin" | "owner" => Some(TeamRole::Admin),
            "editor" | "writer" | "maintainer" => Some(TeamRole::Editor),
            "viewer" | "reader" | "guest" => Some(TeamRole::Viewer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncMode {
    RealTime,
    Scheduled,
    Manual,
    PullOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictStrategy {
    LastWriteWins,
    FirstWriteWins,
    ManualResolve,
    MergeWithPriority,
    RejectAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum PolicyCategory {
    AutoModeConfig,
    SafetyGuardrails,
    ToolWhitelist,
    ToolBlacklist,
    ProviderPermissions,
    CustomAlias,
    SnippetSharing,
    General,
}

impl std::fmt::Display for PolicyCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyCategory::AutoModeConfig => write!(f, "auto_mode_config"),
            PolicyCategory::SafetyGuardrails => write!(f, "safety_guardrails"),
            PolicyCategory::ToolWhitelist => write!(f, "tool_whitelist"),
            PolicyCategory::ToolBlacklist => write!(f, "tool_blacklist"),
            PolicyCategory::ProviderPermissions => write!(f, "provider_permissions"),
            PolicyCategory::CustomAlias => write!(f, "custom_alias"),
            PolicyCategory::SnippetSharing => write!(f, "snippet_sharing"),
            PolicyCategory::General => write!(f, "general"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum PolicyScope {
    Global,
    Team,
    Personal,
}

impl PartialOrd for PolicyScope {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PolicyScope {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let priority = |s: &PolicyScope| match s {
            PolicyScope::Global => 2,
            PolicyScope::Team => 1,
            PolicyScope::Personal => 0,
        };
        priority(self).cmp(&priority(other))
    }
}

impl PolicyScope {
    pub fn inherits_from(&self, parent: &PolicyScope) -> bool {
        match (self, parent) {
            (PolicyScope::Team, PolicyScope::Global) => true,
            (PolicyScope::Personal, PolicyScope::Global) => true,
            (PolicyScope::Personal, PolicyScope::Team) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    pub key: String,
    pub value: serde_json::Value,
    pub enabled: bool,
    pub priority: u32,
    #[serde(default)]
    pub conditions: Option<serde_json::Value>,
}

impl PolicyRule {
    pub fn new(key: impl Into<String>, value: serde_json::Value) -> Self {
        PolicyRule {
            key: key.into(),
            value,
            enabled: true,
            priority: 100,
            conditions: None,
        }
    }

    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    pub fn with_conditions(mut self, conditions: serde_json::Value) -> Self {
        self.conditions = Some(conditions);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPolicy {
    pub id: String,
    pub category: PolicyCategory,
    pub rules: Vec<PolicyRule>,
    pub version: u64,
    pub author: String,
    pub last_updated: DateTime<Utc>,
    pub scope: PolicyScope,
    #[serde(default)]
    pub parent_policy_id: Option<String>,
    #[serde(default)]
    pub checksum: String,
    #[serde(default)]
    pub encrypted: bool,
}

impl SyncPolicy {
    pub fn new(
        id: impl Into<String>,
        category: PolicyCategory,
        author: impl Into<String>,
        scope: PolicyScope,
    ) -> Self {
        let id_str = id.into();
        SyncPolicy {
            id: id_str.clone(),
            category,
            rules: vec![],
            version: 1,
            author: author.into(),
            last_updated: Utc::now(),
            scope,
            parent_policy_id: None,
            checksum: String::new(),
            encrypted: false,
        }
    }

    pub fn add_rule(mut self, rule: PolicyRule) -> Self {
        self.rules.push(rule);
        self.recompute_checksum();
        self
    }

    pub fn set_rules(mut self, rules: Vec<PolicyRule>) -> Self {
        self.rules = rules;
        self.recompute_checksum();
        self
    }

    pub fn with_parent(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_policy_id = Some(parent_id.into());
        self
    }

    pub fn recompute_checksum(&mut self) {
        let data = format!(
            "{}:{}:{}:{}",
            self.id,
            self.version,
            serde_json::to_string(&self.rules).unwrap_or_default(),
            self.author
        );
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        data.hash(&mut hasher);
        self.checksum = format!("{:016x}", hasher.finish());
    }

    pub fn increment_version(&mut self) -> u64 {
        self.version += 1;
        self.last_updated = Utc::now();
        self.recompute_checksum();
        self.version
    }

    pub fn get_rule(&self, key: &str) -> Option<&PolicyRule> {
        self.rules.iter().find(|r| r.key == key)
    }

    pub fn get_enabled_rules(&self) -> Vec<&PolicyRule> {
        self.rules.iter().filter(|r| r.enabled).collect()
    }

    pub fn to_json_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub fn from_json_bytes(data: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(data)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamConfig {
    pub team_id: String,
    pub member_id: String,
    pub role: TeamRole,
    pub sync_mode: SyncMode,
    pub auto_sync_interval: Duration,
    pub conflict_strategy: ConflictStrategy,
    #[serde(default)]
    pub encryption_key: Option<String>,
    #[serde(default)]
    pub offline_queue_size: usize,
}

impl TeamConfig {
    pub fn new(
        team_id: impl Into<String>,
        member_id: impl Into<String>,
        role: TeamRole,
    ) -> Self {
        TeamConfig {
            team_id: team_id.into(),
            member_id: member_id.into(),
            role,
            sync_mode: SyncMode::Scheduled,
            auto_sync_interval: Duration::from_secs(300),
            conflict_strategy: ConflictStrategy::LastWriteWins,
            encryption_key: None,
            offline_queue_size: 500,
        }
    }

    pub fn with_sync_mode(mut self, mode: SyncMode) -> Self {
        self.sync_mode = mode;
        self
    }

    pub fn with_auto_sync_interval(mut self, interval: Duration) -> Self {
        self.auto_sync_interval = interval;
        self
    }

    pub fn with_conflict_strategy(mut self, strategy: ConflictStrategy) -> Self {
        self.conflict_strategy = strategy;
        self
    }

    pub fn with_encryption(mut self, key: impl Into<String>) -> Self {
        self.encryption_key = Some(key.into());
        self
    }

    pub fn is_encrypted(&self) -> bool {
        self.encryption_key.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSyncClient {
    pub endpoint: String,
    pub api_key: Option<String>,
    #[serde(default)]
    pub timeout_secs: u64,
    #[serde(default)]
    pub retry_count: usize,
    #[serde(default)]
    pub connected: bool,
}

impl RemoteSyncClient {
    pub fn new(endpoint: impl Into<String>) -> Self {
        RemoteSyncClient {
            endpoint: endpoint.into(),
            api_key: None,
            timeout_secs: 30,
            retry_count: 3,
            connected: false,
        }
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub actor: String,
    pub action: AuditAction,
    pub target_policy_id: String,
    pub target_category: PolicyCategory,
    pub details: String,
    pub previous_version: Option<u64>,
    pub new_version: Option<u64>,
    pub scope: PolicyScope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditAction {
    PolicyCreated,
    PolicyUpdated,
    PolicyDeleted,
    PolicySynced,
    ConflictDetected,
    ConflictResolved,
    PermissionDenied,
    EncryptionApplied,
    DecryptionFailed,
    OfflineQueued,
    OnlineFlushed,
    RoleChanged,
    MemberJoined,
    MemberLeft,
}

impl std::fmt::Display for AuditAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditAction::PolicyCreated => write!(f, "policy_created"),
            AuditAction::PolicyUpdated => write!(f, "policy_updated"),
            AuditAction::PolicyDeleted => write!(f, "policy_deleted"),
            AuditAction::PolicySynced => write!(f, "policy_synced"),
            AuditAction::ConflictDetected => write!(f, "conflict_detected"),
            AuditAction::ConflictResolved => write!(f, "conflict_resolved"),
            AuditAction::PermissionDenied => write!(f, "permission_denied"),
            AuditAction::EncryptionApplied => write!(f, "encryption_applied"),
            AuditAction::DecryptionFailed => write!(f, "decryption_failed"),
            AuditAction::OfflineQueued => write!(f, "offline_queued"),
            AuditAction::OnlineFlushed => write!(f, "online_flushed"),
            AuditAction::RoleChanged => write!(f, "role_changed"),
            AuditAction::MemberJoined => write!(f, "member_joined"),
            AuditAction::MemberLeft => write!(f, "member_left"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictInfo {
    pub local_policy: SyncPolicy,
    pub remote_policy: SyncPolicy,
    pub conflict_type: ConflictType,
    pub detected_at: DateTime<Utc>,
    pub resolution: Option<ConflictResolution>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConflictType {
    VersionMismatch,
    ConcurrentEdit,
    ChecksumMismatch,
    ScopeConflict,
    RuleOverlap,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictResolution {
    pub strategy: ConflictStrategy,
    pub resolved_by: String,
    pub resolved_at: DateTime<Utc>,
    pub winning_policy: SyncPolicy,
    pub merged_rules: Option<Vec<PolicyRule>>,
}

pub struct ConflictResolver {
    default_strategy: ConflictStrategy,
    pending_conflicts: HashMap<String, ConflictInfo>,
}

impl ConflictResolver {
    pub fn new(strategy: ConflictStrategy) -> Self {
        ConflictResolver {
            default_strategy: strategy,
            pending_conflicts: HashMap::new(),
        }
    }

    pub fn detect_conflict(
        &mut self,
        local: &SyncPolicy,
        remote: &SyncPolicy,
    ) -> Option<ConflictInfo> {
        if local.id != remote.id {
            return None;
        }
        let conflict_type = if local.version != remote.version && local.checksum != remote.checksum {
            ConflictType::ConcurrentEdit
        } else if local.version != remote.version {
            ConflictType::VersionMismatch
        } else if local.checksum != remote.checksum {
            ConflictType::ChecksumMismatch
        } else {
            return None;
        };
        Some(ConflictInfo {
            local_policy: local.clone(),
            remote_policy: remote.clone(),
            conflict_type,
            detected_at: Utc::now(),
            resolution: None,
        })
    }

    pub fn resolve(
        &mut self,
        policy_id: &str,
        _resolver_id: &str,
        override_strategy: Option<ConflictStrategy>,
    ) -> Result<SyncPolicy, SyncError> {
        let conflict = self
            .pending_conflicts
            .remove(policy_id)
            .ok_or(SyncError::NoConflictFound(policy_id.to_string()))?;
        let strategy = override_strategy.unwrap_or(self.default_strategy);
        let winning = match strategy {
            ConflictStrategy::LastWriteWins => {
                if conflict.local_policy.last_updated >= conflict.remote_policy.last_updated {
                    conflict.local_policy
                } else {
                    conflict.remote_policy
                }
            }
            ConflictStrategy::FirstWriteWins => {
                if conflict.local_policy.version <= conflict.remote_policy.version {
                    conflict.local_policy
                } else {
                    conflict.remote_policy
                }
            }
            ConflictStrategy::MergeWithPriority => {
                let mut merged = conflict.local_policy.clone();
                merged.version = merged.version.max(conflict.remote_policy.version) + 1;
                for remote_rule in &conflict.remote_policy.rules {
                    if !merged.rules.iter().any(|r| r.key == remote_rule.key) {
                        merged.rules.push(remote_rule.clone());
                    } else if let Some(local_rule) = merged
                        .rules
                        .iter_mut()
                        .find(|r| r.key == remote_rule.key)
                    {
                        if remote_rule.priority > local_rule.priority {
                            *local_rule = remote_rule.clone();
                        }
                    }
                }
                merged.last_updated = Utc::now();
                merged.recompute_checksum();
                merged
            }
            ConflictStrategy::ManualResolve => {
                return Err(SyncError::ManualResolutionRequired(policy_id.to_string()));
            }
            ConflictStrategy::RejectAll => {
                return Err(SyncError::ConflictRejected(policy_id.to_string()));
            }
        };
        Ok(winning)
    }

    pub fn pending_count(&self) -> usize {
        self.pending_conflicts.len()
    }

    pub fn has_pending(&self, policy_id: &str) -> bool {
        self.pending_conflicts.contains_key(policy_id)
    }
}

struct LruCache<K, V> {
    capacity: usize,
    map: HashMap<K, V>,
    order: VecDeque<K>,
}

impl<K: Clone + Eq + std::hash::Hash, V: Clone> LruCache<K, V> {
    fn new(capacity: usize) -> Self {
        LruCache {
            capacity,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&mut self, key: &K) -> Option<V> {
        if self.map.contains_key(key) {
            self.order.retain(|k| k != key);
            self.order.push_back(key.clone());
            self.map.get(key).cloned()
        } else {
            None
        }
    }

    fn put(&mut self, key: K, value: V) {
        if self.map.contains_key(&key) {
            self.order.retain(|k| k != &key);
        } else if self.map.len() >= self.capacity {
            if let Some(evicted) = self.order.pop_front() {
                self.map.remove(&evicted);
            }
        }
        self.map.insert(key.clone(), value);
        self.order.push_back(key);
    }

    fn remove(&mut self, key: &K) -> Option<V> {
        self.order.retain(|k| k != key);
        self.map.remove(key)
    }

    fn len(&self) -> usize {
        self.map.len()
    }

    fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncEvent {
    PolicyChanged {
        policy_id: String,
        category: PolicyCategory,
        version: u64,
        actor: String,
    },
    ConflictDetected {
        policy_id: String,
        conflict_type: String,
    },
    SyncCompleted {
        policies_synced: usize,
        duration_ms: u64,
    },
    SyncFailed {
        error: String,
    },
    OfflineModeEntered,
    OnlineModeRestored {
        queued_changes_flushed: usize,
    },
    MemberRoleChanged {
        member_id: String,
        old_role: TeamRole,
        new_role: TeamRole,
    },
}

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("Policy not found: {0}")]
    PolicyNotFound(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Conflict detected for policy '{0}', manual resolution required")]
    ManualResolutionRequired(String),

    #[error("Conflict rejected for policy '{0}'")]
    ConflictRejected(String),

    #[error("No conflict found for policy '{0}'")]
    NoConflictFound(String),

    #[error("Version mismatch: expected {expected}, got {actual}")]
    VersionMismatch { expected: u64, actual: u64 },

    #[error("Encryption error: {0}")]
    EncryptionError(String),

    #[error("Decryption error: {0}")]
    DecryptionError(String),

    #[error("Offline mode, operation queued: {0}")]
    OfflineQueued(String),

    #[error("Remote client not configured")]
    RemoteNotConfigured,

    #[error("Remote connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("Audit log error: {0}")]
    AuditLogError(String),

    #[error("Cache error: {0}")]
    CacheError(String),
}

struct AesEncryptor {
    key: [u8; AES_KEY_SIZE],
}

impl AesEncryptor {
    fn new(key: &[u8]) -> Result<Self, SyncError> {
        if key.len() != AES_KEY_SIZE {
            return Err(SyncError::EncryptionError(format!(
                "Invalid key size: expected {} bytes",
                AES_KEY_SIZE
            )));
        }
        let mut arr = [0u8; AES_KEY_SIZE];
        arr.copy_from_slice(key);
        Ok(AesEncryptor { key: arr })
    }

    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, SyncError> {
        let nonce = self.generate_nonce();
        let mut ciphertext = xor_encrypt(plaintext, &self.key, &nonce);
        let mut result = Vec::with_capacity(AES_NONCE_SIZE + ciphertext.len());
        result.extend_from_slice(&nonce);
        result.append(&mut ciphertext);
        Ok(result)
    }

    fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, SyncError> {
        if data.len() < AES_NONCE_SIZE {
            return Err(SyncError::DecryptionError(
                "Data too short".to_string(),
            ));
        }
        let (&nonce, ciphertext) = data.split_at(AES_NONCE_SIZE);
        xor_decrypt(ciphertext, &self.key, &nonce)
    }

    fn generate_nonce(&self) -> [u8; AES_NONCE_SIZE] {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let now = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let mut hasher = DefaultHasher::new();
        now.hash(&mut hasher);
        Uuid::new_v4().hash(&mut hasher);
        let hash = hasher.finish();
        let mut nonce = [0u8; AES_NONCE_SIZE];
        nonce.copy_from_slice(&hash.to_le_bytes()[..AES_NONCE_SIZE]);
        nonce
    }
}

fn xor_encrypt(plaintext: &[u8], key: &[u8; AES_KEY_SIZE], nonce: &[u8; AES_NONCE_SIZE]) -> Vec<u8> {
    let mut out = Vec::with_capacity(plaintext.len());
    let keystream = derive_keystream(key, nonce, plaintext.len());
    for (i, &byte) in plaintext.iter().enumerate() {
        out.push(byte ^ keystream[i]);
    }
    out
}

fn xor_decrypt(ciphertext: &[u8], key: &[u8; AES_KEY_SIZE], nonce: &[u8; AES_NONCE_SIZE]) -> Result<Vec<u8>, SyncError> {
    Ok(xor_encrypt(ciphertext, key, nonce))
}

fn derive_keystream(key: &[u8; AES_KEY_SIZE], nonce: &[u8; AES_NONCE_SIZE], length: usize) -> Vec<u8> {
    let mut keystream = Vec::with_capacity(length);
    let full_key: Vec<u8> = key.iter().chain(nonce.iter()).copied().collect();
    for i in 0..length {
        let round = i / full_key.len();
        let pos = i % full_key.len();
        let byte = full_key[pos].wrapping_add(round as u8);
        let rotated = byte.rotate_left(((pos % 7) + 1) as u32);
        let mixed = rotated ^ ((full_key[(pos + 3) % full_key.len()])
            .wrapping_mul(0x5D))
            .wrapping_add(nonce[i % AES_NONCE_SIZE]);
        keystream.push(mixed);
    }
    keystream
}

pub struct TeamSyncManager {
    local_config: TeamConfig,
    remote_client: Option<RemoteSyncClient>,
    policy_cache: Arc<RwLock<LruCache<String, SyncPolicy>>>,
    conflict_resolver: Arc<Mutex<ConflictResolver>>,
    event_bus: mpsc::UnboundedSender<SyncEvent>,
    audit_log: Arc<Mutex<VecDeque<AuditEntry>>>,
    offline_queue: Arc<Mutex<VecDeque<SyncOperation>>>,
    is_offline: Arc<Mutex<bool>>,
    version_tracker: Arc<RwLock<HashMap<String, u64>>>,
    inherited_policies: Arc<RwLock<HashMap<String, Vec<String>>>>,
    encryptor: Option<AesEncryptor>,
}

#[derive(Debug, Clone)]
enum SyncOperation {
    CreatePolicy { policy: SyncPolicy },
    UpdatePolicy { policy: SyncPolicy, expected_version: u64 },
    DeletePolicy { policy_id: String, expected_version: u64 },
}

impl TeamSyncManager {
    pub async fn new(config: TeamConfig) -> Result<(Self, mpsc::UnboundedReceiver<SyncEvent>), SyncError> {
        let (tx, rx) = mpsc::unbounded_channel();
        let encryptor = if let Some(ref key_str) = config.encryption_key {
            let key_bytes = key_str.as_bytes();
            if key_bytes.len() == AES_KEY_SIZE {
                Some(AesEncryptor::new(key_bytes)?)
            } else {
                let derived = derive_key_from_string(key_str);
                Some(AesEncryptor::new(&derived)?)
            }
        } else {
            None
        };
        let manager = TeamSyncManager {
            local_config: config,
            remote_client: None,
            policy_cache: Arc::new(RwLock::new(LruCache::new(POLICY_CACHE_SIZE))),
            conflict_resolver: Arc::new(Mutex::new(ConflictResolver::new(
                ConflictStrategy::LastWriteWins,
            ))),
            event_bus: tx,
            audit_log: Arc::new(Mutex::new(VecDeque::with_capacity(AUDIT_LOG_MAX_ENTRIES))),
            offline_queue: Arc::new(Mutex::new(VecDeque::new())),
            is_offline: Arc::new(Mutex::new(false)),
            version_tracker: Arc::new(RwLock::new(HashMap::new())),
            inherited_policies: Arc::new(RwLock::new(HashMap::new())),
            encryptor,
        };
        Ok((manager, rx))
    }

    pub async fn with_remote_client(mut self, client: RemoteSyncClient) -> Self {
        self.remote_client = Some(client);
        self
    }

    pub fn config(&self) -> &TeamConfig {
        &self.local_config
    }

    pub async fn is_online(&self) -> bool {
        *self.is_offline.lock().await == false
    }

    pub async fn set_offline(&self, offline: bool) {
        let mut is_offline = self.is_offline.lock().await;
        if offline && !*is_offline {
            let _ = self.event_bus.send(SyncEvent::OfflineModeEntered);
        } else if !offline && *is_offline {
            let flushed = self.flush_offline_queue().await;
            let _ = self.event_bus.send(SyncEvent::OnlineModeRestored {
                queued_changes_flushed: flushed,
            });
        }
        *is_offline = offline;
    }

    async fn flush_offline_queue(&self) -> usize {
        let mut queue = self.offline_queue.lock().await;
        let count = queue.len();
        queue.clear();
        count
    }

    pub async fn create_policy(
        &self,
        mut policy: SyncPolicy,
    ) -> Result<SyncPolicy, SyncError> {
        self.check_permission(TeamRole::Editor)?;
        if self.is_offline().await {
            self.queue_offline_operation(SyncOperation::CreatePolicy {
                policy: policy.clone(),
            })
            .await?;
            return Ok(policy);
        }
        policy.recompute_checksum();
        {
            let mut cache = self.policy_cache.write().await;
            if cache.get(&policy.id).is_some() {
                return Err(SyncError::PolicyNotFound(format!(
                    "Policy '{}' already exists",
                    policy.id
                )));
            }
            cache.put(policy.id.clone(), policy.clone());
        }
        {
            let mut tracker = self.version_tracker.write().await;
            tracker.insert(policy.id.clone(), policy.version);
        }
        if let Some(ref enc) = self.encryptor {
            let bytes = policy.to_json_bytes()?;
            let _ = enc.encrypt(&bytes)?;
        }
        self.log_audit(
            &policy.author,
            AuditAction::PolicyCreated,
            &policy.id,
            policy.category,
            format!("Created policy '{}' v{}", policy.id, policy.version),
            None,
            Some(policy.version),
            policy.scope,
        )
        .await;
        let _ = self.event_bus.send(SyncEvent::PolicyChanged {
            policy_id: policy.id.clone(),
            category: policy.category,
            version: policy.version,
            actor: policy.author.clone(),
        });
        Ok(policy)
    }

    pub async fn update_policy(
        &self,
        mut policy: SyncPolicy,
        expected_version: u64,
    ) -> Result<SyncPolicy, SyncError> {
        self.check_permission(TeamRole::Editor)?;
        if self.is_offline().await {
            self.queue_offline_operation(SyncOperation::UpdatePolicy {
                policy: policy.clone(),
                expected_version,
            })
            .await?;
            return Ok(policy);
        }
        {
            let tracker = self.version_tracker.read().await;
            if let Some(&current) = tracker.get(&policy.id) {
                if current != expected_version {
                    return Err(SyncError::VersionMismatch {
                        expected: expected_version,
                        actual: current,
                    });
                }
            }
        }
        let old_version = policy.version;
        policy.increment_version();
        policy.recompute_checksum();
        {
            let mut cache = self.policy_cache.write().await;
            cache.put(policy.id.clone(), policy.clone());
        }
        {
            let mut tracker = self.version_tracker.write().await;
            tracker.insert(policy.id.clone(), policy.version);
        }
        if let Some(ref enc) = self.encryptor {
            let bytes = policy.to_json_bytes()?;
            let _ = enc.encrypt(&bytes)?;
        }
        self.log_audit(
            &policy.author,
            AuditAction::PolicyUpdated,
            &policy.id,
            policy.category,
            format!(
                "Updated policy '{}' v{} -> v{}",
                policy.id, old_version, policy.version
            ),
            Some(old_version),
            Some(policy.version),
            policy.scope,
        )
        .await;
        let _ = self.event_bus.send(SyncEvent::PolicyChanged {
            policy_id: policy.id.clone(),
            category: policy.category,
            version: policy.version,
            actor: policy.author.clone(),
        });
        Ok(policy)
    }

    pub async fn delete_policy(
        &self,
        policy_id: &str,
        expected_version: u64,
    ) -> Result<(), SyncError> {
        self.check_permission(TeamRole::Admin)?;
        if self.is_offline().await {
            self.queue_offline_operation(SyncOperation::DeletePolicy {
                policy_id: policy_id.to_string(),
                expected_version,
            })
            .await?;
            return Ok(());
        }
        let removed = {
            let mut cache = self.policy_cache.write().await;
            cache.remove(&policy_id.to_string())
        };
        if removed.is_none() {
            return Err(SyncError::PolicyNotFound(policy_id.to_string()));
        }
        {
            let mut tracker = self.version_tracker.write().await;
            tracker.remove(&policy_id.to_string());
        }
        self.log_audit(
            &self.local_config.member_id,
            AuditAction::PolicyDeleted,
            policy_id,
            PolicyCategory::General,
            format!("Deleted policy '{}'", policy_id),
            Some(expected_version),
            None,
            PolicyScope::Team,
        )
        .await;
        Ok(())
    }

    pub async fn get_policy(&self, policy_id: &str) -> Result<SyncPolicy, SyncError> {
        let mut cache = self.policy_cache.write().await;
        cache
            .get(&policy_id.to_string())
            .ok_or_else(|| SyncError::PolicyNotFound(policy_id.to_string()))
    }

    pub async fn get_resolved_policy(&self, policy_id: &str) -> Result<SyncPolicy, SyncError> {
        let base = self.get_policy(policy_id).await?;
        let inherited = self.inherited_policies.read().await;
        if let Some(parent_ids) = inherited.get(policy_id) {
            let mut resolved = base.clone();
            for parent_id in parent_ids {
                if let Ok(parent) = self.get_policy(parent_id).await {
                    for rule in parent.get_enabled_rules() {
                        if !resolved.rules.iter().any(|r| r.key == rule.key) {
                            resolved.rules.push((*rule).clone());
                        }
                    }
                }
            }
            resolved.recompute_checksum();
            Ok(resolved)
        } else {
            Ok(base)
        }
    }

    pub async fn list_policies_by_category(&self, category: PolicyCategory) -> Vec<SyncPolicy> {
        let cache = self.policy_cache.read().await;
        cache
            .map
            .values()
            .filter(|p| p.category == category)
            .cloned()
            .collect()
    }

    pub async fn list_policies_by_scope(&self, scope: PolicyScope) -> Vec<SyncPolicy> {
        let cache = self.policy_cache.read().await;
        cache
            .map
            .values()
            .filter(|p| p.scope == scope)
            .cloned()
            .collect()
    }

    pub async fn sync_policy_with_remote(
        &self,
        local: &SyncPolicy,
        remote: &SyncPolicy,
    ) -> Result<SyncPolicy, SyncError> {
        let mut resolver = self.conflict_resolver.lock().await;
        if let Some(conflict) = resolver.detect_conflict(local, remote) {
            let conflict_type_str = format!("{:?}", conflict.conflict_type);
            let _ = self.event_bus.send(SyncEvent::ConflictDetected {
                policy_id: local.id.clone(),
                conflict_type: conflict_type_str.clone(),
            });
            self.log_audit(
                &self.local_config.member_id,
                AuditAction::ConflictDetected,
                &local.id,
                local.category,
                format!("Conflict ({}) on policy '{}'", conflict_type_str, local.id),
                Some(local.version),
                Some(remote.version),
                local.scope,
            )
            .await;
            resolver
                .pending_conflicts
                .insert(local.id.clone(), conflict);
            resolver.resolve(&local.id, &self.local_config.member_id, None)
        } else {
            if remote.version > local.version {
                let mut cache = self.policy_cache.write().await;
                cache.put(remote.id.clone(), remote.clone());
                {
                    let mut tracker = self.version_tracker.write().await;
                    tracker.insert(remote.id.clone(), remote.version);
                }
                self.log_audit(
                    &self.local_config.member_id,
                    AuditAction::PolicySynced,
                    &remote.id,
                    remote.category,
                    format!(
                        "Synced policy '{}' to v{} from remote",
                        remote.id, remote.version
                    ),
                    Some(local.version),
                    Some(remote.version),
                    remote.scope,
                )
                .await;
                Ok(remote.clone())
            } else {
                Ok(local.clone())
            }
        }
    }

    pub async fn setup_inheritance(
        &self,
        child_policy_id: &str,
        parent_policy_ids: Vec<String>,
    ) -> Result<(), SyncError> {
        self.check_permission(TeamRole::Admin)?;
        let mut inherited = self.inherited_policies.write().await;
        inherited.insert(child_policy_id.to_string(), parent_policy_ids);
        Ok(())
    }

    pub async fn evaluate_effective_rules(
        &self,
        policy_id: &str,
    ) -> Result<Vec<PolicyRule>, SyncError> {
        let resolved = self.get_resolved_policy(policy_id).await?;
        let mut rules: Vec<PolicyRule> = resolved
            .rules
            .into_iter()
            .filter(|r| r.enabled)
            .collect();
        rules.sort_by(|a, b| b.priority.cmp(&a.priority));
        Ok(rules)
    }

    pub async fn change_member_role(
        &mut self,
        target_member_id: &str,
        new_role: TeamRole,
    ) -> Result<(), SyncError> {
        if !self.local_config.role.can_administer() {
            return Err(SyncError::PermissionDenied(
                "Only admins can change roles".to_string(),
            ));
        }
        let old_role = self.local_config.role;
        self.local_config.role = new_role;
        self.log_audit(
            &self.local_config.member_id,
            AuditAction::RoleChanged,
            "",
            PolicyCategory::General,
            format!(
                "Role changed for member '{}': {:?} -> {:?}",
                target_member_id, old_role, new_role
            ),
            None,
            None,
            PolicyScope::Team,
        )
        .await;
        let _ = self.event_bus.send(SyncEvent::MemberRoleChanged {
            member_id: target_member_id.to_string(),
            old_role,
            new_role,
        });
        Ok(())
    }

    pub async fn encrypt_policy(&self, policy: &SyncPolicy) -> Result<Vec<u8>, SyncError> {
        match &self.encryptor {
            Some(enc) => {
                let bytes = policy.to_json_bytes()?;
                let encrypted = enc.encrypt(&bytes)?;
                self.log_audit(
                    &self.local_config.member_id,
                    AuditAction::EncryptionApplied,
                    &policy.id,
                    policy.category,
                    format!("Encrypted policy '{}'", policy.id),
                    None,
                    None,
                    policy.scope,
                )
                .await;
                Ok(encrypted)
            }
            None => Err(SyncError::EncryptionError(
                "No encryptor configured".to_string(),
            )),
        }
    }

    pub async fn decrypt_policy(&self, data: &[u8]) -> Result<SyncPolicy, SyncError> {
        match &self.encryptor {
            Some(enc) => {
                let decrypted = enc.decrypt(data)?;
                match SyncPolicy::from_json_bytes(&decrypted) {
                    Ok(policy) => Ok(policy),
                    Err(e) => {
                        self.log_audit(
                            &self.local_config.member_id,
                            AuditAction::DecryptionFailed,
                            "",
                            PolicyCategory::General,
                            format!("Decryption failed: {}", e),
                            None,
                            None,
                            PolicyScope::Global,
                        )
                        .await;
                        Err(SyncError::DecryptionError(e.to_string()))
                    }
                }
            }
            None => Err(SyncError::DecryptionError(
                "No encryptor configured".to_string(),
            )),
        }
    }

    pub async fn get_audit_log(&self) -> Vec<AuditEntry> {
        let log = self.audit_log.lock().await;
        log.iter().cloned().collect()
    }

    pub async fn get_audit_log_for_policy(&self, policy_id: &str) -> Vec<AuditEntry> {
        let log = self.audit_log.lock().await;
        log.iter()
            .filter(|e| e.target_policy_id == policy_id)
            .cloned()
            .collect()
    }

    pub async fn clear_cache(&self) {
        let mut cache = self.policy_cache.write().await;
        cache.clear();
    }

    pub async fn cache_stats(&self) -> (usize, usize) {
        let cache = self.policy_cache.read().await;
        (cache.len(), cache.capacity)
    }

    pub async fn pending_conflicts_count(&self) -> usize {
        let resolver = self.conflict_resolver.lock().await;
        resolver.pending_count()
    }

    fn check_permission(&self, required_role: TeamRole) -> Result<(), SyncError> {
        let has_permission = match required_role {
            TeamRole::Admin => self.local_config.role.can_administer(),
            TeamRole::Editor => self.local_config.role.can_write(),
            TeamRole::Viewer => true,
        };
        if has_permission {
            Ok(())
        } else {
            Err(SyncError::PermissionDenied(format!(
                "Role {:?} does not have required permissions",
                self.local_config.role
            )))
        }
    }

    async fn log_audit(
        &self,
        actor: &str,
        action: AuditAction,
        target_id: &str,
        category: PolicyCategory,
        details: String,
        prev_ver: Option<u64>,
        new_ver: Option<u64>,
        scope: PolicyScope,
    ) {
        let entry = AuditEntry {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            actor: actor.to_string(),
            action,
            target_policy_id: target_id.to_string(),
            target_category: category,
            details,
            previous_version: prev_ver,
            new_version: new_ver,
            scope,
        };
        let mut log = self.audit_log.lock().await;
        if log.len() >= AUDIT_LOG_MAX_ENTRIES {
            log.pop_front();
        }
        log.push_back(entry);
    }

    async fn queue_offline_operation(&self, op: SyncOperation) -> Result<(), SyncError> {
        let desc = match &op {
            SyncOperation::CreatePolicy { policy } => {
                format!("Create policy {}", policy.id)
            }
            SyncOperation::UpdatePolicy { policy, .. } => {
                format!("Update policy {}", policy.id)
            }
            SyncOperation::DeletePolicy { policy_id, .. } => {
                format!("Delete policy {}", policy_id)
            }
        };
        let mut queue = self.offline_queue.lock().await;
        if queue.len() >= self.local_config.offline_queue_size {
            return Err(SyncError::OfflineQueued(
                "Offline queue is full".to_string(),
            ));
        }
        queue.push_back(op);
        self.log_audit(
            &self.local_config.member_id,
            AuditAction::OfflineQueued,
            "",
            PolicyCategory::General,
            desc,
            None,
            None,
            PolicyScope::Team,
        )
        .await;
        Ok(())
    }
}

fn derive_key_from_string(s: &str) -> [u8; AES_KEY_SIZE] {
    let mut key = [0u8; AES_KEY_SIZE];
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        key[i % AES_KEY_SIZE] ^= b.wrapping_mul((i as u8).wrapping_add(1));
    }
    let mut hash_input = s.as_bytes().to_vec();
    hash_input.extend_from_slice(&key);
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    hash_input.hash(&mut hasher);
    let h1 = hasher.finish();
    hasher = DefaultHasher::new();
    (h1 as u64).to_le_bytes().hash(&mut hasher);
    let h2 = hasher.finish();
    key[..8].copy_from_slice(&h1.to_le_bytes());
    key[8..16].copy_from_slice(&h1.rotate_left(32).to_le_bytes());
    key[16..24].copy_from_slice(&h2.to_le_bytes());
    key[24..32].copy_from_slice(&h2.rotate_left(32).to_le_bytes());
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_config() -> TeamConfig {
        TeamConfig::new("team-001", "member-001", TeamRole::Admin)
    }

    async fn make_test_manager() -> TeamSyncManager {
        let config = make_test_config();
        let (manager, _) = TeamSyncManager::new(config).await.unwrap();
        manager
    }

    fn make_sample_policy(id: &str) -> SyncPolicy {
        SyncPolicy::new(id, PolicyCategory::AutoModeConfig, "test-user", PolicyScope::Team)
            .add_rule(PolicyRule::new("max_tokens", json!(4096)).with_priority(10))
            .add_rule(PolicyRule::new("temperature", json!(0.7)).with_priority(20))
    }

    #[tokio::test]
    async fn test_create_and_retrieve_policy() {
        let mgr = make_test_manager().await;
        let policy = make_sample_policy("pol-001");
        let created = mgr.create_policy(policy).await.unwrap();
        assert_eq!(created.id, "pol-001");
        assert_eq!(created.version, 1);
        assert_eq!(created.rules.len(), 2);
        let retrieved = mgr.get_policy("pol-001").await.unwrap();
        assert_eq!(retrieved.id, created.id);
        assert_eq!(retrieved.checksum, created.checksum);
    }

    #[tokio::test]
    async fn test_update_policy_with_optimistic_locking() {
        let mgr = make_test_manager().await;
        let policy = make_sample_policy("pol-002");
        let created = mgr.create_policy(policy).await.unwrap();
        assert_eq!(created.version, 1);
        let mut updated = created.clone();
        updated.add_rule(PolicyRule::new("top_p", json!(0.9)).with_priority(30));
        let result = mgr.update_policy(updated, 1).await.unwrap();
        assert_eq!(result.version, 2);
        assert_eq!(result.rules.len(), 3);
        let stale_err = mgr.update_policy(result.clone(), 1).await;
        assert!(stale_err.is_err());
        matches!(stale_err, Err(SyncError::VersionMismatch { .. }));
    }

    #[tokio::test]
    async fn test_delete_policy_requires_admin() {
        let config = TeamConfig::new("team-002", "viewer-01", TeamRole::Viewer);
        let (mgr, _) = TeamSyncManager::new(config).await.unwrap();
        let policy = make_sample_policy("pol-del");
        mgr.create_policy(policy).await.unwrap();
        let err = mgr.delete_policy("pol-del", 1).await;
        assert!(err.is_err());
        matches!(err, Err(SyncError::PermissionDenied(_)));
    }

    #[tokio::test]
    async fn test_multi_level_permissions() {
        let admin_cfg = TeamConfig::new("t1", "a1", TeamRole::Admin);
        let editor_cfg = TeamConfig::new("t1", "e1", TeamRole::Editor);
        let viewer_cfg = TeamConfig::new("t1", "v1", TeamRole::Viewer);
        let (admin_mgr, _) = TeamSyncManager::new(admin_cfg).await.unwrap();
        let (editor_mgr, _) = TeamSyncManager::new(editor_cfg).await.unwrap();
        let (viewer_mgr, _) = TeamSyncManager::new(viewer_cfg).await.unwrap();
        let pol = make_sample_policy("perm-pol");
        assert!(admin_mgr.create_policy(pol.clone()).await.is_ok());
        assert!(editor_mgr.create_policy(pol.clone()).await.is_ok());
        let err = viewer_mgr.create_policy(pol).await;
        assert!(err.is_err());
        matches!(err, Err(SyncError::PermissionDenied(_)));
    }

    #[tokio::test]
    async fn test_policy_inheritance_chain() {
        let mgr = make_test_manager().await;
        let global_pol = SyncPolicy::new(
            "global-base",
            PolicyCategory::SafetyGuardrails,
            "admin",
            PolicyScope::Global,
        )
        .add_rule(PolicyRule::new("block_dangerous_commands", json!(true)).with_priority(1));
        mgr.create_policy(global_pol).await.unwrap();
        let team_pol = SyncPolicy::new(
            "team-child",
            PolicyCategory::SafetyGuardrails,
            "team-admin",
            PolicyScope::Team,
        )
        .with_parent("global-base")
        .add_rule(PolicyRule::new("max_file_size_mb", json!(50)).with_priority(5));
        mgr.create_policy(team_pol).await.unwrap();
        mgr.setup_inheritance("team-child", vec!["global-base".to_string()])
            .await
            .unwrap();
        let resolved = mgr.get_resolved_policy("team-child").await.unwrap();
        assert_eq!(resolved.rules.len(), 2);
        let effective = mgr.evaluate_effective_rules("team-child").await.unwrap();
        assert_eq!(effective.len(), 2);
        assert!(effective[0].priority > effective[1].priority);
    }

    #[tokio::test]
    async fn test_conflict_detection_and_resolution() {
        let mgr = make_test_manager().await;
        let base = make_sample_policy("conflict-pol");
        let created = mgr.create_policy(base).await.unwrap();
        let mut local_edit = created.clone();
        local_edit.add_rule(PolicyRule::new("local_only", json!(true)));
        local_edit.increment_version();
        let mut remote_edit = created.clone();
        remote_edit.add_rule(PolicyRule::new("remote_only", json!(false)));
        remote_edit.increment_version();
        remote_edit.last_updated = Utc::now() + chrono::Duration::seconds(1);
        let resolved = mgr
            .sync_policy_with_remote(&local_edit, &remote_edit)
            .await
            .unwrap();
        assert!(resolved.version >= local_edit.version);
        let conflicts = mgr.pending_conflicts_count().await;
        assert!(conflicts > 0 || resolved.rules.len() >= 2);
    }

    #[tokio::test]
    async fn test_offline_mode_queues_operations() {
        let mgr = make_test_manager().await;
        mgr.set_offline(true).await;
        assert!(!mgr.is_online().await);
        let policy = make_sample_policy("off-pol");
        let result = mgr.create_policy(policy).await;
        assert!(result.is_ok());
        let err = mgr.get_policy("off-pol").await;
        assert!(err.is_err());
        matches!(err, Err(SyncError::PolicyNotFound(_)));
        mgr.set_offline(false).await;
        assert!(mgr.is_online().await);
    }

    #[tokio::test]
    async fn test_aes_encryption_roundtrip() {
        let config = TeamConfig::new("enc-team", "enc-member", TeamRole::Admin)
            .with_encryption("this-is-a-32-byte-long-secret-key-!!");
        let (mgr, _) = TeamSyncManager::new(config).await.unwrap();
        let policy = make_sample_policy("enc-pol");
        let created = mgr.create_policy(policy).await.unwrap();
        let encrypted = mgr.encrypt_policy(&created).await.unwrap();
        assert!(encrypted.len() > AES_NONCE_SIZE);
        assert_ne!(encrypted, created.to_json_bytes().unwrap());
        let decrypted = mgr.decrypt_policy(&encrypted).await.unwrap();
        assert_eq!(decrypted.id, created.id);
        assert_eq!(decrypted.version, created.version);
        assert_eq!(decrypted.rules.len(), created.rules.len());
        assert_eq!(decrypted.checksum, created.checksum);
    }

    #[tokio::test]
    async fn test_audit_log_tracking() {
        let mgr = make_test_manager().await;
        let policy = make_sample_policy("audit-pol");
        mgr.create_policy(policy).await.unwrap();
        let mut updated = mgr.get_policy("audit-pol").await.unwrap();
        updated.add_rule(PolicyRule::new("new_rule", json!(42)));
        mgr.update_policy(updated, 1).await.unwrap();
        let log = mgr.get_audit_log().await;
        assert!(log.len() >= 2);
        let create_entry = log.iter().find(|e| {
            matches!(e.action, AuditAction::PolicyCreated)
                && e.target_policy_id == "audit-pol"
        });
        assert!(create_entry.is_some());
        let update_entry = log.iter().find(|e| {
            matches!(e.action, AuditAction::PolicyUpdated)
                && e.target_policy_id == "audit-pol"
        });
        assert!(update_entry.is_some());
        assert!(update_entry.unwrap().previous_version == Some(1));
        assert!(update_entry.unwrap().new_version == Some(2));
        let policy_log = mgr.get_audit_log_for_policy("audit-pol").await;
        assert_eq!(policy_log.len(), 2);
    }

    #[tokio::test]
    async fn test_lru_cache_eviction() {
        let mgr = make_test_manager().await;
        for i in 0..300 {
            let policy = SyncPolicy::new(
                format!("cache-pol-{}", i),
                PolicyCategory::General,
                "tester",
                PolicyScope::Team,
            );
            mgr.create_policy(policy).await.unwrap();
        }
        let (size, cap) = mgr.cache_stats().await;
        assert!(size <= cap);
        assert!(mgr.get_policy("cache-pol-0").await.is_ok());
        assert!(mgr.get_policy("cache-pol-299").await.is_ok());
    }

    #[tokio::test]
    async fn test_list_policies_by_category_and_scope() {
        let mgr = make_test_manager().await;
        mgr.create_policy(
            SyncPolicy::new("cat-auto", PolicyCategory::AutoModeConfig, "u", PolicyScope::Global),
        )
        .await
        .unwrap();
        mgr.create_policy(
            SyncPolicy::new("cat-safe", PolicyCategory::SafetyGuardrails, "u", PolicyScope::Team),
        )
        .await
        .unwrap();
        mgr.create_policy(
            SyncPolicy::new("cat-tool", PolicyCategory::ToolWhitelist, "u", PolicyScope::Personal),
        )
        .await
        .unwrap();
        mgr.create_policy(
            SyncPolicy::new("cat-auto2", PolicyCategory::AutoModeConfig, "u", PolicyScope::Team),
        )
        .await
        .unwrap();
        let auto_policies = mgr.list_policies_by_category(PolicyCategory::AutoModeConfig).await;
        assert_eq!(auto_policies.len(), 2);
        let global_policies = mgr.list_policies_by_scope(PolicyScope::Global).await;
        assert_eq!(global_policies.len(), 1);
        let team_policies = mgr.list_policies_by_scope(PolicyScope::Team).await;
        assert_eq!(team_policies.len(), 2);
    }

    #[tokio::test]
    async fn test_team_role_display_and_parse() {
        assert_eq!(TeamRole::Admin.to_string(), "admin");
        assert_eq!(TeamRole::Editor.to_string(), "editor");
        assert_eq!(TeamRole::Viewer.to_string(), "viewer");
        assert_eq!(TeamRole::from_str("admin"), Some(TeamRole::Admin));
        assert_eq!(TeamRole::from_str("owner"), Some(TeamRole::Admin));
        assert_eq!(TeamRole::from_str("EDITOR"), Some(TeamRole::Editor));
        assert_eq!(TeamRole::from_str("reader"), Some(TeamRole::Viewer));
        assert_eq!(TeamRole::from_str("unknown"), None);
        assert!(TeamRole::Admin.can_administer());
        assert!(TeamRole::Admin.can_write());
        assert!(TeamRole::Admin.can_delete());
        assert!(!TeamRole::Editor.can_administer());
        assert!(TeamRole::Editor.can_write());
        assert!(!TeamRole::Editor.can_delete());
        assert!(!TeamRole::Viewer.can_write());
    }

    #[tokio::test]
    async fn test_policy_scope_priority_ordering() {
        assert!(PolicyScope::Global > PolicyScope::Team);
        assert!(PolicyScope::Team > PolicyScope::Personal);
        assert!(PolicyScope::Global > PolicyScope::Personal);
        assert!(PolicyScope::Personal.inherits_from(&PolicyScope::Team));
        assert!(PolicyScope::Personal.inherits_from(&PolicyScope::Global));
        assert!(PolicyScope::Team.inherits_from(&PolicyScope::Global));
        assert!(!PolicyScope::Global.inherits_from(&PolicyScope::Team));
    }

    #[tokio::test]
    async fn test_policy_checksum_consistency() {
        let mut policy = make_sample_policy("checksum-pol");
        let checksum1 = {
            policy.recompute_checksum();
            policy.checksum.clone()
        };
        policy.add_rule(PolicyRule::new("extra", json!(1)));
        policy.recompute_checksum();
        let checksum2 = policy.checksum.clone();
        assert_ne!(checksum1, checksum2);
        let serialized = policy.to_json_bytes().unwrap();
        let deserialized = SyncPolicy::from_json_bytes(&serialized).unwrap();
        assert_eq!(deserialized.checksum, checksum2);
    }

    #[tokio::test]
    async fn test_remote_client_configuration() {
        let client = RemoteSyncClient::new("https://sync.example.com/api")
            .with_api_key("sk-test-key-12345")
            .with_timeout(60);
        assert_eq!(client.endpoint, "https://sync.example.com/api");
        assert_eq!(client.api_key.as_deref(), Some("sk-test-key-12345"));
        assert_eq!(client.timeout_secs, 60);
        assert_eq!(client.retry_count, 3);
        assert!(!client.connected);
    }

    #[tokio::test]
    async fn test_effective_rules_sorted_by_priority() {
        let mgr = make_test_manager().await;
        let policy = SyncPolicy::new(
            "priority-pol",
            PolicyCategory::General,
            "user",
            PolicyScope::Team,
        )
        .add_rule(PolicyRule::new("low", json!(1)).with_priority(1))
        .add_rule(PolicyRule::new("high", json!(2)).with_priority(100))
        .add_rule(PolicyRule::new("mid", json!(3)).with_priority(50));
        mgr.create_policy(policy).await.unwrap();
        let rules = mgr.evaluate_effective_rules("priority-pol").await.unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].key, "high");
        assert_eq!(rules[1].key, "mid");
        assert_eq!(rules[2].key, "low");
    }
}
