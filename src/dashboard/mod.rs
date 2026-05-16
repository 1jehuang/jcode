//! # Web Dashboard - 可视化监控面板
//!
//! 提供基于Web的实时监控系统，包括：
//! - **系统状态** - CPU/内存/磁盘使用率
//! - **任务追踪** - 实时任务进度
//! - **性能指标** - 响应时间、吞吐量
//! - **会话管理** - 活跃会话列表
//! - **插件状态** - 已安装插件运行情况
//!
//! ## 架构设计
//!
//! ```
//! Browser <--> WebSocket/SSE <--> Dashboard Server
//!     |                         |
//!     |                    +----+----+
//!     |                    | Metrics | Collector
//!     |                    +----+----+
//!     |                         |
//!     |              +----------+----------+
//!     |              |          |          |
//!     |         TaskManager PluginRegistry AutoModeEngine
//! ```

pub mod server;
pub mod routes;
pub mod metrics;
pub mod templates;

pub use server::DashboardServer;
pub use metrics::SystemMetrics;
pub use routes::DashboardRoutes;