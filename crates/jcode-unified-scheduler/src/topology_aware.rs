//! NUMA/GPU Topology-Aware Scheduler
//!
//! Provides hardware topology awareness for optimal task placement,
//! considering NUMA nodes, GPU interconnects (NVLink/P2P), and PCIe bandwidth.
//!
//! ## Features
//! 1. **NUMA Awareness**: Avoid cross-NUMA memory access penalties
//! 2. **GPU Topology**: Prefer NVLink-connected GPUs for communication-heavy tasks
//! 3. **PCIe Bandwidth**: Consider bus bandwidth for data transfer planning
//! 4. **Cache Affinity**: Keep related tasks on same NUMA node for cache locality

use std::collections::{HashMap, HashSet};
use std::fmt;
use tracing::{info, warn, debug};
use serde::{Serialize, Deserialize};

// ============================================================================
//! NUMA Node Representation
// ============================================================================

/// NUMA node information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumaNode {
    /// NUMA node ID
    pub node_id: u32,
    /// CPU cores belonging to this NUMA node
    pub cpu_cores: Vec<u32>,
    /// Total memory available (GB)
    pub memory_gb: f64,
    /// Available memory (GB)
    pub available_memory_gb: f64,
    /// GPUs directly attached to this NUMA node
    pub attached_gpus: Vec<GpuInfo>,
    /// Memory bandwidth (GB/s)
    pub memory_bandwidth_gbs: f64,
}

impl NumaNode {
    pub fn new(node_id: u32, cpu_cores: Vec<u32>, memory_gb: f64) -> Self {
        Self {
            node_id,
            cpu_cores,
            memory_gb,
            available_memory_gb: memory_gb,
            attached_gpus: Vec::new(),
            memory_bandwidth_gbs: 50.0, // Default DDR4 bandwidth estimate
        }
    }

    /// Check if a GPU is attached to this NUMA node
    pub fn has_gpu(&self, gpu_id: &str) -> bool {
        self.attached_gpus.iter().any(|g| g.gpu_id == gpu_id)
    }

    /// Get available memory percentage
    pub fn memory_utilization(&self) -> f64 {
        if self.memory_gb == 0.0 {
            return 0.0;
        }
        1.0 - (self.available_memory_gb / self.memory_gb)
    }
}

// ============================================================================
//! GPU Information and Topology
// ============================================================================

/// GPU device information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    /// Unique GPU identifier
    pub gpu_id: String,
    /// GPU model name (e.g., "RTX-4090")
    pub model: String,
    /// PCIe bus ID
    pub pci_bus_id: String,
    /// NUMA node this GPU is attached to
    pub numa_node_id: u32,
    /// Total VRAM (GB)
    pub vram_gb: f64,
    /// Available VRAM (GB)
    pub available_vram_gb: f64,
    /// Compute capability (e.g., 8.9 for RTX-4090)
    pub compute_capability: f32,
    /// TFLOPS (FP16)
    pub tflops_fp16: f64,
    /// Memory bandwidth (GB/s)
    pub memory_bandwidth_gbs: f64,
}

impl GpuInfo {
    pub fn vrAM_utilization(&self) -> f64 {
        if self.vram_gb == 0.0 {
            return 0.0;
        }
        1.0 - (self.available_vram_gb / self.vram_gb)
    }
}

// ============================================================================
//! GPU Interconnect Topology
// ============================================================================

/// Type of GPU interconnect
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InterconnectType {
    /// No direct connection (must go through CPU/system memory)
    None,
    /// PCIe connection (typical multi-GPU setup)
    Pcie,
    /// NVLink connection (high-bandwidth GPU-to-GPU)
    NvLink,
    /// AMD Infinity Fabric
    InfinityFabric,
    /// Shared same GPU (different partitions)
    SameGpu,
}

impl InterconnectType {
    /// Get approximate bandwidth (GB/s)
    pub fn bandwidth_gbs(&self) -> f64 {
        match self {
            Self::None => 0.0,
            Self::Pcie => 32.0,  // PCIe 4.0 x16 ~32 GB/s
            Self::NvLink => 300.0,  // NVLink v3 ~300 GB/s per link
            Self::InfinityFabric => 150.0,
            Self::SameGpu => 1000.0,  // Internal bandwidth
        }
    }

    /// Get approximate latency (microseconds)
    pub fn latency_us(&self) -> f64 {
        match self {
            Self::None => 100.0,
            Self::Pcie => 10.0,
            Self::NvLink => 1.0,
            Self::InfinityFabric => 2.0,
            Self::SameGpu => 0.1,
        }
    }
}

impl fmt::Display for InterconnectType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::Pcie => write!(f, "PCIe"),
            Self::NvLink => write!(f, "NVLink"),
            Self::InfinityFabric => write!(f, "InfinityFabric"),
            Self::SameGpu => write!(f, "SameGPU"),
        }
    }
}

/// Connection between two GPUs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuLink {
    pub gpu_a: String,
    pub gpu_b: String,
    pub interconnect: InterconnectType,
    /// Number of links (e.g., multiple NVLink connections)
    pub link_count: u32,
    /// Effective bandwidth (GB/s) = base_bandwidth * link_count
    pub effective_bandwidth_gbs: f64,
}

impl GpuLink {
    pub fn new(gpu_a: String, gpu_b: String, interconnect: InterconnectType, link_count: u32) -> Self {
        let effective_bandwidth_gbs = interconnect.bandwidth_gbs() * link_count as f64;
        Self {
            gpu_a,
            gpu_b,
            interconnect,
            link_count,
            effective_bandwidth_gbs,
        }
    }

    /// Check if this link involves a specific GPU
    pub fn involves_gpu(&self, gpu_id: &str) -> bool {
        self.gpu_a == gpu_id || self.gpu_b == gpu_id
    }

    /// Get the other GPU in this link
    pub fn other_gpu(&self, gpu_id: &str) -> Option<&str> {
        if self.gpu_a == gpu_id {
            Some(&self.gpu_b)
        } else if self.gpu_b == gpu_id {
            Some(&self.gpu_a)
        } else {
            None
        }
    }
}

// ============================================================================
//! Hardware Topology Graph
// ============================================================================

/// Complete hardware topology for a node
pub struct HardwareTopology {
    /// NUMA nodes
    pub numa_nodes: HashMap<u32, NumaNode>,
    /// All GPUs
    pub gpus: HashMap<String, GpuInfo>,
    /// GPU interconnect graph
    pub gpu_links: Vec<GpuLink>,
    /// Adjacency list for GPU graph (gpu_id -> [(neighbor_gpu_id, link)])
    pub gpu_adjacency: HashMap<String, Vec<(String, GpuLink)>>,
}

impl HardwareTopology {
    pub fn new() -> Self {
        Self {
            numa_nodes: HashMap::new(),
            gpus: HashMap::new(),
            gpu_links: Vec::new(),
            gpu_adjacency: HashMap::new(),
        }
    }

    /// Add a NUMA node
    pub fn add_numa_node(&mut self, node: NumaNode) {
        self.numa_nodes.insert(node.node_id, node);
    }

    /// Add a GPU
    pub fn add_gpu(&mut self, gpu: GpuInfo) {
        // Also add to NUMA node's attached GPUs
        if let Some(numa_node) = self.numa_nodes.get_mut(&gpu.numa_node_id) {
            numa_node.attached_gpus.push(gpu.clone());
        }
        self.gpus.insert(gpu.gpu_id.clone(), gpu);
    }

    /// Add a GPU link
    pub fn add_gpu_link(&mut self, link: GpuLink) {
        let gpu_a = link.gpu_a.clone();
        let gpu_b = link.gpu_b.clone();

        self.gpu_adjacency
            .entry(gpu_a.clone())
            .or_insert_with(Vec::new)
            .push((gpu_b.clone(), link.clone()));

        self.gpu_adjacency
            .entry(gpu_b.clone())
            .or_insert_with(Vec::new)
            .push((gpu_a.clone(), link.clone()));

        self.gpu_links.push(link);
    }

    /// Get best GPU for a task based on topology
    ///
    /// Preferences:
    /// 1. GPU with most available VRAM
    /// 2. GPU on NUMA node with most available memory
    /// 3. GPU with best interconnect to other required GPUs
    pub fn select_best_gpu(
        &self,
        required_vram_gb: f64,
        preferred_numa_node: Option<u32>,
        communicate_with_gpus: &[String],
    ) -> Option<String> {
        // Filter GPUs with enough VRAM
        let candidates: Vec<&GpuInfo> = self.gpus.values()
            .filter(|g| g.available_vram_gb >= required_vram_gb)
            .collect();

        if candidates.is_empty() {
            return None;
        }

        // Score each candidate
        let mut best_gpu: Option<&GpuInfo> = None;
        let mut best_score = f64::NEG_INFINITY;

        for gpu in &candidates {
            let mut score = 0.0;

            // Prefer more available VRAM
            score += gpu.available_vram_gb * 10.0;

            // Prefer NUMA node affinity
            if let Some(preferred_numa) = preferred_numa_node {
                if gpu.numa_node_id == preferred_numa {
                    score += 100.0; // Strong preference for local NUMA
                }
            }

            // Prefer better interconnect to communication partners
            for partner_gpu in communicate_with_gpus {
                if let Some(links) = self.gpu_adjacency.get(&gpu.gpu_id) {
                    for (neighbor, link) in links {
                        if neighbor == partner_gpu {
                            // Higher bandwidth = better score
                            score += link.effective_bandwidth_gbs;
                            // Lower latency = better score
                            score -= link.interconnect.latency_us() * 0.1;
                        }
                    }
                }
            }

            // Prefer NUMA node with more available memory
            if let Some(numa_node) = self.numa_nodes.get(&gpu.numa_node_id) {
                score += numa_node.available_memory_gb * 5.0;
            }

            if score > best_score {
                best_score = score;
                best_gpu = Some(gpu);
            }
        }

        best_gpu.map(|g| g.gpu_id.clone())
    }

    /// Get shortest path between two GPUs (by latency)
    pub fn find_shortest_path(&self, from_gpu: &str, to_gpu: &str) -> Option<Vec<String>> {
        // BFS for shortest path
        let mut visited = HashSet::new();
        let mut queue = vec![(from_gpu.to_string(), vec![from_gpu.to_string()])];
        visited.insert(from_gpu.to_string());

        while let Some((current, path)) = queue.pop() {
            if current == to_gpu {
                return Some(path);
            }

            if let Some(neighbors) = self.gpu_adjacency.get(&current) {
                for (neighbor, _link) in neighbors {
                    if !visited.contains(neighbor) {
                        visited.insert(neighbor.clone());
                        let mut new_path = path.clone();
                        new_path.push(neighbor.clone());
                        queue.push((neighbor.clone(), new_path));
                    }
                }
            }
        }

        None // No path found
    }

    /// Calculate total bandwidth between two GPUs (sum of all paths)
    pub fn calculate_total_bandwidth(&self, from_gpu: &str, to_gpu: &str) -> f64 {
        // Direct link bandwidth
        if let Some(links) = self.gpu_adjacency.get(from_gpu) {
            for (neighbor, link) in links {
                if neighbor == to_gpu {
                    return link.effective_bandwidth_gbs;
                }
            }
        }

        // No direct link - must go through system memory (slow)
        0.0
    }

    /// Get all GPUs on a specific NUMA node
    pub fn get_gpus_on_numa_node(&self, numa_node_id: u32) -> Vec<&GpuInfo> {
        self.gpus.values()
            .filter(|g| g.numa_node_id == numa_node_id)
            .collect()
    }

    /// Check if two GPUs are on the same NUMA node
    pub fn same_numa_node(&self, gpu_a: &str, gpu_b: &str) -> bool {
        if let Some(gpu_a_info) = self.gpus.get(gpu_a) {
            if let Some(gpu_b_info) = self.gpus.get(gpu_b) {
                return gpu_a_info.numa_node_id == gpu_b_info.numa_node_id;
            }
        }
        false
    }

    /// Detect if system has NVLink connectivity
    pub fn has_nvlink(&self) -> bool {
        self.gpu_links.iter().any(|l| l.interconnect == InterconnectType::NvLink)
    }

    /// Get topology summary for logging
    pub fn get_summary(&self) -> TopologySummary {
        TopologySummary {
            total_numa_nodes: self.numa_nodes.len(),
            total_gpus: self.gpus.len(),
            total_nvlink_connections: self.gpu_links.iter()
                .filter(|l| l.interconnect == InterconnectType::NvLink)
                .count(),
            total_pcie_connections: self.gpu_links.iter()
                .filter(|l| l.interconnect == InterconnectType::Pcie)
                .count(),
            avg_gpu_vram_gb: if self.gpus.is_empty() { 0.0 } else {
                self.gpus.values().map(|g| g.vram_gb).sum::<f64>() / self.gpus.len() as f64
            },
        }
    }
}

impl Default for HardwareTopology {
    fn default() -> Self {
        Self::new()
    }
}

/// Summary of hardware topology
#[derive(Debug, Clone, Serialize)]
pub struct TopologySummary {
    pub total_numa_nodes: usize,
    pub total_gpus: usize,
    pub total_nvlink_connections: usize,
    pub total_pcie_connections: usize,
    pub avg_gpu_vram_gb: f64,
}

// ============================================================================
//! Topology-Aware Task Placement
// ============================================================================

/// Task placement recommendation
#[derive(Debug, Clone, Serialize)]
pub struct PlacementRecommendation {
    pub primary_gpu: String,
    pub secondary_gpus: Vec<String>,
    pub numa_node: u32,
    pub reasoning: String,
    pub estimated_communication_cost: f64, // Lower is better
}

/// Topology-aware scheduler
pub struct TopologyAwareScheduler {
    topology: HardwareTopology,
}

impl TopologyAwareScheduler {
    pub fn new(topology: HardwareTopology) -> Self {
        let summary = topology.get_summary();
        info!(
            "TopologyAwareScheduler initialized: {} NUMA nodes, {} GPUs, {} NVLink connections",
            summary.total_numa_nodes,
            summary.total_gpus,
            summary.total_nvlink_connections
        );

        Self { topology }
    }

    /// Recommend optimal GPU placement for a task
    pub fn recommend_placement(
        &self,
        required_vram_gb: f64,
        communicate_with_gpus: &[String],
        prefer_local_numa: Option<u32>,
    ) -> Option<PlacementRecommendation> {
        let primary_gpu = self.topology.select_best_gpu(
            required_vram_gb,
            prefer_local_numa,
            communicate_with_gpus,
        )?;

        let gpu_info = self.topology.gpus.get(&primary_gpu)?;
        let numa_node = gpu_info.numa_node_id;

        // Find secondary GPUs on same NUMA node for pipeline parallelism
        let secondary_gpus: Vec<String> = self.topology.get_gpus_on_numa_node(numa_node)
            .into_iter()
            .filter(|g| g.gpu_id != primary_gpu)
            .map(|g| g.gpu_id.clone())
            .collect();

        // Estimate communication cost
        let mut total_cost = 0.0;
        for partner in communicate_with_gpus {
            let bandwidth = self.topology.calculate_total_bandwidth(&primary_gpu, partner);
            if bandwidth > 0.0 {
                total_cost += 1.0 / bandwidth; // Lower bandwidth = higher cost
            } else {
                total_cost += 10.0; // No direct link = very high cost
            }
        }

        let reasoning = format!(
            "Selected GPU {} on NUMA node {} (VRAM: {:.1}GB available, {} secondaries on same NUMA)",
            primary_gpu,
            numa_node,
            gpu_info.available_vram_gb,
            secondary_gpus.len()
        );

        Some(PlacementRecommendation {
            primary_gpu,
            secondary_gpus,
            numa_node,
            reasoning,
            estimated_communication_cost: total_cost,
        })
    }

    /// Get the underlying topology
    pub fn topology(&self) -> &HardwareTopology {
        &self.topology
    }
}

// ============================================================================
//! System Topology Detection (Linux-specific via sysfs)
// ============================================================================

/// Detect hardware topology from system (Linux-only via sysfs)
pub fn detect_system_topology() -> anyhow::Result<HardwareTopology> {
    let mut topology = HardwareTopology::new();

    // Detect NUMA nodes
    if let Ok(numa_nodes) = std::fs::read_dir("/sys/devices/system/node") {
        for entry in numa_nodes {
            if let Ok(entry) = entry {
                if let Some(name) = entry.file_name().to_str() {
                    if name.starts_with("node") {
                        if let Ok(node_id) = name[4..].parse::<u32>() {
                            // Read memory info
                            let meminfo_path = entry.path().join("meminfo");
                            let mut memory_gb = 0.0;

                            if let Ok(content) = std::fs::read_to_string(&meminfo_path) {
                                for line in content.lines() {
                                    if line.contains("MemTotal") {
                                        if let Some(kb_str) = line.split_whitespace().nth(1) {
                                            if let Ok(kb) = kb_str.parse::<u64>() {
                                                memory_gb = kb as f64 / 1024.0 / 1024.0;
                                            }
                                        }
                                    }
                                }
                            }

                            // Read CPU cores
                            let cpulist_path = entry.path().join("cpulist");
                            let mut cpu_cores = Vec::new();

                            if let Ok(content) = std::fs::read_to_string(&cpulist_path) {
                                // Parse CPU list (e.g., "0-7,16-23")
                                for range in content.trim().split(',') {
                                    if let Some((start, end)) = range.split_once('-') {
                                        if let (Ok(s), Ok(e)) = (start.parse::<u32>(), end.parse::<u32>()) {
                                            cpu_cores.extend(s..=e);
                                        }
                                    } else if let Ok(core) = range.trim().parse::<u32>() {
                                        cpu_cores.push(core);
                                    }
                                }
                            }

                            let numa_node = NumaNode::new(node_id, cpu_cores, memory_gb);
                            topology.add_numa_node(numa_node);
                        }
                    }
                }
            }
        }
    }

    // Note: GPU detection would require nvidia-smi or rocm-smi integration
    // This is a simplified implementation

    info!("Detected {} NUMA nodes", topology.numa_nodes.len());
    Ok(topology)
}

// ============================================================================
//! Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_topology() -> HardwareTopology {
        let mut topology = HardwareTopology::new();

        // Create 2 NUMA nodes
        let numa0 = NumaNode::new(0, vec![0, 1, 2, 3], 64.0);
        let numa1 = NumaNode::new(1, vec![4, 5, 6, 7], 64.0);
        topology.add_numa_node(numa0);
        topology.add_numa_node(numa1);

        // Create 4 GPUs (2 per NUMA node)
        let gpu0 = GpuInfo {
            gpu_id: "gpu-0".to_string(),
            model: "RTX-4090".to_string(),
            pci_bus_id: "0000:01:00.0".to_string(),
            numa_node_id: 0,
            vram_gb: 24.0,
            available_vram_gb: 20.0,
            compute_capability: 8.9,
            tflops_fp16: 82.0,
            memory_bandwidth_gbs: 1008.0,
        };

        let gpu1 = GpuInfo {
            gpu_id: "gpu-1".to_string(),
            model: "RTX-4090".to_string(),
            pci_bus_id: "0000:02:00.0".to_string(),
            numa_node_id: 0,
            vram_gb: 24.0,
            available_vram_gb: 18.0,
            compute_capability: 8.9,
            tflops_fp16: 82.0,
            memory_bandwidth_gbs: 1008.0,
        };

        let gpu2 = GpuInfo {
            gpu_id: "gpu-2".to_string(),
            model: "RTX-4090".to_string(),
            pci_bus_id: "0000:03:00.0".to_string(),
            numa_node_id: 1,
            vram_gb: 24.0,
            available_vram_gb: 22.0,
            compute_capability: 8.9,
            tflops_fp16: 82.0,
            memory_bandwidth_gbs: 1008.0,
        };

        let gpu3 = GpuInfo {
            gpu_id: "gpu-3".to_string(),
            model: "RTX-4090".to_string(),
            pci_bus_id: "0000:04:00.0".to_string(),
            numa_node_id: 1,
            vram_gb: 24.0,
            available_vram_gb: 15.0,
            compute_capability: 8.9,
            tflops_fp16: 82.0,
            memory_bandwidth_gbs: 1008.0,
        };

        topology.add_gpu(gpu0);
        topology.add_gpu(gpu1);
        topology.add_gpu(gpu2);
        topology.add_gpu(gpu3);

        // Add NVLink between GPU 0 and 1 (same NUMA)
        let nvlink_01 = GpuLink::new("gpu-0".to_string(), "gpu-1".to_string(), InterconnectType::NvLink, 1);
        topology.add_gpu_link(nvlink_01);

        // Add NVLink between GPU 2 and 3 (same NUMA)
        let nvlink_23 = GpuLink::new("gpu-2".to_string(), "gpu-3".to_string(), InterconnectType::NvLink, 1);
        topology.add_gpu_link(nvlink_23);

        // Add PCIe between NUMA nodes
        let pcie_02 = GpuLink::new("gpu-0".to_string(), "gpu-2".to_string(), InterconnectType::Pcie, 1);
        topology.add_gpu_link(pcie_02);

        topology
    }

    #[test]
    fn test_numa_node_creation() {
        let node = NumaNode::new(0, vec![0, 1, 2, 3], 64.0);
        assert_eq!(node.node_id, 0);
        assert_eq!(node.cpu_cores.len(), 4);
        assert_eq!(node.memory_gb, 64.0);
    }

    #[test]
    fn test_gpu_selection_same_numa() {
        let topology = create_test_topology();
        let scheduler = TopologyAwareScheduler::new(topology);

        // Request GPU with preference for NUMA node 0
        let recommendation = scheduler.recommend_placement(
            10.0,  // Need 10GB VRAM
            &[],   // No communication requirements
            Some(0), // Prefer NUMA node 0
        );

        assert!(recommendation.is_some());
        let rec = recommendation.unwrap();
        assert_eq!(rec.numa_node, 0);
        assert!(rec.primary_gpu.starts_with("gpu-"));
    }

    #[test]
    fn test_gpu_selection_with_communication() {
        let topology = create_test_topology();
        let scheduler = TopologyAwareScheduler::new(topology);

        // Request GPU that needs to communicate with gpu-0
        // Should prefer gpu-1 (NVLink) over gpu-2 (PCIe)
        let recommendation = scheduler.recommend_placement(
            10.0,
            &["gpu-0".to_string()],
            None,
        );

        assert!(recommendation.is_some());
        let rec = recommendation.unwrap();
        // gpu-1 has NVLink to gpu-0, should be preferred
        assert_eq!(rec.primary_gpu, "gpu-1");
    }

    #[test]
    fn test_same_numa_node_check() {
        let topology = create_test_topology();

        assert!(topology.same_numa_node("gpu-0", "gpu-1")); // Both on NUMA 0
        assert!(topology.same_numa_node("gpu-2", "gpu-3")); // Both on NUMA 1
        assert!(!topology.same_numa_node("gpu-0", "gpu-2")); // Different NUMA
    }

    #[test]
    fn test_nvlink_detection() {
        let topology = create_test_topology();
        assert!(topology.has_nvlink());
    }

    #[test]
    fn test_shortest_path() {
        let topology = create_test_topology();

        // Direct NVLink path
        let path = topology.find_shortest_path("gpu-0", "gpu-1");
        assert!(path.is_some());
        let path = path.unwrap();
        assert_eq!(path.len(), 2);

        // Direct PCIe path
        let path = topology.find_shortest_path("gpu-0", "gpu-2");
        assert!(path.is_some());
    }

    #[test]
    fn test_topology_summary() {
        let topology = create_test_topology();
        let summary = topology.get_summary();

        assert_eq!(summary.total_numa_nodes, 2);
        assert_eq!(summary.total_gpus, 4);
        assert_eq!(summary.total_nvlink_connections, 2);
        assert_eq!(summary.total_pcie_connections, 1);
        assert!((summary.avg_gpu_vram_gb - 24.0).abs() < 0.1);
    }

    #[test]
    fn test_interconnect_bandwidth() {
        assert!(InterconnectType::NvLink.bandwidth_gbs() > InterconnectType::Pcie.bandwidth_gbs());
        assert!(InterconnectType::Pcie.bandwidth_gbs() > InterconnectType::None.bandwidth_gbs());
    }
}
