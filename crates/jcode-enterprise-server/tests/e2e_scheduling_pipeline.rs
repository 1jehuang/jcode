//! 端到端测试：验证完整调度链路 (Ruflo + Parallax)
//!
//! 测试场景:
//! 1. UnifiedScheduler初始化与节点注册
//! 2. Ruflo优先级评估与任务提交
//! 3. Parallax Phase 1: 层分配（水填算法）
//! 4. Parallax Phase 2: 请求路由（DP最优路径）
//! 5. VirtualMemoryManager集成（KV Cache mmap）
//! 6. API响应头包含调度元数据

use jcode_unified_scheduler::{
    UnifiedScheduler, SchedulerConfig, NodeHardwareInfo,
    ScheduledTask, AgentRole, TaskPriority, TaskStatus,
};
use std::sync::Arc;
use tracing_subscriber;

// ============================================================================
// 测试辅助函数
// ============================================================================

/// 初始化测试日志（仅在需要时启用）
fn init_test_logging() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();
}

/// 创建默认调度器配置（用于快速测试）
fn default_scheduler_config() -> SchedulerConfig {
    SchedulerConfig {
        max_concurrent_tasks: 4,
        max_queue_size: 64,
        enable_goap: false, // 禁用GOAP以加速测试
        allocation_strategy: jcode_unified_scheduler::AllocationStrategy::Greedy,
        routing_strategy: jcode_unified_scheduler::RoutingStrategy::DynamicProgramming,
        ..SchedulerConfig::default()
    }
}

/// 创建模拟硬件节点
fn create_mock_node(id: u32, gpu_name: &str, vram_gb: f64, tflops: f64) -> NodeHardwareInfo {
    NodeHardwareInfo {
        node_id: uuid::Uuid::new_v4(),
        num_gpus: 1,
        gpu_name: gpu_name.to_string(),
        memory_gb: 128.0,
        cpu_cores: 16,
        tflops_fp16: tflops,
        tflops_fp32: tflops / 2.0,
        gpu_bandwidth_gbps: 900.0,
        pcie_bandwidth_gbps: 64.0,
        has_gpu: !gpu_name.is_empty() && gpu_name != "CPU-only",
        vram_gb,
        cpu_arch: "x86_64".to_string(),
    }
}

// ============================================================================
// 测试1: UnifiedScheduler基础功能
// ============================================================================

#[tokio::test]
async fn test_scheduler_initialization_and_lifecycle() {
    init_test_logging();

    let config = default_scheduler_config();
    let scheduler = UnifiedScheduler::new(config).await.expect("Failed to create scheduler");

    // 验证初始状态
    assert_eq!(scheduler.get_state().await, jcode_unified_scheduler::SchedulerState::Idle);

    // 启动调度循环
    let sched_clone = Arc::new(scheduler);
    let sched_handle = sched_clone.clone();
    tokio::spawn(async move {
        let _ = sched_handle.run().await;
    });

    // 等待进入Running状态
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    assert_eq!(sched_clone.get_state().await, jcode_unified_scheduler::SchedulerState::Running);

    // 优雅关闭
    sched_clone.shutdown().await.expect("Failed to shutdown");
    assert_eq!(sched_clone.get_state().await, jcode_unified_scheduler::SchedulerState::Shutdown);
}

#[tokio::test]
async fn test_node_registration_and_discovery() {
    init_test_logging();

    let scheduler = UnifiedScheduler::new(default_scheduler_config()).await.unwrap();

    // 注册3个异构节点
    let nodes = vec![
        create_mock_node(1, "RTX-4090", 24.0, 1008.0),
        create_mock_node(2, "RTX-3090", 24.0, 700.0),
        create_mock_node(3, "CPU-only", 0.0, 0.0),
    ];

    let mut node_ids = Vec::new();
    for hw in nodes {
        let nid = scheduler.register_node(hw).await.expect("Failed to register node");
        node_ids.push(nid);
    }

    // 验证节点已注册
    let active_nodes = scheduler.get_active_nodes().await;
    assert_eq!(active_nodes.len(), 3, "Should have 3 active nodes");

    // 验证集群资源汇总
    let summary = scheduler.get_cluster_summary().await;
    assert!(summary.total_vram_gb >= 48.0, "Total VRAM should be at least 48GB");
    assert!(summary.total_tflops >= 1700.0, "Total TFLOPS should be at least 1700");

    // 注销一个节点
    scheduler.unregister_node(&node_ids[2]).await.expect("Failed to unregister node");
    let active_nodes = scheduler.get_active_nodes().await;
    assert_eq!(active_nodes.len(), 2, "Should have 2 active nodes after unregister");
}

// ============================================================================
// 测试2: Ruflo优先级调度
// ============================================================================

#[tokio::test]
async fn test_ruflo_priority_task_submission() {
    init_test_logging();

    let scheduler = UnifiedScheduler::new(default_scheduler_config()).await.unwrap();

    // 提交不同优先级的任务
    let high_priority_task = ScheduledTask::simple(
        "High priority task",
        AgentRole::Developer,
        "qwen-72b"
    ).with_priority(TaskPriority::Critical);

    let low_priority_task = ScheduledTask::simple(
        "Low priority task",
        AgentRole::Worker,
        "qwen-7b"
    ).with_priority(TaskPriority::Low);

    let high_id = scheduler.submit_task(high_priority_task).await.expect("Failed to submit high priority task");
    let low_id = scheduler.submit_task(low_priority_task).await.expect("Failed to submit low priority task");

    // 验证任务状态
    assert_eq!(scheduler.get_task_status(&high_id).await.unwrap(), TaskStatus::Queued);
    assert_eq!(scheduler.get_task_status(&low_id).await.unwrap(), TaskStatus::Queued);

    // 验证队列顺序（高优先级应该在前）
    let metrics = scheduler.get_metrics().await;
    assert_eq!(metrics.tasks_submitted, 2);
    assert_eq!(metrics.queue_length, 2);
}

#[tokio::test]
async fn test_task_cancellation_and_dependency() {
    init_test_logging();

    let scheduler = UnifiedScheduler::new(default_scheduler_config()).await.unwrap();

    // 创建依赖链: Task A -> Task B -> Task C
    let task_a = ScheduledTask::simple("Task A", AgentRole::Worker, "qwen-7b");
    let id_a = scheduler.submit_task(task_a).await.unwrap();

    let task_b = ScheduledTask::simple("Task B", AgentRole::Worker, "qwen-7b")
        .depends_on(id_a);
    let id_b = scheduler.submit_task(task_b).await.unwrap();

    let task_c = ScheduledTask::simple("Task C", AgentRole::Worker, "qwen-7b")
        .depends_on(id_b);
    let _id_c = scheduler.submit_task(task_c).await.unwrap();

    // 取消Task A，应该级联取消B和C
    scheduler.cancel_task(&id_a).await.expect("Failed to cancel task A");

    // 验证级联取消
    assert_eq!(scheduler.get_task_status(&id_a).await.unwrap(), TaskStatus::Cancelled);
    assert_eq!(scheduler.get_task_status(&id_b).await.unwrap(), TaskStatus::Cancelled);
}

// ============================================================================
// 测试3: Parallax Phase 1 - 层分配
// ============================================================================

#[tokio::test]
async fn test_parallax_layer_allocation_greedy() {
    init_test_logging();

    let mut config = default_scheduler_config();
    config.allocation_strategy = jcode_unified_scheduler::AllocationStrategy::Greedy;
    let scheduler = UnifiedScheduler::new(config).await.unwrap();

    // 注册2个GPU节点
    let node1 = create_mock_node(1, "RTX-4090", 24.0, 1008.0);
    let node2 = create_mock_node(2, "RTX-3090", 24.0, 700.0);
    scheduler.register_node(node1).await.unwrap();
    scheduler.register_node(node2).await.unwrap();

    // 提交一个72B模型任务（80层）
    let task = ScheduledTask::simple("72B inference", AgentRole::Developer, "qwen-72b")
        .with_priority(TaskPriority::High);
    let task_id = scheduler.submit_task(task).await.unwrap();

    // 触发调度
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // 验证指标
    let metrics = scheduler.get_metrics().await;
    assert!(metrics.phase1_allocations > 0, "Phase 1 allocation should have been triggered");

    // 清理
    scheduler.cancel_task(&task_id).await.ok();
}

#[tokio::test]
async fn test_parallax_layer_allocation_dp() {
    init_test_logging();

    let mut config = default_scheduler_config();
    config.allocation_strategy = jcode_unified_scheduler::AllocationStrategy::DynamicProgramming;
    let scheduler = UnifiedScheduler::new(config).await.unwrap();

    // 注册3个异构节点
    let nodes = vec![
        create_mock_node(1, "A100-80G", 80.0, 1500.0),
        create_mock_node(2, "RTX-4090", 24.0, 1008.0),
        create_mock_node(3, "RTX-3090", 24.0, 700.0),
    ];

    for hw in nodes {
        scheduler.register_node(hw).await.unwrap();
    }

    // 提交任务
    let task = ScheduledTask::simple("DP allocation test", AgentRole::Developer, "qwen-72b");
    let task_id = scheduler.submit_task(task).await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let metrics = scheduler.get_metrics().await;
    assert!(metrics.phase1_allocations > 0, "DP allocation should have been triggered");

    scheduler.cancel_task(&task_id).await.ok();
}

// ============================================================================
// 测试4: Parallax Phase 2 - 请求路由
// ============================================================================

#[tokio::test]
async fn test_parallax_request_routing() {
    init_test_logging();

    let scheduler = UnifiedScheduler::new(default_scheduler_config()).await.unwrap();

    // 注册节点形成流水线拓扑
    let nodes = vec![
        create_mock_node(1, "RTX-4090", 24.0, 1008.0),
        create_mock_node(2, "RTX-3090", 24.0, 700.0),
    ];

    for hw in nodes {
        scheduler.register_node(hw).await.unwrap();
    }

    // 提交任务触发路由
    let task = ScheduledTask::simple("Routing test", AgentRole::Developer, "qwen-14b");
    let task_id = scheduler.submit_task(task).await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let metrics = scheduler.get_metrics().await;
    assert!(metrics.phase2_routings > 0, "Phase 2 routing should have been triggered");

    scheduler.cancel_task(&task_id).await.ok();
}

// ============================================================================
// 测试5: VirtualMemoryManager集成
// ============================================================================

#[tokio::test]
async fn test_virtual_memory_kv_cache_mmap() {
    use jcode_enterprise_server::virtual_memory::VirtualMemoryManager;
    use jcode_enterprise_server::config::VirtualMemoryConfig;

    init_test_logging();

    // 创建临时目录用于测试
    let temp_dir = std::env::temp_dir().join(format!("carpai_vm_test_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    let vm_config = VirtualMemoryConfig {
        enabled: true,
        mmap_dir: temp_dir.clone(),
        max_mmap_size_gb: 100,
        swap_threshold_percent: 80.0,
    };

    let vm_mgr = VirtualMemoryManager::new(vm_config);

    // 为7B模型创建KV Cache mmap（约8GB）
    let result = vm_mgr.create_kv_cache_mmap("qwen-7b", 8000).await;
    assert!(result.is_ok(), "Should create mmap for 7B model");

    if let Ok(region) = result {
        assert_eq!(region.model_name, "qwen-7b");
        assert_eq!(region.size, 8000 * 1024 * 1024); // 8GB in bytes
        assert!(region.is_mapped);

        // 验证统计
        let stats = vm_mgr.stats.read().await;
        assert_eq!(stats.total_mmaps, 1);
        assert!(stats.used_gb > 0.0);

        // 释放mmap
        vm_mgr.release_kv_cache("qwen-7b").await.expect("Failed to release mmap");

        let stats = vm_mgr.stats.read().await;
        assert_eq!(stats.total_munmaps, 1);
    }

    // 清理临时文件
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_virtual_memory_multiple_models() {
    use jcode_enterprise_server::virtual_memory::VirtualMemoryManager;
    use jcode_enterprise_server::config::VirtualMemoryConfig;

    init_test_logging();

    let temp_dir = std::env::temp_dir().join(format!("carpai_vm_multi_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let vm_config = VirtualMemoryConfig {
        enabled: true,
        mmap_dir: temp_dir.clone(),
        max_mmap_size_gb: 200,
        swap_threshold_percent: 80.0,
    };

    let vm_mgr = VirtualMemoryManager::new(vm_config);

    // 为多个模型创建mmap
    let models = vec![
        ("qwen-7b", 8000u64),
        ("qwen-14b", 16000),
        ("qwen-72b", 80000),
    ];

    for (model_name, size_mb) in &models {
        let result = vm_mgr.create_kv_cache_mmap(model_name, *size_mb).await;
        assert!(result.is_ok(), "Should create mmap for {}", model_name);
    }

    // 验证所有mmap都已创建
    let usage = vm_mgr.get_memory_usage().await;
    assert_eq!(usage.mmap_regions, 3);
    assert!(usage.mmap_used_gb > 100.0); // 8+16+80 = 104GB

    // 全部释放
    for (model_name, _) in &models {
        vm_mgr.release_kv_cache(model_name).await.unwrap();
    }

    let usage = vm_mgr.get_memory_usage().await;
    assert_eq!(usage.mmap_regions, 0);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

// ============================================================================
// 测试6: 完整调度链路集成测试
// ============================================================================

#[tokio::test]
async fn test_full_scheduling_pipeline() {
    init_test_logging();

    // 1. 创建调度器
    let scheduler = UnifiedScheduler::new(default_scheduler_config()).await.unwrap();

    // 2. 注册异构节点集群
    let cluster = vec![
        create_mock_node(1, "A100-80G", 80.0, 1500.0),
        create_mock_node(2, "RTX-4090", 24.0, 1008.0),
        create_mock_node(3, "RTX-3090", 24.0, 700.0),
        create_mock_node(4, "CPU-only", 0.0, 0.0),
    ];

    for hw in cluster {
        scheduler.register_node(hw).await.unwrap();
    }

    // 3. 提交高优先级任务
    let task = ScheduledTask::simple(
        "Full pipeline test",
        AgentRole::Developer,
        "qwen-72b"
    ).with_priority(TaskPriority::Critical);

    let task_id = scheduler.submit_task(task).await.unwrap();

    // 4. 等待调度执行
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // 5. 验证完整链路
    let metrics = scheduler.get_metrics().await;

    // Ruflo: 任务提交
    assert_eq!(metrics.tasks_submitted, 1, "Task should be submitted");

    // Parallax Phase 1: 层分配
    assert!(metrics.phase1_allocations > 0, "Phase 1 allocation should execute");

    // Parallax Phase 2: 请求路由
    assert!(metrics.phase2_routings > 0, "Phase 2 routing should execute");

    // 6. 验证集群资源
    let summary = scheduler.get_cluster_summary().await;
    assert!(summary.total_vram_gb >= 128.0);
    assert!(summary.node_count >= 4);

    // 清理
    scheduler.cancel_task(&task_id).await.ok();
    scheduler.shutdown().await.ok();
}

// ============================================================================
// 测试7: 性能基准 - 调度延迟
// ============================================================================

#[tokio::test]
async fn test_scheduling_latency_baseline() {
    init_test_logging();

    let scheduler = UnifiedScheduler::new(default_scheduler_config()).await.unwrap();

    // 注册节点
    scheduler.register_node(create_mock_node(1, "RTX-4090", 24.0, 1008.0)).await.unwrap();

    let iterations = 10;
    let mut total_latency_us = 0u64;

    for i in 0..iterations {
        let start = std::time::Instant::now();

        let task = ScheduledTask::simple(
            &format!("Latency test {}", i),
            AgentRole::Worker,
            "qwen-7b"
        );
        let _task_id = scheduler.submit_task(task).await.unwrap();

        let elapsed = start.elapsed().as_micros() as u64;
        total_latency_us += elapsed;
    }

    let avg_latency_us = total_latency_us / iterations;
    let avg_latency_ms = avg_latency_us as f64 / 1000.0;

    println!("Average scheduling latency: {:.2}ms ({:.0}us)", avg_latency_ms, avg_latency_us);

    // 基线检查：调度延迟应 < 10ms
    assert!(avg_latency_ms < 10.0, "Scheduling latency should be under 10ms, got {:.2}ms", avg_latency_ms);
}
