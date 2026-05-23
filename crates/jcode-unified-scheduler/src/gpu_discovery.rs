//! NVIDIA NVML GPU discovery module
//!
//! Provides real-time GPU information using NVIDIA Management Library (NVML).
//! This is a wrapper around the nvml-wrapper crate for safe Rust access.
//!
//! Requires:
//! - NVIDIA GPU with drivers installed
//! - NVML library available (libnvidia-ml.so on Linux, nvml.dll on Windows)
//! - Feature flag: gpu-discovery

use super::gpu_load_balancer::{GpuInfo, GpuTopology};
use std::collections::{HashMap, HashSet};

/// Discover GPUs using NVML
#[cfg(feature = "gpu-discovery")]
pub fn discover_gpus_nvml() -> Result<GpuTopology, String> {
    use nvml_wrapper::Nvml;

    // Initialize NVML
    let nvml = Nvml::init().map_err(|e| format!("Failed to initialize NVML: {:?}", e))?;

    // Get device count
    let device_count = nvml.device_count().map_err(|e| format!("Failed to get device count: {:?}", e))?;

    if device_count == 0 {
        return Err("No NVIDIA GPUs found".to_string());
    }

    let mut gpus = HashMap::new();
    let mut numa_nodes: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut nvlink_groups: Vec<HashSet<u32>> = Vec::new();
    let mut interconnect_matrix: HashMap<u32, HashMap<u32, f64>> = HashMap::new();

    for i in 0..device_count {
        let device = nvml.device_by_index(i).map_err(|e| format!("Failed to get device {}: {:?}", i, e))?;

        // Get basic info
        let name = device.name().map_err(|e| format!("Failed to get GPU name: {:?}", e))?;
        let uuid = device.uuid().map_err(|e| format!("Failed to get GPU UUID: {:?}", e))?;

        // Memory info
        let mem_info = device.memory_info().map_err(|e| format!("Failed to get memory info: {:?}", e))?;
        let total_vram = mem_info.total;
        let used_vram = mem_info.used;
        let available_vram = total_vram - used_vram;

        // Compute capability
        let compute_cap = device.cuda_compute_capability().map_err(|e| format!("Failed to get compute cap: {:?}", e))?;

        // Utilization
        let utilization = device.utilization_rates().map_err(|e| format!("Failed to get utilization: {:?}", e))?;
        let gpu_util = utilization.gpu; // 0-100 scale

        // Temperature
        let temp = device.temperature(nvml_wrapper::enum_wrappers::TemperatureSensor::Gpu)
            .unwrap_or(0);

        // Power usage
        let power = device.power_usage().map_err(|e| format!("Failed to get power: {:?}", e)).unwrap_or(0) / 1000; // mW -> W
        let power_limit = device.enforced_power_limit().map_err(|e| format!("Failed to get power limit: {:?}", e)).unwrap_or(0) / 1000;

        // PCIe and NUMA info
        let pci_info = device.pci_info().map_err(|e| format!("Failed to get PCI info: {:?}", e))?;
        let numa_node = 0; // TODO: Get actual NUMA node from system

        // Build NVLink topology
        let mut nvlink_bandwidth = HashMap::new();
        let peer_count = device.num_gpus();
        for peer_idx in 0..peer_count {
            if let Ok(peer_device) = nvml.device_by_index(peer_idx) {
                if let Ok(link_count) = device.get_nv_link_state(peer_idx as u32) {
                    if link_count == nvml_wrapper::enum_wrappers::NvLinkState::Active {
                        if let Ok(bandwidth) = device.get_nv_link_capacity(peer_idx as u32, nvml_wrapper::enum_wrappers::NvLinkCapability::Bandwidth) {
                            nvlink_bandwidth.insert(peer_idx as u32, bandwidth as f64);
                        }
                    }
                }
            }
        }

        let gpu_id = i as u32;
        let gpu_info = GpuInfo {
            gpu_id,
            model: name,
            total_vram_bytes: total_vram,
            available_vram_bytes: available_vram,
            compute_capability: compute_cap.major as f32 + compute_cap.minor as f32 / 10.0,
            has_tensor_cores: compute_cap.major >= 7, // Tensor cores since Volta
            fp16_tflops: estimate_fp16_tflops(&name, gpu_util),
            int8_tops: estimate_int8_tops(&name, gpu_util),
            memory_bandwidth_gbps: estimate_memory_bandwidth(&name),
            nvlink_bandwidth,
            pcie_gen: pci_info.pci_generation.unwrap_or(4) as u32,
            pcie_lanes: pci_info.pci_max_link_width.unwrap_or(16) as u32,
            numa_node: numa_node as u32,
            utilization: (gpu_util * 100) as u32, // Scale to 0-10000
            temperature_c: temp as u32,
            power_watts: power as u32,
            max_power_watts: power_limit as u32,
        };

        gpus.insert(gpu_id, gpu_info);
        numa_nodes.entry(numa_node as u32).or_insert_with(Vec::new).push(gpu_id);

        // Initialize interconnect matrix
        interconnect_matrix.entry(gpu_id).or_insert_with(HashMap::new);
    }

    // Detect NVLink groups (GPUs connected via NVLink form a group)
    nvlink_groups = detect_nvlink_groups(&gpus);

    // Fill interconnect matrix
    for (&gpu_id, gpu) in &gpus {
        for (&peer_id, bandwidth) in &gpu.nvlink_bandwidth {
            interconnect_matrix
                .entry(gpu_id)
                .or_insert_with(HashMap::new)
                .insert(peer_id, *bandwidth);
        }
    }

    Ok(GpuTopology {
        gpus,
        numa_nodes,
        nvlink_groups,
        interconnect_matrix,
    })
}

/// Estimate FP16 TFLOPS based on GPU model
pub fn estimate_fp16_tflops(model: &str, utilization_pct: u32) -> f64 {
    let base_tflops = if model.contains("H100") {
        989.0
    } else if model.contains("A100") {
        312.0
    } else if model.contains("A10") || model.contains("V100") {
        125.0
    } else if model.contains("3090") || model.contains("4090") {
        163.0
    } else if model.contains("3080") || model.contains("4080") {
        120.0
    } else {
        50.0 // Conservative default
    };

    // Adjust based on current utilization (thermal throttling consideration)
    let util_factor = if utilization_pct > 90 {
        0.85 // Throttling likely
    } else {
        1.0
    };

    base_tflops * util_factor
}

/// Estimate INT8 TOPS based on GPU model
pub fn estimate_int8_tops(model: &str, utilization_pct: u32) -> f64 {
    // INT8 is typically 2x FP16 for GPUs with Tensor Cores
    estimate_fp16_tflops(model, utilization_pct) * 2.0
}

/// Estimate memory bandwidth based on GPU model
pub fn estimate_memory_bandwidth(model: &str) -> f64 {
    if model.contains("H100") {
        3350.0
    } else if model.contains("A100-SXM") {
        2039.0
    } else if model.contains("A100-PCIE") {
        1555.0
    } else if model.contains("A10") {
        600.0
    } else if model.contains("V100") {
        900.0
    } else if model.contains("4090") {
        1008.0
    } else if model.contains("3090") {
        936.0
    } else {
        400.0 // Conservative default
    }
}

/// Detect NVLink groups from GPU topology
pub fn detect_nvlink_groups(gpus: &HashMap<u32, GpuInfo>) -> Vec<HashSet<u32>> {
    let mut groups: Vec<HashSet<u32>> = Vec::new();
    let mut visited: HashSet<u32> = HashSet::new();

    for &gpu_id in gpus.keys() {
        if visited.contains(&gpu_id) {
            continue;
        }

        let mut group = HashSet::new();
        group.insert(gpu_id);

        // BFS to find all connected GPUs via NVLink
        let mut queue = vec![gpu_id];
        while let Some(current) = queue.pop() {
            if let Some(gpu) = gpus.get(&current) {
                for &peer_id in gpu.nvlink_bandwidth.keys() {
                    if !visited.contains(&peer_id) && !group.contains(&peer_id) {
                        group.insert(peer_id);
                        queue.push(peer_id);
                    }
                }
            }
        }

        if group.len() > 1 {
            groups.push(group.clone());
            visited.extend(group);
        }
    }

    groups
}

/// Fallback discovery when NVML is not available
#[cfg(not(feature = "gpu-discovery"))]
pub fn discover_gpus_nvml() -> Result<GpuTopology, String> {
    Err("NVML GPU discovery not enabled. Enable 'gpu-discovery' feature.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // Requires actual GPU
    fn test_nvml_discovery() {
        match discover_gpus_nvml() {
            Ok(topology) => {
                assert!(!topology.gpus.is_empty());
                println!("Found {} GPUs", topology.gpus.len());
                for (_, gpu) in &topology.gpus {
                    println!("  - {} ({}GB VRAM)", gpu.model, gpu.total_vram_bytes / (1024^3));
                }
            }
            Err(e) => {
                println!("GPU discovery failed (expected if no GPU): {}", e);
            }
        }
    }
}
