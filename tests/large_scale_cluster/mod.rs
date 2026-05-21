//! Large-Scale Cluster Integration Tests (18-node scenario)
//!
//! Tests for 3 main nodes + 15 cafe machines dynamic deployment scenario.

use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn, error};

// Test modules
mod cluster_stability;
mod dynamic_node_management;
mod fault_injection;
mod performance_benchmarks;

pub use cluster_stability::*;
pub use dynamic_node_management::*;
pub use fault_injection::*;
pub use performance_benchmarks::*;

/// Helper: Create test node hardware info simulating different GPU types
fn create_test_node(id: usize, gpu_type: &str) -> jcode_unified_scheduler::NodeHardwareInfo {
    match gpu_type {
        "RTX-4090" => jcode_unified_scheduler::NodeHardwareInfo::gpu(
            gpu_type, 1, 82.0, 24.0, 1008.0
        ),
        "RTX-3090" => jcode_unified_scheduler::NodeHardwareInfo::gpu(
            gpu_type, 1, 71.0, 24.0, 936.0
        ),
        "RTX-4080" => jcode_unified_scheduler::NodeHardwareInfo::gpu(
            gpu_type, 1, 49.0, 16.0, 717.0
        ),
        "RTX-3080" => jcode_unified_scheduler::NodeHardwareInfo::gpu(
            gpu_type, 1, 45.0, 10.0, 760.0
        ),
        _ => jcode_unified_scheduler::NodeHardwareInfo::gpu(
            gpu_type, 1, 50.0, 12.0, 800.0
        ),
    }
}

/// Helper: Wait for condition with timeout
async fn wait_for_condition<F>(mut condition: F, timeout_ms: u64, check_interval_ms: u64) -> bool
where
    F: FnMut() -> bool,
{
    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(timeout_ms);
    let interval = Duration::from_millis(check_interval_ms);

    while start.elapsed() < timeout {
        if condition() {
            return true;
        }
        sleep(interval).await;
    }
    false
}

/// Helper: Generate unique node ID for testing
fn generate_node_id(prefix: &str, index: usize) -> String {
    format!("{}-{:03}", prefix, index)
}
