//! Cluster Metrics and Monitoring
//!
//! Provides Prometheus-compatible metrics for cluster monitoring
//! and structured logging for operational visibility.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, debug};

/// Cluster metrics collector
pub struct ClusterMetrics {
    /// Total number of elections initiated
    elections_initiated: Arc<RwLock<u64>>,

    /// Total number of elections won
    elections_won: Arc<RwLock<u64>>,

    /// Total number of votes cast
    votes_cast: Arc<RwLock<u64>>,

    /// Current term number
    current_term: Arc<RwLock<u64>>,

    /// Number of nodes in cluster
    cluster_size: Arc<RwLock<usize>>,

    /// Number of healthy nodes
    healthy_nodes: Arc<RwLock<usize>>,

    /// Whether this node is leader
    is_leader: Arc<RwLock<bool>>,

    /// Heartbeat count sent
    heartbeats_sent: Arc<RwLock<u64>>,

    /// Heartbeat count received
    heartbeats_received: Arc<RwLock<u64>>,

    /// Failed heartbeat count
    failed_heartbeats: Arc<RwLock<u64>>,

    /// Node status map (node_id -> status)
    node_statuses: Arc<RwLock<HashMap<String, String>>>,

    /// Uptime start time
    start_time: chrono::DateTime<chrono::Utc>,
}

impl ClusterMetrics {
    /// Create new metrics collector
    pub fn new() -> Self {
        Self {
            elections_initiated: Arc::new(RwLock::new(0)),
            elections_won: Arc::new(RwLock::new(0)),
            votes_cast: Arc::new(RwLock::new(0)),
            current_term: Arc::new(RwLock::new(0)),
            cluster_size: Arc::new(RwLock::new(0)),
            healthy_nodes: Arc::new(RwLock::new(0)),
            is_leader: Arc::new(RwLock::new(false)),
            heartbeats_sent: Arc::new(RwLock::new(0)),
            heartbeats_received: Arc::new(RwLock::new(0)),
            failed_heartbeats: Arc::new(RwLock::new(0)),
            node_statuses: Arc::new(RwLock::new(HashMap::new())),
            start_time: chrono::Utc::now(),
        }
    }

    /// Record election initiated
    pub async fn record_election_initiated(&self) {
        let mut count = self.elections_initiated.write().await;
        *count += 1;
        debug!("Election initiated (total: {})", *count);
    }

    /// Record election won
    pub async fn record_election_won(&self) {
        let mut count = self.elections_won.write().await;
        *count += 1;
        info!("Election won (total: {})", *count);
    }

    /// Record vote cast
    pub async fn record_vote_cast(&self) {
        let mut count = self.votes_cast.write().await;
        *count += 1;
    }

    /// Update current term
    pub async fn update_term(&self, term: u64) {
        let mut current = self.current_term.write().await;
        *current = term;
        debug!("Term updated to {}", term);
    }

    /// Update cluster size
    pub async fn update_cluster_size(&self, size: usize) {
        let mut current = self.cluster_size.write().await;
        *current = size;
    }

    /// Update healthy node count
    pub async fn update_healthy_nodes(&self, count: usize) {
        let mut current = self.healthy_nodes.write().await;
        *current = count;
    }

    /// Update leader status
    pub async fn update_leader_status(&self, is_leader: bool) {
        let mut current = self.is_leader.write().await;
        *current = is_leader;
        if is_leader {
            info!("This node became LEADER");
        } else {
            debug!("This node is no longer leader");
        }
    }

    /// Record heartbeat sent
    pub async fn record_heartbeat_sent(&self) {
        let mut count = self.heartbeats_sent.write().await;
        *count += 1;
    }

    /// Record heartbeat received
    pub async fn record_heartbeat_received(&self) {
        let mut count = self.heartbeats_received.write().await;
        *count += 1;
    }

    /// Record failed heartbeat
    pub async fn record_failed_heartbeat(&self) {
        let mut count = self.failed_heartbeats.write().await;
        *count += 1;
        debug!("Failed heartbeat detected (total: {})", *count);
    }

    /// Update node status
    pub async fn update_node_status(&self, node_id: String, status: String) {
        let mut statuses = self.node_statuses.write().await;
        statuses.insert(node_id.clone(), status.clone());
        debug!("Node {} status: {}", node_id, status);
    }

    /// Get uptime in seconds
    pub async fn get_uptime_seconds(&self) -> f64 {
        let now = chrono::Utc::now();
        (now - self.start_time).num_milliseconds() as f64 / 1000.0
    }

    /// Generate Prometheus-compatible metrics text
    pub async fn generate_prometheus_metrics(&self) -> String {
        let mut output = String::new();

        // Helper macro to add metric
        macro_rules! metric {
            ($name:expr, $help:expr, $type:expr, $value:expr) => {
                output.push_str(&format!(
                    "# HELP {} {}\n# TYPE {} {}\n{} {}\n\n",
                    $name, $help, $name, $type, $name, $value
                ));
            };
        }

        // Election metrics
        metric!(
            "cluster_elections_initiated_total",
            "Total number of elections initiated",
            "counter",
            *self.elections_initiated.read().await
        );

        metric!(
            "cluster_elections_won_total",
            "Total number of elections won by this node",
            "counter",
            *self.elections_won.read().await
        );

        metric!(
            "cluster_votes_cast_total",
            "Total number of votes cast",
            "counter",
            *self.votes_cast.read().await
        );

        metric!(
            "cluster_current_term",
            "Current Raft term number",
            "gauge",
            *self.current_term.read().await
        );

        // Cluster size metrics
        metric!(
            "cluster_size",
            "Total number of nodes in cluster",
            "gauge",
            *self.cluster_size.read().await
        );

        metric!(
            "cluster_healthy_nodes",
            "Number of healthy nodes",
            "gauge",
            *self.healthy_nodes.read().await
        );

        // Leader status
        metric!(
            "cluster_is_leader",
            "Whether this node is the leader (1 = yes, 0 = no)",
            "gauge",
            if *self.is_leader.read().await { 1 } else { 0 }
        );

        // Heartbeat metrics
        metric!(
            "cluster_heartbeats_sent_total",
            "Total heartbeats sent by this node",
            "counter",
            *self.heartbeats_sent.read().await
        );

        metric!(
            "cluster_heartbeats_received_total",
            "Total heartbeats received",
            "counter",
            *self.heartbeats_received.read().await
        );

        metric!(
            "cluster_failed_heartbeats_total",
            "Total failed heartbeat detections",
            "counter",
            *self.failed_heartbeats.read().await
        );

        // Uptime
        metric!(
            "cluster_uptime_seconds",
            "Seconds since cluster service started",
            "gauge",
            self.get_uptime_seconds().await
        );

        // Node statuses (as labels)
        let statuses = self.node_statuses.read().await;
        for (node_id, status) in statuses.iter() {
            output.push_str(&format!(
                "cluster_node_status{{node_id=\"{}\"}} {}\n",
                node_id,
                if status == "healthy" { 1 } else { 0 }
            ));
        }
        if !statuses.is_empty() {
            output.push('\n');
        }

        output
    }
}

impl Default for ClusterMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Global metrics instance
static CLUSTER_METRICS: std::sync::OnceLock<Arc<ClusterMetrics>> = std::sync::OnceLock::new();

/// Get or initialize global metrics collector
pub fn get_metrics() -> Arc<ClusterMetrics> {
    CLUSTER_METRICS
        .get_or_init(|| Arc::new(ClusterMetrics::new()))
        .clone()
}

/// Structured log helper for cluster events
pub mod structured_log {
    use tracing::{info, warn, error, debug};

    /// Log cluster initialization
    pub fn cluster_initialized(node_id: &str, host: &str, port: u16) {
        info!(
            target: "cluster.lifecycle",
            event = "cluster_initialized",
            node_id = node_id,
            host = host,
            port = port,
            "Cluster node initialized"
        );
    }

    /// Log election start
    pub fn election_started(node_id: &str, term: u64) {
        info!(
            target: "cluster.election",
            event = "election_started",
            node_id = node_id,
            term = term,
            "Election started"
        );
    }

    /// Log election result
    pub fn election_result(node_id: &str, term: u64, won: bool) {
        if won {
            info!(
                target: "cluster.election",
                event = "election_won",
                node_id = node_id,
                term = term,
                "Election won - became leader"
            );
        } else {
            debug!(
                target: "cluster.election",
                event = "election_lost",
                node_id = node_id,
                term = term,
                "Election lost"
            );
        }
    }

    /// Log vote request
    pub fn vote_requested(candidate_id: &str, voter_id: &str, term: u64) {
        debug!(
            target: "cluster.election",
            event = "vote_requested",
            candidate = candidate_id,
            voter = voter_id,
            term = term,
            "Vote requested"
        );
    }

    /// Log vote granted
    pub fn vote_granted(voter_id: &str, candidate_id: &str, term: u64) {
        debug!(
            target: "cluster.election",
            event = "vote_granted",
            voter = voter_id,
            candidate = candidate_id,
            term = term,
            "Vote granted"
        );
    }

    /// Log heartbeat sent
    pub fn heartbeat_sent(leader_id: &str, follower_count: usize) {
        debug!(
            target: "cluster.heartbeat",
            event = "heartbeat_sent",
            leader = leader_id,
            followers = follower_count,
            "Heartbeat sent"
        );
    }

    /// Log heartbeat timeout
    pub fn heartbeat_timeout(node_id: &str, missed_count: u32) {
        warn!(
            target: "cluster.heartbeat",
            event = "heartbeat_timeout",
            node_id = node_id,
            missed_count = missed_count,
            "Heartbeat timeout detected"
        );
    }

    /// Log node registered
    pub fn node_registered(node_id: &str, address: &str) {
        info!(
            target: "cluster.membership",
            event = "node_registered",
            node_id = node_id,
            address = address,
            "Node registered"
        );
    }

    /// Log node removed
    pub fn node_removed(node_id: &str, reason: &str) {
        warn!(
            target: "cluster.membership",
            event = "node_removed",
            node_id = node_id,
            reason = reason,
            "Node removed from cluster"
        );
    }

    /// Log quorum lost
    pub fn quorum_lost(healthy_count: usize, required_count: usize) {
        error!(
            target: "cluster.quorum",
            event = "quorum_lost",
            healthy = healthy_count,
            required = required_count,
            "Quorum lost - cluster may be unavailable"
        );
    }

    /// Log quorum restored
    pub fn quorum_restored(healthy_count: usize, required_count: usize) {
        info!(
            target: "cluster.quorum",
            event = "quorum_restored",
            healthy = healthy_count,
            required = required_count,
            "Quorum restored"
        );
    }

    /// Log cluster shutdown
    pub fn cluster_shutdown(reason: &str) {
        info!(
            target: "cluster.lifecycle",
            event = "cluster_shutdown",
            reason = reason,
            "Cluster service shutting down"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_metrics_initialization() {
        let metrics = ClusterMetrics::new();
        assert_eq!(*metrics.elections_initiated.read().await, 0);
        assert_eq!(*metrics.elections_won.read().await, 0);
        assert_eq!(*metrics.current_term.read().await, 0);
    }

    #[tokio::test]
    async fn test_record_election() {
        let metrics = ClusterMetrics::new();
        metrics.record_election_initiated().await;
        assert_eq!(*metrics.elections_initiated.read().await, 1);

        metrics.record_election_won().await;
        assert_eq!(*metrics.elections_won.read().await, 1);
    }

    #[tokio::test]
    async fn test_update_term() {
        let metrics = ClusterMetrics::new();
        metrics.update_term(5).await;
        assert_eq!(*metrics.current_term.read().await, 5);
    }

    #[tokio::test]
    async fn test_prometheus_output() {
        let metrics = ClusterMetrics::new();
        metrics.record_election_initiated().await;
        metrics.update_term(1).await;
        metrics.update_cluster_size(3).await;

        let output = metrics.generate_prometheus_metrics().await;
        assert!(output.contains("cluster_elections_initiated_total"));
        assert!(output.contains("cluster_current_term"));
        assert!(output.contains("cluster_size"));
    }

    #[tokio::test]
    async fn test_uptime() {
        let metrics = ClusterMetrics::new();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        let uptime = metrics.get_uptime_seconds().await;
        assert!(uptime >= 0.1);
    }

    #[test]
    fn test_global_metrics() {
        let m1 = get_metrics();
        let m2 = get_metrics();
        assert!(Arc::ptr_eq(&m1, &m2));
    }
}
