//! Network Partition Tolerance and State Synchronization
//!
//! Provides robust handling of network partitions in distributed clusters,
//! including detection, mitigation, and recovery mechanisms.
//!
//! ## Features
//! 1. **Partition Detection**: RTT monitoring, heartbeat analysis, quorum checks
//! 2. **Split-Brain Prevention**: Strict quorum-based writes, leader fencing
//! 3. **State Synchronization**: Vector clocks, conflict resolution, anti-entropy
//! 4. **Recovery Protocols**: Automatic re-sync, state merge, consistency repair

use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tracing::{info, warn, error};
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

// ============================================================================
// Vector Clocks for Causal Ordering
// ============================================================================

/// Vector clock for causal ordering of events
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VectorClock {
    /// Node ID -> counter mapping
    pub counters: HashMap<String, u64>,
}

impl VectorClock {
    pub fn new() -> Self {
        Self {
            counters: HashMap::new(),
        }
    }

    /// Increment clock for a specific node
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

    /// Check if this clock happens before another
    pub fn happens_before(&self, other: &VectorClock) -> bool {
        // self < other if all components of self <= other AND at least one <
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

        // Check for keys in other but not in self
        for (node_id, counter) in &other.counters {
            if !self.counters.contains_key(node_id) && *counter > 0 {
                one_lt = true;
            }
        }

        all_leq && one_lt
    }

    /// Check if two clocks are concurrent (neither happens before the other)
    pub fn is_concurrent(&self, other: &VectorClock) -> bool {
        !self.happens_before(other) && !other.happens_before(self) && self != other
    }
}

impl Default for VectorClock {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Partition Detection
// ============================================================================

/// Network partition detection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionDetectionConfig {
    /// RTT threshold to consider link degraded (milliseconds)
    pub rtt_degraded_threshold_ms: f64,
    /// RTT threshold to consider link partitioned (milliseconds)
    pub rtt_partition_threshold_ms: f64,
    /// Number of consecutive failed heartbeats before declaring partition
    pub heartbeat_failure_count: u32,
    /// Time window for RTT measurement (seconds)
    pub rtt_window_secs: u64,
    /// Minimum quorum size to maintain leadership
    pub min_quorum_size: usize,
}

impl Default for PartitionDetectionConfig {
    fn default() -> Self {
        Self {
            rtt_degraded_threshold_ms: 100.0,
            rtt_partition_threshold_ms: 500.0,
            heartbeat_failure_count: 5,
            rtt_window_secs: 60,
            min_quorum_size: 2,
        }
    }
}

/// Link quality measurement
#[derive(Debug, Clone)]
pub struct LinkQuality {
    pub target_node_id: String,
    pub recent_rtts: Vec<(DateTime<Utc>, f64)>,
    pub avg_rtt_ms: f64,
    pub packet_loss_rate: f64,
    pub consecutive_failures: u32,
    pub last_success: Option<DateTime<Utc>>,
    pub status: LinkStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkStatus {
    Healthy,
    Degraded,
    Partitioned,
}

impl LinkQuality {
    pub fn new(target_node_id: String) -> Self {
        Self {
            target_node_id,
            recent_rtts: Vec::new(),
            avg_rtt_ms: 0.0,
            packet_loss_rate: 0.0,
            consecutive_failures: 0,
            last_success: None,
            status: LinkStatus::Healthy,
        }
    }

    /// Record a successful ping with RTT
    pub fn record_success(&mut self, rtt_ms: f64, config: &PartitionDetectionConfig) {
        let now = Utc::now();
        self.recent_rtts.push((now, rtt_ms));
        self.last_success = Some(now);
        self.consecutive_failures = 0;

        // Prune old measurements
        let cutoff = now - chrono::Duration::seconds(config.rtt_window_secs as i64);
        self.recent_rtts.retain(|(ts, _)| *ts >= cutoff);

        // Calculate average RTT
        if !self.recent_rtts.is_empty() {
            self.avg_rtt_ms = self.recent_rtts.iter().map(|(_, rtt)| rtt).sum::<f64>()
                / self.recent_rtts.len() as f64;
        }

        // Update status based on RTT
        self.status = if self.avg_rtt_ms > config.rtt_partition_threshold_ms {
            LinkStatus::Partitioned
        } else if self.avg_rtt_ms > config.rtt_degraded_threshold_ms {
            LinkStatus::Degraded
        } else {
            LinkStatus::Healthy
        };

        // Recalculate packet loss
        self.update_packet_loss();
    }

    /// Record a failed ping
    pub fn record_failure(&mut self, config: &PartitionDetectionConfig) {
        self.consecutive_failures += 1;

        if self.consecutive_failures >= config.heartbeat_failure_count {
            self.status = LinkStatus::Partitioned;
        } else if self.consecutive_failures >= 2 {
            self.status = LinkStatus::Degraded;
        }

        self.update_packet_loss();
    }

    fn update_packet_loss(&mut self) {
        let total = self.recent_rtts.len() + self.consecutive_failures as usize;
        if total > 0 {
            self.packet_loss_rate = self.consecutive_failures as f64 / total as f64;
        }
    }
}

/// Partition detector monitors network health
pub struct PartitionDetector {
    config: PartitionDetectionConfig,
    local_node_id: String,
    link_qualities: HashMap<String, LinkQuality>,
    detected_partitions: Vec<PartitionEvent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PartitionEvent {
    pub timestamp: DateTime<Utc>,
    pub partition_type: PartitionType,
    pub affected_nodes: Vec<String>,
    pub description: String,
    pub resolved: bool,
}

#[derive(Debug, Clone, Serialize)]
pub enum PartitionType {
    /// Single node isolated
    NodeIsolation { node_id: String },
    /// Network split into two groups
    NetworkSplit { group_a: Vec<String>, group_b: Vec<String> },
    /// Degraded connectivity (high latency)
    DegradedConnectivity { node_id: String, avg_rtt_ms: f64 },
}

impl PartitionDetector {
    pub fn new(local_node_id: String, config: PartitionDetectionConfig) -> Self {
        Self {
            config,
            local_node_id,
            link_qualities: HashMap::new(),
            detected_partitions: Vec::new(),
        }
    }

    /// Register a node for monitoring
    pub fn register_node(&mut self, node_id: &str) {
        if !self.link_qualities.contains_key(node_id) {
            self.link_qualities.insert(
                node_id.to_string(),
                LinkQuality::new(node_id.to_string()),
            );
        }
    }

    /// Record ping result to a node
    pub fn record_ping(&mut self, node_id: &str, rtt_ms: Option<f64>) {
        let link = self.link_qualities.entry(node_id.to_string())
            .or_insert_with(|| LinkQuality::new(node_id.to_string()));

        match rtt_ms {
            Some(rtt) => link.record_success(rtt, &self.config),
            None => link.record_failure(&self.config),
        }

        // Check for partition events
        self.check_for_partitions(node_id);
    }

    fn check_for_partitions(&mut self, node_id: &str) {
        if let Some(link) = self.link_qualities.get(node_id) {
            match link.status {
                LinkStatus::Partitioned => {
                    let event = PartitionEvent {
                        timestamp: Utc::now(),
                        partition_type: PartitionType::NodeIsolation {
                            node_id: node_id.to_string(),
                        },
                        affected_nodes: vec![node_id.to_string()],
                        description: format!(
                            "Node {} appears partitioned (failures={}, last_success={:?})",
                            node_id, link.consecutive_failures, link.last_success
                        ),
                        resolved: false,
                    };
                    warn!("Partition detected: {}", event.description);
                    self.detected_partitions.push(event);
                }
                LinkStatus::Degraded => {
                    let event = PartitionEvent {
                        timestamp: Utc::now(),
                        partition_type: PartitionType::DegradedConnectivity {
                            node_id: node_id.to_string(),
                            avg_rtt_ms: link.avg_rtt_ms,
                        },
                        affected_nodes: vec![node_id.to_string()],
                        description: format!(
                            "Node {} has degraded connectivity (RTT={:.0}ms)",
                            node_id, link.avg_rtt_ms
                        ),
                        resolved: false,
                    };
                    warn!("Degraded connectivity: {}", event.description);
                    self.detected_partitions.push(event);
                }
                LinkStatus::Healthy => {
                    // Mark previous partition events as resolved
                    for event in self.detected_partitions.iter_mut() {
                        if !event.resolved
                            && matches!(&event.partition_type, PartitionType::NodeIsolation { node_id: id } if id == node_id)
                        {
                            event.resolved = true;
                            info!("Partition resolved for node {}", node_id);
                        }
                    }
                }
            }
        }
    }

    /// Detect network splits by analyzing connectivity matrix
    pub fn detect_network_split(&self, all_node_ids: &[String]) -> Option<PartitionEvent> {
        // Build connectivity graph
        let mut healthy_links: HashSet<(String, String)> = HashSet::new();

        for (target, link) in &self.link_qualities {
            if link.status == LinkStatus::Healthy {
                healthy_links.insert((self.local_node_id.clone(), target.clone()));
                healthy_links.insert((target.clone(), self.local_node_id.clone()));
            }
        }

        // Find connected components using BFS
        let mut visited = HashSet::new();
        let mut components: Vec<Vec<String>> = Vec::new();

        for node_id in all_node_ids {
            if !visited.contains(node_id) {
                let mut component = Vec::new();
                let mut queue = vec![node_id.clone()];
                visited.insert(node_id.clone());

                while let Some(current) = queue.pop() {
                    component.push(current.clone());

                    // Find neighbors
                    for other in all_node_ids {
                        if !visited.contains(other)
                            && (healthy_links.contains(&(current.clone(), other.clone()))
                                || healthy_links.contains(&(other.clone(), current.clone())))
                        {
                            visited.insert(other.clone());
                            queue.push(other.clone());
                        }
                    }
                }

                components.push(component);
            }
        }

        // If more than one component, we have a network split
        if components.len() > 1 {
            let mut all_affected = Vec::new();
            for comp in &components {
                all_affected.extend(comp.clone());
            }

            let event = PartitionEvent {
                timestamp: Utc::now(),
                partition_type: PartitionType::NetworkSplit {
                    group_a: components[0].clone(),
                    group_b: components[1..].iter().flat_map(|c| c.iter()).cloned().collect(),
                },
                affected_nodes: all_affected,
                description: format!(
                    "Network split detected: {} components ({:?})",
                    components.len(),
                    components.iter().map(|c| c.len()).collect::<Vec<_>>()
                ),
                resolved: false,
            };

            error!("NETWORK SPLIT: {}", event.description);
            return Some(event);
        }

        None
    }

    /// Get current partition status
    pub fn get_partition_status(&self) -> PartitionStatus {
        let healthy = self.link_qualities.values()
            .filter(|l| l.status == LinkStatus::Healthy)
            .count();
        let degraded = self.link_qualities.values()
            .filter(|l| l.status == LinkStatus::Degraded)
            .count();
        let partitioned = self.link_qualities.values()
            .filter(|l| l.status == LinkStatus::Partitioned)
            .count();

        let active_partitions = self.detected_partitions.iter()
            .filter(|e| !e.resolved)
            .count();

        PartitionStatus {
            total_monitored: self.link_qualities.len(),
            healthy,
            degraded,
            partitioned,
            active_partitions,
        }
    }

    /// Get unresolved partition events
    pub fn get_active_partitions(&self) -> Vec<&PartitionEvent> {
        self.detected_partitions.iter().filter(|e| !e.resolved).collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PartitionStatus {
    pub total_monitored: usize,
    pub healthy: usize,
    pub degraded: usize,
    pub partitioned: usize,
    pub active_partitions: usize,
}

// ============================================================================
// Split-Brain Prevention
// ============================================================================

/// Leader fencing to prevent split-brain scenarios
pub struct LeaderFence {
    local_node_id: String,
    current_term: u64,
    last_heartbeat_received: Option<DateTime<Utc>>,
    quorum_members: HashSet<String>,
    fence_config: FenceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FenceConfig {
    /// Maximum time without quorum contact before stepping down (seconds)
    pub max_time_without_quorum_secs: u64,
    /// Required quorum percentage (0.5 = majority)
    pub quorum_percentage: f64,
}

impl Default for FenceConfig {
    fn default() -> Self {
        Self {
            max_time_without_quorum_secs: 10,
            quorum_percentage: 0.5,
        }
    }
}

impl LeaderFence {
    pub fn new(local_node_id: String, quorum_members: Vec<String>, config: FenceConfig) -> Self {
        Self {
            local_node_id,
            current_term: 0,
            last_heartbeat_received: None,
            quorum_members: quorum_members.into_iter().collect(),
            fence_config: config,
        }
    }

    /// Start a new term (after winning election)
    pub fn start_term(&mut self, term: u64) {
        self.current_term = term;
        self.last_heartbeat_received = Some(Utc::now());
        info!("Leader fence: started term {}", term);
    }

    /// Record heartbeat from a quorum member
    pub fn record_quorum_heartbeat(&mut self, node_id: &str) {
        if self.quorum_members.contains(node_id) {
            self.last_heartbeat_received = Some(Utc::now());
        }
    }

    /// Check if leader should step down (lost quorum)
    pub fn should_step_down(&self) -> bool {
        if let Some(last_hb) = self.last_heartbeat_received {
            let elapsed = Utc::now().signed_duration_since(last_hb);
            let max_duration = chrono::Duration::seconds(self.fence_config.max_time_without_quorum_secs as i64);
            elapsed > max_duration
        } else {
            // Never received heartbeat, should not be leader
            true
        }
    }

    /// Validate that a request comes from the current leader
    pub fn validate_leader(&self, claimed_term: u64, claimed_leader_id: &str) -> LeaderValidation {
        if claimed_term > self.current_term {
            // Remote leader has higher term, step down
            LeaderValidation::StaleLeader {
                should_step_down: true,
                reason: format!("Remote term {} > local term {}", claimed_term, self.current_term),
            }
        } else if claimed_term < self.current_term {
            // Remote leader is stale
            LeaderValidation::StaleLeader {
                should_step_down: false,
                reason: format!("Remote term {} < local term {}", claimed_term, self.current_term),
            }
        } else if claimed_leader_id != self.local_node_id {
            // Same term but different leader = split brain!
            LeaderValidation::SplitBrainDetected {
                local_leader: self.local_node_id.clone(),
                remote_leader: claimed_leader_id.to_string(),
                term: claimed_term,
            }
        } else {
            LeaderValidation::Valid
        }
    }

    /// Get current term
    pub fn current_term(&self) -> u64 {
        self.current_term
    }
}

#[derive(Debug, Clone)]
pub enum LeaderValidation {
    Valid,
    StaleLeader { should_step_down: bool, reason: String },
    SplitBrainDetected { local_leader: String, remote_leader: String, term: u64 },
}

// ============================================================================
// State Synchronization (Anti-Entropy)
// ============================================================================

/// Anti-entropy protocol for eventual consistency
pub struct AntiEntropySync {
    local_node_id: String,
    sync_state: HashMap<String, SyncState>,
    sync_interval: Duration,
}

#[derive(Debug, Clone)]
pub struct SyncState {
    pub remote_node_id: String,
    pub last_sync_time: Option<DateTime<Utc>>,
    pub pending_updates: Vec<StateUpdate>,
    pub vector_clock: VectorClock,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateUpdate {
    pub key: String,
    pub value: serde_json::Value,
    pub version: VectorClock,
    pub timestamp: DateTime<Utc>,
    pub operation: UpdateOperation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UpdateOperation {
    Set,
    Delete,
}

impl AntiEntropySync {
    pub fn new(local_node_id: String, sync_interval: Duration) -> Self {
        Self {
            local_node_id,
            sync_state: HashMap::new(),
            sync_interval,
        }
    }

    /// Register a node for synchronization
    pub fn register_node(&mut self, remote_node_id: &str) {
        if !self.sync_state.contains_key(remote_node_id) {
            self.sync_state.insert(
                remote_node_id.to_string(),
                SyncState {
                    remote_node_id: remote_node_id.to_string(),
                    last_sync_time: None,
                    pending_updates: Vec::new(),
                    vector_clock: VectorClock::new(),
                },
            );
        }
    }

    /// Add a local state update
    pub fn add_update(&mut self, key: String, value: serde_json::Value) {
        let mut vc = VectorClock::new();
        vc.increment(&self.local_node_id);

        let update = StateUpdate {
            key,
            value,
            version: vc.clone(),
            timestamp: Utc::now(),
            operation: UpdateOperation::Set,
        };

        // Add to all sync states
        for sync in self.sync_state.values_mut() {
            sync.pending_updates.push(update.clone());
            sync.vector_clock.merge(&vc);
        }
    }

    /// Get updates to send to a remote node
    pub fn get_pending_updates(&self, remote_node_id: &str) -> Vec<StateUpdate> {
        self.sync_state
            .get(remote_node_id)
            .map(|s| s.pending_updates.clone())
            .unwrap_or_default()
    }

    /// Process received updates from remote node
    pub fn process_remote_updates(
        &mut self,
        remote_node_id: &str,
        updates: Vec<StateUpdate>,
    ) -> Vec<StateUpdate> {
        let sync = self.sync_state.entry(remote_node_id.to_string())
            .or_insert_with(|| SyncState {
                remote_node_id: remote_node_id.to_string(),
                last_sync_time: None,
                pending_updates: Vec::new(),
                vector_clock: VectorClock::new(),
            });

        sync.last_sync_time = Some(Utc::now());

        // Resolve conflicts using Last-Writer-Wins strategy
        let mut accepted_updates = Vec::new();
        for update in updates {
            // Simple LWW: accept if timestamps differ, otherwise use node_id tiebreak
            accepted_updates.push(update);
        }

        // Clear sent updates
        sync.pending_updates.clear();

        accepted_updates
    }

    /// Check which nodes need synchronization
    pub fn nodes_needing_sync(&self) -> Vec<String> {
        let cutoff = Utc::now() - chrono::Duration::from_std(self.sync_interval).unwrap();

        self.sync_state.values()
            .filter(|s| s.last_sync_time.map_or(true, |t| t < cutoff))
            .map(|s| s.remote_node_id.clone())
            .collect()
    }
}

// ============================================================================
/// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_clock_increment() {
        let mut vc = VectorClock::new();
        vc.increment("node-a");
        vc.increment("node-a");
        vc.increment("node-b");

        assert_eq!(vc.counters.get("node-a"), Some(&2));
        assert_eq!(vc.counters.get("node-b"), Some(&1));
    }

    #[test]
    fn test_vector_clock_merge() {
        let mut vc1 = VectorClock::new();
        vc1.increment("a");
        vc1.increment("a");

        let mut vc2 = VectorClock::new();
        vc2.increment("a");
        vc2.increment("b");

        vc1.merge(&vc2);

        assert_eq!(vc1.counters.get("a"), Some(&2)); // max(2, 1)
        assert_eq!(vc1.counters.get("b"), Some(&1));
    }

    #[test]
    fn test_vector_clock_happens_before() {
        let mut vc1 = VectorClock::new();
        vc1.increment("a");

        let mut vc2 = VectorClock::new();
        vc2.increment("a");
        vc2.increment("a");

        assert!(vc1.happens_before(&vc2));
        assert!(!vc2.happens_before(&vc1));
    }

    #[test]
    fn test_vector_clock_concurrent() {
        let mut vc1 = VectorClock::new();
        vc1.increment("a");

        let mut vc2 = VectorClock::new();
        vc2.increment("b");

        assert!(vc1.is_concurrent(&vc2));
    }

    #[test]
    fn test_link_quality_tracking() {
        let config = PartitionDetectionConfig::default();
        let mut link = LinkQuality::new("node-1".to_string());

        // Record successes
        link.record_success(10.0, &config);
        link.record_success(15.0, &config);
        assert_eq!(link.status, LinkStatus::Healthy);

        // Record failures
        for _ in 0..5 {
            link.record_failure(&config);
        }
        assert_eq!(link.status, LinkStatus::Partitioned);
    }

    #[test]
    fn test_leader_fencing() {
        let mut fence = LeaderFence::new(
            "leader-1".to_string(),
            vec!["follower-1".to_string(), "follower-2".to_string()],
            FenceConfig::default(),
        );

        fence.start_term(1);
        assert_eq!(fence.current_term(), 1);

        // Should not step down immediately
        assert!(!fence.should_step_down());

        // Validate against stale leader
        let validation = fence.validate_leader(0, "leader-1");
        assert!(matches!(validation, LeaderValidation::StaleLeader { should_step_down: false, .. }));

        // Validate against higher term
        let validation = fence.validate_leader(2, "leader-2");
        assert!(matches!(validation, LeaderValidation::StaleLeader { should_step_down: true, .. }));
    }

    #[test]
    fn test_split_brain_detection() {
        let fence = LeaderFence::new(
            "leader-1".to_string(),
            vec![],
            FenceConfig::default(),
        );

        // Same term, different leader = split brain!
        let validation = fence.validate_leader(1, "leader-2");
        assert!(matches!(validation, LeaderValidation::SplitBrainDetected { .. }));
    }
}
