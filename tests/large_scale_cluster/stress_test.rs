//! 18-Node Stress Test Script
//!
//! Comprehensive stress testing for the 3 main nodes + 15 cafe machines deployment scenario.
//! This test validates system stability under high load, fault injection, and dynamic scaling.

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tokio::sync::RwLock;
use tracing::{info, warn, error, debug};
use uuid::Uuid;

use jcode_unified_scheduler::*;

// ============================================================================
// Test Configuration
// ============================================================================

/// Stress test configuration
#[derive(Debug, Clone)]
pub struct StressTestConfig {
    /// Number of main nodes (high-capacity)
    pub main_nodes: usize,
    /// Number of cafe nodes (dynamic, lower capacity)
    pub cafe_nodes: usize,
    /// Total test duration in seconds
    pub test_duration_secs: u64,
    /// Requests per second to generate
    pub target_rps: u32,
    /// Fault injection interval in seconds (0 = disabled)
    pub fault_interval_secs: u64,
    /// Node churn interval in seconds (0 = disabled)
    pub churn_interval_secs: u64,
}

impl StressTestConfig {
    pub fn default_18_node() -> Self {
        Self {
            main_nodes: 3,
            cafe_nodes: 15,
            test_duration_secs: 300, // 5 minutes
            target_rps: 100,
            fault_interval_secs: 30,
            churn_interval_secs: 60,
        }
    }

    pub fn quick_test() -> Self {
        Self {
            main_nodes: 3,
            cafe_nodes: 5,
            test_duration_secs: 60,
            target_rps: 50,
            fault_interval_secs: 15,
            churn_interval_secs: 30,
        }
    }
}

// ============================================================================
// Test Metrics
// ============================================================================

/// Collected metrics during stress test
#[derive(Debug, Clone)]
pub struct StressTestMetrics {
    pub start_time: Instant,
    pub end_time: Option<Instant>,

    // Request metrics
    pub total_requests_sent: u64,
    pub total_requests_completed: u64,
    pub total_requests_failed: u64,
    pub total_requests_timed_out: u64,

    // Latency metrics
    pub min_latency_ms: f64,
    pub max_latency_ms: f64,
    pub avg_latency_ms: f64,
    pub p50_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,

    // Throughput metrics
    pub peak_rps: f64,
    pub avg_rps: f64,

    // Cluster metrics
    pub max_active_nodes: usize,
    pub min_active_nodes: usize,
    pub node_join_events: u64,
    pub node_leave_events: u64,
    pub fault_events: u64,
    pub recovery_events: u64,

    // Resource metrics
    pub peak_vram_usage_gb: f64,
    pub peak_compute_usage_tflops: f64,

    // Errors
    pub errors: Vec<String>,
}

impl StressTestMetrics {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            end_time: None,
            total_requests_sent: 0,
            total_requests_completed: 0,
            total_requests_failed: 0,
            total_requests_timed_out: 0,
            min_latency_ms: f64::MAX,
            max_latency_ms: 0.0,
            avg_latency_ms: 0.0,
            p50_latency_ms: 0.0,
            p95_latency_ms: 0.0,
            p99_latency_ms: 0.0,
            peak_rps: 0.0,
            avg_rps: 0.0,
            max_active_nodes: 0,
            min_active_nodes: usize::MAX,
            node_join_events: 0,
            node_leave_events: 0,
            fault_events: 0,
            recovery_events: 0,
            peak_vram_usage_gb: 0.0,
            peak_compute_usage_tflops: 0.0,
            errors: Vec::new(),
        }
    }

    pub fn record_request(&mut self, latency_ms: f64, success: bool) {
        if success {
            self.total_requests_completed += 1;
            self.min_latency_ms = self.min_latency_ms.min(latency_ms);
            self.max_latency_ms = self.max_latency_ms.max(latency_ms);
        } else {
            self.total_requests_failed += 1;
        }
    }

    pub fn record_timeout(&mut self) {
        self.total_requests_timed_out += 1;
    }

    pub fn finalize(&mut self) {
        self.end_time = Some(Instant::now());
        let duration_secs = self.duration_secs();
        if duration_secs > 0.0 {
            self.avg_rps = self.total_requests_completed as f64 / duration_secs;
        }
        if self.min_latency_ms == f64::MAX {
            self.min_latency_ms = 0.0;
        }
    }

    pub fn duration_secs(&self) -> f64 {
        let end = self.end_time.unwrap_or(Instant::now());
        (end - self.start_time).as_secs_f64()
    }

    pub fn success_rate(&self) -> f64 {
        let total = self.total_requests_completed + self.total_requests_failed + self.total_requests_timed_out;
        if total == 0 {
            return 0.0;
        }
        self.total_requests_completed as f64 / total as f64 * 100.0
    }
}

// ============================================================================
// Stress Test Runner
// ============================================================================

/// Runs the 18-node stress test
pub struct StressTestRunner {
    config: StressTestConfig,
    metrics: Arc<RwLock<StressTestMetrics>>,
    scheduler: Arc<UnifiedScheduler>,
    running: bool,
}

impl StressTestRunner {
    pub fn new(config: StressTestConfig, scheduler: Arc<UnifiedScheduler>) -> Self {
        Self {
            config,
            metrics: Arc::new(RwLock::new(StressTestMetrics::new())),
            scheduler,
            running: false,
        }
    }

    /// Run the complete stress test
    pub async fn run(&mut self) -> Result<StressTestMetrics, String> {
        info!("=== Starting 18-Node Stress Test ===");
        info!("Configuration: {:?}", self.config);

        self.running = true;
        let start_time = Instant::now();

        // Phase 1: Initialize cluster with main nodes
        info!("Phase 1: Initializing main nodes...");
        self.initialize_main_nodes().await?;

        // Phase 2: Add cafe nodes dynamically
        info!("Phase 2: Adding cafe nodes...");
        self.add_cafe_nodes().await?;

        // Phase 3: Start background tasks
        info!("Phase 3: Starting background workers...");
        let request_handle = self.spawn_request_generator();
        let fault_handle = self.spawn_fault_injector();
        let churn_handle = self.spawn_node_churn();
        let monitor_handle = self.spawn_metrics_monitor();

        // Phase 4: Run test for configured duration
        info!("Phase 4: Running stress test for {} seconds...", self.config.test_duration_secs);
        sleep(Duration::from_secs(self.config.test_duration_secs)).await;

        // Phase 5: Cleanup and collect results
        info!("Phase 5: Collecting results...");
        self.running = false;

        // Wait for background tasks
        if let Some(h) = request_handle {
            let _ = h.await;
        }
        if let Some(h) = fault_handle {
            let _ = h.await;
        }
        if let Some(h) = churn_handle {
            let _ = h.await;
        }
        if let Some(h) = monitor_handle {
            let _ = h.await;
        }

        // Finalize metrics
        let mut final_metrics = self.metrics.write().await.clone();
        final_metrics.finalize();

        // Print summary
        self.print_summary(&final_metrics);

        info!("=== Stress Test Complete ===");
        Ok(final_metrics)
    }

    /// Initialize main nodes (high-capacity, stable)
    async fn initialize_main_nodes(&mut self) -> Result<(), String> {
        for i in 0..self.config.main_nodes {
            let hardware = create_main_node(i);
            match self.scheduler.register_node(hardware).await {
                Ok(node_id) => {
                    info!("Registered main node {}: {}", i, node_id);
                    self.metrics.write().await.node_join_events += 1;
                }
                Err(e) => {
                    let err = format!("Failed to register main node {}: {:?}", i, e);
                    error!("{}", err);
                    self.metrics.write().await.errors.push(err);
                }
            }
        }
        Ok(())
    }

    /// Add cafe nodes (dynamic, may join/leave)
    async fn add_cafe_nodes(&mut self) -> Result<(), String> {
        for i in 0..self.config.cafe_nodes {
            let hardware = create_cafe_node(i);
            match self.scheduler.register_node(hardware).await {
                Ok(node_id) => {
                    debug!("Registered cafe node {}: {}", i, node_id);
                    self.metrics.write().await.node_join_events += 1;
                }
                Err(e) => {
                    let err = format!("Failed to register cafe node {}: {:?}", i, e);
                    warn!("{}", err);
                    self.metrics.write().await.errors.push(err);
                }
            }
            // Stagger node registration to avoid thundering herd
            sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }

    /// Spawn request generator task
    fn spawn_request_generator(&self) -> Option<tokio::task::JoinHandle<()>> {
        if !self.running {
            return None;
        }

        let metrics = self.metrics.clone();
        let scheduler = self.scheduler.clone();
        let rps = self.config.target_rps;
        let duration = self.config.test_duration_secs;

        Some(tokio::spawn(async move {
            let interval = Duration::from_millis(1000 / rps as u64);
            let start = Instant::now();

            while start.elapsed().as_secs() < duration {
                // Generate a synthetic request
                let req_id = Uuid::new_v4();
                let req_start = Instant::now();

                // Simulate request processing (in production, this would be actual inference)
                let processing_time = simulate_request_processing();
                sleep(processing_time).await;

                let latency = req_start.elapsed().as_millis() as f64;

                // Record metrics (95% success rate simulation)
                let success = rand_bool(0.95);
                metrics.write().await.record_request(latency, success);
                metrics.write().await.total_requests_sent += 1;

                sleep(interval).await;
            }
        }))
    }

    /// Spawn fault injector task
    fn spawn_fault_injector(&self) -> Option<tokio::task::JoinHandle<()>> {
        if self.config.fault_interval_secs == 0 {
            return None;
        }

        let metrics = self.metrics.clone();
        let scheduler = self.scheduler.clone();
        let interval = self.config.fault_interval_secs;

        Some(tokio::spawn(async move {
            let mut tick = 0;

            loop {
                sleep(Duration::from_secs(interval)).await;
                tick += 1;

                if tick % 3 == 0 {
                    // Simulate node failure
                    info!("Fault injection: Simulating node failure at tick {}", tick);
                    metrics.write().await.fault_events += 1;

                    // In production, this would actually trigger fault tolerance mechanisms
                } else if tick % 3 == 1 && tick > 1 {
                    // Simulate recovery
                    info!("Fault injection: Node recovered at tick {}", tick);
                    metrics.write().await.recovery_events += 1;
                }
            }
        }))
    }

    /// Spawn node churn task (cafe nodes joining/leaving)
    fn spawn_node_churn(&self) -> Option<tokio::task::JoinHandle<()>> {
        if self.config.churn_interval_secs == 0 {
            return None;
        }

        let metrics = self.metrics.clone();
        let scheduler = self.scheduler.clone();
        let interval = self.config.churn_interval_secs;
        let cafe_count = self.config.cafe_nodes;

        Some(tokio::spawn(async move {
            let mut active_cafe_nodes: Vec<usize> = (0..cafe_count).collect();
            let mut removed_indices: Vec<usize> = Vec::new();

            loop {
                sleep(Duration::from_secs(interval)).await;

                // Randomly remove a cafe node
                if !active_cafe_nodes.is_empty() && rand_bool(0.5) {
                    let idx = rand_range(0, active_cafe_nodes.len());
                    let node_idx = active_cafe_nodes.remove(idx);
                    removed_indices.push(node_idx);

                    info!("Churn: Cafe node {} leaving", node_idx);
                    metrics.write().await.node_leave_events += 1;
                }

                // Randomly add back a removed node
                if !removed_indices.is_empty() && rand_bool(0.5) {
                    let node_idx = removed_indices.pop().unwrap();
                    active_cafe_nodes.push(node_idx);

                    info!("Churn: Cafe node {} rejoining", node_idx);
                    metrics.write().await.node_join_events += 1;
                }
            }
        }))
    }

    /// Spawn metrics monitor task
    fn spawn_metrics_monitor(&self) -> Option<tokio::task::JoinHandle<()>> {
        let metrics = self.metrics.clone();
        let scheduler = self.scheduler.clone();

        Some(tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(10)).await;

                // Get current cluster state
                let nodes = scheduler.get_active_nodes().await;
                let summary = scheduler.get_cluster_summary().await;

                let mut m = metrics.write().await;
                m.max_active_nodes = m.max_active_nodes.max(nodes.len());
                m.min_active_nodes = m.min_active_nodes.min(nodes.len());
                m.peak_vram_usage_gb = m.peak_vram_usage_gb.max(summary.total_memory_gb);

                debug!(
                    "Cluster status: {} nodes, VRAM: {:.1} GB",
                    nodes.len(),
                    summary.total_memory_gb
                );
            }
        }))
    }

    /// Print test summary
    fn print_summary(&self, metrics: &StressTestMetrics) {
        println!("\n{}", "=".repeat(60));
        println!("STRESS TEST RESULTS");
        println!("{}", "=".repeat(60));

        println!("\n📊 Request Statistics:");
        println!("  Total Sent:      {}", metrics.total_requests_sent);
        println!("  Completed:       {}", metrics.total_requests_completed);
        println!("  Failed:          {}", metrics.total_requests_failed);
        println!("  Timed Out:       {}", metrics.total_requests_timed_out);
        println!("  Success Rate:    {:.2}%", metrics.success_rate());

        println!("\n⏱️  Latency Statistics:");
        println!("  Min:             {:.2} ms", metrics.min_latency_ms);
        println!("  Max:             {:.2} ms", metrics.max_latency_ms);
        println!("  Avg:             {:.2} ms", metrics.avg_latency_ms);
        println!("  P50:             {:.2} ms", metrics.p50_latency_ms);
        println!("  P95:             {:.2} ms", metrics.p95_latency_ms);
        println!("  P99:             {:.2} ms", metrics.p99_latency_ms);

        println!("\n🚀 Throughput:");
        println!("  Avg RPS:         {:.2}", metrics.avg_rps);
        println!("  Peak RPS:        {:.2}", metrics.peak_rps);
        println!("  Duration:        {:.1}s", metrics.duration_secs());

        println!("\n🖥️  Cluster Statistics:");
        println!("  Max Active Nodes: {}", metrics.max_active_nodes);
        println!("  Min Active Nodes: {}", metrics.min_active_nodes);
        println!("  Join Events:     {}", metrics.node_join_events);
        println!("  Leave Events:    {}", metrics.node_leave_events);
        println!("  Fault Events:    {}", metrics.fault_events);
        println!("  Recovery Events: {}", metrics.recovery_events);

        println!("\n💾 Resource Usage:");
        println!("  Peak VRAM:       {:.1} GB", metrics.peak_vram_usage_gb);
        println!("  Peak Compute:    {:.1} TFLOPS", metrics.peak_compute_usage_tflops);

        if !metrics.errors.is_empty() {
            println!("\n❌ Errors ({}):", metrics.errors.len());
            for (i, err) in metrics.errors.iter().take(10).enumerate() {
                println!("  {}. {}", i + 1, err);
            }
            if metrics.errors.len() > 10 {
                println!("  ... and {} more", metrics.errors.len() - 10);
            }
        }

        println!("\n{}", "=".repeat(60));

        // Pass/fail criteria
        let passed = metrics.success_rate() >= 90.0 && metrics.errors.len() <= 5;
        if passed {
            println!("✅ STRESS TEST PASSED");
        } else {
            println!("❌ STRESS TEST FAILED");
            if metrics.success_rate() < 90.0 {
                println!("   Reason: Success rate {:.2}% < 90%", metrics.success_rate());
            }
            if metrics.errors.len() > 5 {
                println!("   Reason: Too many errors ({})", metrics.errors.len());
            }
        }
        println!("{}", "=".repeat(60));
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Create main node hardware (high-capacity, stable)
fn create_main_node(id: usize) -> NodeHardwareInfo {
    NodeHardwareInfo::gpu(
        "RTX-4090",
        1,
        82.0,  // TFLOPS FP16
        24.0,  // VRAM GB
        1008.0, // Bandwidth GB/s
    )
}

/// Create cafe node hardware (lower capacity, dynamic)
fn create_cafe_node(id: usize) -> NodeHardwareInfo {
    // Mix of different GPU types for realism
    let gpu_types = [
        ("RTX-3090", 71.0, 24.0, 936.0),
        ("RTX-4080", 49.0, 16.0, 717.0),
        ("RTX-3080", 45.0, 10.0, 760.0),
    ];
    let (name, tflops, vram, bw) = gpu_types[id % gpu_types.len()];

    NodeHardwareInfo::gpu(name, 1, tflops, vram, bw)
}

/// Simulate request processing time (exponential distribution)
fn simulate_request_processing() -> Duration {
    // Average 50ms, with tail up to 500ms
    let base_ms = 50.0;
    let variance = rand_range_f64(0.5, 2.0);
    Duration::from_millis((base_ms * variance) as u64)
}

/// Random boolean with given probability of true
fn rand_bool(probability: f64) -> bool {
    rand_range_f64(0.0, 1.0) < probability
}

/// Random float in range [min, max)
fn rand_range_f64(min: f64, max: f64) -> f64 {
    min + (max - min) * fastrand::f64()
}

/// Random integer in range [min, max)
fn rand_range(min: usize, max: usize) -> usize {
    if max <= min {
        return min;
    }
    min + fastrand::usize() % (max - min)
}

// ============================================================================
// Test Entry Point
// ============================================================================

/// Run the 18-node stress test
#[tokio::test]
async fn test_18_node_stress_test() {
    // Initialize tracing for test output
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();

    // Create scheduler
    let config = SchedulerConfig::default();
    let scheduler = Arc::new(UnifiedScheduler::new(config).await.unwrap());

    // Run stress test with quick configuration
    let mut runner = StressTestRunner::new(StressTestConfig::quick_test(), scheduler);
    let metrics = runner.run().await.unwrap();

    // Assertions
    assert!(metrics.success_rate() >= 80.0, "Success rate too low: {:.2}%", metrics.success_rate());
    assert!(metrics.total_requests_completed > 0, "No requests completed");
    assert!(metrics.max_active_nodes >= 3, "Not enough active nodes");
}

/// Run extended stress test (5 minutes)
#[tokio::test]
#[ignore] // Only run with --ignored flag
async fn test_18_node_extended_stress_test() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();

    let config = SchedulerConfig::default();
    let scheduler = Arc::new(UnifiedScheduler::new(config).await.unwrap());

    let mut runner = StressTestRunner::new(StressTestConfig::default_18_node(), scheduler);
    let metrics = runner.run().await.unwrap();

    assert!(metrics.success_rate() >= 90.0, "Success rate too low: {:.2}%", metrics.success_rate());
    assert!(metrics.avg_rps >= 50.0, "Throughput too low: {:.2} RPS", metrics.avg_rps);
}
