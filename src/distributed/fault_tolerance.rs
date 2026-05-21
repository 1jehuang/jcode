//! Fault Tolerance Manager - Graded health states and automatic recovery
//!
//! This module provides:
//! 1. Graded health state tracking (Healthy → Warning → Critical → Offline)
//! 2. Configurable failure thresholds
//! 3. Automatic fault transfer decisions
//! 4. Alert notification system (log/webhook/email)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{info, warn, error, debug};
use chrono::{DateTime, Utc};

/// Node health state with grading
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeHealthState {
    /// Normal operation
    Healthy,
    /// 2 consecutive heartbeat timeouts - monitoring closely
    Warning,
    /// 5 consecutive heartbeat timeouts - preparing for removal
    Critical,
    /// Exceeded timeout threshold - will be removed
    Offline,
}

impl NodeHealthState {
    pub fn severity(&self) -> u8 {
        match self {
            Self::Healthy => 0,
            Self::Warning => 1,
            Self::Critical => 2,
            Self::Offline => 3,
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Healthy => "Node is operating normally",
            Self::Warning => "Node showing signs of instability",
            Self::Critical => "Node is critically unhealthy, removal imminent",
            Self::Offline => "Node is offline and will be removed",
        }
    }
}

/// Failure event record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureEvent {
    pub timestamp: DateTime<Utc>,
    pub event_type: FailureType,
    pub details: String,
    pub resolved: bool,
}

/// Type of failure detected
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FailureType {
    HeartbeatTimeout,
    HighLatency,
    ResourceExhaustion,
    NetworkPartition,
    Unknown,
}

/// Configuration for fault tolerance behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultToleranceConfig {
    /// Number of consecutive failures before Warning state
    pub warning_threshold: u32,
    /// Number of consecutive failures before Critical state
    pub critical_threshold: u32,
    /// Number of consecutive failures before Offline state
    pub offline_threshold: u32,
    /// Time window for counting failures (seconds)
    pub failure_window_secs: u64,
    /// Enable automatic node removal
    pub auto_removal_enabled: bool,
    /// Cooldown period after removal before allowing rejoin (seconds)
    pub removal_cooldown_secs: u64,
    /// Enable alert notifications
    pub alerts_enabled: bool,
    /// Webhook URL for alerts (optional)
    pub webhook_url: Option<String>,
    /// Maximum retries for failed operations
    pub max_retry_count: u32,
}

impl Default for FaultToleranceConfig {
    fn default() -> Self {
        Self {
            warning_threshold: 2,
            critical_threshold: 5,
            offline_threshold: 10,
            failure_window_secs: 300, // 5 minutes
            auto_removal_enabled: true,
            removal_cooldown_secs: 600, // 10 minutes
            alerts_enabled: true,
            webhook_url: None,
            max_retry_count: 3,
        }
    }
}

/// Alert notification payload
#[derive(Debug, Clone, Serialize)]
pub struct AlertNotification {
    pub timestamp: DateTime<Utc>,
    pub node_id: String,
    pub cluster_id: String,
    pub severity: String,
    pub state: NodeHealthState,
    pub message: String,
    pub consecutive_failures: u32,
    pub action_taken: String,
}

/// Tracks health state and failures for a single node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeHealthTracker {
    pub node_id: String,
    pub current_state: NodeHealthState,
    pub consecutive_failures: u32,
    pub total_failures: u32,
    pub last_failure_time: Option<DateTime<Utc>>,
    pub last_success_time: DateTime<Utc>,
    pub failure_history: Vec<FailureEvent>,
    pub state_transitions: Vec<(DateTime<Utc>, NodeHealthState)>,
    pub removal_timestamp: Option<DateTime<Utc>>,
}

impl NodeHealthTracker {
    pub fn new(node_id: String) -> Self {
        Self {
            node_id,
            current_state: NodeHealthState::Healthy,
            consecutive_failures: 0,
            total_failures: 0,
            last_failure_time: None,
            last_success_time: Utc::now(),
            failure_history: Vec::new(),
            state_transitions: vec![(Utc::now(), NodeHealthState::Healthy)],
            removal_timestamp: None,
        }
    }

    /// Record a successful heartbeat
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.last_success_time = Utc::now();

        // If we were in Warning state and now succeeding, go back to Healthy
        if self.current_state == NodeHealthState::Warning {
            self.transition_to(NodeHealthState::Healthy);
        }
    }

    /// Record a failure event
    pub fn record_failure(
        &mut self,
        failure_type: FailureType,
        details: String,
    ) -> NodeHealthState {
        let now = Utc::now();
        self.consecutive_failures += 1;
        self.total_failures += 1;
        self.last_failure_time = Some(now);

        // Add to failure history
        self.failure_history.push(FailureEvent {
            timestamp: now,
            event_type: failure_type,
            details,
            resolved: false,
        });

        // Prune old failures outside the window
        self.prune_old_failures(300); // 5 minute window

        // Determine new state based on consecutive failures
        let new_state = if self.consecutive_failures >= 10 {
            NodeHealthState::Offline
        } else if self.consecutive_failures >= 5 {
            NodeHealthState::Critical
        } else if self.consecutive_failures >= 2 {
            NodeHealthState::Warning
        } else {
            NodeHealthState::Healthy
        };

        if new_state != self.current_state {
            self.transition_to(new_state);
        }

        self.current_state
    }

    /// Transition to a new state
    fn transition_to(&mut self, new_state: NodeHealthState) {
        info!(
            "Node {} health state changed: {:?} -> {:?}",
            self.node_id, self.current_state, new_state
        );
        self.current_state = new_state;
        self.state_transitions.push((Utc::now(), new_state));
    }

    /// Remove old failure events outside the time window
    fn prune_old_failures(&mut self, window_secs: u64) {
        let cutoff = Utc::now() - chrono::Duration::seconds(window_secs as i64);
        self.failure_history.retain(|event| event.timestamp >= cutoff);
    }

    /// Check if node is ready for removal
    pub fn should_remove(&self, config: &FaultToleranceConfig) -> bool {
        self.consecutive_failures >= config.offline_threshold
    }

    /// Get failure rate in the current window
    pub fn failure_rate(&self) -> f64 {
        if self.failure_history.is_empty() {
            return 0.0;
        }

        let window_start = Utc::now() - chrono::Duration::seconds(300);
        let recent_failures = self
            .failure_history
            .iter()
            .filter(|e| e.timestamp >= window_start)
            .count();

        recent_failures as f64 / 300.0 // failures per second
    }
}

/// Main fault tolerance manager
pub struct FaultToleranceManager {
    config: FaultToleranceConfig,
    node_trackers: HashMap<String, NodeHealthTracker>,
    cluster_id: String,
    alert_count: u64,
}

impl FaultToleranceManager {
    pub fn new(config: FaultToleranceConfig, cluster_id: String) -> Self {
        info!(
            "FaultToleranceManager initialized: cluster={}, auto_removal={}",
            cluster_id, config.auto_removal_enabled
        );

        Self {
            config,
            node_trackers: HashMap::new(),
            cluster_id,
            alert_count: 0,
        }
    }

    /// Register a node for health tracking
    pub fn register_node(&mut self, node_id: &str) {
        if !self.node_trackers.contains_key(node_id) {
            self.node_trackers.insert(
                node_id.to_string(),
                NodeHealthTracker::new(node_id.to_string()),
            );
            debug!("Registered node {} for health tracking", node_id);
        }
    }

    /// Unregister a node from health tracking
    pub fn unregister_node(&mut self, node_id: &str) {
        if let Some(tracker) = self.node_trackers.remove(node_id) {
            info!("Unregistered node {} from health tracking", node_id);
            debug!(
                "Node {} had {} total failures before removal",
                node_id, tracker.total_failures
            );
        }
    }

    /// Record a successful heartbeat
    pub fn record_heartbeat(&mut self, node_id: &str) {
        let tracker = self
            .node_trackers
            .entry(node_id.to_string())
            .or_insert_with(|| NodeHealthTracker::new(node_id.to_string()));
        tracker.record_success();
    }

    /// Record a heartbeat failure
    pub fn record_heartbeat_failure(&mut self, node_id: &str, details: String) -> NodeHealthState {
        let tracker = self
            .node_trackers
            .entry(node_id.to_string())
            .or_insert_with(|| NodeHealthTracker::new(node_id.to_string()));

        let new_state = tracker.record_failure(FailureType::HeartbeatTimeout, details);

        // Send alert if state changed to Warning or worse
        if new_state.severity() >= NodeHealthState::Warning.severity() {
            self.send_alert(node_id, new_state, tracker.consecutive_failures);
        }

        new_state
    }

    /// Check if a node should be removed
    pub fn should_remove_node(&self, node_id: &str) -> bool {
        if let Some(tracker) = self.node_trackers.get(node_id) {
            tracker.should_remove(&self.config)
        } else {
            false
        }
    }

    /// Get nodes that should be removed
    pub fn get_nodes_for_removal(&self) -> Vec<String> {
        self.node_trackers
            .iter()
            .filter(|(_, tracker)| tracker.should_remove(&self.config))
            .map(|(node_id, _)| node_id.clone())
            .collect()
    }

    /// Get current health state of a node
    pub fn get_node_state(&self, node_id: &str) -> Option<NodeHealthState> {
        self.node_trackers
            .get(node_id)
            .map(|tracker| tracker.current_state)
    }

    /// Get health summary for all nodes
    pub fn get_health_summary(&self) -> HealthSummary {
        let mut summary = HealthSummary {
            total_nodes: self.node_trackers.len(),
            healthy: 0,
            warning: 0,
            critical: 0,
            offline: 0,
            nodes_for_removal: Vec::new(),
        };

        for (node_id, tracker) in &self.node_trackers {
            match tracker.current_state {
                NodeHealthState::Healthy => summary.healthy += 1,
                NodeHealthState::Warning => summary.warning += 1,
                NodeHealthState::Critical => summary.critical += 1,
                NodeHealthState::Offline => {
                    summary.offline += 1;
                    summary.nodes_for_removal.push(node_id.clone());
                }
            }
        }

        summary
    }

    /// Send alert notification
    fn send_alert(&mut self, node_id: &str, state: NodeHealthState, consecutive_failures: u32) {
        if !self.config.alerts_enabled {
            return;
        }

        let alert = AlertNotification {
            timestamp: Utc::now(),
            node_id: node_id.to_string(),
            cluster_id: self.cluster_id.clone(),
            severity: format!("{:?}", state),
            state,
            message: format!(
                "Node {} entered {:?} state after {} consecutive failures",
                node_id, state, consecutive_failures
            ),
            consecutive_failures,
            action_taken: if state == NodeHealthState::Offline && self.config.auto_removal_enabled {
                "Node will be automatically removed".to_string()
            } else {
                "Monitoring".to_string()
            },
        };

        // Log the alert
        match state {
            NodeHealthState::Warning => {
                warn!("ALERT [WARNING]: {}", alert.message);
            }
            NodeHealthState::Critical => {
                error!("ALERT [CRITICAL]: {}", alert.message);
            }
            NodeHealthState::Offline => {
                error!("ALERT [OFFLINE]: {}", alert.message);
            }
            _ => {}
        }

        // Send webhook if configured
        if let Some(ref webhook_url) = self.config.webhook_url {
            self.send_webhook_alert(webhook_url, &alert);
        }

        self.alert_count += 1;
    }

    /// Send webhook alert (simplified implementation)
    fn send_webhook_alert(&self, url: &str, alert: &AlertNotification) {
        // In production, use reqwest or similar HTTP client
        // For now, just log that we would send it
        debug!("Would send webhook to {}: {:?}", url, alert);
        // TODO: Implement actual HTTP POST
        // let client = reqwest::Client::new();
        // let _ = client.post(url).json(alert).send().await;
    }

    /// Mark a node as removed
    pub fn mark_node_removed(&mut self, node_id: &str) {
        if let Some(tracker) = self.node_trackers.get_mut(node_id) {
            tracker.removal_timestamp = Some(Utc::now());
            info!("Marked node {} as removed at {:?}", node_id, tracker.removal_timestamp);
        }
    }

    /// Check if a node is in cooldown period after removal
    pub fn is_in_cooldown(&self, node_id: &str) -> bool {
        if let Some(tracker) = self.node_trackers.get(node_id) {
            if let Some(removal_time) = tracker.removal_timestamp {
                let elapsed = Utc::now().signed_duration_since(removal_time);
                let cooldown_duration = chrono::Duration::seconds(self.config.removal_cooldown_secs as i64);
                elapsed < cooldown_duration
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Get alert statistics
    pub fn get_alert_stats(&self) -> (u64, u64) {
        let total_alerts = self.alert_count;
        let active_alerts = self
            .node_trackers
            .values()
            .filter(|t| t.current_state.severity() >= NodeHealthState::Warning.severity())
            .count() as u64;
        (total_alerts, active_alerts)
    }

    /// Clean up trackers for removed nodes
    pub fn cleanup_removed_nodes(&mut self, older_than_secs: u64) {
        let cutoff = Utc::now() - chrono::Duration::seconds(older_than_secs as i64);
        let mut to_remove = Vec::new();

        for (node_id, tracker) in &self.node_trackers {
            if let Some(removal_time) = tracker.removal_timestamp {
                if removal_time < cutoff {
                    to_remove.push(node_id.clone());
                }
            }
        }

        for node_id in to_remove {
            self.node_trackers.remove(&node_id);
            debug!("Cleaned up tracker for removed node {}", node_id);
        }
    }
}

/// Summary of cluster health
#[derive(Debug, Clone, Serialize)]
pub struct HealthSummary {
    pub total_nodes: usize,
    pub healthy: usize,
    pub warning: usize,
    pub critical: usize,
    pub offline: usize,
    pub nodes_for_removal: Vec<String>,
}

impl HealthSummary {
    pub fn is_healthy(&self) -> bool {
        self.warning == 0 && self.critical == 0 && self.offline == 0
    }

    pub fn needs_attention(&self) -> bool {
        self.critical > 0 || self.offline > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_healthy_node_transitions() {
        let config = FaultToleranceConfig::default();
        let mut manager = FaultToleranceManager::new(config, "test-cluster".to_string());

        manager.register_node("node-1");

        // Should start healthy
        assert_eq!(
            manager.get_node_state("node-1"),
            Some(NodeHealthState::Healthy)
        );

        // Record successes
        manager.record_heartbeat("node-1");
        assert_eq!(
            manager.get_node_state("node-1"),
            Some(NodeHealthState::Healthy)
        );
    }

    #[test]
    fn test_failure_progression() {
        let config = FaultToleranceConfig::default();
        let mut manager = FaultToleranceManager::new(config, "test-cluster".to_string());

        manager.register_node("node-1");

        // 2 failures -> Warning
        manager.record_heartbeat_failure("node-1", "timeout".to_string());
        manager.record_heartbeat_failure("node-1", "timeout".to_string());
        assert_eq!(
            manager.get_node_state("node-1"),
            Some(NodeHealthState::Warning)
        );

        // 5 failures -> Critical
        for _ in 0..3 {
            manager.record_heartbeat_failure("node-1", "timeout".to_string());
        }
        assert_eq!(
            manager.get_node_state("node-1"),
            Some(NodeHealthState::Critical)
        );

        // 10 failures -> Offline
        for _ in 0..5 {
            manager.record_heartbeat_failure("node-1", "timeout".to_string());
        }
        assert_eq!(
            manager.get_node_state("node-1"),
            Some(NodeHealthState::Offline)
        );

        // Should be marked for removal
        assert!(manager.should_remove_node("node-1"));
    }

    #[test]
    fn test_recovery_from_warning() {
        let config = FaultToleranceConfig::default();
        let mut manager = FaultToleranceManager::new(config, "test-cluster".to_string());

        manager.register_node("node-1");

        // 2 failures -> Warning
        manager.record_heartbeat_failure("node-1", "timeout".to_string());
        manager.record_heartbeat_failure("node-1", "timeout".to_string());
        assert_eq!(
            manager.get_node_state("node-1"),
            Some(NodeHealthState::Warning)
        );

        // Success should recover to Healthy
        manager.record_heartbeat("node-1");
        assert_eq!(
            manager.get_node_state("node-1"),
            Some(NodeHealthState::Healthy)
        );
    }

    #[test]
    fn test_health_summary() {
        let config = FaultToleranceConfig::default();
        let mut manager = FaultToleranceManager::new(config, "test-cluster".to_string());

        manager.register_node("node-1");
        manager.register_node("node-2");
        manager.register_node("node-3");

        // Make node-2 Warning
        manager.record_heartbeat_failure("node-2", "timeout".to_string());
        manager.record_heartbeat_failure("node-2", "timeout".to_string());

        // Make node-3 Offline
        for _ in 0..10 {
            manager.record_heartbeat_failure("node-3", "timeout".to_string());
        }

        let summary = manager.get_health_summary();
        assert_eq!(summary.total_nodes, 3);
        assert_eq!(summary.healthy, 1);
        assert_eq!(summary.warning, 1);
        assert_eq!(summary.critical, 0);
        assert_eq!(summary.offline, 1);
        assert_eq!(summary.nodes_for_removal.len(), 1);

        assert!(!summary.is_healthy());
        assert!(summary.needs_attention());
    }
}
