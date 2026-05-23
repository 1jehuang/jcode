//! **Fine-grained Resource Tracker** �?Tracks VRAM, memory bandwidth, and compute utilization at a granular level.
//!
//! ## Features
//!
//! 1. **VRAM Accounting**: Track model weights, KV cache, and activation memory per task
//! 2. **Bandwidth Reservation**: Reserve network/memory bandwidth for data transfer
//! 3. **Compute Quotas**: Allocate TFLOPS budgets to prevent resource monopolization
//! 4. **Multi-tenant Isolation**: Ensure fair sharing across concurrent tasks

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::NodeId;

// ============================================================================
// Resource Types
// ============================================================================

/// Unique identifier for a resource allocation
pub type AllocationId = Uuid;

/// Represents a specific resource requirement for a task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceRequirement {
    /// Required VRAM in GB (for model weights + KV cache + activations)
    pub vram_gb: f64,
    /// Required memory bandwidth in GB/s
    pub memory_bandwidth_gbs: f64,
    /// Required compute in TFLOPS
    pub compute_tflops: f64,
    /// Estimated duration of resource usage
    pub estimated_duration_ms: u64,
}

impl ResourceRequirement {
    pub fn new(vram_gb: f64, memory_bandwidth_gbs: f64, compute_tflops: f64, estimated_duration_ms: u64) -> Self {
        Self {
            vram_gb,
            memory_bandwidth_gbs,
            compute_tflops,
            estimated_duration_ms,
        }
    }

    /// Estimate VRAM requirements for a transformer model
    ///
    /// Formula:
    /// - Model weights: params * 2 bytes (FP16) / 1e9 = GB
    /// - KV Cache: batch_size * seq_len * num_layers * hidden_size * 2 * 2 bytes / 1e9
    /// - Activations: ~20% of model weights
    pub fn estimate_for_transformer(
        params_billions: f64,
        num_layers: u32,
        hidden_size: u32,
        batch_size: u32,
        seq_len: u32,
        estimated_duration_ms: u64,
    ) -> Self {
        // Model weights (FP16 = 2 bytes per param)
        let model_weights_gb = params_billions * 2.0 / 1e3; // billions * 2 bytes / 1024

        // KV Cache (key + value for each layer)
        // Each token needs: num_layers * hidden_size * 2 (k+v) * 2 bytes (FP16)
        let kv_cache_gb = (num_layers as f64 * hidden_size as f64 * batch_size as f64 * seq_len as f64 * 2.0 * 2.0) / 1e9;

        // Activations (~20% of model weights during inference)
        let activations_gb = model_weights_gb * 0.2;

        let total_vram = model_weights_gb + kv_cache_gb + activations_gb;

        // Estimate memory bandwidth (assume full model read per forward pass)
        // For decoder: IO_bytes �?model_weights * 2 (read + write)
        let memory_bandwidth = model_weights_gb * 2.0 / (estimated_duration_ms as f64 / 1000.0);

        // Compute estimate (TFLOPS based on FLOPs formula for transformers)
        // FLOPs �?2 * params * seq_len * batch_size
        let flops = 2.0 * params_billions * 1e9 * seq_len as f64 * batch_size as f64;
        let compute_tflops = flops / 1e12 / (estimated_duration_ms as f64 / 1000.0);

        Self {
            vram_gb: total_vram,
            memory_bandwidth_gbs: memory_bandwidth,
            compute_tflops,
            estimated_duration_ms,
        }
    }
}

/// Represents an active resource allocation on a node
#[derive(Debug, Clone, Serialize)]
pub struct ResourceAllocation {
    pub allocation_id: AllocationId,
    pub node_id: NodeId,
    pub task_id: Option<Uuid>,
    pub requirement: ResourceRequirement,
    #[serde(skip)]
    pub allocated_at: Instant,
    #[serde(skip)]
    pub expires_at: Option<Instant>,
    pub status: AllocationStatus,
}

impl ResourceAllocation {
    pub fn new(node_id: NodeId, task_id: Option<Uuid>, requirement: ResourceRequirement) -> Self {
        let now = Instant::now();
        let estimated_duration_ms = requirement.estimated_duration_ms;
        Self {
            allocation_id: Uuid::new_v4(),
            node_id,
            task_id,
            requirement,
            allocated_at: now,
            expires_at: Some(now + Duration::from_millis(estimated_duration_ms)),
            status: AllocationStatus::Active,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at.map_or(false, |exp| Instant::now() >= exp)
    }

    pub fn remaining_time_ms(&self) -> u64 {
        self.expires_at
            .map(|exp| exp.saturating_duration_since(Instant::now()).as_millis() as u64)
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AllocationStatus {
    Active,
    Released,
    Expired,
}

// ============================================================================
// Node Resource State
// ============================================================================

/// Tracks the current resource state of a single node
#[derive(Debug, Clone)]
pub struct NodeResourceState {
    pub node_id: NodeId,

    // === VRAM Tracking ===
    pub total_vram_gb: f64,
    pub used_vram_gb: f64,
    pub reserved_vram_gb: f64,

    // === Memory Bandwidth Tracking ===
    pub total_memory_bandwidth_gbs: f64,
    pub used_memory_bandwidth_gbs: f64,
    pub reserved_memory_bandwidth_gbs: f64,

    // === Compute Tracking ===
    pub total_compute_tflops: f64,
    pub used_compute_tflops: f64,
    pub reserved_compute_tflops: f64,

    // === Active Allocations ===
    pub allocations: HashMap<AllocationId, ResourceAllocation>,
}

impl NodeResourceState {
    pub fn new(node_id: NodeId, total_vram_gb: f64, total_memory_bandwidth_gbs: f64, total_compute_tflops: f64) -> Self {
        Self {
            node_id,
            total_vram_gb,
            used_vram_gb: 0.0,
            reserved_vram_gb: 0.0,
            total_memory_bandwidth_gbs,
            used_memory_bandwidth_gbs: 0.0,
            reserved_memory_bandwidth_gbs: 0.0,
            total_compute_tflops,
            used_compute_tflops: 0.0,
            reserved_compute_tflops: 0.0,
            allocations: HashMap::new(),
        }
    }

    /// Check if the node has sufficient resources for a requirement
    pub fn can_allocate(&self, req: &ResourceRequirement) -> bool {
        let available_vram = self.total_vram_gb - self.used_vram_gb - self.reserved_vram_gb;
        let available_bw = self.total_memory_bandwidth_gbs - self.used_memory_bandwidth_gbs - self.reserved_memory_bandwidth_gbs;
        let available_compute = self.total_compute_tflops - self.used_compute_tflops - self.reserved_compute_tflops;

        available_vram >= req.vram_gb && available_bw >= req.memory_bandwidth_gbs && available_compute >= req.compute_tflops
    }

    /// Allocate resources for a task
    pub fn allocate(&mut self, task_id: Option<Uuid>, req: ResourceRequirement) -> Result<AllocationId, String> {
        if !self.can_allocate(&req) {
            return Err(format!(
                "Insufficient resources on node {}: VRAM={:.1}/{:.1} GB, BW={:.1}/{:.1} GB/s, Compute={:.1}/{:.1} TFLOPS",
                self.node_id,
                self.used_vram_gb + self.reserved_vram_gb,
                self.total_vram_gb,
                self.used_memory_bandwidth_gbs + self.reserved_memory_bandwidth_gbs,
                self.total_memory_bandwidth_gbs,
                self.used_compute_tflops + self.reserved_compute_tflops,
                self.total_compute_tflops
            ));
        }

        let alloc = ResourceAllocation::new(self.node_id, task_id, req.clone());
        let alloc_id = alloc.allocation_id;

        // Update usage counters
        self.used_vram_gb += req.vram_gb;
        self.used_memory_bandwidth_gbs += req.memory_bandwidth_gbs;
        self.used_compute_tflops += req.compute_tflops;

        // Store allocation
        self.allocations.insert(alloc_id, alloc);

        debug!(
            "Allocated resources on node {}: VRAM +{:.1} GB, BW +{:.1} GB/s, Compute +{:.1} TFLOPS",
            self.node_id, req.vram_gb, req.memory_bandwidth_gbs, req.compute_tflops
        );

        Ok(alloc_id)
    }

    /// Release previously allocated resources
    pub fn release(&mut self, alloc_id: &AllocationId) -> Result<(), String> {
        if let Some(alloc) = self.allocations.remove(alloc_id) {
            if alloc.status != AllocationStatus::Active {
                return Err("Allocation already released".to_string());
            }

            // Update usage counters
            self.used_vram_gb -= alloc.requirement.vram_gb;
            self.used_memory_bandwidth_gbs -= alloc.requirement.memory_bandwidth_gbs;
            self.used_compute_tflops -= alloc.requirement.compute_tflops;

            // Clamp to zero to avoid floating point errors
            self.used_vram_gb = self.used_vram_gb.max(0.0);
            self.used_memory_bandwidth_gbs = self.used_memory_bandwidth_gbs.max(0.0);
            self.used_compute_tflops = self.used_compute_tflops.max(0.0);

            debug!(
                "Released resources on node {}: VRAM -{:.1} GB, BW -{:.1} GB/s, Compute -{:.1} TFLOPS",
                self.node_id,
                alloc.requirement.vram_gb,
                alloc.requirement.memory_bandwidth_gbs,
                alloc.requirement.compute_tflops
            );

            Ok(())
        } else {
            Err("Allocation not found".to_string())
        }
    }

    /// Reserve resources (without immediate usage)
    pub fn reserve(&mut self, req: &ResourceRequirement) -> Result<(), String> {
        let available_vram = self.total_vram_gb - self.used_vram_gb - self.reserved_vram_gb;
        let available_bw = self.total_memory_bandwidth_gbs - self.used_memory_bandwidth_gbs - self.reserved_memory_bandwidth_gbs;
        let available_compute = self.total_compute_tflops - self.used_compute_tflops - self.reserved_compute_tflops;

        if available_vram < req.vram_gb || available_bw < req.memory_bandwidth_gbs || available_compute < req.compute_tflops {
            return Err("Insufficient resources for reservation".to_string());
        }

        self.reserved_vram_gb += req.vram_gb;
        self.reserved_memory_bandwidth_gbs += req.memory_bandwidth_gbs;
        self.reserved_compute_tflops += req.compute_tflops;

        Ok(())
    }

    /// Cancel a reservation
    pub fn unreserve(&mut self, req: &ResourceRequirement) {
        self.reserved_vram_gb = (self.reserved_vram_gb - req.vram_gb).max(0.0);
        self.reserved_memory_bandwidth_gbs = (self.reserved_memory_bandwidth_gbs - req.memory_bandwidth_gbs).max(0.0);
        self.reserved_compute_tflops = (self.reserved_compute_tflops - req.compute_tflops).max(0.0);
    }

    /// Get utilization ratios (0.0 - 1.0)
    pub fn utilization(&self) -> ResourceUtilization {
        ResourceUtilization {
            vram_ratio: self.used_vram_gb / self.total_vram_gb.max(0.001),
            memory_bw_ratio: self.used_memory_bandwidth_gbs / self.total_memory_bandwidth_gbs.max(0.001),
            compute_ratio: self.used_compute_tflops / self.total_compute_tflops.max(0.001),
        }
    }

    /// Clean up expired allocations
    pub fn cleanup_expired(&mut self) -> Vec<AllocationId> {
        let expired_ids: Vec<AllocationId> = self
            .allocations
            .iter()
            .filter(|(_, alloc)| alloc.is_expired())
            .map(|(id, _)| *id)
            .collect();

        for id in &expired_ids {
            if let Err(e) = self.release(id) {
                warn!("Failed to release expired allocation {:?}: {}", id, e);
            }
        }

        expired_ids
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ResourceUtilization {
    pub vram_ratio: f64,
    pub memory_bw_ratio: f64,
    pub compute_ratio: f64,
}

impl ResourceUtilization {
    pub fn is_overloaded(&self, threshold: f64) -> bool {
        self.vram_ratio > threshold || self.memory_bw_ratio > threshold || self.compute_ratio > threshold
    }
}

// ============================================================================
// Global Resource Manager
// ============================================================================

/// Manages fine-grained resources across all nodes in the cluster
#[derive(Debug)]
pub struct ResourceManager {
    node_states: HashMap<NodeId, NodeResourceState>,
    default_reservation_ratio: f64, // Percentage of resources reserved for system overhead
}

impl ResourceManager {
    pub fn new(default_reservation_ratio: f64) -> Self {
        Self {
            node_states: HashMap::new(),
            default_reservation_ratio,
        }
    }

    /// Register a new node with its hardware capabilities
    pub fn register_node(&mut self, node_id: NodeId, vram_gb: f64, memory_bandwidth_gbs: f64, compute_tflops: f64) {
        let reserved_vram = vram_gb * self.default_reservation_ratio;
        let reserved_bw = memory_bandwidth_gbs * self.default_reservation_ratio;
        let reserved_compute = compute_tflops * self.default_reservation_ratio;

        let mut state = NodeResourceState::new(node_id, vram_gb, memory_bandwidth_gbs, compute_tflops);
        state.reserved_vram_gb = reserved_vram;
        state.reserved_memory_bandwidth_gbs = reserved_bw;
        state.reserved_compute_tflops = reserved_compute;

        self.node_states.insert(node_id, state);

        debug!(
            "Registered node {} with VRAM={} GB, BW={} GB/s, Compute={} TFLOPS (reserved {:.0}%)",
            node_id,
            vram_gb,
            memory_bandwidth_gbs,
            compute_tflops,
            self.default_reservation_ratio * 100.0
        );
    }

    /// Unregister a node
    pub fn unregister_node(&mut self, node_id: &NodeId) -> Result<(), String> {
        if let Some(state) = self.node_states.remove(node_id) {
            if !state.allocations.is_empty() {
                warn!("Unregistering node {} with {} active allocations", node_id, state.allocations.len());
            }
            Ok(())
        } else {
            Err(format!("Node {} not found", node_id))
        }
    }

    /// Allocate resources on a specific node
    pub fn allocate_on_node(
        &mut self,
        node_id: &NodeId,
        task_id: Option<Uuid>,
        req: ResourceRequirement,
    ) -> Result<AllocationId, String> {
        if let Some(state) = self.node_states.get_mut(node_id) {
            state.allocate(task_id, req)
        } else {
            Err(format!("Node {} not found", node_id))
        }
    }

    /// Find the best node for a resource requirement (greedy by available VRAM)
    pub fn find_best_node(&self, req: &ResourceRequirement) -> Option<NodeId> {
        self.node_states
            .iter()
            .filter(|(_, state)| state.can_allocate(req))
            .max_by(|(_, a), (_, b)| {
                let util_a = a.utilization();
                let util_b = b.utilization();
                // Prefer nodes with lower utilization
                let score_a = util_a.vram_ratio + util_a.memory_bw_ratio + util_a.compute_ratio;
                let score_b = util_b.vram_ratio + util_b.memory_bw_ratio + util_b.compute_ratio;
                score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(id, _)| *id)
    }

    /// Release an allocation
    pub fn release_allocation(&mut self, node_id: &NodeId, alloc_id: &AllocationId) -> Result<(), String> {
        if let Some(state) = self.node_states.get_mut(node_id) {
            state.release(alloc_id)
        } else {
            Err(format!("Node {} not found", node_id))
        }
    }

    /// Get resource utilization for a node
    pub fn get_utilization(&self, node_id: &NodeId) -> Option<ResourceUtilization> {
        self.node_states.get(node_id).map(|state| state.utilization())
    }

    /// Get cluster-wide resource summary
    pub fn cluster_summary(&self) -> ClusterResourceSummary {
        let mut total_vram = 0.0;
        let mut used_vram = 0.0;
        let mut total_bw = 0.0;
        let mut used_bw = 0.0;
        let mut total_compute = 0.0;
        let mut used_compute = 0.0;

        for state in self.node_states.values() {
            total_vram += state.total_vram_gb;
            used_vram += state.used_vram_gb;
            total_bw += state.total_memory_bandwidth_gbs;
            used_bw += state.used_memory_bandwidth_gbs;
            total_compute += state.total_compute_tflops;
            used_compute += state.used_compute_tflops;
        }

        ClusterResourceSummary {
            total_nodes: self.node_states.len(),
            total_vram_gb: total_vram,
            used_vram_gb: used_vram,
            vram_utilization: if total_vram > 0.0 { used_vram / total_vram } else { 0.0 },
            total_memory_bandwidth_gbs: total_bw,
            used_memory_bandwidth_gbs: used_bw,
            total_compute_tflops: total_compute,
            used_compute_tflops: used_compute,
        }
    }

    /// Cleanup expired allocations across all nodes
    pub fn cleanup_all_expired(&mut self) -> usize {
        let mut total_cleaned = 0;
        for state in self.node_states.values_mut() {
            total_cleaned += state.cleanup_expired().len();
        }
        total_cleaned
    }
}

#[derive(Debug, Clone)]
pub struct ClusterResourceSummary {
    pub total_nodes: usize,
    pub total_vram_gb: f64,
    pub used_vram_gb: f64,
    pub vram_utilization: f64,
    pub total_memory_bandwidth_gbs: f64,
    pub used_memory_bandwidth_gbs: f64,
    pub total_compute_tflops: f64,
    pub used_compute_tflops: f64,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_requirement_estimation() {
        // Qwen-7B: 7B params, 32 layers, hidden_size=4096
        let req = ResourceRequirement::estimate_for_transformer(
            7.0,   // 7B params
            32,    // layers
            4096,  // hidden size
            1,     // batch size
            512,   // sequence length
        );

        assert!(req.vram_gb > 10.0, "Expected >10GB VRAM for 7B model");
        assert!(req.vram_gb < 20.0, "Expected <20GB VRAM for 7B model");
    }

    #[test]
    fn test_node_allocation_and_release() {
        let node_id = NodeId::new();
        let mut state = NodeResourceState::new(node_id, 24.0, 1000.0, 100.0);

        let req = ResourceRequirement::new(8.0, 200.0, 30.0, 1000);
        let alloc_id = state.allocate(None, req.clone()).unwrap();

        assert!((state.used_vram_gb - 8.0).abs() < 0.01);
        assert!((state.used_memory_bandwidth_gbs - 200.0).abs() < 0.01);

        state.release(&alloc_id).unwrap();
        assert!(state.used_vram_gb < 0.01);
    }

    #[test]
    fn test_allocation_failure_on_insufficient_resources() {
        let node_id = NodeId::new();
        let mut state = NodeResourceState::new(node_id, 24.0, 1000.0, 100.0);

        let req = ResourceRequirement::new(30.0, 200.0, 30.0, 1000);
        let result = state.allocate(None, req);

        assert!(result.is_err());
    }

    #[test]
    fn test_resource_manager_finds_best_node() {
        let mut manager = ResourceManager::new(0.1);

        let node1 = NodeId::new();
        let node2 = NodeId::new();

        manager.register_node(node1, 24.0, 1000.0, 100.0);
        manager.register_node(node2, 80.0, 2000.0, 400.0);

        let req = ResourceRequirement::new(10.0, 200.0, 30.0, 1000);
        let best = manager.find_best_node(&req).unwrap();

        // Should prefer node2 (larger capacity, same load = lower utilization)
        assert_eq!(best, node2);
    }

    #[test]
    fn test_cluster_summary() {
        let mut manager = ResourceManager::new(0.1);

        manager.register_node(NodeId::new(), 24.0, 1000.0, 100.0);
        manager.register_node(NodeId::new(), 80.0, 2000.0, 400.0);

        let summary = manager.cluster_summary();

        assert_eq!(summary.total_nodes, 2);
        assert!((summary.total_vram_gb - 104.0).abs() < 0.1);
    }

    #[test]
    fn test_utilization_tracking() {
        let node_id = NodeId::new();
        let mut state = NodeResourceState::new(node_id, 24.0, 1000.0, 100.0);

        let req = ResourceRequirement::new(12.0, 500.0, 50.0, 1000);
        state.allocate(None, req).unwrap();

        let util = state.utilization();
        assert!((util.vram_ratio - 0.5).abs() < 0.01);
        assert!((util.memory_bw_ratio - 0.5).abs() < 0.01);
        assert!((util.compute_ratio - 0.5).abs() < 0.01);
    }
}
