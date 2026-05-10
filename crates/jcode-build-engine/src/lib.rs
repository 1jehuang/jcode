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
}
