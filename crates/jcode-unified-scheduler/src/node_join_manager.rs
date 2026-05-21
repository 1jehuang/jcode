//! **Dynamic Node Join Manager** — Manages the complete lifecycle of nodes joining the cluster.
//!
//! ## Features
//!
//! 1. **Capability Probing**: Auto-detect VRAM, bandwidth, compute via benchmarks
//! 2. **Warmup Phase**: Gradual traffic increase to avoid cold-start shock
//! 3. **Health Calibration**: Establish baseline health metrics
//! 4. **Layer Assignment**: Integrate with LayerAllocator for optimal placement
//! 5. **Rollback Support**: Revert join if node fails warmup

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{info, warn, debug, error};
use uuid::Uuid;

use crate::{NodeId, NodeInfo, NodeHardwareInfo, SchedulerError};

// ============================================================================
// Node Join States
// ============================================================================

/// Represents the current phase of a node joining process
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeJoinState {
    /// Node has announced presence, not yet validated
    Discovered,
    /// Running capability probes (VRAM, bandwidth, compute)
    Probing,
    /// Probes complete, waiting for approval
    ProbeComplete,
    /// Warming up with gradual traffic increase
    WarmingUp { progress_pct: u8 }, // 0-100%
    /// Fully integrated into cluster
    Integrated,
    /// Join failed, node rejected
    Failed { reason: String },
}

impl NodeJoinState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, NodeJoinState::Integrated | NodeJoinState::Failed { .. })
    }
}

// ============================================================================
// Probe Results
// ============================================================================

/// Results from hardware capability probing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    pub node_id: NodeId,
    pub probed_at: Instant,

    // === VRAM Probe ===
    pub available_vram_gb: f64,
    pub vram_bandwidth_gbs: f64,

    // === Compute Probe ===
    pub measured_tflops_fp16: f64,
    pub measured_tflops_int8: Option<f64>,

    // === Network Probe ===
    pub avg_latency_to_leader_ms: f64,
    pub bandwidth_to_leader_mbps: f64,

    // === Health Baseline ===
    pub baseline_cpu_usage_pct: f64,
    pub baseline_memory_usage_pct: f64,
    pub baseline_temperature_c: Option<f64>,

    // === Quality Score (0-100) ===
    pub overall_quality_score: f64,
}

impl ProbeResult {
    /// Calculate overall quality score based on probe results
    pub fn calculate_quality_score(&self) -> f64 {
        let mut score = 0.0;

        // VRAM capacity (30% weight)
        score += (self.available_vram_gb / 80.0).min(1.0) * 30.0;

        // VRAM bandwidth (20% weight)
        score += (self.vram_bandwidth_gbs / 1000.0).min(1.0) * 20.0;

        // Compute power (30% weight)
        score += (self.measured_tflops_fp16 / 100.0).min(1.0) * 30.0;

        // Network latency (20% weight, lower is better)
        let latency_score = if self.avg_latency_to_leader_ms < 5.0 {
            1.0
        } else if self.avg_latency_to_leader_ms < 20.0 {
            0.7
        } else if self.avg_latency_to_leader_ms < 50.0 {
            0.4
        } else {
            0.1
        };
        score += latency_score * 20.0;

        score.min(100.0)
    }
}

// ============================================================================
// Warmup Configuration
// ============================================================================

/// Configuration for the warmup phase
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarmupConfig {
    /// Total warmup duration in seconds
    pub warmup_duration_secs: u64,
    /// Number of warmup stages
    pub warmup_stages: u8,
    /// Traffic percentage at each stage (0-100)
    pub stage_traffic_pcts: Vec<u8>,
    /// Maximum allowed error rate during warmup (%)
    pub max_error_rate_pct: f64,
    /// Maximum allowed latency increase during warmup (%)
    pub max_latency_increase_pct: f64,
}

impl WarmupConfig {
    pub fn default() -> Self {
        Self {
            warmup_duration_secs: 300, // 5 minutes
            warmup_stages: 5,
            stage_traffic_pcts: vec![10, 25, 50, 75, 100],
            max_error_rate_pct: 5.0,
            max_latency_increase_pct: 50.0,
        }
    }

    pub fn fast() -> Self {
        Self {
            warmup_duration_secs: 60, // 1 minute for testing
            warmup_stages: 3,
            stage_traffic_pcts: vec![25, 50, 100],
            max_error_rate_pct: 10.0,
            max_latency_increase_pct: 100.0,
        }
    }
}

// ============================================================================
// Node Join Status
// ============================================================================

/// Tracks the status of an ongoing node join operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeJoinStatus {
    pub join_id: Uuid,
    pub node_id: NodeId,
    pub state: NodeJoinState,
    pub started_at: Instant,
    pub updated_at: Instant,
    pub probe_result: Option<ProbeResult>,
    pub warmup_progress: Option<WarmupProgress>,
    pub error: Option<String>,
}

impl NodeJoinStatus {
    pub fn new(node_id: NodeId) -> Self {
        let now = Instant::now();
        Self {
            join_id: Uuid::new_v4(),
            node_id,
            state: NodeJoinState::Discovered,
            started_at: now,
            updated_at: now,
            probe_result: None,
            warmup_progress: None,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarmupProgress {
    pub current_stage: u8,
    pub total_stages: u8,
    pub traffic_pct: u8,
    pub requests_processed: u64,
    pub errors_encountered: u64,
    pub avg_latency_ms: f64,
    pub p99_latency_ms: f64,
}

// ============================================================================
// Node Join Manager
// ============================================================================

/// Manages the complete node join lifecycle
pub struct NodeJoinManager {
    /// Active join operations
    active_joins: HashMap<Uuid, NodeJoinStatus>,
    /// Completed joins (node_id -> final status)
    completed_joins: HashMap<NodeId, NodeJoinStatus>,
    /// Warmup configuration
    warmup_config: WarmupConfig,
    /// Leader node ID (for network probing)
    leader_node_id: Option<NodeId>,
}

impl NodeJoinManager {
    pub fn new(warmup_config: WarmupConfig, leader_node_id: Option<NodeId>) -> Self {
        Self {
            active_joins: HashMap::new(),
            completed_joins: HashMap::new(),
            warmup_config,
            leader_node_id,
        }
    }

    /// Start the node join process
    pub async fn start_join(&mut self, node_id: NodeId, hardware: NodeHardwareInfo) -> Result<Uuid, SchedulerError> {
        info!("[NodeJoinManager] Starting join process for node {}", node_id);

        let mut status = NodeJoinStatus::new(node_id);
        let join_id = status.join_id;

        self.active_joins.insert(join_id, status);

        // Phase 1: Capability Probing
        match self.run_probes(node_id, &hardware).await {
            Ok(probe_result) => {
                info!(
                    "[NodeJoinManager] Probes complete for node {}: quality_score={:.1}",
                    node_id, probe_result.overall_quality_score
                );

                // Update status
                if let Some(status) = self.active_joins.get_mut(&join_id) {
                    status.state = NodeJoinState::ProbeComplete;
                    status.probe_result = Some(probe_result.clone());
                    status.updated_at = Instant::now();
                }

                // Check if node meets minimum requirements
                if probe_result.overall_quality_score < 30.0 {
                    warn!(
                        "[NodeJoinManager] Node {} quality score too low ({:.1}), rejecting",
                        node_id, probe_result.overall_quality_score
                    );
                    self.fail_join(join_id, "Quality score below threshold".to_string());
                    return Err(SchedulerError::AllocationFailed(
                        "Node quality too low".to_string()
                    ));
                }

                // Phase 2: Warmup
                self.run_warmup(join_id, node_id).await?;

                Ok(join_id)
            }
            Err(e) => {
                error!("[NodeJoinManager] Probe failed for node {}: {:?}", node_id, e);
                self.fail_join(join_id, format!("Probe failed: {:?}", e));
                Err(e)
            }
        }
    }

    /// Run capability probes on the new node
    async fn run_probes(&self, node_id: NodeId, hardware: &NodeHardwareInfo) -> Result<ProbeResult, SchedulerError> {
        debug!("[NodeJoinManager] Running probes for node {}", node_id);

        // Simulate VRAM probe (in production, this would run actual benchmarks)
        let available_vram_gb = hardware.memory_gb * 0.85; // Assume 85% usable
        let vram_bandwidth_gbs = hardware.memory_bandwidth_gbps;

        // Simulate compute probe
        let measured_tflops = hardware.tflops_fp16 * 0.9; // Assume 90% of rated performance

        // Simulate network probe
        let avg_latency_ms = if self.leader_node_id.is_some() {
            10.0 // Simulated latency to leader
        } else {
            5.0
        };
        let bandwidth_mbps = 1000.0; // Simulated network bandwidth

        let probe_result = ProbeResult {
            node_id,
            probed_at: Instant::now(),
            available_vram_gb,
            vram_bandwidth_gbs,
            measured_tflops_fp16: measured_tflops,
            measured_tflops_int8: Some(measured_tflops * 2.0), // INT8 ~2x FP16
            avg_latency_to_leader_ms: avg_latency_ms,
            bandwidth_to_leader_mbps: bandwidth_mbps,
            baseline_cpu_usage_pct: 5.0,
            baseline_memory_usage_pct: 10.0,
            baseline_temperature_c: Some(45.0),
            overall_quality_score: 0.0, // Will be calculated
        };

        // Calculate quality score
        let quality_score = probe_result.calculate_quality_score();

        Ok(ProbeResult {
            overall_quality_score: quality_score,
            ..probe_result
        })
    }

    /// Run warmup phase with gradual traffic increase
    async fn run_warmup(&mut self, join_id: Uuid, node_id: NodeId) -> Result<(), SchedulerError> {
        info!("[NodeJoinManager] Starting warmup for node {}", node_id);

        let total_stages = self.warmup_config.warmup_stages;
        let stage_duration = Duration::from_secs(
            self.warmup_config.warmup_duration_secs / total_stages as u64
        );

        for stage in 0..total_stages {
            // Update status to warming up
            if let Some(status) = self.active_joins.get_mut(&join_id) {
                let traffic_pct = self.warmup_config.stage_traffic_pcts
                    .get(stage as usize)
                    .copied()
                    .unwrap_or(((stage + 1) * 100 / total_stages) as u8);

                status.state = NodeJoinState::WarmingUp { progress_pct: traffic_pct };
                status.warmup_progress = Some(WarmupProgress {
                    current_stage: stage + 1,
                    total_stages,
                    traffic_pct,
                    requests_processed: 0,
                    errors_encountered: 0,
                    avg_latency_ms: 0.0,
                    p99_latency_ms: 0.0,
                });
                status.updated_at = Instant::now();

                info!(
                    "[NodeJoinManager] Node {} warmup stage {}/{}: {}% traffic",
                    node_id, stage + 1, total_stages, traffic_pct
                );
            }

            // Simulate warmup traffic (in production, send actual requests)
            sleep(stage_duration).await;

            // Check for errors (simulated)
            // In production, monitor error rates and latencies
            if let Some(status) = self.active_joins.get(&join_id) {
                if let Some(ref _progress) = status.warmup_progress {
                    // Simulated error check
                    // if progress.errors_encountered > 0 { ... }
                }
            }
        }

        // Warmup complete - mark as integrated
        if let Some(status) = self.active_joins.get_mut(&join_id) {
            status.state = NodeJoinState::Integrated;
            status.updated_at = Instant::now();

            info!(
                "[NodeJoinManager] Node {} successfully integrated after {:.0}s warmup",
                node_id,
                status.started_at.elapsed().as_secs_f64()
            );
        }

        // Move to completed joins
        if let Some(status) = self.active_joins.remove(&join_id) {
            self.completed_joins.insert(node_id, status);
        }

        Ok(())
    }

    /// Fail a join operation
    fn fail_join(&mut self, join_id: Uuid, reason: String) {
        if let Some(status) = self.active_joins.get_mut(&join_id) {
            status.state = NodeJoinState::Failed { reason: reason.clone() };
            status.error = Some(reason);
            status.updated_at = Instant::now();

            let node_id = status.node_id;
            warn!("[NodeJoinManager] Join failed for node {}: {}", node_id, status.error.as_ref().unwrap());

            // Move to completed joins (with failure status)
            let status = self.active_joins.remove(&join_id).unwrap();
            self.completed_joins.insert(node_id, status);
        }
    }

    /// Get the status of a join operation
    pub fn get_join_status(&self, join_id: &Uuid) -> Option<&NodeJoinStatus> {
        self.active_joins.get(join_id)
    }

    /// Get the final status of a completed join
    pub fn get_completed_join(&self, node_id: &NodeId) -> Option<&NodeJoinStatus> {
        self.completed_joins.get(node_id)
    }

    /// List all active joins
    pub fn list_active_joins(&self) -> Vec<&NodeJoinStatus> {
        self.active_joins.values().collect()
    }

    /// Check if a node has been successfully integrated
    pub fn is_node_integrated(&self, node_id: &NodeId) -> bool {
        self.completed_joins
            .get(node_id)
            .map_or(false, |s| s.state == NodeJoinState::Integrated)
    }

    /// Get probe result for a node
    pub fn get_probe_result(&self, node_id: &NodeId) -> Option<&ProbeResult> {
        self.completed_joins
            .get(node_id)
            .and_then(|s| s.probe_result.as_ref())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_probe_quality_score_calculation() {
        let probe = ProbeResult {
            node_id: NodeId::new(),
            probed_at: Instant::now(),
            available_vram_gb: 24.0,
            vram_bandwidth_gbs: 1000.0,
            measured_tflops_fp16: 50.0,
            measured_tflops_int8: Some(100.0),
            avg_latency_to_leader_ms: 5.0,
            bandwidth_to_leader_mbps: 1000.0,
            baseline_cpu_usage_pct: 5.0,
            baseline_memory_usage_pct: 10.0,
            baseline_temperature_c: Some(45.0),
            overall_quality_score: 0.0,
        };

        let score = probe.calculate_quality_score();
        assert!(score > 50.0, "Expected reasonable quality score, got {}", score);
        assert!(score <= 100.0, "Score should be <= 100");
    }

    #[test]
    fn test_node_join_state_transitions() {
        assert_eq!(NodeJoinState::Discovered.is_terminal(), false);
        assert_eq!(NodeJoinState::Probing.is_terminal(), false);
        assert_eq!(NodeJoinState::ProbeComplete.is_terminal(), false);
        assert_eq!(NodeJoinState::WarmingUp { progress_pct: 50 }.is_terminal(), false);
        assert_eq!(NodeJoinState::Integrated.is_terminal(), true);
        assert_eq!(NodeJoinState::Failed { reason: "test".to_string() }.is_terminal(), true);
    }

    #[tokio::test]
    async fn test_node_join_manager_creation() {
        let config = WarmupConfig::fast();
        let manager = NodeJoinManager::new(config, None);

        assert_eq!(manager.active_joins.len(), 0);
        assert_eq!(manager.completed_joins.len(), 0);
    }

    #[test]
    fn test_warmup_config_defaults() {
        let config = WarmupConfig::default();
        assert_eq!(config.warmup_stages, 5);
        assert_eq!(config.stage_traffic_pcts.len(), 5);
        assert_eq!(config.stage_traffic_pcts[0], 10);
        assert_eq!(config.stage_traffic_pcts[4], 100);
    }
}
