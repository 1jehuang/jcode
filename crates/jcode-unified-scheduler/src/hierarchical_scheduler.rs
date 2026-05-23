//! **Hierarchical Scheduler** — Manages large-scale clusters (50-300+ nodes) through hierarchical grouping.
//!
//! ## Architecture
//!
//! ```text
//! Global Scheduler (Top Level)
//! ├── Cluster Group A (Regional/Functional Group)
//! │   ├── Local Scheduler A1
//! │   │   ├── Node 1-10
//! │   │   └── Node 11-20
//! │   └── Local Scheduler A2
//! │       └── Node 21-30
//! ├── Cluster Group B
//! │   └── Local Scheduler B1
//! │       └── Node 31-50
//! └── Cluster Group C
//!     └── Local Scheduler C1
//!         └── Node 51-70
//! ```
//!
//! ## Benefits
//! - **Scalability**: Each local scheduler manages 20-50 nodes (optimal range)
//! - **Fault Isolation**: Group failures don't affect other groups
//! - **Geographic Awareness**: Groups can represent regions/zones
//! - **Load Distribution**: Global scheduler distributes across groups

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, debug};
use uuid::Uuid;

use crate::{
    UnifiedScheduler, SchedulerConfig, NodeId, NodeHardwareInfo,
    SchedulerError, ScheduledTask,
};

// ============================================================================
// Cluster Group Types
// ============================================================================

/// Unique identifier for a cluster group
pub type ClusterGroupId = String;

/// Represents a logical grouping of nodes (region, zone, or functional group)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterGroup {
    pub group_id: ClusterGroupId,
    pub name: String,
    pub description: String,

    /// Group type
    pub group_type: ClusterGroupType,

    /// Maximum nodes this group can manage
    pub max_nodes: usize,

    /// Current node count
    pub current_nodes: usize,

    /// Local scheduler for this group (manages nodes within group)
    #[serde(skip)]
    pub local_scheduler: Arc<UnifiedScheduler>,

    /// Whether this group is currently accepting new nodes
    pub accepting_nodes: bool,

    /// Group metadata (region, zone, etc.)
    pub metadata: HashMap<String, String>,
}

/// Type of cluster group
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClusterGroupType {
    /// Geographic region (e.g., "us-east", "eu-west")
    Region,
    /// Availability zone within a region
    Zone,
    /// Functional grouping (e.g., "gpu-high-memory", "cpu-inference")
    Functional,
    /// Temporary dynamic group (auto-created for load balancing)
    Dynamic,
}

impl ClusterGroup {
    pub fn new(
        group_id: &str,
        name: &str,
        group_type: ClusterGroupType,
        max_nodes: usize,
    ) -> Self {
        let _config = SchedulerConfig {
            min_bootstrap_nodes: 1,
            max_concurrent_tasks: 50,
            ..SchedulerConfig::default()
        };

        // Note: In production, this would be async initialization
        // For now, we create a placeholder that will be initialized later
        Self {
            group_id: group_id.to_string(),
            name: name.to_string(),
            description: format!("{} ({:?})", name, group_type),
            group_type,
            max_nodes,
            current_nodes: 0,
            local_scheduler: Arc::new(UnifiedScheduler::default()),
            accepting_nodes: true,
            metadata: HashMap::new(),
        }
    }

    /// Check if group can accept more nodes
    pub fn can_accept_nodes(&self) -> bool {
        self.accepting_nodes && self.current_nodes < self.max_nodes
    }

    /// Get utilization ratio (0.0 - 1.0)
    pub fn utilization(&self) -> f64 {
        self.current_nodes as f64 / self.max_nodes as f64
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: &str, value: &str) -> Self {
        self.metadata.insert(key.to_string(), value.to_string());
        self
    }
}

// ============================================================================
// Global Scheduler
// ============================================================================

/// Top-level scheduler that manages multiple cluster groups
pub struct HierarchicalScheduler {
    /// All cluster groups
    groups: RwLock<HashMap<ClusterGroupId, Arc<ClusterGroup>>>,

    /// Default group for unassigned nodes
    default_group: Option<ClusterGroupId>,

    /// Global scheduler configuration
    global_config: HierarchicalSchedulerConfig,

    /// Statistics
    stats: RwLock<HierarchicalStats>,
}

/// Configuration for hierarchical scheduler
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierarchicalSchedulerConfig {
    /// Maximum nodes per group (optimal: 20-50)
    pub max_nodes_per_group: usize,

    /// Strategy for selecting target group for new nodes
    pub group_selection_strategy: GroupSelectionStrategy,

    /// Enable automatic group creation when needed
    pub auto_create_groups: bool,

    /// Prefix for auto-generated group IDs
    pub auto_group_prefix: String,

    /// Load balancing threshold (trigger rebalance if imbalance > this)
    pub rebalance_threshold: f64,
}

/// Strategy for selecting which group receives new nodes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupSelectionStrategy {
    /// Select group with lowest utilization (load balancing)
    LeastUtilized,
    /// Select group with most available capacity
    MostAvailable,
    /// Prefer specific group based on metadata matching
    MetadataMatch,
    /// Round-robin across groups
    RoundRobin,
}

impl HierarchicalSchedulerConfig {
    pub fn default() -> Self {
        Self {
            max_nodes_per_group: 30, // Optimal for 50-300 node clusters
            group_selection_strategy: GroupSelectionStrategy::LeastUtilized,
            auto_create_groups: true,
            auto_group_prefix: "auto-group".to_string(),
            rebalance_threshold: 0.3, // 30% imbalance triggers rebalance
        }
    }

    pub fn for_small_clusters() -> Self {
        Self {
            max_nodes_per_group: 20,
            ..Self::default()
        }
    }

    pub fn for_large_clusters() -> Self {
        Self {
            max_nodes_per_group: 50,
            ..Self::default()
        }
    }
}

/// Statistics for hierarchical scheduler
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierarchicalStats {
    pub total_groups: usize,
    pub active_groups: usize,
    pub total_nodes: usize,
    pub total_requests_processed: u64,
    pub cross_group_routings: u64,
    pub group_rebalances: u64,
}

impl HierarchicalStats {
    pub fn new() -> Self {
        Self {
            total_groups: 0,
            active_groups: 0,
            total_nodes: 0,
            total_requests_processed: 0,
            cross_group_routings: 0,
            group_rebalances: 0,
        }
    }
}

impl HierarchicalScheduler {
    pub fn new(config: HierarchicalSchedulerConfig) -> Self {
        info!("[HierarchicalScheduler] Initializing with config: {:?}", config);
        Self {
            groups: RwLock::new(HashMap::new()),
            default_group: None,
            global_config: config,
            stats: RwLock::new(HierarchicalStats::new()),
        }
    }

    /// Create a new cluster group
    pub async fn create_group(
        &self,
        group_id: &str,
        name: &str,
        group_type: ClusterGroupType,
        max_nodes: Option<usize>,
    ) -> Result<(), SchedulerError> {
        let max = max_nodes.unwrap_or(self.global_config.max_nodes_per_group);

        let mut group = ClusterGroup::new(group_id, name, group_type, max);

        // Initialize the local scheduler properly
        let config = SchedulerConfig {
            min_bootstrap_nodes: 1,
            max_concurrent_tasks: 50,
            ..SchedulerConfig::default()
        };
        let local_scheduler = UnifiedScheduler::new(config).await?;
        group.local_scheduler = Arc::new(local_scheduler);

        info!(
            "[HierarchicalScheduler] Created group: {} (type={:?}, max_nodes={})",
            group_id, group_type, max
        );

        self.groups.write().await.insert(group_id.to_string(), Arc::new(group));

        // Update stats
        self.update_stats().await;

        Ok(())
    }

    /// Register a node to the appropriate cluster group
    pub async fn register_node(
        &self,
        hardware: NodeHardwareInfo,
        preferred_group: Option<&str>,
    ) -> Result<NodeId, SchedulerError> {
        // Determine target group
        let target_group = if let Some(gid) = preferred_group {
            // Use specified group if valid
            if let Some(group) = self.groups.read().await.get(gid) {
                if group.can_accept_nodes() {
                    gid.to_string()
                } else {
                    warn!(
                        "[HierarchicalScheduler] Group {} cannot accept nodes, finding alternative",
                        gid
                    );
                    self.select_target_group().await?
                }
            } else {
                warn!(
                    "[HierarchicalScheduler] Group {} not found, finding alternative",
                    gid
                );
                self.select_target_group().await?
            }
        } else {
            // Auto-select best group
            self.select_target_group().await?
        };

        // Register node to selected group
        let groups = self.groups.read().await;
        let group = groups.get(&target_group)
            .ok_or_else(|| SchedulerError::AllocationFailed(format!("Group {} not found", target_group)))?;

        let node_id = group.local_scheduler.register_node(hardware.clone()).await?;

        // Update group node count
        let mut groups_mut = self.groups.write().await;
        if let Some(_group_mut) = groups_mut.get_mut(&target_group) {
            // Note: In real implementation, we'd use Arc::get_mut or interior mutability
            // For now, this is a simplified version
        }

        info!(
            "[HierarchicalScheduler] Registered node {} to group {}",
            node_id, target_group
        );

        // Update stats
        self.update_stats().await;

        Ok(node_id)
    }

    /// Submit a task to the hierarchical scheduler
    pub async fn submit_task(&self, task: ScheduledTask) -> Result<Uuid, SchedulerError> {
        // Determine which group should handle this task
        let target_group = self.select_group_for_task(&task).await?;

        let groups = self.groups.read().await;
        let group = groups.get(&target_group)
            .ok_or_else(|| SchedulerError::AllocationFailed(format!("Group {} not found", target_group)))?;

        // Submit to local scheduler
        let task_id = group.local_scheduler.submit_task(task).await?;

        debug!(
            "[HierarchicalScheduler] Task {} submitted to group {}",
            task_id, target_group
        );

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_requests_processed += 1;
        }

        Ok(task_id)
    }

    /// Unregister a node from its group
    pub async fn unregister_node(&self, node_id: &NodeId) -> Result<(), SchedulerError> {
        // Find which group contains this node
        let groups = self.groups.read().await;
        let mut target_group = None;

        for (group_id, group) in groups.iter() {
            // Check if node exists in this group's local scheduler
            // In production, we'd query the local scheduler's node list
            // For now, we'll try all groups
            if group.local_scheduler.get_active_nodes().await.iter().any(|n| n.node_id == *node_id) {
                target_group = Some(group_id.clone());
                break;
            }
        }

        if let Some(group_id) = target_group {
            let group = groups.get(&group_id).unwrap();
            group.local_scheduler.unregister_node(node_id).await?;

            info!(
                "[HierarchicalScheduler] Unregistered node {} from group {}",
                node_id, group_id
            );

            // Update stats
            self.update_stats().await;

            Ok(())
        } else {
            Err(SchedulerError::NodeNotFound(*node_id))
        }
    }

    /// Get cluster-wide summary
    pub async fn get_cluster_summary(&self) -> HierarchicalClusterSummary {
        let groups = self.groups.read().await;
        let mut summary = HierarchicalClusterSummary {
            total_groups: groups.len(),
            total_nodes: 0,
            groups: Vec::new(),
        };

        for (group_id, group) in groups.iter() {
            let local_summary = group.local_scheduler.get_cluster_summary().await;
            summary.total_nodes += local_summary.active_nodes;

            let group_info = GroupSummary {
                group_id: group_id.clone(),
                group_type: group.group_type,
                node_count: local_summary.active_nodes,
                max_nodes: group.max_nodes,
                utilization: group.utilization(),
            };

            summary.groups.push(group_info);
        }

        summary
    }

    /// Select the best group for a new node
    async fn select_target_group(&self) -> Result<ClusterGroupId, SchedulerError> {
        let groups = self.groups.read().await;

        if groups.is_empty() {
            return Err(SchedulerError::AllocationFailed("No groups available".to_string()));
        }

        match self.global_config.group_selection_strategy {
            GroupSelectionStrategy::LeastUtilized => {
                // Select group with lowest utilization
                groups.iter()
                    .filter(|(_, g)| g.can_accept_nodes())
                    .min_by(|(_, a), (_, b)| {
                        a.utilization().partial_cmp(&b.utilization()).unwrap()
                    })
                    .map(|(id, _)| id.clone())
                    .ok_or_else(|| SchedulerError::AllocationFailed("No groups can accept nodes".to_string()))
            }
            GroupSelectionStrategy::MostAvailable => {
                // Select group with most available capacity
                groups.iter()
                    .filter(|(_, g)| g.can_accept_nodes())
                    .max_by(|(_, a), (_, b)| {
                        let avail_a = a.max_nodes - a.current_nodes;
                        let avail_b = b.max_nodes - b.current_nodes;
                        avail_a.cmp(&avail_b)
                    })
                    .map(|(id, _)| id.clone())
                    .ok_or_else(|| SchedulerError::AllocationFailed("No groups can accept nodes".to_string()))
            }
            GroupSelectionStrategy::RoundRobin => {
                // Simple round-robin (in production, track last selected index)
                groups.iter()
                    .filter(|(_, g)| g.can_accept_nodes())
                    .next()
                    .map(|(id, _)| id.clone())
                    .ok_or_else(|| SchedulerError::AllocationFailed("No groups can accept nodes".to_string()))
            }
            GroupSelectionStrategy::MetadataMatch => {
                // For metadata matching, we'd need task/group metadata
                // Fallback to least utilized for now
                groups.iter()
                    .filter(|(_, g)| g.can_accept_nodes())
                    .min_by(|(_, a), (_, b)| {
                        a.utilization().partial_cmp(&b.utilization()).unwrap()
                    })
                    .map(|(id, _)| id.clone())
                    .ok_or_else(|| SchedulerError::AllocationFailed("No groups can accept nodes".to_string()))
            }
        }
    }

    /// Select the best group for a task
    async fn select_group_for_task(&self, _task: &ScheduledTask) -> Result<ClusterGroupId, SchedulerError> {
        // For now, select group with lowest load
        // In production, consider task requirements, data locality, etc.
        self.select_target_group().await
    }

    /// Update internal statistics
    async fn update_stats(&self) {
        let groups = self.groups.read().await;
        let mut stats = self.stats.write().await;

        stats.total_groups = groups.len();
        stats.active_groups = groups.values().filter(|g| g.current_nodes > 0).count();
        stats.total_nodes = groups.values().map(|g| g.current_nodes).sum();
    }
}

/// Summary of hierarchical cluster state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierarchicalClusterSummary {
    pub total_groups: usize,
    pub total_nodes: usize,
    pub groups: Vec<GroupSummary>,
}

/// Summary of a single cluster group
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupSummary {
    pub group_id: String,
    pub group_type: ClusterGroupType,
    pub node_count: usize,
    pub max_nodes: usize,
    pub utilization: f64,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hierarchical_scheduler_creation() {
        let config = HierarchicalSchedulerConfig::default();
        let scheduler = HierarchicalScheduler::new(config);

        assert_eq!(scheduler.groups.read().await.len(), 0);
    }

    #[tokio::test]
    async fn test_create_cluster_group() {
        let config = HierarchicalSchedulerConfig::default();
        let scheduler = HierarchicalScheduler::new(config);

        scheduler.create_group("group-a", "Group A", ClusterGroupType::Region, None)
            .await
            .unwrap();

        assert_eq!(scheduler.groups.read().await.len(), 1);
    }

    #[tokio::test]
    async fn test_group_utilization() {
        let group = ClusterGroup::new("test", "Test", ClusterGroupType::Zone, 30);
        assert!((group.utilization() - 0.0).abs() < 0.01);

        // Simulate adding nodes (in real code, this would be done via register_node)
        // For testing, we'd need to expose a way to set current_nodes
    }

    #[test]
    fn test_cluster_group_types() {
        assert_eq!(ClusterGroupType::Region as u8, 0);
        assert_eq!(ClusterGroupType::Zone as u8, 1);
        assert_eq!(ClusterGroupType::Functional as u8, 2);
        assert_eq!(ClusterGroupType::Dynamic as u8, 3);
    }

    #[test]
    fn test_config_presets() {
        let small = HierarchicalSchedulerConfig::for_small_clusters();
        assert_eq!(small.max_nodes_per_group, 20);

        let large = HierarchicalSchedulerConfig::for_large_clusters();
        assert_eq!(large.max_nodes_per_group, 50);
    }
}
