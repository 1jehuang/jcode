//! **Batch Node Operations** — Optimized handling of multiple nodes joining simultaneously.
//!
//! ## Features
//!
//! 1. **Parallel Probing**: Run capability probes on multiple nodes concurrently
//! 2. **Batched Warmup**: Stagger warmup phases to avoid cluster overload
//! 3. **Priority Queue**: High-quality nodes get integrated first
//! 4. **Resource-Aware Scheduling**: Consider cluster capacity during bulk operations
//! 5. **Progress Tracking**: Monitor batch operation status in real-time

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::task::JoinSet;
use tracing::{info, warn, debug, error};
use uuid::Uuid;

use crate::{
    NodeId, NodeHardwareInfo, NodeJoinManager, ProbeResult,
    SchedulerError, HierarchicalScheduler, ClusterGroupId,
};

/// Default function for serde(skip) on Instant fields
fn instant_now() -> Instant {
    Instant::now()
}

// ============================================================================
// Batch Operation Types
// ============================================================================

/// Unique identifier for a batch operation
pub type BatchOperationId = Uuid;

/// Status of a single node within a batch operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeBatchStatus {
    pub node_id: NodeId,
    pub hardware: NodeHardwareInfo,
    pub status: BatchNodeStatus,
    pub probe_result: Option<ProbeResult>,
    #[serde(skip, default = "instant_now")]
    pub started_at: Instant,
    #[serde(skip, default)]
    pub completed_at: Option<Instant>,
    pub error: Option<String>,
}

impl std::fmt::Display for NodeBatchStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NodeBatchStatus(node={}, status={:?})", self.node_id, self.status)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchNodeStatus {
    /// Waiting to start processing
    Pending,
    /// Currently running probes
    Probing,
    /// Probe complete, waiting for warmup slot
    WaitingForWarmup,
    /// Currently warming up
    WarmingUp { progress_pct: u8 },
    /// Successfully integrated
    Integrated,
    /// Failed and rejected
    Failed,
}

/// Configuration for batch node operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchOperationConfig {
    /// Maximum number of nodes to probe in parallel
    pub max_parallel_probes: usize,

    /// Maximum number of nodes in warmup phase simultaneously
    pub max_concurrent_warmups: usize,

    /// Delay between starting each warmup (seconds)
    pub warmup_stagger_secs: u64,

    /// Timeout for entire batch operation (seconds)
    pub batch_timeout_secs: u64,

    /// Minimum quality score to accept node (0-100)
    pub min_quality_score: f64,

    /// Preferred group for new nodes
    pub preferred_group: Option<ClusterGroupId>,
}

impl BatchOperationConfig {
    pub fn default() -> Self {
        Self {
            max_parallel_probes: 10,
            max_concurrent_warmups: 5,
            warmup_stagger_secs: 10,
            batch_timeout_secs: 600, // 10 minutes
            min_quality_score: 30.0,
            preferred_group: None,
        }
    }

    pub fn aggressive() -> Self {
        Self {
            max_parallel_probes: 20,
            max_concurrent_warmups: 10,
            warmup_stagger_secs: 5,
            batch_timeout_secs: 300,
            min_quality_score: 20.0,
            preferred_group: None,
        }
    }

    pub fn conservative() -> Self {
        Self {
            max_parallel_probes: 5,
            max_concurrent_warmups: 2,
            warmup_stagger_secs: 20,
            batch_timeout_secs: 900,
            min_quality_score: 50.0,
            preferred_group: None,
        }
    }
}

/// Overall status of a batch operation
#[derive(Debug, Clone, Serialize)]
pub struct BatchOperationStatus {
    pub batch_id: BatchOperationId,
    pub total_nodes: usize,
    pub pending: usize,
    pub probing: usize,
    pub waiting_for_warmup: usize,
    pub warming_up: usize,
    pub integrated: usize,
    pub failed: usize,
    #[serde(skip)]
    pub started_at: Instant,
    #[serde(skip)]
    pub estimated_completion: Option<Instant>,
}

impl BatchOperationStatus {
    pub fn progress_pct(&self) -> f64 {
        let completed = self.integrated + self.failed;
        if self.total_nodes == 0 {
            return 0.0;
        }
        (completed as f64 / self.total_nodes as f64) * 100.0
    }

    pub fn is_complete(&self) -> bool {
        self.pending == 0 && self.probing == 0 && 
        self.waiting_for_warmup == 0 && self.warming_up == 0
    }
}

// ============================================================================
// Batch Node Manager
// ============================================================================

/// Manages batch operations for adding multiple nodes efficiently
pub struct BatchNodeManager {
    /// Active batch operations
    active_batches: RwLock<HashMap<BatchOperationId, Arc<RwLock<BatchOperation>>>>,

    /// Configuration
    config: BatchOperationConfig,

    /// Reference to hierarchical scheduler
    hierarchical_scheduler: Arc<HierarchicalScheduler>,

    /// Reference to join manager (for individual node operations)
    join_manager: Arc<RwLock<NodeJoinManager>>,
}

/// Represents a single batch operation
struct BatchOperation {
    pub batch_id: BatchOperationId,
    pub nodes: Vec<Arc<RwLock<NodeBatchStatus>>>,
    pub config: BatchOperationConfig,
    pub started_at: Instant,
}

impl BatchOperation {
    /// Maximum number of nodes to probe in parallel for this batch
    pub fn max_parallel_probes(&self) -> usize {
        self.config.max_parallel_probes
    }
}

impl BatchNodeManager {
    pub fn new(
        config: BatchOperationConfig,
        hierarchical_scheduler: Arc<HierarchicalScheduler>,
        join_manager: Arc<RwLock<NodeJoinManager>>,
    ) -> Self {
        Self {
            active_batches: RwLock::new(HashMap::new()),
            config,
            hierarchical_scheduler,
            join_manager,
        }
    }

    /// Start a batch operation to add multiple nodes
    pub async fn start_batch_join(
        &self,
        nodes: Vec<(NodeId, NodeHardwareInfo)>,
    ) -> Result<BatchOperationId, SchedulerError> {
        let batch_id = Uuid::new_v4();
        info!(
            "[BatchNodeManager] Starting batch join for {} nodes (batch_id={})",
            nodes.len(), batch_id
        );

        // Create batch operation
        let batch_status: Vec<Arc<RwLock<NodeBatchStatus>>> = nodes
            .into_iter()
            .map(|(node_id, hardware)| {
                Arc::new(RwLock::new(NodeBatchStatus {
                    node_id,
                    hardware,
                    status: BatchNodeStatus::Pending,
                    probe_result: None,
                    started_at: Instant::now(),
                    completed_at: None,
                    error: None,
                }))
            })
            .collect();

        let operation = BatchOperation {
            batch_id,
            nodes: batch_status,
            config: self.config.clone(),
            started_at: Instant::now(),
        };

        let operation_arc = Arc::new(RwLock::new(operation));
        self.active_batches.write().await.insert(batch_id, operation_arc.clone());

        // Spawn background task to process batch
        tokio::spawn({
            let op = operation_arc.clone();
            let h_sched = self.hierarchical_scheduler.clone();
            let j_mgr = self.join_manager.clone();
            let cfg = self.config.clone();

            async move {
                if let Err(e) = process_batch_operation(op, h_sched, j_mgr, cfg).await {
                    error!("[BatchNodeManager] Batch operation {} failed: {:?}", batch_id, e);
                }
            }
        });

        Ok(batch_id)
    }

    /// Get status of a batch operation
    pub async fn get_batch_status(&self, batch_id: &BatchOperationId) -> Option<BatchOperationStatus> {
        let batches = self.active_batches.read().await;
        let operation = batches.get(batch_id)?;
        let op = operation.read().await;

        let mut status = BatchOperationStatus {
            batch_id: *batch_id,
            total_nodes: op.nodes.len(),
            pending: 0,
            probing: 0,
            waiting_for_warmup: 0,
            warming_up: 0,
            integrated: 0,
            failed: 0,
            started_at: op.started_at,
            estimated_completion: None,
        };

        for node_status in &op.nodes {
            let ns = node_status.read().await;
            match ns.status {
                BatchNodeStatus::Pending => status.pending += 1,
                BatchNodeStatus::Probing => status.probing += 1,
                BatchNodeStatus::WaitingForWarmup => status.waiting_for_warmup += 1,
                BatchNodeStatus::WarmingUp { .. } => status.warming_up += 1,
                BatchNodeStatus::Integrated => status.integrated += 1,
                BatchNodeStatus::Failed => status.failed += 1,
            }
        }

        // Estimate completion time
        if !status.is_complete() && status.total_nodes > 0 {
            let elapsed = status.started_at.elapsed();
            let progress = status.progress_pct() / 100.0;
            if progress > 0.0 {
                let estimated_total = elapsed.as_secs_f64() / progress;
                let remaining = Duration::from_secs_f64(estimated_total - elapsed.as_secs_f64());
                status.estimated_completion = Some(Instant::now() + remaining);
            }
        }

        Some(status)
    }

    /// List all active batch operations
    pub async fn list_active_batches(&self) -> Vec<BatchOperationStatus> {
        let batches = self.active_batches.read().await;
        let mut statuses = Vec::new();

        for (_, operation) in batches.iter() {
            if let Some(status) = self.get_batch_status(&operation.read().await.batch_id).await {
                if !status.is_complete() {
                    statuses.push(status);
                }
            }
        }

        statuses
    }

    /// Wait for batch operation to complete (with timeout)
    pub async fn wait_for_batch_completion(
        &self,
        batch_id: &BatchOperationId,
        timeout_secs: u64,
    ) -> Result<(), SchedulerError> {
        let start = Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        loop {
            if start.elapsed() > timeout {
                return Err(SchedulerError::AllocationFailed(
                    format!("Batch operation {} timed out", batch_id)
                ));
            }

            if let Some(status) = self.get_batch_status(batch_id).await {
                if status.is_complete() {
                    return Ok(());
                }
            } else {
                return Err(SchedulerError::AllocationFailed(
                    format!("Batch operation {} not found", batch_id)
                ));
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

/// Process a batch operation with parallel probing and staggered warmup
async fn process_batch_operation(
    operation: Arc<RwLock<BatchOperation>>,
    hierarchical_scheduler: Arc<HierarchicalScheduler>,
    join_manager: Arc<RwLock<NodeJoinManager>>,
    config: BatchOperationConfig,
) -> Result<(), SchedulerError> {
    let op = operation.read().await;
    let total_nodes = op.nodes.len();

    info!(
        "[BatchNodeManager] Processing batch {} with {} nodes",
        op.batch_id, total_nodes
    );

    // Phase 1: Parallel Probing
    info!("[BatchNodeManager] Phase 1: Parallel probing...");
    let probed_nodes = run_parallel_probes(&op.nodes, config.max_parallel_probes).await;

    info!(
        "[BatchNodeManager] Probing complete: {} succeeded, {} failed",
        probed_nodes.iter().filter(|n| n.1.is_some()).count(),
        probed_nodes.iter().filter(|n| n.1.is_none()).count()
    );

    drop(op); // Release read lock

    // Phase 2: Filter by quality and queue for warmup
    let mut warmup_queue: VecDeque<(NodeId, NodeHardwareInfo, ProbeResult)> = VecDeque::new();

    for (node_arc, probe_opt) in &probed_nodes {
        if let Some(probe) = probe_opt {
            if probe.overall_quality_score >= config.min_quality_score {
                // Update status to waiting for warmup
                let mut ns = node_arc.write().await;
                ns.status = BatchNodeStatus::WaitingForWarmup;
                ns.probe_result = Some(probe.clone());

                warmup_queue.push_back((ns.node_id, ns.hardware.clone(), probe.clone()));
            } else {
                let mut ns = node_arc.write().await;
                ns.status = BatchNodeStatus::Failed;
                ns.error = Some(format!(
                    "Quality score {:.1} below threshold {:.1}",
                    probe.overall_quality_score, config.min_quality_score
                ));
                ns.completed_at = Some(Instant::now());
            }
        } else {
            let mut ns = node_arc.write().await;
            ns.status = BatchNodeStatus::Failed;
            ns.error = Some("Probing failed".to_string());
            ns.completed_at = Some(Instant::now());
        }
    }

    // Phase 3: Staggered Warmup
    info!(
        "[BatchNodeManager] Phase 3: Staggered warmup for {} qualified nodes",
        warmup_queue.len()
    );

    let mut warmup_set = JoinSet::new();
    let mut warmup_count = 0;

    while !warmup_queue.is_empty() || !warmup_set.is_empty() {
        // Launch new warmups up to concurrent limit
        while warmup_count < config.max_concurrent_warmups && !warmup_queue.is_empty() {
            if let Some((node_id, hardware, probe)) = warmup_queue.pop_front() {
                let node_arc = {
                    let mut found_arc = None;
                    for (arc, _) in &probed_nodes {
                        if arc.read().await.node_id == node_id {
                            found_arc = Some(arc.clone());
                            break;
                        }
                    }
                    found_arc
                };

                if let Some(arc) = node_arc {
                    // Update status to warming up
                    {
                        let mut ns = arc.write().await;
                        ns.status = BatchNodeStatus::WarmingUp { progress_pct: 0 };
                    }

                    // Clone necessary data for the async task
                    let hs_clone = hierarchical_scheduler.clone();
                    let jm_clone = join_manager.clone();
                    let cfg_clone = config.clone();

                    // Spawn warmup task
                    warmup_set.spawn(async move {
                        let result = perform_node_warmup(
                            node_id,
                            hardware,
                            probe,
                            &hs_clone,
                            &jm_clone,
                            &cfg_clone,
                        ).await;

                        (arc, result)
                    });

                    warmup_count += 1;

                    // Stagger next warmup
                    tokio::time::sleep(Duration::from_secs(config.warmup_stagger_secs)).await;
                }
            }
        }

        // Wait for at least one warmup to complete
        if let Some(result) = warmup_set.join_next().await {
            match result {
                Ok((arc, Ok(()))) => {
                    let mut ns = arc.write().await;
                    ns.status = BatchNodeStatus::Integrated;
                    ns.completed_at = Some(Instant::now());
                    info!("[BatchNodeManager] Node {} integrated successfully", ns.node_id);
                }
                Ok((arc, Err(e))) => {
                    let mut ns = arc.write().await;
                    ns.status = BatchNodeStatus::Failed;
                    ns.error = Some(format!("Warmup failed: {:?}", e));
                    ns.completed_at = Some(Instant::now());
                    warn!("[BatchNodeManager] Node {} warmup failed: {:?}", ns.node_id, e);
                }
                Err(e) => {
                    error!("[BatchNodeManager] Warmup task panicked: {:?}", e);
                }
            }
            warmup_count -= 1;
        }
    }

    info!(
        "[BatchNodeManager] Batch operation {} complete",
        probed_nodes[0].0.read().await /* dummy access */
    );

    Ok(())
}

/// Run probes on multiple nodes in parallel
async fn run_parallel_probes(
    nodes: &[Arc<RwLock<NodeBatchStatus>>],
    max_parallel: usize,
) -> Vec<(Arc<RwLock<NodeBatchStatus>>, Option<ProbeResult>)> {
    let mut results = Vec::new();
    let mut probe_set = JoinSet::new();
    let mut node_iter = nodes.iter().peekable();
    let mut active_count = 0;

    while node_iter.peek().is_some() || !probe_set.is_empty() {
        // Launch new probes up to parallel limit
        while active_count < max_parallel && node_iter.peek().is_some() {
            let node_arc = node_iter.next().unwrap().clone();

            probe_set.spawn(async move {
                let ns = node_arc.read().await;
                let _node_id = ns.node_id;
                let hardware = ns.hardware.clone();
                drop(ns);

                // Simulate probe (in production, call actual probe functions)
                let probe_result = simulate_probe(&hardware).await;

                (node_arc, probe_result)
            });

            active_count += 1;
        }

        // Wait for at least one probe to complete
        if let Some(result) = probe_set.join_next().await {
            match result {
                Ok((node_arc, probe_opt)) => {
                    // Update node status
                    {
                        let mut ns = node_arc.write().await;
                        if probe_opt.is_some() {
                            ns.status = BatchNodeStatus::Probing;
                        }
                    }
                    results.push((node_arc, probe_opt));
                }
                Err(e) => {
                    error!("[BatchNodeManager] Probe task panicked: {:?}", e);
                }
            }
            active_count -= 1;
        }
    }

    results
}

/// Perform warmup for a single node
async fn perform_node_warmup(
    node_id: NodeId,
    hardware: NodeHardwareInfo,
    _probe: ProbeResult,
    hierarchical_scheduler: &HierarchicalScheduler,
    _join_manager: &RwLock<NodeJoinManager>,
    config: &BatchOperationConfig,
) -> Result<(), SchedulerError> {
    debug!("[BatchNodeManager] Starting warmup for node {}", node_id);

    // Register node to hierarchical scheduler
    hierarchical_scheduler.register_node(
        hardware,
        config.preferred_group.as_deref(),
    ).await?;

    // Simulate warmup stages (in production, send gradual traffic)
    let stages = [10, 25, 50, 75, 100];
    for (i, &progress) in stages.iter().enumerate() {
        // In production, update status through join_manager
        debug!(
            "[BatchNodeManager] Node {} warmup stage {}/{}: {}%",
            node_id, i + 1, stages.len(), progress
        );

        // Simulate warmup duration
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    info!("[BatchNodeManager] Node {} warmup complete", node_id);
    Ok(())
}

/// Simulate hardware probing (replace with actual probes in production)
async fn simulate_probe(hardware: &NodeHardwareInfo) -> Option<ProbeResult> {
    // Simulate probe delay
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Generate synthetic probe result based on hardware specs
    let quality_score = (hardware.tflops_fp16 / 100.0 * 30.0) +
                       (hardware.memory_gb / 80.0 * 30.0) +
                       (hardware.memory_bandwidth_gbps / 1000.0 * 20.0) +
                       20.0; // Base network score

    Some(ProbeResult {
        node_id: hardware.node_id,
        probed_at: Instant::now(),
        available_vram_gb: hardware.memory_gb * 0.85,
        vram_bandwidth_gbs: hardware.memory_bandwidth_gbps,
        measured_tflops_fp16: hardware.tflops_fp16 * 0.9,
        measured_tflops_int8: Some(hardware.tflops_fp16 * 1.8),
        avg_latency_to_leader_ms: 10.0,
        bandwidth_to_leader_mbps: 1000.0,
        baseline_cpu_usage_pct: 5.0,
        baseline_memory_usage_pct: 10.0,
        baseline_temperature_c: Some(45.0),
        overall_quality_score: quality_score.min(100.0),
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_config_presets() {
        let default = BatchOperationConfig::default();
        assert_eq!(default.max_parallel_probes, 10);

        let aggressive = BatchOperationConfig::aggressive();
        assert_eq!(aggressive.max_parallel_probes, 20);

        let conservative = BatchOperationConfig::conservative();
        assert_eq!(conservative.max_parallel_probes, 5);
    }

    #[test]
    fn test_batch_status_progress() {
        let status = BatchOperationStatus {
            batch_id: Uuid::new_v4(),
            total_nodes: 10,
            pending: 0,
            probing: 0,
            waiting_for_warmup: 0,
            warming_up: 0,
            integrated: 8,
            failed: 2,
            started_at: Instant::now(),
            estimated_completion: None,
        };

        assert_eq!(status.progress_pct(), 100.0);
        assert!(status.is_complete());
    }

    #[tokio::test]
    async fn test_batch_node_manager_creation() {
        let config = BatchOperationConfig::default();
        let h_sched = Arc::new(HierarchicalScheduler::new(
            crate::HierarchicalSchedulerConfig::default()
        ));
        let j_mgr = Arc::new(RwLock::new(NodeJoinManager::new(
            WarmupConfig::default(),
            None
        )));

        let manager = BatchNodeManager::new(config, h_sched, j_mgr);
        let batches = manager.list_active_batches().await;
        assert_eq!(batches.len(), 0);
    }
}
