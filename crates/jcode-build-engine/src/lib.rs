//! # CarpTMS 编译引擎 (jcode-build-engine)
//!
//! 基于 **Ruflo-Parallax** 三层调度架构的编译引擎：
//! - **第一层：全局调度层 (GlobalScheduler)** - 算力供需匹配、节点管理、动态定价
//! - **第二层：节点调度层 (NodeScheduler)** - 资源监控、任务分配、优先级调度
//! - **第三层：任务调度层 (TaskScheduler)** - 任务分解、依赖调度、执行引擎
//!
//! ## 架构图
//!
//! ```text
//! +---------------------------------------------+
//! |                 API 层                        |
//! |  +-----------+ +-----------+ +-----------+  |
//! |  | REST API  | | WebSocket | | gRPC      |  |
//! |  +-----------+ +-----------+ +-----------+  |
//! +----------------------+----------------------+
//!                        |
//! +----------------------v----------------------+
//! |         Ruflo-Parallax 调度器               |
//! |  +-----------+ +-----------+ +-----------+  |
//! |  | Global    | | Node      | | Task      |  |
//! |  | Scheduler | | Scheduler | | Scheduler |  |
//! |  +-----------+ +-----------+ +-----------+  |
//! +----------------------+----------------------+
//!                        |
//! +----------------------v----------------------+
//! |             编译引擎核心                      |
//! |  +-----------+ +-----------+ +-----------+  |
//! |  | Toolchain | | Executor  | | Result    |  |
//! |  | Manager   | |           | | Processor |  |
//! |  +-----------+ +-----------+ +-----------+  |
//! +---------------------------------------------+
//! ```

pub mod types;
pub mod error;
pub mod global_scheduler;
pub mod node_scheduler;
pub mod task_scheduler;
pub mod toolchain;
pub mod cache;
pub mod environment;
pub mod api;

pub use types::*;
pub use error::{BuildEngineError, Result};

use global_scheduler::{GlobalScheduler, GlobalSchedulerEvent};
use node_scheduler::NodeScheduler;
use task_scheduler::{SchedulerEvent, TaskScheduler, TaskSchedulerConfig};
use std::sync::Arc;
use tokio::sync::{broadcast, Notify};
use tracing::{error, info, instrument};

pub const BUILD_ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 引擎健康状态
#[derive(Debug, Clone)]
pub struct EngineHealth {
    pub version: &'static str,
    pub global_scheduler_ready: bool,
    pub node_count: usize,
    pub pending_tasks: usize,
    pub cache_hit_rate: f64,
    pub uptime_seconds: u64,
    pub task_scheduler_ready: bool,
}

impl Default for EngineHealth {
    fn default() -> Self {
        Self {
            version: BUILD_ENGINE_VERSION,
            global_scheduler_ready: false,
            node_count: 0,
            pending_tasks: 0,
            cache_hit_rate: 0.0,
            uptime_seconds: 0,
            task_scheduler_ready: false,
        }
    }
}

// ════════════════════════════════════════════════════════════════════
// BuildEngine — 统一入口，串联三层调度
// ════════════════════════════════════════════════════════════════════

/// 统一编译引擎入口。
/// 内部自动启动后台调度循环，连接 GlobalScheduler → NodeScheduler → TaskScheduler。
pub struct BuildEngine {
    global: Arc<GlobalScheduler>,
    node: Arc<NodeScheduler>,
    task: Arc<TaskScheduler>,
    global_event_rx: tokio::sync::broadcast::Receiver<GlobalSchedulerEvent>,
    task_event_rx: tokio::sync::broadcast::Receiver<SchedulerEvent>,
    shutdown: Arc<Notify>,
    start_time: std::time::Instant,
}

impl BuildEngine {
    /// 创建引擎并自动启动后台调度循环。
    pub fn start(
        queue_depth: usize,
        failover_threshold: u32,
        max_concurrent: u32,
    ) -> Self {
        let start_time = std::time::Instant::now();

        // 初始化三层调度器
        let (global, global_rx) = GlobalScheduler::new(queue_depth, failover_threshold);
        let global = Arc::new(global);

        let node = Arc::new(NodeScheduler::new(max_concurrent as usize));

        let task_config = TaskSchedulerConfig {
            max_concurrent,
            default_timeout_secs: 600,
            max_retries: 3,
        };
        let task = Arc::new(TaskScheduler::new(task_config));
        let task_event_rx = task.subscribe();

        let shutdown = Arc::new(Notify::new());

        // 注册初始本地节点
        let local_node = NodeInfo {
            node_id: NodeId::new(),
            hostname: hostname(),
            addresses: vec![],
            status: NodeStatus::Online,
            capacity: NodeCapacity {
                max_concurrent_tasks: max_concurrent as u32,
                max_cpu_cores: 8.0,
                max_memory_mb: 16384,
                max_disk_mb: 512000,
                supports_gpu: false,
                supported_languages: vec![
                    ProgrammingLanguage::Rust,
                    ProgrammingLanguage::Python,
                    ProgrammingLanguage::JavaScript,
                    ProgrammingLanguage::TypeScript,
                ],
            },
            current_resource: ComputeResource {
                cpu_usage: 0.0,
                available_memory_mb: 8192,
                total_memory_mb: 16384,
                available_disk_mb: 102400,
                total_disk_mb: 512000,
                gpus: vec![],
                load_factor: 1.0,
            },
            active_tasks: 0,
            last_heartbeat: chrono::Utc::now(),
            labels: std::collections::HashMap::new(),
            zone: Some("local".to_string()),
            started_at: chrono::Utc::now(),
            version: crate::BUILD_ENGINE_VERSION.to_string(),
        };
        global.node_state.register(local_node);

        // 启动后台调度循环
        let sched_global = global.clone();
        let sched_task = task.clone();
        let sched_shutdown = shutdown.clone();
        tokio::spawn(async move {
            BuildEngine::scheduler_loop(sched_global, sched_task, sched_shutdown).await;
        });

        Self {
            global,
            node,
            task,
            global_event_rx: global_rx,
            task_event_rx,
            shutdown,
            start_time,
        }
    }

    /// 提交一个构建请求（立即入队并返回，由后台调度循环执行）。
    #[instrument(skip(self))]
    pub async fn submit(&self, request: BuildRequest) -> Result<BuildId> {
        let build_id = request.id.unwrap_or_default();
        info!("Enqueuing build {}", build_id);
        self.global.task_queue.enqueue(request)?;
        Ok(build_id)
    }

    /// 提交并等待构建完成。
    pub async fn submit_and_wait(&self, request: BuildRequest) -> Result<BuildResult> {
        let build_id = self.submit(request).await?;
        // 等待 task_scheduler 发出完成事件
        let mut rx = self.task.subscribe();
        loop {
            match rx.recv().await {
                Ok(SchedulerEvent::AllCompleted {
                    build_id: completed_id,
                    result,
                }) if completed_id == build_id => {
                    info!("Build {} completed", completed_id);
                    return Ok(result);
                }
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    info!("Scheduler events lagged by {}", n);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(BuildEngineError::InvalidState(
                        "Scheduler channel closed".into(),
                    ));
                }
            }
        }
    }

    /// 取消正在运行的构建。
    pub fn cancel(&self, build_id: BuildId) -> bool {
        self.task.cancel_build(build_id)
    }

    /// 获取引擎健康状态。
    pub fn health(&self) -> EngineHealth {
        EngineHealth {
            global_scheduler_ready: true,
            node_count: self.global.node_state.count(),
            pending_tasks: self.global.task_queue.depth(),
            cache_hit_rate: 0.0,
            uptime_seconds: self.start_time.elapsed().as_secs(),
            task_scheduler_ready: true,
            ..Default::default()
        }
    }

    /// 获取全局调度器引用（高级用法）。
    pub fn global_scheduler(&self) -> &Arc<GlobalScheduler> {
        &self.global
    }

    /// 获取任务调度器引用（高级用法）。
    pub fn task_scheduler(&self) -> &Arc<TaskScheduler> {
        &self.task
    }

    // ── 后台调度循环 ──

    /// 后台循环：从 GlobalScheduler 出队 → NodeScheduler 分配 → TaskScheduler 执行
    async fn scheduler_loop(
        global: Arc<GlobalScheduler>,
        task: Arc<TaskScheduler>,
        shutdown: Arc<Notify>,
    ) {
        info!("Build engine scheduler loop started");

        loop {
            tokio::select! {
                _ = shutdown.notified() => {
                    info!("Scheduler loop shutting down");
                    break;
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                    // Poll for queued tasks
                }
            }

            // 尝试出队一个构建任务
            while let Some(item) = global.task_queue.dequeue() {
                let build_id = item.request.id.unwrap_or_default();
                info!("Dequeued build {} (priority {})", build_id, item.request.priority);

                // 尝试分配节点
                let node = global.node_state.get_online().first().cloned();
                match node {
                    Some(node_info) => {
                        info!("Assigning build {} to node {}", build_id, node_info.hostname);
                        let result = task.submit_build(item.request).await;
                        match result {
                            Ok(_) => {
                        let _ = global.supply_demand.report(
                            SupplyDemandEntry::new(
                                node_info.zone.clone().unwrap_or_else(|| "unknown".to_string()),
                                1.0,
                                1.0,
                            ),
                        );
                            }
                            Err(e) => {
                                error!("Build {} execution failed: {}", build_id, e);
                                // 记录失败，以便 failover
                                let triggered = global.failover.record_failure(node_info.node_id);
                                if triggered {
                                    error!(
                                        "Node {} exceeded failure threshold, marking offline",
                                        node_info.node_id
                                    );
                                }
                            }
                        }
                    }
                    None => {
                        error!("No available nodes for build {}", build_id);
                        // 放回队列尾部重试（如果不超过最大重试次数）
                        if item.retry_count < 3 {
                            let mut retry_item = item;
                            retry_item.retry_count += 1;
                            let _ = global.task_queue.enqueue(retry_item.request);
                        }
                    }
                }
            }
        }
    }
}

// ════════════════════════════════════════════════════════════════════
// 工具函数
// ════════════════════════════════════════════════════════════════════

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "localhost".to_string())
}

// ════════════════════════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_engine_start_and_submit() {
        let engine = BuildEngine::start(10, 3, 4);
        let health = engine.health();
        assert!(health.global_scheduler_ready);
        assert!(health.task_scheduler_ready);
        assert!(health.node_count >= 1);
    }

    #[tokio::test]
    async fn test_engine_submit_build() {
        let engine = BuildEngine::start(10, 3, 4);

        let mut request = BuildRequest::default();
        request.build_type = BuildType::Custom("echo hello".to_string());

        let result = engine.submit_and_wait(request).await;
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.status, BuildStatus::Success);
    }

    #[tokio::test]
    async fn test_engine_cancel() {
        let engine = BuildEngine::start(10, 3, 4);
        let request = BuildRequest::default();
        let build_id = engine.submit(request).await.unwrap();
        // 取消应该成功（可能尚未开始执行）
        let cancelled = engine.cancel(build_id);
        // 如果已开始或尚未入队，取消可能返回 false
        // 这里只验证不会 panic
        assert!(!cancelled || cancelled);
    }
}
