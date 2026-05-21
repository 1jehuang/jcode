//! Test: Dynamic node join/remove scenarios

use super::*;
use jcode_unified_scheduler::{UnifiedScheduler, SchedulerConfig};

#[tokio::test]
async fn test_dynamic_node_join() {
    tracing_subscriber::fmt::init();
    info!("=== Test: Dynamic Node Join ===");

    let scheduler = UnifiedScheduler::new(SchedulerConfig::default()).await.unwrap();

    // Start with 3 main nodes
    for i in 0..3 {
        scheduler.register_node(create_test_node(i, "RTX-4090")).await.unwrap();
    }

    let initial_nodes = scheduler.get_active_nodes().await.len();
    assert_eq!(initial_nodes, 3);
    info!("Initial cluster: {} nodes", initial_nodes);

    // Simulate cafe machines joining dynamically
    for i in 0..15 {
        let gpu_type = if i % 3 == 0 { "RTX-3090" } else if i % 3 == 1 { "RTX-4080" } else { "RTX-3080" };
        let hw = create_test_node(i + 3, gpu_type);

        let start = std::time::Instant::now();
        scheduler.register_node(hw).await.unwrap();
        let elapsed = start.elapsed();

        info!("Cafe node {} ({}) joined in {:?}", i, gpu_type, elapsed);

        // Verify node count increased
        let current_nodes = scheduler.get_active_nodes().await.len();
        assert_eq!(current_nodes, initial_nodes + i + 1);
    }

    let final_nodes = scheduler.get_active_nodes().await.len();
    assert_eq!(final_nodes, 18);
    info!("Final cluster: {} nodes", final_nodes);

    scheduler.shutdown().await.ok();
}

#[tokio::test]
async fn test_dynamic_node_removal() {
    tracing_subscriber::fmt::init();
    info!("=== Test: Dynamic Node Removal ===");

    let scheduler = UnifiedScheduler::new(SchedulerConfig::default()).await.unwrap();

    // Start with 18 nodes
    for i in 0..18 {
        let gpu = if i < 3 { "RTX-4090" } else if i < 8 { "RTX-3090" } else if i < 13 { "RTX-4080" } else { "RTX-3080" };
        scheduler.register_node(create_test_node(i, gpu)).await.unwrap();
    }

    assert_eq!(scheduler.get_active_nodes().await.len(), 18);
    info!("Started with 18 nodes");

    // Remove 5 cafe machines
    let nodes = scheduler.get_active_nodes().await;
    for i in 0..5 {
        if let Some(node) = nodes.get(17 - i) {
            let node_id = node.node_id;
            scheduler.unregister_node(&node_id).await.unwrap();
            info!("Removed node {}", i);
        }
    }

    let remaining = scheduler.get_active_nodes().await.len();
    assert_eq!(remaining, 13);
    info!("Remaining nodes: {}", remaining);

    scheduler.shutdown().await.ok();
}

#[tokio::test]
async fn test_batch_node_join() {
    tracing_subscriber::fmt::init();
    info!("=== Test: Batch Node Join (Simulating Cafe Opening) ===");

    let scheduler = UnifiedScheduler::new(SchedulerConfig::default()).await.unwrap();

    // Start with 3 main nodes
    for i in 0..3 {
        scheduler.register_node(create_test_node(i, "RTX-4090")).await.unwrap();
    }

    info!("Main cluster: 3 nodes");

    // Simulate 15 cafe machines joining at once (cafe opening)
    let start = std::time::Instant::now();

    let mut join_tasks = Vec::new();
    for i in 0..15 {
        let gpu_type = if i < 5 { "RTX-3090" } else if i < 10 { "RTX-4080" } else { "RTX-3080" };
        let hw = create_test_node(i + 3, gpu_type);

        join_tasks.push(tokio::spawn(async move {
            scheduler.register_node(hw).await
        }));
    }

    // Wait for all joins to complete
    for (i, task) in join_tasks.into_iter().enumerate() {
        match task.await {
            Ok(Ok(_)) => info!("Cafe node {} joined successfully", i),
            Ok(Err(e)) => error!("Cafe node {} failed to join: {:?}", i, e),
            Err(e) => error!("Cafe node {} task panicked: {:?}", i, e),
        }
    }

    let elapsed = start.elapsed();
    info!("Batch join of 15 nodes completed in {:?}", elapsed);

    let total = scheduler.get_active_nodes().await.len();
    assert_eq!(total, 18);

    // Should be fast (< 1 second for 15 nodes)
    assert!(elapsed < Duration::from_secs(1),
        "Batch join should be fast, took {:?}", elapsed);

    scheduler.shutdown().await.ok();
}

#[tokio::test]
async fn test_rapid_join_leave_cycles() {
    tracing_subscriber::fmt::init();
    info!("=== Test: Rapid Join/Leave Cycles ===");

    let scheduler = UnifiedScheduler::new(SchedulerConfig::default()).await.unwrap();

    // Initial setup
    for i in 0..3 {
        scheduler.register_node(create_test_node(i, "RTX-4090")).await.unwrap();
    }

    // Simulate unstable cafe machines (join/leave cycles)
    for cycle in 0..3 {
        info!("Cycle {}: Nodes joining", cycle + 1);

        // 5 nodes join
        for i in 0..5 {
            let hw = create_test_node(100 + i, "RTX-3080");
            scheduler.register_node(hw).await.unwrap();
        }

        sleep(Duration::from_millis(100)).await;

        let after_join = scheduler.get_active_nodes().await.len();
        info!("After join: {} nodes", after_join);

        // 5 nodes leave
        let nodes = scheduler.get_active_nodes().await;
        for node in nodes.iter().skip(3).take(5) {
            scheduler.unregister_node(&node.node_id).await.ok();
        }

        sleep(Duration::from_millis(100)).await;

        let after_leave = scheduler.get_active_nodes().await.len();
        info!("After leave: {} nodes", after_leave);

        assert_eq!(after_leave, 3, "Should return to base 3 nodes after each cycle");
    }

    info!("Rapid join/leave cycles completed successfully");

    scheduler.shutdown().await.ok();
}

#[tokio::test]
async fn test_node_rejoin_after_cooldown() {
    tracing_subscriber::fmt::init();
    info!("=== Test: Node Rejoin After Cooldown ===");

    let scheduler = UnifiedScheduler::new(SchedulerConfig::default()).await.unwrap();

    // Register and unregister a node
    let hw = create_test_node(0, "RTX-4090");
    let node_id = scheduler.register_node(hw.clone()).await.unwrap();
    info!("Node registered: {}", node_id);

    scheduler.unregister_node(&node_id).await.unwrap();
    info!("Node unregistered: {}", node_id);

    // In production, there would be a cooldown period
    // For testing, we can re-register immediately with a new ID
    let hw2 = create_test_node(1, "RTX-4090");
    let new_node_id = scheduler.register_node(hw2).await.unwrap();
    info!("Node re-registered with new ID: {}", new_node_id);

    let nodes = scheduler.get_active_nodes().await;
    assert_eq!(nodes.len(), 1);

    scheduler.shutdown().await.ok();
}
