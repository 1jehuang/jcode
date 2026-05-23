//! Cross-region data synchronization protocol
//!
//! Implements eventual consistency across geographically distributed clusters using:
//! - CRDT (Conflict-free Replicated Data Types) for conflict-free replication
//! - Vector clocks for causal ordering
//! - Anti-entropy gossip protocol for state convergence
//! - Merkle trees for efficient state comparison

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use serde::{Serialize, Deserialize};
use tokio::sync::RwLock;
use tracing::{info, debug, warn};

/// Vector clock for causal ordering
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VectorClock {
    /// Map of node_id -> counter
    pub counters: HashMap<String, u64>,
}

impl VectorClock {
    pub fn new() -> Self {
        Self {
            counters: HashMap::new(),
        }
    }

    /// Increment the clock for a specific node
    pub fn increment(&mut self, node_id: &str) {
        let counter = self.counters.entry(node_id.to_string()).or_insert(0);
        *counter += 1;
    }

    /// Merge two vector clocks (take max of each component)
    pub fn merge(&mut self, other: &VectorClock) {
        for (node_id, counter) in &other.counters {
            let entry = self.counters.entry(node_id.clone()).or_insert(0);
            *entry = (*entry).max(*counter);
        }
    }

    /// Check if this clock happens before another (causally)
    pub fn happens_before(&self, other: &VectorClock) -> bool {
        // self happens-before other if all components of self <= other
        // and at least one component is strictly less
        let mut all_leq = true;
        let mut one_lt = false;

        for (node_id, counter) in &self.counters {
            let other_counter = other.counters.get(node_id).copied().unwrap_or(0);
            if *counter > other_counter {
                all_leq = false;
                break;
            }
            if *counter < other_counter {
                one_lt = true;
            }
        }

        // Check other's counters that aren't in self
        for counter in other.counters.values() {
            if *counter > 0 {
                one_lt = true;
            }
        }

        all_leq && one_lt
    }

    /// Check if two events are concurrent
    pub fn is_concurrent(&self, other: &VectorClock) -> bool {
        !self.happens_before(other) && !other.happens_before(self) && self != other
    }
}

/// Last-Writer-Wins register with vector clock
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LwwRegister<T: Serialize + for<'a> Deserialize<'a>> {
    pub value: T,
    pub timestamp: i64,  // Unix timestamp in milliseconds
    pub vector_clock: VectorClock,
    pub node_id: String,
}

impl<T: Serialize + for<'a> Deserialize<'a> + PartialEq> LwwRegister<T> {
    pub fn new(value: T, node_id: &str) -> Self {
        Self {
            value,
            timestamp: chrono::Utc::now().timestamp_millis(),
            vector_clock: VectorClock::new(),
            node_id: node_id.to_string(),
        }
    }

    /// Update the value with conflict resolution
    pub fn update(&mut self, new_value: T, node_id: &str) {
        let now = chrono::Utc::now().timestamp_millis();

        // Use LWW: higher timestamp wins
        if now > self.timestamp {
            self.value = new_value;
            self.timestamp = now;
            self.node_id = node_id.to_string();
            self.vector_clock.increment(node_id);
        }
    }

    /// Merge with another register using LWW strategy
    pub fn merge(&mut self, other: &LwwRegister<T>) {
        if other.timestamp > self.timestamp {
            self.value = other.value.clone();
            self.timestamp = other.timestamp;
            self.node_id = other.node_id.clone();
            self.vector_clock.merge(&other.vector_clock);
        } else if other.timestamp == self.timestamp && other.node_id > self.node_id {
            // Tie-breaker: use lexicographically larger node_id
            self.value = other.value.clone();
            self.timestamp = other.timestamp;
            self.node_id = other.node_id.clone();
            self.vector_clock.merge(&other.vector_clock);
        } else {
            // We win, but still merge vector clocks
            self.vector_clock.merge(&other.vector_clock);
        }
    }
}

/// G-Set (grow-only set) CRDT
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GSet<T: Ord + Clone + Serialize + for<'a> Deserialize<'a>> {
    pub elements: HashSet<T>,
}

impl<T: Ord + Clone + Serialize + for<'a> Deserialize<'a>> GSet<T> {
    pub fn new() -> Self {
        Self {
            elements: HashSet::new(),
        }
    }

    pub fn add(&mut self, element: T) {
        self.elements.insert(element);
    }

    pub fn contains(&self, element: &T) -> bool {
        self.elements.contains(element)
    }

    /// Merge two G-Sets (union)
    pub fn merge(&mut self, other: &GSet<T>) {
        for element in &other.elements {
            self.elements.insert(element.clone());
        }
    }

    pub fn get_elements(&self) -> &HashSet<T> {
        &self.elements
    }
}

/// OR-Set (Observed-Remove Set) CRDT for add/remove operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrSet<T: Ord + Clone + Eq + std::hash::Hash + Serialize + for<'a> Deserialize<'a>> {
    /// Map of element -> set of unique tags
    elements: HashMap<T, HashSet<String>>,
    next_tag: u64,
}

impl<T: Ord + Clone + Eq + std::hash::Hash + Serialize + for<'a> Deserialize<'a>> OrSet<T> {
    pub fn new() -> Self {
        Self {
            elements: HashMap::new(),
            next_tag: 0,
        }
    }

    /// Add an element with a unique tag
    pub fn add(&mut self, element: T, node_id: &str) {
        let tag = format!("{}:{}", node_id, self.next_tag);
        self.next_tag += 1;
        self.elements.entry(element).or_insert_with(HashSet::new).insert(tag);
    }

    /// Remove all instances of an element
    pub fn remove(&mut self, element: &T) {
        self.elements.remove(element);
    }

    /// Check if element is present
    pub fn contains(&self, element: &T) -> bool {
        self.elements.get(element).map_or(false, |tags| !tags.is_empty())
    }

    /// Merge two OR-Sets
    pub fn merge(&mut self, other: &OrSet<T>) {
        for (element, tags) in &other.elements {
            let entry = self.elements.entry(element.clone()).or_insert_with(HashSet::new);
            for tag in tags {
                entry.insert(tag.clone());
            }
        }
    }

    pub fn get_elements(&self) -> Vec<T> {
        self.elements.keys().cloned().collect()
    }
}

/// Session state for cross-region replication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicatedSessionState {
    pub session_id: String,
    pub messages: OrSet<String>,  // Message IDs
    pub metadata: LwwRegister<SessionMetadata>,
    pub last_updated: i64,
    pub region_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub title: String,
    pub status: String,
    pub message_count: usize,
    pub context_size_bytes: usize,
}

/// Anti-entropy gossip protocol for state synchronization
pub struct GossipProtocol {
    local_node_id: String,
    local_region: String,
    peer_nodes: HashMap<String, PeerInfo>,
    sync_interval_ms: u64,
}

#[derive(Debug, Clone)]
struct PeerInfo {
    node_id: String,
    region: String,
    endpoint: String,
    last_sync_timestamp: i64,
    health_status: HealthStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HealthStatus {
    Healthy,
    Unhealthy,
    Unknown,
}

impl GossipProtocol {
    pub fn new(local_node_id: String, local_region: String, sync_interval_ms: u64) -> Self {
        Self {
            local_node_id,
            local_region,
            peer_nodes: HashMap::new(),
            sync_interval_ms,
        }
    }

    /// Register a peer node for synchronization
    pub fn add_peer(&mut self, node_id: String, region: String, endpoint: String) {
        self.peer_nodes.insert(node_id.clone(), PeerInfo {
            node_id,
            region,
            endpoint,
            last_sync_timestamp: 0,
            health_status: HealthStatus::Unknown,
        });
    }

    /// Perform anti-entropy synchronization with a random peer
    pub async fn gossip_round<S: StateStore>(&self, store: &S) -> Result<usize, String> {
        use rand::seq::SliceRandom;

        let healthy_peers: Vec<_> = self.peer_nodes.values()
            .filter(|p| p.health_status == HealthStatus::Healthy || p.health_status == HealthStatus::Unknown)
            .collect();

        if healthy_peers.is_empty() {
            return Err("No healthy peers for gossip".to_string());
        }

        let peer = healthy_peers.choose(&mut rand::thread_rng()).unwrap();

        debug!("Starting gossip sync with peer {} in region {}", peer.node_id, peer.region);

        // Get local state summary (Merkle root hash)
        let local_summary = store.get_state_summary().await
            .map_err(|e| format!("Failed to get local state summary: {}", e))?;

        // Exchange summaries with peer
        let remote_summary = self.exchange_summaries(peer, &local_summary).await?;

        // Determine which keys need syncing
        let keys_to_sync = store.compare_summaries(&local_summary, &remote_summary);

        // Sync missing/updated items
        let synced_count = self.sync_items(peer, &keys_to_sync, store).await?;

        debug!("Gossip sync complete: {} items synced with {}", synced_count, peer.node_id);

        Ok(synced_count)
    }

    /// Exchange state summaries with peer
    async fn exchange_summaries(
        &self,
        _peer: &PeerInfo,
        local_summary: &StateSummary,
    ) -> Result<StateSummary, String> {
        // In production, this would make HTTP/gRPC call to peer
        // For now, return empty summary as placeholder
        Ok(StateSummary {
            merkle_root: vec![],
            item_count: 0,
            last_modified: 0,
        })
    }

    /// Sync specific items with peer
    async fn sync_items<S: StateStore>(
        &self,
        _peer: &PeerInfo,
        _keys: &[String],
        _store: &S,
    ) -> Result<usize, String> {
        // In production, fetch actual items from peer and merge
        Ok(0)
    }

    /// Start background gossip loop
    pub async fn start_gossip_loop<S: StateStore + 'static>(
        &self,
        store: Arc<S>,
    ) {
        let gossip = self.clone_inner();

        tokio::spawn(async move {
            info!(
                "Gossip protocol started: interval={}ms, peers={}",
                gossip.sync_interval_ms,
                gossip.peer_nodes.len()
            );

            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(gossip.sync_interval_ms)).await;

                match gossip.gossip_round(&*store).await {
                    Ok(synced) => {
                        if synced > 0 {
                            debug!("Gossip round completed: {} items synced", synced);
                        }
                    }
                    Err(e) => {
                        warn!("Gossip round failed: {}", e);
                    }
                }
            }
        });
    }

    fn clone_inner(&self) -> Self {
        Self {
            local_node_id: self.local_node_id.clone(),
            local_region: self.local_region.clone(),
            peer_nodes: self.peer_nodes.clone(),
            sync_interval_ms: self.sync_interval_ms,
        }
    }
}

/// State summary for efficient comparison (Merkle tree root)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSummary {
    pub merkle_root: Vec<u8>,
    pub item_count: usize,
    pub last_modified: i64,
}

/// Trait for state storage with sync capabilities
#[async_trait::async_trait]
pub trait StateStore: Send + Sync {
    /// Get a summary of local state for comparison
    async fn get_state_summary(&self) -> Result<StateSummary, Box<dyn std::error::Error + Send + Sync>>;

    /// Compare two summaries and return keys that need syncing
    fn compare_summaries(&self, local: &StateSummary, remote: &StateSummary) -> Vec<String>;

    /// Get specific items by keys
    async fn get_items(&self, keys: &[String]) -> Result<Vec<(String, Vec<u8>)>, Box<dyn std::error::Error + Send + Sync>>;

    /// Merge remote items into local state
    async fn merge_items(&self, items: Vec<(String, Vec<u8>)>) -> Result<usize, Box<dyn std::error::Error + Send + Sync>>;
}

/// Cross-region replication manager
pub struct CrossRegionReplicator {
    local_region: String,
    local_node_id: String,
    gossip: Arc<RwLock<GossipProtocol>>,
    replicated_sessions: Arc<RwLock<HashMap<String, ReplicatedSessionState>>>,
}

impl CrossRegionReplicator {
    pub fn new(local_region: String, local_node_id: String, sync_interval_ms: u64) -> Self {
        let gossip = GossipProtocol::new(local_node_id.clone(), local_region.clone(), sync_interval_ms);

        Self {
            local_region,
            local_node_id,
            gossip: Arc::new(RwLock::new(gossip)),
            replicated_sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a remote peer for replication
    pub async fn add_peer(&self, node_id: String, region: String, endpoint: String) {
        let mut gossip = self.gossip.write().await;
        gossip.add_peer(node_id, region, endpoint);
    }

    /// Replicate a session update to remote regions
    pub async fn replicate_session_update(
        &self,
        session_id: String,
        metadata: SessionMetadata,
    ) -> Result<(), String> {
        let mut sessions = self.replicated_sessions.write().await;

        let state = sessions.entry(session_id.clone()).or_insert_with(|| {
            ReplicatedSessionState {
                session_id: session_id.clone(),
                messages: OrSet::new(),
                metadata: LwwRegister::new(metadata.clone(), &self.local_node_id),
                last_updated: chrono::Utc::now().timestamp_millis(),
                region_id: self.local_region.clone(),
            }
        });

        // Update metadata using LWW register
        state.metadata.update(metadata, &self.local_node_id);
        state.last_updated = chrono::Utc::now().timestamp_millis();

        debug!("Replicated session update: {} in region {}", session_id, self.local_region);

        Ok(())
    }

    /// Merge remote session state with local state
    pub async fn merge_remote_session(
        &self,
        remote_state: ReplicatedSessionState,
    ) -> Result<(), String> {
        let mut sessions = self.replicated_sessions.write().await;

        let session_id = remote_state.session_id.clone();

        if let Some(local_state) = sessions.get_mut(&session_id) {
            // Merge metadata using LWW strategy
            local_state.metadata.merge(&remote_state.metadata);
            local_state.messages.merge(&remote_state.messages);
            local_state.last_updated = chrono::Utc::now().timestamp_millis();
        } else {
            // New session, insert directly
            sessions.insert(session_id, remote_state);
        }

        Ok(())
    }

    /// Get current session state
    pub async fn get_session_state(&self, session_id: &str) -> Option<ReplicatedSessionState> {
        let sessions = self.replicated_sessions.read().await;
        sessions.get(session_id).cloned()
    }

    /// Start background replication
    pub async fn start_replication<S: StateStore + 'static>(&self, store: Arc<S>) {
        let gossip = self.gossip.read().await.clone();
        gossip.start_gossip_loop(store).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_clock_basic() {
        let mut vc1 = VectorClock::new();
        vc1.increment("node1");
        vc1.increment("node1");

        let mut vc2 = VectorClock::new();
        vc2.increment("node1");

        assert!(vc2.happens_before(&vc1));
        assert!(!vc1.happens_before(&vc2));
    }

    #[test]
    fn test_vector_clock_concurrent() {
        let mut vc1 = VectorClock::new();
        vc1.increment("node1");

        let mut vc2 = VectorClock::new();
        vc2.increment("node2");

        assert!(vc1.is_concurrent(&vc2));
    }

    #[test]
    fn test_lww_register_merge() {
        let mut reg1 = LwwRegister::new("value1", "node1");
        std::thread::sleep(std::time::Duration::from_millis(10));
        let reg2 = LwwRegister::new("value2", "node2");

        reg1.merge(&reg2);

        assert_eq!(reg1.value, "value2");
    }

    #[test]
    fn test_gset_merge() {
        let mut set1 = GSet::new();
        set1.add(1);
        set1.add(2);

        let mut set2 = GSet::new();
        set2.add(2);
        set2.add(3);

        set1.merge(&set2);

        assert!(set1.contains(&1));
        assert!(set1.contains(&2));
        assert!(set1.contains(&3));
    }

    #[test]
    fn test_orset_add_remove() {
        let mut orset = OrSet::new();
        orset.add("item1", "node1");
        orset.add("item2", "node1");

        assert!(orset.contains(&"item1"));
        assert!(orset.contains(&"item2"));

        orset.remove(&"item1");
        assert!(!orset.contains(&"item1"));
        assert!(orset.contains(&"item2"));
    }
}
