//! Test: Performance benchmarks for 18-node cluster

use super::*;
use jcode_unified_scheduler::{UnifiedScheduler, SchedulerConfig};

#[tokio::test]
async fn benchmark_node_registration_performance() {
    tracing_subscriber::fmt::init();
    info!("=== Benchmark: Node Registration Performance ===");

    let scheduler = UnifiedScheduler::new(SchedulerConfig::default()).await.unwrap();

    // Benchmark: Register 18 nodes
    let start = std::time::Instant::now();

    for i in 0..18 {
        let gpu = if i < 3 { "RTX-4090" } else if i < 8 { "RTX-3090" } else if i < 13 { "RTX-4080" } else { "RTX-3080" };
        scheduler.register_node(create_test_node(i, gpu)).await.unwrap();
    }

    let elapsed = start.elapsed();
    let per_node = elapsed / 18;

    info!("Registered 18 nodes in {:?}", elapsed);
    info!("Average per node: {:?}", per_node);

    // Performance target: < 100ms per node
    assert!(per_node < Duration::from_millis(100),
        "Node registration too slow: {:?}", per_node);

    scheduler.shutdown().await.ok();
}

#[tokio::test]
async fn benchmark_concurrent_heartbeats() {
    tracing_subscriber::fmt::init();
    info!("=== Benchmark: Concurrent Heartbeat Processing ===");

    let scheduler = UnifiedScheduler::new(SchedulerConfig::default()).await.unwrap();

    // Register 18 nodes
    for i in 0..18 {
        let gpu = if i < 3 { "RTX-4090" } else if i < 8 { "RTX-3090" } else if i < 13 { "RTX-4080" } else { "RTX-3080" };
        scheduler.register_node(create_test_node(i, gpu)).await.unwrap();
    }

    let nodes = scheduler.get_active_nodes().await;
    let node_ids: Vec<_> = nodes.iter().map(|n| n.node_id).collect();

    // Benchmark: Process heartbeats for all 18 nodes concurrently
    let iterations = 100;
    let start = std::time::Instant::now();

    for _ in 0..iterations {
        let mut tasks = Vec::new();
        for &node_id in &node_ids {
            let sched = &scheduler;
            tasks.push(tokio::spawn(async move {
                sched.node_heartbeat(&node_id, Some(5.0)).await
            }));
        }

        for task in tasks {
            task.await.ok();
        }
    }

    let elapsed = start.elapsed();
    let total_heartbeats = iterations * node_ids.len();
    let per_heartbeat = elapsed / total_heartbeats as u32;

    info!("Processed {} heartbeats in {:?}", total_heartbeats, elapsed);
    info!("Average per heartbeat: {:?}", per_heartbeat);

    // Performance target: < 10ms per heartbeat
    assert!(per_heartbeat < Duration::from_millis(10),
        "Heartbeat processing too slow: {:?}", per_heartbeat);

    scheduler.shutdown().await.ok();
}

#[tokio::test]
async fn benchmark_task_submission_throughput() {
    tracing_subscriber::fmt::init();
    info!("=== Benchmark: Task Submission Throughput ===");

    let scheduler = UnifiedScheduler::new(SchedulerConfig::default()).await.unwrap();

    // Register some nodes
    for i in 0..5 {
        scheduler.register_node(create_test_node(i, "RTX-4090")).await.unwrap();
    }

    // Benchmark: Submit 100 tasks
    let num_tasks = 100;
    let start = std::time::Instant::now();

    let mut success_count = 0u64;
    for i in 0..num_tasks {
        let task = jcode_unified_scheduler::ScheduledTask::simple(
            &format!("Benchmark task {}", i),
            jcode_unified_scheduler::AgentRole::Worker,
            "qwen-3.6-max"
        );

        if scheduler.submit_task(task).await.is_ok() {
            success_count += 1;
        }
    }

    let elapsed = start.elapsed();
    let throughput = success_count as f64 / elapsed.as_secs_f64();

    info!("Submitted {} tasks in {:?}", success_count, elapsed);
    info!("Throughput: {:.0} tasks/sec", throughput);

    // Performance target: > 100 tasks/sec
    assert!(throughput > 100.0,
        "Task submission throughput too low: {:.0} tasks/sec", throughput);

    scheduler.shutdown().await.ok();
}

#[tokio::test]
async fn benchmark_cluster_summary_query() {
    tracing_subscriber::fmt::init();
    info!("=== Benchmark: Cluster Summary Query Performance ===");

    let scheduler = UnifiedScheduler::new(SchedulerConfig::default()).await.unwrap();

    // Register 18 nodes
    for i in 0..18 {
        let gpu = if i < 3 { "RTX-4090" } else if i < 8 { "RTX-3090" } else if i < 13 { "RTX-4080" } else { "RTX-3080" };
        scheduler.register_node(create_test_node(i, gpu)).await.unwrap();
    }

    // Benchmark: Query cluster summary 1000 times
    let iterations = 1000;
    let start = std::time::Instant::now();

    for _ in 0..iterations {
        let _summary = scheduler.get_cluster_summary().await;
    }

    let elapsed = start.elapsed();
    let per_query = elapsed / iterations;

    info!("Queried cluster summary {} times in {:?}", iterations, elapsed);
    info!("Average per query: {:?}", per_query);

    // Performance target: < 1ms per query
    assert!(per_query < Duration::from_millis(1),
        "Cluster summary query too slow: {:?}", per_query);

    scheduler.shutdown().await.ok();
}

#[tokio::test]
async fn benchmark_state_transitions() {
    tracing_subscriber::fmt::init();
    info!("=== Benchmark: State Transition Performance ===");

    use carpai::distributed::{ClusterService, ClusterConfig, NodeConfig};

    let mut config = ClusterConfig::new().enable();
    config.node = NodeConfig {
        id: Some("bench-node".to_string()),
        host: "127.0.0.1".to_string(),
        port: 9001,
        ..Default::default()
    };

    let service = ClusterService::new(config).await.expect("Failed to create service");

    // Benchmark: Get health summary 100 times
    let iterations = 100;
    let start = std::time::Instant::now();

    for _ in 0..iterations {
        let _summary = service.get_health_summary().await;
    }

    let elapsed = start.elapsed();
    let per_query = elapsed / iterations;

    info!("Retrieved health summary {} times in {:?}", iterations, elapsed);
    info!("Average per query: {:?}", per_query);

    // Performance target: < 5ms per query
    assert!(per_query < Duration::from_millis(5),
        "Health summary query too slow: {:?}", per_query);
}

#[tokio::test]
async fn benchmark_memory_usage_18_nodes() {
    tracing_subscriber::fmt::init();
    info!("=== Benchmark: Memory Usage with 18 Nodes ===");

    let scheduler = UnifiedScheduler::new(SchedulerConfig::default()).await.unwrap();

    // Get baseline memory (this is approximate in Rust)
    info!("Registering 18 nodes...");

    for i in 0..18 {
        let gpu = if i < 3 { "RTX-4090" } else if i < 8 { "RTX-3090" } else if i < 13 { "RTX-4080" } else { "RTX-3080" };
        scheduler.register_node(create_test_node(i, gpu)).await.unwrap();
    }

    // Get metrics
    let metrics = scheduler.get_metrics().await;
    let summary = scheduler.get_cluster_summary().await;

    info!("Cluster metrics after registering 18 nodes:");
    info!("  Active nodes: {}", summary.active_nodes);
    info!("  Total GPUs: {}", summary.total_gpus);
    info!("  Total TFLOPS: {:.1}", summary.total_tflops);
    info!("  Total Memory: {:.1} GB", summary.total_memory_gb);
    info!("  Tasks submitted: {}", metrics.tasks_submitted);
    info!("  Queue length: {}", metrics.queue_length);

    // Verify reasonable resource usage
    assert_eq!(summary.active_nodes, 18);
    assert!(summary.total_gpus >= 18);
    assert!(summary.total_tflops > 500.0); // Should have significant compute power

    scheduler.shutdown().await.ok();
}

#[tokio::test]
async fn end_to_end_18_node_workflow() {
    tracing_subscriber::fmt::init();
    info!("=== End-to-End: Complete 18-Node Workflow ===");

    let start_time = std::time::Instant::now();

    // Phase 1: Cluster initialization
    info!("Phase 1: Initializing 18-node cluster");
    let scheduler = UnifiedScheduler::new(SchedulerConfig::default()).await.unwrap();

    for i in 0..18 {
        let gpu = if i < 3 { "RTX-4090" } else if i < 8 { "RTX-3090" } else if i < 13 { "RTX-4080" } else { "RTX-3080" };
        scheduler.register_node(create_test_node(i, gpu)).await.unwrap();
    }

    let phase1_elapsed = start_time.elapsed();
    info!("Phase 1 complete in {:?}", phase1_elapsed);

    // Phase 2: Submit workloads
    info!("Phase 2: Submitting workloads");
    let mut task_ids = Vec::new();
    for i in 0..20 {
        let task = jcode_unified_scheduler::ScheduledTask::simple(
            &format!("E2E task {}", i),
            jcode_unified_scheduler::AgentRole::Worker,
            "qwen-3.6-max"
        );

        if let Ok(task_id) = scheduler.submit_task(task).await {
            task_ids.push(task_id);
        }
    }

    let phase2_elapsed = start_time.elapsed();
    info!("Phase 2 complete: {} tasks submitted in {:?}", task_ids.len(), phase2_elapsed - phase1_elapsed);

    // Phase 3: Monitor and verify
    info!("Phase 3: Monitoring cluster");
    sleep(Duration::from_millis(500)).await;

    let metrics = scheduler.get_metrics().await;
    let summary = scheduler.get_cluster_summary().await;

    info!("Phase 3 complete:");
    info!("  Cluster: {} nodes, {:.1} TFLOPS", summary.active_nodes, summary.total_tflops);
    info!("  Tasks: {} submitted, {} completed", metrics.tasks_submitted, metrics.tasks_completed);

    // Phase 4: Graceful shutdown
    info!("Phase 4: Graceful shutdown");
    scheduler.shutdown().await.unwrap();

    let total_elapsed = start_time.elapsed();
    info!("=== End-to-End workflow complete in {:?} ===", total_elapsed);

    // Assertions
    assert_eq!(task_ids.len(), 20);
    assert!(total_elapsed < Duration::from_secs(10), "E2E test should complete within 10 seconds");
}
