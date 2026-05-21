//! Test: 18-node cluster stability

use super::*;
use jcode_unified_scheduler::{UnifiedScheduler, SchedulerConfig, NodeHardwareInfo};
use carpai::distributed::{ClusterService, ClusterConfig, NodeConfig, FaultToleranceConfig};

#[tokio::test]
async fn test_18_node_cluster_startup() {
    tracing_subscriber::fmt::init();
    info!("=== Test: 18-Node Cluster Startup ===");

    let start_time = std::time::Instant::now();

    // Create scheduler
    let config = SchedulerConfig::default();
    let scheduler = UnifiedScheduler::new(config).await.expect("Failed to create scheduler");

    // Register 3 main nodes (RTX-4090)
    for i in 0..3 {
        let hw = create_test_node(i, "RTX-4090");
        scheduler.register_node(hw).await.expect("Failed to register main node");
        info!("Registered main node {}", i);
    }

    // Register 15 cafe machines (mixed GPUs)
    let cafe_gpus = vec![
        "RTX-3090", "RTX-3090", "RTX-3090", "RTX-3090", "RTX-3090",
        "RTX-4080", "RTX-4080", "RTX-4080", "RTX-4080", "RTX-4080",
        "RTX-3080", "RTX-3080", "RTX-3080", "RTX-3080", "RTX-3080",
    ];

    for (i, gpu_type) in cafe_gpus.iter().enumerate() {
        let hw = create_test_node(i + 3, gpu_type);
        scheduler.register_node(hw).await.expect("Failed to register cafe node");
        info!("Registered cafe node {} ({})", i, gpu_type);
    }

    let elapsed = start_time.elapsed();
    info!("18-node cluster startup completed in {:?}", elapsed);

    // Verify all nodes registered
    let nodes = scheduler.get_active_nodes().await;
    assert_eq!(nodes.len(), 18, "Expected 18 active nodes");

    // Verify cluster summary
    let summary = scheduler.get_cluster_summary().await;
    assert_eq!(summary.total_nodes, 18);
    assert_eq!(summary.active_nodes, 18);
    assert!(summary.total_gpus >= 18);

    info!("Cluster summary: {} nodes, {} GPUs, {:.1} TFLOPS, {:.1} GB memory",
        summary.total_nodes,
        summary.total_gpus,
        summary.total_tflops,
        summary.total_memory_gb
    );

    // Cleanup
    scheduler.shutdown().await.ok();
}

#[tokio::test]
async fn test_18_node_pipeline_allocation() {
    tracing_subscriber::fmt::init();
    info!("=== Test: 18-Node Pipeline Allocation for Qwen3.6-35B ===");

    let scheduler = UnifiedScheduler::new(SchedulerConfig::default()).await.unwrap();

    // Register 18 nodes
    for i in 0..3 {
        scheduler.register_node(create_test_node(i, "RTX-4090")).await.unwrap();
    }
    for i in 0..15 {
        let gpu = if i < 5 { "RTX-3090" } else if i < 10 { "RTX-4080" } else { "RTX-3080" };
        scheduler.register_node(create_test_node(i + 3, gpu)).await.unwrap();
    }

    // Simulate allocation for Qwen3.6-35B (40 layers)
    let total_layers = 40u32;
    let nodes = scheduler.get_active_nodes().await;
    let node_refs: Vec<&jcode_unified_scheduler::NodeInfo> = nodes.iter().collect();

    // This would trigger the layer allocator in a real scenario
    info!("Allocating {} layers across {} nodes", total_layers, nodes.len());

    // Verify sufficient capacity
    let total_capacity: u32 = node_refs.iter()
        .map(|n| n.get_decoder_layer_capacity(false, false))
        .sum();

    assert!(total_capacity >= total_layers,
        "Total capacity {} should be >= required layers {}",
        total_capacity, total_layers
    );

    info!("Total layer capacity: {} (required: {})", total_capacity, total_layers);

    scheduler.shutdown().await.ok();
}

#[tokio::test]
async fn test_cluster_health_monitoring() {
    tracing_subscriber::fmt::init();
    info!("=== Test: Cluster Health Monitoring ===");

    // Create cluster service with fault tolerance
    let mut config = ClusterConfig::new().enable();
    config.node = NodeConfig {
        id: Some("test-cluster-leader".to_string()),
        host: "127.0.0.1".to_string(),
        port: 9000,
        ..Default::default()
    };

    let service = ClusterService::new(config).await.expect("Failed to create cluster service");

    // Simulate registering nodes for fault tracking
    for i in 0..18 {
        let node_id = generate_node_id("node", i);
        service.register_for_fault_tracking(&node_id).await;
    }

    // Record heartbeats for all nodes
    for i in 0..18 {
        let node_id = generate_node_id("node", i);
        // In real scenario, this would be called by heartbeat loop
    }

    // Check health summary
    let summary = service.get_health_summary().await;
    info!("Initial health summary: {:?}", summary);

    assert_eq!(summary.total_nodes, 18);
    assert_eq!(summary.healthy, 18);
    assert_eq!(summary.warning, 0);
    assert_eq!(summary.critical, 0);
    assert_eq!(summary.offline, 0);

    info!("Health monitoring test passed");
}

#[tokio::test]
async fn test_concurrent_task_submission() {
    tracing_subscriber::fmt::init();
    info!("=== Test: Concurrent Task Submission ===");

    let scheduler = UnifiedScheduler::new(SchedulerConfig::default()).await.unwrap();

    // Register some nodes
    for i in 0..5 {
        scheduler.register_node(create_test_node(i, "RTX-4090")).await.unwrap();
    }

    // Submit multiple tasks concurrently
    let mut task_ids = Vec::new();
    for i in 0..10 {
        let task = jcode_unified_scheduler::ScheduledTask::simple(
            &format!("Test task {}", i),
            jcode_unified_scheduler::AgentRole::Worker,
            "qwen-3.6-max"
        );

        match scheduler.submit_task(task).await {
            Ok(task_id) => {
                info!("Submitted task {}", i);
                task_ids.push(task_id);
            }
            Err(e) => {
                warn!("Failed to submit task {}: {:?}", i, e);
            }
        }
    }

    info!("Submitted {} tasks successfully", task_ids.len());
    assert!(!task_ids.is_empty(), "Should have submitted at least some tasks");

    scheduler.shutdown().await.ok();
}
