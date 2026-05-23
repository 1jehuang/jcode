//! GPU inference load balancer
//!
//! Provides intelligent GPU resource allocation and load balancing for AI inference:
//! - GPU topology awareness (NUMA, NVLink, PCIe)
//! - Model placement optimization
//! - Dynamic batch sizing based on GPU memory
//! - Multi-GPU pipeline parallelism
//! - MIG (Multi-Instance GPU) support for A100/H100

use std::collections::{HashMap, HashSet};
use serde::{Serialize, Deserialize};

// ============================================================================
// GPU Hardware Abstraction
// ============================================================================

/// GPU device information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    /// Unique GPU identifier
    pub gpu_id: u32,
    /// GPU model name (e.g., "NVIDIA A100-SXM4-80GB")
    pub model: String,
    /// Total VRAM in bytes
    pub total_vram_bytes: u64,
    /// Available VRAM in bytes
    pub available_vram_bytes: u64,
    /// GPU compute capability (e.g., 8.0 for A100)
    pub compute_capability: f32,
    /// Tensor cores available
    pub has_tensor_cores: bool,
    /// FP16 TFLOPS
    pub fp16_tflops: f64,
    /// INT8 TOPS
    pub int8_tops: f64,
    /// Memory bandwidth (GB/s)
    pub memory_bandwidth_gbps: f64,
    /// NVLink bandwidth to other GPUs (gpu_id -> bandwidth_gbps)
    pub nvlink_bandwidth: HashMap<u32, f64>,
    /// PCIe generation and lanes
    pub pcie_gen: u32,
    pub pcie_lanes: u32,
    /// NUMA node affinity
    pub numa_node: u32,
    /// Current utilization (0-10000 scale)
    pub utilization: u32,
    /// Temperature in Celsius
    pub temperature_c: u32,
    /// Power usage (watts)
    pub power_watts: u32,
    /// Max power limit (watts)
    pub max_power_watts: u32,
}

impl GpuInfo {
    /// Create estimated GpuInfo from model name (for systems without NVML)
    pub fn estimate(model: &str, gpu_id: u32) -> Self {
        let vram_gb =
            if model.contains("H100") || model.contains("A100") { 80.0 }
            else if model.contains("A10") || model.contains("4090") || model.contains("3090") { 24.0 }
            else if model.contains("V100") { 32.0 }
            else if model.contains("3080") { 12.0 }
            else if model.contains("4080") { 16.0 }
            else if model.contains("4060") { 8.0 }
            else { 16.0 };

        let vram_bytes = (vram_gb * 1024.0 * 1024.0 * 1024.0) as u64;
        Self {
            gpu_id,
            model: model.to_string(),
            total_vram_bytes: vram_bytes,
            available_vram_bytes: vram_bytes,
            compute_capability: if model.contains("H100") { 9.0 } else if model.contains("A100") { 8.0 } else if model.contains("V100") { 7.0 } else { 8.5 },
            has_tensor_cores: true,
            fp16_tflops: crate::gpu_discovery::estimate_fp16_tflops(model, 0),
            int8_tops: crate::gpu_discovery::estimate_int8_tops(model, 0),
            memory_bandwidth_gbps: crate::gpu_discovery::estimate_memory_bandwidth(model),
            nvlink_bandwidth: HashMap::new(),
            pcie_gen: 4,
            pcie_lanes: 16,
            numa_node: 0,
            utilization: 0,
            temperature_c: 40,
            power_watts: 0,
            max_power_watts: 0,
        }
    }

    /// Calculate remaining VRAM percentage
    pub fn vram_usage_percent(&self) -> f64 {
        if self.total_vram_bytes == 0 {
            return 0.0;
        }
        let used = self.total_vram_bytes - self.available_vram_bytes;
        (used as f64 / self.total_vram_bytes as f64) * 100.0
    }

    /// Check if GPU can fit a model of given size
    pub fn can_fit_model(&self, model_size_bytes: u64, batch_size: u32, seq_len: u32) -> bool {
        // Estimate KV cache size: 2 * num_layers * hidden_size * batch * seq_len * sizeof(float16)
        // Simplified: assume ~2KB per token per layer for large models
        let kv_cache_estimate = 2048 * seq_len as u64 * batch_size as u64;
        let total_needed = model_size_bytes + kv_cache_estimate;

        // Leave 10% headroom for fragmentation
        self.available_vram_bytes >= (total_needed as f64 * 1.1) as u64
    }

    /// Get effective compute score (higher = better)
    pub fn compute_score(&self) -> f64 {
        let vram_score = self.available_vram_bytes as f64 / self.total_vram_bytes.max(1) as f64;
        let util_penalty = (10000 - self.utilization) as f64 / 10000.0;
        let temp_penalty = if self.temperature_c > 80 {
            0.7
        } else if self.temperature_c > 70 {
            0.85
        } else {
            1.0
        };

        self.fp16_tflops * vram_score * util_penalty * temp_penalty
    }
}

/// GPU cluster topology
#[derive(Debug, Clone)]
pub struct GpuTopology {
    /// All GPUs in the system
    pub gpus: HashMap<u32, GpuInfo>,
    /// NUMA nodes
    pub numa_nodes: HashMap<u32, Vec<u32>>, // numa_node_id -> [gpu_ids]
    /// NVLink groups (fully connected groups)
    pub nvlink_groups: Vec<HashSet<u32>>,
    /// Fastest path between GPUs (gpu_id -> gpu_id -> bandwidth_gbps)
    pub interconnect_matrix: HashMap<u32, HashMap<u32, f64>>,
}

impl GpuTopology {
    /// Create from system discovery using NVML
    pub fn discover() -> Result<Self, String> {
        // Use NVML-based discovery if available
        #[cfg(feature = "gpu-discovery")]
        {
            crate::gpu_discovery::discover_gpus_nvml()
        }
        #[cfg(not(feature = "gpu-discovery"))]
        {
            Ok(Self::estimate())
        }
    }

    /// Create estimated GPU topology (for systems without NVML)
    pub fn estimate() -> Self {
        let known_models = [
            "NVIDIA H100 80GB",
            "NVIDIA A100 80GB",
            "NVIDIA A10 24GB",
            "NVIDIA V100 32GB",
            "NVIDIA GeForce RTX 4090",
            "NVIDIA GeForce RTX 3090",
        ];
        let mut gpus = HashMap::new();
        let mut interconnect_matrix = HashMap::new();
        for (i, name) in known_models.iter().enumerate() {
            let gpu_id = i as u32;
            let gpu = GpuInfo::estimate(name, gpu_id);
            interconnect_matrix.entry(gpu_id).or_insert_with(HashMap::new);
            gpus.insert(gpu_id, gpu);
        }
        let nvlink_groups = crate::gpu_discovery::detect_nvlink_groups(&gpus);
        Self { gpus, numa_nodes: HashMap::new(), nvlink_groups, interconnect_matrix }
    }

    /// Find best GPU for a model based on VRAM and compute
    pub fn find_best_gpu(&self, model_size_bytes: u64, batch_size: u32, seq_len: u32) -> Option<u32> {
        let mut best_gpu: Option<(u32, f64)> = None;

        for (&gpu_id, gpu) in &self.gpus {
            if gpu.can_fit_model(model_size_bytes, batch_size, seq_len) {
                let score = gpu.compute_score();
                match best_gpu {
                    None => best_gpu = Some((gpu_id, score)),
                    Some((_, best_score)) if score > best_score => {
                        best_gpu = Some((gpu_id, score));
                    }
                    _ => {}
                }
            }
        }

        best_gpu.map(|(id, _)| id)
    }

    /// Find optimal GPU placement for multi-GPU model parallelism
    pub fn find_multi_gpu_placement(
        &self,
        num_gpus_needed: u32,
        model_size_per_gpu: u64,
    ) -> Option<Vec<u32>> {
        // Prefer GPUs within same NVLink group for faster communication
        for group in &self.nvlink_groups {
            if group.len() >= num_gpus_needed as usize {
                let candidates: Vec<_> = group.iter().cloned().collect();
                let mut selected = Vec::new();

                for &gpu_id in &candidates {
                    if let Some(gpu) = self.gpus.get(&gpu_id) {
                        if gpu.available_vram_bytes >= model_size_per_gpu {
                            selected.push(gpu_id);
                            if selected.len() == num_gpus_needed as usize {
                                return Some(selected);
                            }
                        }
                    }
                }
            }
        }

        // Fallback: select top-N GPUs by compute score regardless of topology
        let mut scored_gpus: Vec<_> = self.gpus.iter()
            .filter(|(_, gpu)| gpu.available_vram_bytes >= model_size_per_gpu)
            .map(|(&id, gpu)| (id, gpu.compute_score()))
            .collect();

        scored_gpus.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        Some(
            scored_gpus.into_iter()
                .take(num_gpus_needed as usize)
                .map(|(id, _)| id)
                .collect()
        )
    }

    /// Get bandwidth between two GPUs
    pub fn get_bandwidth(&self, gpu_a: u32, gpu_b: u32) -> f64 {
        self.interconnect_matrix
            .get(&gpu_a)
            .and_then(|m| m.get(&gpu_b))
            .copied()
            .unwrap_or(0.0)
    }
}

// ============================================================================
// GPU Load Balancer
// ============================================================================

/// GPU-aware load balancing strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuLoadBalanceStrategy {
    /// Minimize latency - pick fastest available GPU
    LatencyOptimized,
    /// Maximize throughput - distribute evenly across GPUs
    ThroughputOptimized,
    /// Minimize power - prefer cooler/less utilized GPUs
    PowerOptimized,
    /// Balance all factors
    Balanced,
}

/// Request context for GPU scheduling
#[derive(Debug, Clone)]
pub struct GpuInferenceRequest {
    /// Request ID
    pub request_id: String,
    /// Model identifier
    pub model_name: String,
    /// Model size in bytes
    pub model_size_bytes: u64,
    /// Batch size
    pub batch_size: u32,
    /// Sequence length
    pub seq_len: u32,
    /// Preferred precision
    pub precision: Precision,
    /// Maximum acceptable latency (ms)
    pub max_latency_ms: u64,
    /// Priority (higher = more important)
    pub priority: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    FP16,
    INT8,
    FP32,
    BF16,
}

/// GPU scheduling decision
#[derive(Debug, Clone)]
pub struct GpuSchedulingDecision {
    /// Selected GPU IDs
    pub gpu_ids: Vec<u32>,
    /// Estimated latency (ms)
    pub estimated_latency_ms: f64,
    /// Expected throughput (tokens/sec)
    pub expected_throughput: f64,
    /// Reason for selection
    pub reason: String,
}

/// GPU load balancer
#[derive(Debug)]
pub struct GpuLoadBalancer {
    topology: GpuTopology,
    strategy: GpuLoadBalanceStrategy,
    /// Active requests per GPU
    active_requests: HashMap<u32, usize>,
    /// Historical latency data (gpu_id -> [latency_ms])
    latency_history: HashMap<u32, Vec<f64>>,
}

impl GpuLoadBalancer {
    pub fn new(topology: GpuTopology, strategy: GpuLoadBalanceStrategy) -> Self {
        Self {
            topology,
            strategy,
            active_requests: HashMap::new(),
            latency_history: HashMap::new(),
        }
    }

    /// Schedule an inference request to optimal GPU(s)
    pub fn schedule(&mut self, request: &GpuInferenceRequest) -> Option<GpuSchedulingDecision> {
        match self.strategy {
            GpuLoadBalanceStrategy::LatencyOptimized => {
                self.schedule_for_latency(request)
            }
            GpuLoadBalanceStrategy::ThroughputOptimized => {
                self.schedule_for_throughput(request)
            }
            GpuLoadBalanceStrategy::PowerOptimized => {
                self.schedule_for_power(request)
            }
            GpuLoadBalanceStrategy::Balanced => {
                self.schedule_balanced(request)
            }
        }
    }

    fn schedule_for_latency(&self, request: &GpuInferenceRequest) -> Option<GpuSchedulingDecision> {
        // Find GPU with lowest estimated latency
        let best_gpu = self.topology.find_best_gpu(
            request.model_size_bytes,
            request.batch_size,
            request.seq_len,
        )?;

        let gpu = self.topology.gpus.get(&best_gpu)?;
        let estimated_latency = self.estimate_latency(gpu, request);

        Some(GpuSchedulingDecision {
            gpu_ids: vec![best_gpu],
            estimated_latency_ms: estimated_latency,
            expected_throughput: 1000.0 / estimated_latency.max(0.1),
            reason: format!("Lowest latency GPU (score: {:.2})", gpu.compute_score()),
        })
    }

    fn schedule_for_throughput(&mut self, request: &GpuInferenceRequest) -> Option<GpuSchedulingDecision> {
        // Distribute to least loaded GPU that can fit the model
        let mut candidates: Vec<_> = self.topology.gpus.iter()
            .filter(|(_, gpu)| gpu.can_fit_model(
                request.model_size_bytes,
                request.batch_size,
                request.seq_len,
            ))
            .collect();

        candidates.sort_by(|a, b| {
            let load_a = self.active_requests.get(a.0).unwrap_or(&0);
            let load_b = self.active_requests.get(b.0).unwrap_or(&0);
            load_a.cmp(load_b)
        });

        if let Some((gpu_id, gpu)) = candidates.first() {
            let gpu_id = **gpu_id;
            let estimated_latency = self.estimate_latency(gpu, request);
            *self.active_requests.entry(gpu_id).or_insert(0) += 1;

            Some(GpuSchedulingDecision {
                gpu_ids: vec![gpu_id],
                estimated_latency_ms: estimated_latency,
                expected_throughput: 1000.0 / estimated_latency.max(0.1),
                reason: format!("Least loaded GPU ({} active requests)",
                               self.active_requests.get(&gpu_id).unwrap_or(&0)),
            })
        } else {
            None
        }
    }

    fn schedule_for_power(&self, request: &GpuInferenceRequest) -> Option<GpuSchedulingDecision> {
        // Prefer cooler, less power-hungry GPUs
        let mut candidates: Vec<_> = self.topology.gpus.iter()
            .filter(|(_, gpu)| gpu.can_fit_model(
                request.model_size_bytes,
                request.batch_size,
                request.seq_len,
            ))
            .collect();

        candidates.sort_by(|a, b| {
            // Score based on temperature and power usage
            let score_a = a.1.temperature_c as f64 + (a.1.power_watts as f64 / a.1.max_power_watts.max(1) as f64) * 100.0;
            let score_b = b.1.temperature_c as f64 + (b.1.power_watts as f64 / b.1.max_power_watts.max(1) as f64) * 100.0;
            score_a.partial_cmp(&score_b).unwrap_or(std::cmp::Ordering::Equal)
        });

        if let Some((gpu_id, gpu)) = candidates.first() {
            let gpu_id = **gpu_id;
            let estimated_latency = self.estimate_latency(gpu, request);

            Some(GpuSchedulingDecision {
                gpu_ids: vec![gpu_id],
                estimated_latency_ms: estimated_latency,
                expected_throughput: 1000.0 / estimated_latency.max(0.1),
                reason: format!("Most power-efficient GPU (temp: {}°C, power: {}W)",
                               gpu.temperature_c, gpu.power_watts),
            })
        } else {
            None
        }
    }

    fn schedule_balanced(&self, request: &GpuInferenceRequest) -> Option<GpuSchedulingDecision> {
        // Weighted combination of latency, throughput, and power
        let mut best: Option<(u32, f64, &GpuInfo)> = None;

        for (&gpu_id, gpu) in &self.topology.gpus {
            if !gpu.can_fit_model(request.model_size_bytes, request.batch_size, request.seq_len) {
                continue;
            }

            let load = self.active_requests.get(&gpu_id).unwrap_or(&0);
            let latency_score = 1.0 / self.estimate_latency(gpu, request).max(0.1);
            let load_score = 1.0 / (1.0 + *load as f64);
            let power_score = 1.0 - (gpu.temperature_c as f64 / 100.0);
            let compute_score = gpu.compute_score() / 100.0; // Normalize

            // Weighted combination
            let combined_score = latency_score * 0.4 + load_score * 0.3 + power_score * 0.15 + compute_score * 0.15;

            match best {
                None => best = Some((gpu_id, combined_score, gpu)),
                Some((_, best_score, _)) if combined_score > best_score => {
                    best = Some((gpu_id, combined_score, gpu));
                }
                _ => {}
            }
        }

        if let Some((gpu_id, _, gpu)) = best {
            let estimated_latency = self.estimate_latency(gpu, request);

            Some(GpuSchedulingDecision {
                gpu_ids: vec![gpu_id],
                estimated_latency_ms: estimated_latency,
                expected_throughput: 1000.0 / estimated_latency.max(0.1),
                reason: "Best balanced choice".to_string(),
            })
        } else {
            None
        }
    }

    /// Estimate inference latency for a request on a GPU
    fn estimate_latency(&self, gpu: &GpuInfo, request: &GpuInferenceRequest) -> f64 {
        // Simple roofline model estimation
        let total_flops = (request.seq_len * request.batch_size) as f64 * 2.0; // Approximate

        // Compute-bound time
        let compute_time_ms = match request.precision {
            Precision::FP16 | Precision::BF16 => {
                (total_flops / (gpu.fp16_tflops * 1e12)) * 1000.0
            }
            Precision::INT8 => {
                (total_flops / (gpu.int8_tops * 1e12)) * 1000.0
            }
            Precision::FP32 => {
                (total_flops / (gpu.fp16_tflops * 0.5 * 1e12)) * 1000.0 // FP32 ~ half FP16 throughput
            }
        };

        // Memory-bound time
        let memory_time_ms = (request.model_size_bytes as f64 / (gpu.memory_bandwidth_gbps * 1e9)) * 1000.0;

        // Take the bottleneck
        let base_latency = compute_time_ms.max(memory_time_ms);

        // Add overhead for current load
        let load_factor = 1.0 + (*self.active_requests.get(&gpu.gpu_id).unwrap_or(&0) as f64 * 0.1);

        base_latency * load_factor
    }

    /// Record actual latency for learning
    pub fn record_latency(&mut self, gpu_id: u32, latency_ms: f64) {
        self.latency_history
            .entry(gpu_id)
            .or_default()
            .push(latency_ms);

        // Keep only last 100 samples
        if let Some(history) = self.latency_history.get_mut(&gpu_id) {
            if history.len() > 100 {
                history.drain(..history.len() - 100);
            }
        }
    }

    /// Release GPU resources after request completion
    pub fn release_gpu(&mut self, gpu_id: u32) {
        if let Some(count) = self.active_requests.get_mut(&gpu_id) {
            *count = count.saturating_sub(1);
        }
    }

    /// Get GPU utilization statistics
    pub fn get_stats(&self) -> GpuStats {
        let total_gpus = self.topology.gpus.len();
        let active_gpus = self.active_requests.iter()
            .filter(|(_, count)| **count > 0)
            .count();

        let avg_utilization = if total_gpus > 0 {
            self.topology.gpus.values()
                .map(|gpu| gpu.utilization)
                .sum::<u32>() as f64 / total_gpus as f64
        } else {
            0.0
        };

        let total_vram = self.topology.gpus.values()
            .map(|gpu| gpu.total_vram_bytes)
            .sum::<u64>();

        let used_vram = self.topology.gpus.values()
            .map(|gpu| gpu.total_vram_bytes - gpu.available_vram_bytes)
            .sum::<u64>();

        GpuStats {
            total_gpus,
            active_gpus,
            avg_utilization,
            total_vram_bytes: total_vram,
            used_vram_bytes: used_vram,
            pending_requests: self.active_requests.values().sum(),
        }
    }
}

/// GPU cluster statistics
#[derive(Debug, Clone)]
pub struct GpuStats {
    pub total_gpus: usize,
    pub active_gpus: usize,
    pub avg_utilization: f64,
    pub total_vram_bytes: u64,
    pub used_vram_bytes: u64,
    pub pending_requests: usize,
}

impl GpuStats {
    pub fn vram_usage_percent(&self) -> f64 {
        if self.total_vram_bytes == 0 {
            return 0.0;
        }
        (self.used_vram_bytes as f64 / self.total_vram_bytes as f64) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_topology() -> GpuTopology {
        let mut gpus = HashMap::new();

        gpus.insert(0, GpuInfo {
            gpu_id: 0,
            model: "NVIDIA A100-SXM4-80GB".to_string(),
            total_vram_bytes: 80 * 1024 * 1024 * 1024,
            available_vram_bytes: 60 * 1024 * 1024 * 1024,
            compute_capability: 8.0,
            has_tensor_cores: true,
            fp16_tflops: 312.0,
            int8_tops: 624.0,
            memory_bandwidth_gbps: 2039.0,
            nvlink_bandwidth: HashMap::new(),
            pcie_gen: 4,
            pcie_lanes: 16,
            numa_node: 0,
            utilization: 5000,
            temperature_c: 65,
            power_watts: 250,
            max_power_watts: 400,
        });

        GpuTopology {
            gpus,
            numa_nodes: HashMap::new(),
            nvlink_groups: vec![],
            interconnect_matrix: HashMap::new(),
        }
    }

    #[test]
    fn test_gpu_can_fit_model() {
        let topology = create_test_topology();
        let gpu = topology.gpus.get(&0).unwrap();

        // 10GB model with batch=1, seq_len=512 should fit
        assert!(gpu.can_fit_model(10 * 1024 * 1024 * 1024, 1, 512));

        // 100GB model should not fit
        assert!(!gpu.can_fit_model(100 * 1024 * 1024 * 1024, 1, 512));
    }

    #[test]
    fn test_find_best_gpu() {
        let topology = create_test_topology();

        let result = topology.find_best_gpu(
            10 * 1024 * 1024 * 1024, // 10GB model
            1,
            512,
        );

        assert_eq!(result, Some(0));
    }
}
