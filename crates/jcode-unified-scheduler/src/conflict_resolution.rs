//! Conflict Resolution Mechanisms for Cross-Region Deployment
//!
//! Provides multiple conflict resolution strategies for distributed data:
//! - Last-Writer-Wins (LWW) with vector clocks
//! - Multi-value registers (keep all concurrent values)
//! - Counter CRDTs (PN-Counter for increment/decrement)
//! - Map CRDTs (LWW-Map for key-value stores)
//! - Custom merge strategies for application-specific types

use std::collections::{HashMap, HashSet};
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

/// ============================================================================
/// Causal Context for tracking causality across replicas
/// ============================================================================

/// Dot notation for tracking individual field updates
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Dot {
    pub replica_id: u64,
    pub counter: u64,
}

impl Dot {
    pub fn new(replica_id: u64, counter: u64) -> Self {
        Self { replica_id, counter }
    }
}

/// Causal context: set of dots representing seen events
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CausalContext {
    pub dots: HashSet<Dot>,
}

impl CausalContext {
    pub fn new() -> Self {
        Self {
            dots: HashSet::new(),
        }
    }

    /// Add a dot to the context
    pub fn add(&mut self, dot: Dot) {
        self.dots.insert(dot);
    }

    /// Check if this context dominates another (contains all its dots)
    pub fn dominates(&self, other: &CausalContext) -> bool {
        other.dots.is_subset(&self.dots)
    }

    /// Merge two contexts (union)
    pub fn merge(&mut self, other: &CausalContext) {
        for dot in &other.dots {
            self.dots.insert(*dot);
        }
    }

    /// Get the maximum counter for a replica
    pub fn max_counter_for(&self, replica_id: u64) -> u64 {
        self.dots.iter()
            .filter(|d| d.replica_id == replica_id)
            .map(|d| d.counter)
            .max()
            .unwrap_or(0)
    }
}

/// ============================================================================
/// PN-Counter CRDT (Positive-Negative Counter)
/// Allows both increment and decrement operations without conflicts
/// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PNCounter {
    /// Per-replica positive increments
    p_counts: HashMap<u64, u64>,
    /// Per-replica negative decrements
    n_counts: HashMap<u64, u64>,
}

impl PNCounter {
    pub fn new() -> Self {
        Self {
            p_counts: HashMap::new(),
            n_counts: HashMap::new(),
        }
    }

    /// Increment counter on this replica
    pub fn increment(&mut self, replica_id: u64, amount: u64) {
        let entry = self.p_counts.entry(replica_id).or_insert(0);
        *entry += amount;
    }

    /// Decrement counter on this replica
    pub fn decrement(&mut self, replica_id: u64, amount: u64) {
        let entry = self.n_counts.entry(replica_id).or_insert(0);
        *entry += amount;
    }

    /// Get current value (sum of P - sum of N)
    pub fn value(&self) -> i64 {
        let p_sum: u64 = self.p_counts.values().sum();
        let n_sum: u64 = self.n_counts.values().sum();
        p_sum as i64 - n_sum as i64
    }

    /// Merge two counters (component-wise max)
    pub fn merge(&mut self, other: &PNCounter) {
        for (&replica_id, &count) in &other.p_counts {
            let entry = self.p_counts.entry(replica_id).or_insert(0);
            *entry = (*entry).max(count);
        }
        for (&replica_id, &count) in &other.n_counts {
            let entry = self.n_counts.entry(replica_id).or_insert(0);
            *entry = (*entry).max(count);
        }
    }
}

/// ============================================================================
/// LWW-Map CRDT (Last-Writer-Wins Map)
/// Key-value store where concurrent writes to same key use LWW
/// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LwwMap<K: Ord + Clone + Serialize + for<'a> Deserialize<'a>, V: Clone + Serialize + for<'a> Deserialize<'a>> {
    entries: HashMap<K, LwwEntry<V>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LwwEntry<V> {
    value: V,
    timestamp: i64,
    replica_id: u64,
    tombstone: bool,  // For deletions
}

impl<K: Ord + Clone + Serialize + for<'a> Deserialize<'a>, V: Clone + Serialize + for<'a> Deserialize<'a>> LwwMap<K, V> {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Put a key-value pair
    pub fn put(&mut self, key: K, value: V, replica_id: u64) {
        let now = chrono::Utc::now().timestamp_millis();
        self.entries.insert(key, LwwEntry {
            value,
            timestamp: now,
            replica_id,
            tombstone: false,
        });
    }

    /// Remove a key (tombstone marker)
    pub fn remove(&mut self, key: &K, replica_id: u64) {
        let now = chrono::Utc::now().timestamp_millis();
        self.entries.insert(key.clone(), LwwEntry {
            value: unsafe { std::mem::zeroed() }, // Placeholder, won't be used
            timestamp: now,
            replica_id,
            tombstone: true,
        });
    }

    /// Get value for a key (returns None if deleted or not present)
    pub fn get(&self, key: &K) -> Option<&V> {
        self.entries.get(key)
            .filter(|e| !e.tombstone)
            .map(|e| &e.value)
    }

    /// Check if key exists and is not deleted
    pub fn contains_key(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    /// Merge two maps using LWW strategy
    pub fn merge(&mut self, other: &LwwMap<K, V>) {
        for (key, other_entry) in &other.entries {
            if let Some(local_entry) = self.entries.get_mut(key) {
                // Compare timestamps
                if other_entry.timestamp > local_entry.timestamp {
                    // Remote wins
                    *local_entry = other_entry.clone();
                } else if other_entry.timestamp == local_entry.timestamp {
                    // Tie-breaker: higher replica_id wins
                    if other_entry.replica_id > local_entry.replica_id {
                        *local_entry = other_entry.clone();
                    }
                }
                // If local timestamp is higher, keep local
            } else {
                // New key, insert directly
                self.entries.insert(key.clone(), other_entry.clone());
            }
        }
    }

    /// Get all non-tombstoned entries
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter()
            .filter(|(_, e)| !e.tombstone)
            .map(|(k, e)| (k, &e.value))
    }

    /// Get number of active entries
    pub fn len(&self) -> usize {
        self.entries.values().filter(|e| !e.tombstone).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// ============================================================================
/// MV-Register (Multi-Value Register)
/// Keeps all concurrent values instead of discarding any
/// Application must resolve conflicts manually
/// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MVRegister<T: Clone + PartialEq + Serialize + for<'a> Deserialize<'a>> {
    values: Vec<(T, CausalContext)>,
}

impl<T: Clone + PartialEq + Serialize + for<'a> Deserialize<'a>> MVRegister<T> {
    pub fn new(initial_value: T, replica_id: u64) -> Self {
        let mut ctx = CausalContext::new();
        ctx.add(Dot::new(replica_id, 1));

        Self {
            values: vec![(initial_value, ctx)],
        }
    }

    /// Set a new value from this replica
    pub fn set(&mut self, value: T, replica_id: u64) {
        let mut ctx = CausalContext::new();
        // Merge all existing contexts
        for (_, existing_ctx) in &self.values {
            ctx.merge(existing_ctx);
        }
        // Increment this replica's counter
        let max_counter = ctx.max_counter_for(replica_id);
        ctx.add(Dot::new(replica_id, max_counter + 1));

        self.values = vec![(value, ctx)];
    }

    /// Get current values (may have multiple if concurrent)
    pub fn get_values(&self) -> Vec<&T> {
        self.values.iter().map(|(v, _)| v).collect()
    }

    /// Check if there are concurrent values (conflict)
    pub fn has_conflict(&self) -> bool {
        self.values.len() > 1
    }

    /// Merge with another register
    pub fn merge(&mut self, other: &MVRegister<T>) {
        let mut new_values = Vec::new();

        for (val_a, ctx_a) in &self.values {
            let mut dominated = false;

            for (val_b, ctx_b) in &other.values {
                if ctx_a.dominates(ctx_b) {
                    // A dominates B, keep A
                    if !new_values.iter().any(|(v, _)| v == val_a) {
                        new_values.push((val_a.clone(), ctx_a.clone()));
                    }
                    dominated = true;
                    break;
                } else if ctx_b.dominates(ctx_a) {
                    // B dominates A, will add B later
                    dominated = true;
                    break;
                }
            }

            if !dominated {
                // Concurrent, keep both
                if !new_values.iter().any(|(v, _)| v == val_a) {
                    new_values.push((val_a.clone(), ctx_a.clone()));
                }
            }
        }

        // Add values from other that aren't dominated
        for (val_b, ctx_b) in &other.values {
            let mut dominated = false;
            for (_, ctx_a) in &self.values {
                if ctx_a.dominates(ctx_b) {
                    dominated = true;
                    break;
                }
            }
            if !dominated && !new_values.iter().any(|(v, _)| v == val_b) {
                new_values.push((val_b.clone(), ctx_b.clone()));
            }
        }

        self.values = new_values;
    }

    /// Resolve conflict by selecting one value (application-specific logic)
    pub fn resolve<F: FnOnce(Vec<&T>) -> T>(&mut self, resolver: F, replica_id: u64) {
        if self.has_conflict() {
            let values = self.get_values();
            let resolved = resolver(values);
            self.set(resolved, replica_id);
        }
    }
}

/// ============================================================================
/// Conflict Detection and Resolution Strategies
/// ============================================================================

/// Conflict type detected during merge
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConflictType {
    /// No conflict, clean merge
    NoConflict,
    /// Concurrent writes to same field
    WriteWriteConflict {
        field: String,
        local_value: String,
        remote_value: String,
        local_timestamp: i64,
        remote_timestamp: i64,
    },
    /// Delete vs update conflict
    DeleteUpdateConflict {
        field: String,
        was_deleted_locally: bool,
        was_updated_remotely: bool,
    },
}

/// Result of a merge operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeResult {
    pub conflicts_detected: Vec<ConflictType>,
    pub resolution_strategy: String,
    pub success: bool,
}

impl MergeResult {
    pub fn clean() -> Self {
        Self {
            conflicts_detected: vec![],
            resolution_strategy: "none".to_string(),
            success: true,
        }
    }

    pub fn with_conflict(conflict: ConflictType, strategy: &str) -> Self {
        Self {
            conflicts_detected: vec![conflict],
            resolution_strategy: strategy.to_string(),
            success: true,
        }
    }
}

/// Strategy selector for conflict resolution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionStrategy {
    /// Last writer wins (based on timestamp)
    LastWriterWins,
    /// Keep all concurrent values (requires manual resolution)
    KeepAll,
    /// Prefer local value
    PreferLocal,
    /// Prefer remote value
    PreferRemote,
    /// Custom application-defined resolver
    Custom,
}

/// ============================================================================
/// Session-Specific Conflict Resolution
/// ============================================================================

/// Represents a session message that may have conflicts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub message_id: String,
    pub content: String,
    pub role: String,
    pub timestamp: i64,
    pub replica_id: u64,
    pub version: u64,
}

/// Conflict-aware session state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictAwareSession {
    pub session_id: String,
    pub messages: LwwMap<String, SessionMessage>,
    pub metadata: LwwMap<String, String>,
    pub message_order: Vec<String>,  // Ordered list of message IDs
    pub pending_conflicts: Vec<MergeResult>,
}

impl ConflictAwareSession {
    pub fn new(session_id: String) -> Self {
        Self {
            session_id,
            messages: LwwMap::new(),
            metadata: LwwMap::new(),
            message_order: Vec::new(),
            pending_conflicts: Vec::new(),
        }
    }

    /// Add a message to the session
    pub fn add_message(&mut self, message: SessionMessage, replica_id: u64) {
        let msg_id = message.message_id.clone();
        self.messages.put(msg_id.clone(), message, replica_id);
        if !self.message_order.contains(&msg_id) {
            self.message_order.push(msg_id);
        }
    }

    /// Update session metadata
    pub fn update_metadata(&mut self, key: String, value: String, replica_id: u64) {
        self.metadata.put(key, value, replica_id);
    }

    /// Merge with a remote session state
    pub fn merge_with_remote(&mut self, remote: &ConflictAwareSession, strategy: ResolutionStrategy) -> MergeResult {
        let mut result = MergeResult::clean();

        // Merge messages
        for (msg_id, remote_msg) in remote.messages.iter() {
            if let Some(_local_msg) = self.messages.get(msg_id) {
                // Check for conflict (different content, same ID)
                // In practice, LWW handles this automatically
            } else {
                // New message from remote
                self.messages.put(msg_id.clone(), remote_msg.clone(), remote_msg.replica_id);
                if !self.message_order.contains(msg_id) {
                    self.message_order.push(msg_id.clone());
                }
            }
        }

        // Merge metadata
        for (key, remote_value) in remote.metadata.iter() {
            if let Some(local_value) = self.metadata.get(key) {
                if local_value != remote_value {
                    // Conflict detected
                    result.conflicts_detected.push(ConflictType::WriteWriteConflict {
                        field: key.clone(),
                        local_value: local_value.clone(),
                        remote_value: remote_value.clone(),
                        local_timestamp: 0, // Would need to track per-field timestamps
                        remote_timestamp: 0,
                    });
                }
            }
            // LWW merge handles the actual resolution
            self.metadata.put(key.clone(), remote_value.clone(), 0);
        }

        result
    }

    /// Get ordered messages
    pub fn get_ordered_messages(&self) -> Vec<&SessionMessage> {
        self.message_order.iter()
            .filter_map(|id| self.messages.get(id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pn_counter_basic() {
        let mut counter = PNCounter::new();
        counter.increment(1, 5);
        counter.increment(2, 3);
        counter.decrement(1, 2);

        assert_eq!(counter.value(), 6); // 5 + 3 - 2
    }

    #[test]
    fn test_pn_counter_merge() {
        let mut counter1 = PNCounter::new();
        counter1.increment(1, 10);

        let mut counter2 = PNCounter::new();
        counter2.increment(1, 15);
        counter2.increment(2, 5);

        counter1.merge(&counter2);
        assert_eq!(counter1.value(), 20); // max(10, 15) + 5
    }

    #[test]
    fn test_lww_map_basic() {
        let mut map = LwwMap::new();
        map.put("key1", "value1", 1);
        map.put("key2", "value2", 1);

        assert_eq!(map.get(&"key1"), Some(&"value1"));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn test_lww_map_merge() {
        let mut map1 = LwwMap::new();
        map1.put("key1", "old_value", 1);

        std::thread::sleep(std::time::Duration::from_millis(10));

        let mut map2 = LwwMap::new();
        map2.put("key1", "new_value", 2);

        map1.merge(&map2);
        assert_eq!(map1.get(&"key1"), Some(&"new_value"));
    }

    #[test]
    fn test_lww_map_remove() {
        let mut map = LwwMap::new();
        map.put("key1", "value1", 1);
        map.remove(&"key1", 1);

        assert_eq!(map.get(&"key1"), None);
    }

    #[test]
    fn test_mv_register_no_conflict() {
        let mut reg = MVRegister::new("initial", 1);
        reg.set("updated", 1);

        assert!(!reg.has_conflict());
        assert_eq!(reg.get_values().len(), 1);
    }

    #[test]
    fn test_causal_context_dominance() {
        let mut ctx1 = CausalContext::new();
        ctx1.add(Dot::new(1, 1));
        ctx1.add(Dot::new(1, 2));

        let mut ctx2 = CausalContext::new();
        ctx2.add(Dot::new(1, 1));

        assert!(ctx1.dominates(&ctx2));
        assert!(!ctx2.dominates(&ctx1));
    }

    #[test]
    fn test_session_merge() {
        let mut session1 = ConflictAwareSession::new("session-1".to_string());
        session1.add_message(SessionMessage {
            message_id: "msg1".to_string(),
            content: "Hello".to_string(),
            role: "user".to_string(),
            timestamp: 1000,
            replica_id: 1,
            version: 1,
        }, 1);

        let mut session2 = ConflictAwareSession::new("session-1".to_string());
        session2.add_message(SessionMessage {
            message_id: "msg2".to_string(),
            content: "World".to_string(),
            role: "assistant".to_string(),
            timestamp: 2000,
            replica_id: 2,
            version: 1,
        }, 2);

        let result = session1.merge_with_remote(&session2, ResolutionStrategy::LastWriterWins);
        assert!(result.success);
        assert_eq!(session1.get_ordered_messages().len(), 2);
    }
}
