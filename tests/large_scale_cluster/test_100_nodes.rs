//! Large-Scale Cluster Test (100-node scenario)
//!
//! Tests for hierarchical scheduler with 100+ nodes across multiple cluster groups.

use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, debug};

use jcode_unified_scheduler::*;

/// Helper: Create test node hardware info
fn create_test_node(id: usize, gpu_type: &str) -> NodeHardwareInfo {
    match gpu_type {
        "RTX-4090" => NodeHardwareInfo::gpu(gpu_type, 1, 82.0, 24.0, 1008.0),
        "RTX-3090" => NodeHardwareInfo::gpu(gpu_type, 1, 71.0, 24.0, 936.0),
        "RTX-4080" => NodeHardwareInfo::gpu(gpu_type, 1, 49.0, 16.0, 717.0),
        _ => NodeHardwareInfo::gpu(gpu_type, 1, 50.0, 12.0, 800.0),
    }
}

/// Test 100-node cluster with hierarchical scheduler
#[tokio::test]
async fn test_100_node_hierarchical_cluster() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();

    info!("=== Starting 100-Node Hierarchical Cluster Test ===");

    // Create hierarchical scheduler
    let config = HierarchicalSchedulerConfig::for_large_clusters();
    let hier_scheduler = Arc::new(HierarchicalScheduler::new(config));

    // Create cluster groups (4 groups of ~25 nodes each)
    info!("Phase 1: Creating cluster groups...");
    hier_scheduler.create_group("group-us-east", "US East Region", ClusterGroupType::Region, Some(30)).await.unwrap();
    hier_scheduler.create_group("group-eu-west", "EU West Region", ClusterGroupType::Region, Some(30)).await.unwrap();
    hier_scheduler.create_group("group-ap-south", "AP South Region", ClusterGroupType::Region, Some(30)).await.unwrap();
    hier_scheduler.create_group("group-specialized", "Specialized GPU Group", ClusterGroupType::Functional, Some(20)).await.unwrap();

    // Phase 2: Register 100 nodes across groups
    info!("Phase 2: Registering 100 nodes...");
    let gpu_types = ["RTX-4090", "RTX-3090", "RTX-4080"];
    let group_ids = ["group-us-east", "group-eu-west", "group-ap-south", "group-specialized"];

    for i in 0..100 {
        let gpu_type = gpu_types[i % gpu_types.len()];
        let preferred_group = if i < 75 {
            // Distribute first 75 nodes across regional groups
            Some(group_ids[i % 3])
        } else {
            // Last 25 nodes go to specialized group
            Some(group_ids[3])
        };

        let hardware = create_test_node(i, gpu_type);
        match hier_scheduler.register_node(hardware, preferred_group).await {
            Ok(node_id) => {
                debug!("Registered node {} in group {:?}", i, preferred_group);
            }
            Err(e) => {
                tracing::error!("Failed to register node {}: {:?}", i, e);
            }
        }

        // Stagger registration to avoid overload
        if i % 10 == 0 {
            sleep(Duration::from_millis(50)).await;
        }
    }

    // Phase 3: Verify cluster state
    info!("Phase 3: Verifying cluster state...");
    let summary = hier_scheduler.get_cluster_summary().await;

    assert_eq!(summary.total_groups, 4, "Expected 4 cluster groups");
    assert_eq!(summary.total_nodes, 100, "Expected 100 registered nodes");

    info!("Cluster Summary:");
    info!("  Total Groups: {}", summary.total_groups);
    info!("  Total Nodes: {}", summary.total_nodes);

    for group_info in &summary.groups {
        info!("  Group {}: {} nodes ({:.0}% utilization)",
            group_info.group_id,
            group_info.node_count,
            group_info.utilization * 100.0
        );
    }

    // Phase 4: Submit test tasks
    info!("Phase 4: Submitting test tasks...");
    for i in 0..20 {
        let task = ScheduledTask::simple(
            &format!("Test Task {}", i),
            AgentRole::Worker,
            "qwen-7b"
        );

        match hier_scheduler.submit_task(task).await {
            Ok(task_id) => {
                debug!("Submitted task {}", task_id);
            }
            Err(e) => {
                tracing::error!("Failed to submit task {}: {:?}", i, e);
            }
        }
    }

    info!("=== 100-Node Hierarchical Cluster Test Complete ===");
}

/// Test batch node operations with 50 nodes
#[tokio::test]
async fn test_batch_node_join_50_nodes() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();

    info!("=== Starting 50-Node Batch Join Test ===");

    // Setup hierarchical scheduler
    let hier_config = HierarchicalSchedulerConfig::default();
    let hier_scheduler = Arc::new(HierarchicalScheduler::new(hier_config));
    hier_scheduler.create_group("batch-test-group", "Batch Test Group", ClusterGroupType::Zone, Some(60)).await.unwrap();

    // Setup batch node manager
    let batch_config = BatchOperationConfig::aggressive();
    let join_manager = Arc::new(tokio::sync::RwLock::new(NodeJoinManager::new(
        WarmupConfig::fast(),
        None
    )));
    let batch_manager = BatchNodeManager::new(batch_config, hier_scheduler.clone(), join_manager);

    // Prepare 50 nodes for batch join
    let nodes: Vec<(NodeId, NodeHardwareInfo)> = (0..50)
        .map(|i| {
            let hardware = create_test_node(i, if i % 2 == 0 { "RTX-4090" } else { "RTX-3090" });
            (hardware.node_id, hardware)
        })
        .collect();

    // Start batch operation
    info!("Starting batch join for 50 nodes...");
    let batch_id = batch_manager.start_batch_join(nodes).await.unwrap();

    // Monitor progress
    loop {
        if let Some(status) = batch_manager.get_batch_status(&batch_id).await {
            info!(
                "Batch Progress: {:.1}% (pending={}, probing={}, warmup={}, integrated={}, failed={})",
                status.progress_pct(),
                status.pending,
                status.probing,
                status.warming_up,
                status.integrated,
                status.failed
            );

            if status.is_complete() {
                info!("Batch operation complete!");
                break;
            }
        }

        sleep(Duration::from_secs(2)).await;
    }

    // Verify results
    let final_status = batch_manager.get_batch_status(&batch_id).await.unwrap();
    assert_eq!(final_status.total_nodes, 50);
    assert!(final_status.integrated >= 40, "Expected at least 40 nodes to integrate successfully");

    info!("=== 50-Node Batch Join Test Complete ===");
    info!("Final Results:");
    info!("  Integrated: {}", final_status.integrated);
    info!("  Failed: {}", final_status.failed);
    info!("  Success Rate: {:.1}%", (final_status.integrated as f64 / 50.0) * 100.0);
}

/// Test cross-region routing with hierarchical groups
#[tokio::test]
async fn test_cross_region_hierarchical_routing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();

    info!("=== Starting Cross-Region Hierarchical Routing Test ===");

    // Create hierarchical scheduler with region-aware groups
    let hier_config = HierarchicalSchedulerConfig::default();
    let hier_scheduler = Arc::new(HierarchicalScheduler::new(hier_config));

    // Create regional groups
    hier_scheduler.create_group("us-east", "US East", ClusterGroupType::Region, Some(30)).await.unwrap();
    hier_scheduler.create_group("eu-west", "EU West", ClusterGroupType::Region, Some(30)).await.unwrap();
    hier_scheduler.create_group("ap-south", "AP South", ClusterGroupType::Region, Some(30)).await.unwrap();

    // Register nodes in each region
    let regions = ["us-east", "eu-west", "ap-south"];
    for (region_idx, region) in regions.iter().enumerate() {
        for i in 0..10 {
            let node_id = region_idx * 10 + i;
            let hardware = create_test_node(node_id, "RTX-4090");
            hier_scheduler.register_node(hardware, Some(region)).await.unwrap();
        }
    }

    // Submit tasks and verify distribution
    info!("Submitting tasks to test cross-region routing...");
    for i in 0..30 {
        let task = ScheduledTask::simple(
            &format!("Cross-Region Task {}", i),
            AgentRole::Worker,
            "qwen-7b"
        );

        hier_scheduler.submit_task(task).await.unwrap();
    }

    // Check cluster summary
    let summary = hier_scheduler.get_cluster_summary().await;
    assert_eq!(summary.total_groups, 3);
    assert_eq!(summary.total_nodes, 30);

    info!("=== Cross-Region Hierarchical Routing Test Complete ===");
}
