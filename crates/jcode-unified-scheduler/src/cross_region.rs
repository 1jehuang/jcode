//! **Cross-Region Deployment Manager** — Manages multi-region cluster deployment with latency-aware routing and data locality.
//!
//! ## Features
//!
//! 1. **Region/Zone Hierarchy**: Geographic region → availability zone → node
//! 2. **Latency-Aware Routing**: Prefer intra-region communication, minimize cross-region traffic
//! 3. **Data Locality Constraints**: GDPR/compliance-aware data placement
//! 4. **Region Failure Handling**: Automatic failover to backup regions
//! 5. **Cost Optimization**: Consider cross-region data transfer costs

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use tracing::{info, warn, debug};
use uuid::Uuid;

use crate::NodeId;

// ============================================================================
// Region & Zone Types
// ============================================================================

/// Unique identifier for a geographic region
pub type RegionId = String;

/// Unique identifier for an availability zone within a region
pub type ZoneId = String;

/// Represents a geographic region (e.g., "us-east", "eu-west", "ap-south")
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Region {
    pub region_id: RegionId,
    pub name: String,
    pub description: String,

    /// Availability zones within this region
    pub zones: HashMap<ZoneId, Zone>,

    /// Average inter-region latencies (ms) to other regions
    pub inter_region_latencies: HashMap<RegionId, f64>,

    /// Data transfer cost per GB to other regions (USD)
    pub transfer_costs: HashMap<RegionId, f64>,

    /// Compliance tags (e.g., "GDPR", "HIPAA", "SOC2")
    pub compliance_tags: HashSet<String>,

    /// Whether this region is currently active
    pub is_active: bool,
}

impl Region {
    pub fn new(region_id: &str, name: &str, description: &str) -> Self {
        Self {
            region_id: region_id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            zones: HashMap::new(),
            inter_region_latencies: HashMap::new(),
            transfer_costs: HashMap::new(),
            compliance_tags: HashSet::new(),
            is_active: true,
        }
    }

    /// Add an availability zone to this region
    pub fn add_zone(&mut self, zone: Zone) {
        self.zones.insert(zone.zone_id.clone(), zone);
    }

    /// Get latency to another region
    pub fn latency_to(&self, other_region: &RegionId) -> Option<f64> {
        self.inter_region_latencies.get(other_region).copied()
    }

    /// Get transfer cost to another region
    pub fn cost_to(&self, other_region: &RegionId) -> Option<f64> {
        self.transfer_costs.get(other_region).copied()
    }

    /// Check if this region has a specific compliance certification
    pub fn has_compliance(&self, tag: &str) -> bool {
        self.compliance_tags.contains(tag)
    }
}

/// Represents an availability zone within a region (e.g., "us-east-1a", "us-east-1b")
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Zone {
    pub zone_id: ZoneId,
    pub name: String,

    /// Nodes in this zone
    pub node_ids: HashSet<NodeId>,

    /// Total compute capacity in this zone (TFLOPS)
    pub total_capacity_tflops: f64,

    /// Currently used capacity (TFLOPS)
    pub used_capacity_tflops: f64,

    /// Whether this zone is currently healthy
    pub is_healthy: bool,
}

impl Zone {
    pub fn new(zone_id: &str, name: &str) -> Self {
        Self {
            zone_id: zone_id.to_string(),
            name: name.to_string(),
            node_ids: HashSet::new(),
            total_capacity_tflops: 0.0,
            used_capacity_tflops: 0.0,
            is_healthy: true,
        }
    }

    /// Add a node to this zone
    pub fn add_node(&mut self, node_id: NodeId, capacity_tflops: f64) {
        self.node_ids.insert(node_id);
        self.total_capacity_tflops += capacity_tflops;
    }

    /// Remove a node from this zone
    pub fn remove_node(&mut self, node_id: &NodeId, capacity_tflops: f64) {
        self.node_ids.remove(node_id);
        self.total_capacity_tflops -= capacity_tflops;
        self.used_capacity_tflops = self.used_capacity_tflops.min(self.total_capacity_tflops);
    }

    /// Get utilization ratio (0.0 - 1.0)
    pub fn utilization(&self) -> f64 {
        if self.total_capacity_tflops == 0.0 {
            return 0.0;
        }
        self.used_capacity_tflops / self.total_capacity_tflops
    }

    /// Check if zone can accept more load
    pub fn can_accept_load(&self, threshold: f64) -> bool {
        self.utilization() < threshold && self.is_healthy
    }
}

// ============================================================================
// Node Region Assignment
// ============================================================================

/// Region assignment for a specific node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRegionInfo {
    pub node_id: NodeId,
    pub region_id: RegionId,
    pub zone_id: ZoneId,

    /// Node's compute capacity (TFLOPS)
    pub capacity_tflops: f64,

    /// Compliance constraints for data that can be processed on this node
    pub allowed_data_classes: HashSet<String>,

    /// Last heartbeat timestamp
    pub last_heartbeat: Instant,
}

impl NodeRegionInfo {
    pub fn new(node_id: NodeId, region_id: &str, zone_id: &str, capacity_tflops: f64) -> Self {
        Self {
            node_id,
            region_id: region_id.to_string(),
            zone_id: zone_id.to_string(),
            capacity_tflops,
            allowed_data_classes: HashSet::new(),
            last_heartbeat: Instant::now(),
        }
    }

    /// Check if this node can process data of a specific class
    pub fn can_process_data_class(&self, data_class: &str) -> bool {
        self.allowed_data_classes.is_empty() || self.allowed_data_classes.contains(data_class)
    }
}

// ============================================================================
// Cross-Region Routing
// ============================================================================

/// Routing decision for cross-region requests
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub source_region: RegionId,
    pub target_region: RegionId,
    pub source_zone: Option<ZoneId>,
    pub target_zone: Option<ZoneId>,

    /// Estimated latency (ms)
    pub estimated_latency_ms: f64,

    /// Estimated cost (USD per GB)
    pub estimated_cost_per_gb: f64,

    /// Whether this is intra-region (true) or cross-region (false)
    pub is_intra_region: bool,

    /// Compliance check passed
    pub compliance_ok: bool,
}

impl RoutingDecision {
    /// Calculate a score for this routing decision (lower is better)
    pub fn score(&self, latency_weight: f64, cost_weight: f64) -> f64 {
        let latency_score = self.estimated_latency_ms * latency_weight;
        let cost_score = self.estimated_cost_per_gb * cost_weight;
        latency_score + cost_score
    }
}

// ============================================================================
// Region Manager
// ============================================================================

/// Manages multi-region cluster topology and routing
pub struct RegionManager {
    /// All registered regions
    regions: HashMap<RegionId, Region>,

    /// Node-to-region mapping
    node_regions: HashMap<NodeId, NodeRegionInfo>,

    /// Default region for nodes without explicit assignment
    default_region: Option<RegionId>,

    /// Routing preferences
    routing_config: RoutingConfig,
}

/// Configuration for cross-region routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// Weight for latency in routing decisions (higher = prefer lower latency)
    pub latency_weight: f64,

    /// Weight for cost in routing decisions (higher = prefer lower cost)
    pub cost_weight: f64,

    /// Maximum allowed cross-region latency (ms), 0 = unlimited
    pub max_cross_region_latency_ms: f64,

    /// Prefer intra-region routing even if slightly more expensive
    pub prefer_intra_region: bool,

    /// Enable automatic failover to backup regions
    pub enable_failover: bool,

    /// Backup region mappings (primary -> backup)
    pub backup_regions: HashMap<RegionId, RegionId>,
}

impl RoutingConfig {
    pub fn default() -> Self {
        Self {
            latency_weight: 1.0,
            cost_weight: 0.5,
            max_cross_region_latency_ms: 100.0,
            prefer_intra_region: true,
            enable_failover: true,
            backup_regions: HashMap::new(),
        }
    }

    pub fn latency_optimized() -> Self {
        Self {
            latency_weight: 2.0,
            cost_weight: 0.2,
            max_cross_region_latency_ms: 50.0,
            prefer_intra_region: true,
            enable_failover: true,
            backup_regions: HashMap::new(),
        }
    }

    pub fn cost_optimized() -> Self {
        Self {
            latency_weight: 0.5,
            cost_weight: 2.0,
            max_cross_region_latency_ms: 200.0,
            prefer_intra_region: false,
            enable_failover: true,
            backup_regions: HashMap::new(),
        }
    }
}

impl RegionManager {
    pub fn new(routing_config: RoutingConfig) -> Self {
        Self {
            regions: HashMap::new(),
            node_regions: HashMap::new(),
            default_region: None,
            routing_config,
        }
    }

    /// Register a new region
    pub fn register_region(&mut self, region: Region) {
        info!("[RegionManager] Registered region: {} ({})", region.region_id, region.name);
        self.regions.insert(region.region_id.clone(), region);
    }

    /// Unregister a region
    pub fn unregister_region(&mut self, region_id: &RegionId) -> Result<(), String> {
        if let Some(region) = self.regions.get_mut(region_id) {
            if !region.node_ids_in_region().is_empty() {
                return Err(format!("Cannot unregister region {} with active nodes", region_id));
            }
            region.is_active = false;
            Ok(())
        } else {
            Err(format!("Region {} not found", region_id))
        }
    }

    /// Assign a node to a region and zone
    pub fn assign_node_to_region(
        &mut self,
        node_id: NodeId,
        region_id: &str,
        zone_id: &str,
        capacity_tflops: f64,
    ) -> Result<(), String> {
        // Verify region exists
        if !self.regions.contains_key(region_id) {
            return Err(format!("Region {} not found", region_id));
        }

        // Create node region info
        let node_info = NodeRegionInfo::new(node_id, region_id, zone_id, capacity_tflops);

        // Add node to zone
        if let Some(region) = self.regions.get_mut(region_id) {
            if let Some(zone) = region.zones.get_mut(zone_id) {
                zone.add_node(node_id, capacity_tflops);
            } else {
                // Auto-create zone
                let mut new_zone = Zone::new(zone_id, &format!("{}-{}", region_id, zone_id));
                new_zone.add_node(node_id, capacity_tflops);
                region.add_zone(new_zone);
            }
        }

        self.node_regions.insert(node_id, node_info);

        debug!(
            "[RegionManager] Assigned node {} to region={}, zone={}",
            node_id, region_id, zone_id
        );

        Ok(())
    }

    /// Remove a node from its region assignment
    pub fn remove_node_from_region(&mut self, node_id: &NodeId) -> Result<(), String> {
        if let Some(node_info) = self.node_regions.remove(node_id) {
            if let Some(region) = self.regions.get_mut(&node_info.region_id) {
                if let Some(zone) = region.zones.get_mut(&node_info.zone_id) {
                    zone.remove_node(node_id, node_info.capacity_tflops);
                }
            }
            Ok(())
        } else {
            Err(format!("Node {} not found in any region", node_id))
        }
    }

    /// Get the region assignment for a node
    pub fn get_node_region(&self, node_id: &NodeId) -> Option<&NodeRegionInfo> {
        self.node_regions.get(node_id)
    }

    /// Find the best region for a request with compliance constraints
    pub fn find_best_region(
        &self,
        source_region: Option<&str>,
        required_data_class: Option<&str>,
        required_capacity_tflops: f64,
    ) -> Option<RoutingDecision> {
        let mut best_decision: Option<RoutingDecision> = None;
        let mut best_score = f64::MAX;

        for region in self.regions.values() {
            if !region.is_active {
                continue;
            }

            // Check compliance
            if let Some(data_class) = required_data_class {
                if !region.has_compliance(data_class) {
                    continue;
                }
            }

            // Check capacity
            let available_capacity = region.available_capacity_tflops();
            if available_capacity < required_capacity_tflops {
                continue;
            }

            // Calculate routing decision
            let is_intra_region = source_region.map_or(false, |s| s == region.region_id);

            // Skip cross-region if preferred and source is known
            if self.routing_config.prefer_intra_region && source_region.is_some() && !is_intra_region {
                continue;
            }

            let latency = if is_intra_region {
                5.0 // Intra-region latency estimate
            } else if let Some(src) = source_region {
                region.latency_to(src).unwrap_or(100.0)
            } else {
                50.0 // Default estimate
            };

            let cost = if is_intra_region {
                0.0 // No cross-region cost
            } else if let Some(src) = source_region {
                region.cost_to(src).unwrap_or(0.1)
            } else {
                0.05
            };

            let decision = RoutingDecision {
                source_region: source_region.unwrap_or("unknown").to_string(),
                target_region: region.region_id.clone(),
                source_zone: None,
                target_zone: None,
                estimated_latency_ms: latency,
                estimated_cost_per_gb: cost,
                is_intra_region,
                compliance_ok: true,
            };

            let score = decision.score(self.routing_config.latency_weight, self.routing_config.cost_weight);

            if score < best_score {
                best_score = score;
                best_decision = Some(decision);
            }
        }

        best_decision
    }

    /// Handle region failure by failing over to backup
    pub fn handle_region_failure(&mut self, failed_region: &RegionId) -> Result<Vec<NodeId>, String> {
        if !self.routing_config.enable_failover {
            return Err("Failover is disabled".to_string());
        }

        let backup_region = self.routing_config.backup_regions.get(failed_region)
            .ok_or_else(|| format!("No backup region configured for {}", failed_region))?
            .clone();

        if !self.regions.contains_key(&backup_region) {
            return Err(format!("Backup region {} not found", backup_region));
        }

        // Get all nodes in failed region
        let nodes_to_migrate: Vec<NodeId> = self.node_regions.iter()
            .filter(|(_, info)| info.region_id == *failed_region)
            .map(|(id, _)| *id)
            .collect();

        info!(
            "[RegionManager] Failing over {} nodes from {} to {}",
            nodes_to_migrate.len(), failed_region, backup_region
        );

        // Mark failed region as inactive
        if let Some(region) = self.regions.get_mut(failed_region) {
            region.is_active = false;
        }

        Ok(nodes_to_migrate)
    }

    /// Get cluster-wide region summary
    pub fn region_summary(&self) -> RegionSummary {
        let mut total_regions = 0;
        let mut active_regions = 0;
        let mut total_zones = 0;
        let mut total_nodes = 0;
        let mut total_capacity = 0.0;
        let mut used_capacity = 0.0;

        for region in self.regions.values() {
            total_regions += 1;
            if region.is_active {
                active_regions += 1;
            }
            total_zones += region.zones.len();

            for zone in region.zones.values() {
                total_nodes += zone.node_ids.len();
                total_capacity += zone.total_capacity_tflops;
                used_capacity += zone.used_capacity_tflops;
            }
        }

        RegionSummary {
            total_regions,
            active_regions,
            total_zones,
            total_nodes,
            total_capacity_tflops: total_capacity,
            used_capacity_tflops: used_capacity,
            overall_utilization: if total_capacity > 0.0 { used_capacity / total_capacity } else { 0.0 },
        }
    }

    /// Set default region for unassigned nodes
    pub fn set_default_region(&mut self, region_id: &str) -> Result<(), String> {
        if self.regions.contains_key(region_id) {
            self.default_region = Some(region_id.to_string());
            Ok(())
        } else {
            Err(format!("Region {} not found", region_id))
        }
    }
}

// Helper method for Region
impl Region {
    fn node_ids_in_region(&self) -> Vec<NodeId> {
        self.zones.values()
            .flat_map(|z| z.node_ids.iter())
            .cloned()
            .collect()
    }

    fn available_capacity_tflops(&self) -> f64 {
        self.zones.values()
            .map(|z| z.total_capacity_tflops - z.used_capacity_tflops)
            .sum()
    }
}

/// Summary of multi-region cluster state
#[derive(Debug, Clone)]
pub struct RegionSummary {
    pub total_regions: usize,
    pub active_regions: usize,
    pub total_zones: usize,
    pub total_nodes: usize,
    pub total_capacity_tflops: f64,
    pub used_capacity_tflops: f64,
    pub overall_utilization: f64,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_region(region_id: &str, name: &str) -> Region {
        let mut region = Region::new(region_id, name, &format!("Test region {}", name));
        region.add_zone(Zone::new("zone-a", &format!("{}-a", region_id)));
        region.add_zone(Zone::new("zone-b", &format!("{}-b", region_id)));
        region
    }

    #[test]
    fn test_region_creation() {
        let region = create_test_region("us-east", "US East");
        assert_eq!(region.region_id, "us-east");
        assert_eq!(region.zones.len(), 2);
        assert!(region.is_active);
    }

    #[test]
    fn test_zone_utilization() {
        let mut zone = Zone::new("zone-a", "Test Zone");
        zone.add_node(NodeId::new(), 100.0);
        zone.add_node(NodeId::new(), 100.0);
        zone.used_capacity_tflops = 150.0;

        assert!((zone.utilization() - 0.75).abs() < 0.01);
        assert!(zone.can_accept_load(0.8));
        assert!(!zone.can_accept_load(0.7));
    }

    #[test]
    fn test_region_manager_registration() {
        let config = RoutingConfig::default();
        let mut manager = RegionManager::new(config);

        let region = create_test_region("us-east", "US East");
        manager.register_region(region);

        assert_eq!(manager.regions.len(), 1);
    }

    #[test]
    fn test_node_region_assignment() {
        let config = RoutingConfig::default();
        let mut manager = RegionManager::new(config);

        let mut region = create_test_region("us-east", "US East");
        region.add_zone(Zone::new("zone-a", "us-east-a"));
        manager.register_region(region);

        let node_id = NodeId::new();
        manager.assign_node_to_region(node_id, "us-east", "zone-a", 50.0).unwrap();

        let node_info = manager.get_node_region(&node_id).unwrap();
        assert_eq!(node_info.region_id, "us-east");
        assert_eq!(node_info.zone_id, "zone-a");
    }

    #[test]
    fn test_find_best_region_intra_region() {
        let config = RoutingConfig::default();
        let mut manager = RegionManager::new(config);

        let mut region1 = create_test_region("us-east", "US East");
        region1.zones.get_mut("zone-a").unwrap().total_capacity_tflops = 500.0;
        manager.register_region(region1);

        let decision = manager.find_best_region(Some("us-east"), None, 100.0).unwrap();
        assert!(decision.is_intra_region);
        assert_eq!(decision.target_region, "us-east");
    }

    #[test]
    fn test_compliance_filtering() {
        let config = RoutingConfig::default();
        let mut manager = RegionManager::new(config);

        let mut region1 = create_test_region("us-east", "US East");
        region1.compliance_tags.insert("GDPR".to_string());
        manager.register_region(region1);

        let mut region2 = create_test_region("cn-north", "China North");
        manager.register_region(region2);

        // Request requires GDPR compliance
        let decision = manager.find_best_region(None, Some("GDPR"), 100.0).unwrap();
        assert_eq!(decision.target_region, "us-east");
    }

    #[test]
    fn test_region_summary() {
        let config = RoutingConfig::default();
        let mut manager = RegionManager::new(config);

        let mut region = create_test_region("us-east", "US East");
        region.zones.get_mut("zone-a").unwrap().total_capacity_tflops = 200.0;
        region.zones.get_mut("zone-a").unwrap().used_capacity_tflops = 100.0;
        manager.register_region(region);

        let summary = manager.region_summary();
        assert_eq!(summary.total_regions, 1);
        assert_eq!(summary.active_regions, 1);
        assert!((summary.overall_utilization - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_routing_config_presets() {
        let latency_opt = RoutingConfig::latency_optimized();
        assert!(latency_opt.latency_weight > latency_opt.cost_weight);

        let cost_opt = RoutingConfig::cost_optimized();
        assert!(cost_opt.cost_weight > cost_opt.latency_weight);
    }
}
