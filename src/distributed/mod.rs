//! # Distributed Coordination - 分布式协调系统
//!
//! 提供多节点集群管理能力，包括：
//! - **节点管理** - 发现、注册、健康检查
//! - **领导选举** - Raft一致性协议
//! - **状态同步** - CRDT数据复制
//! - **负载均衡** - 智能请求分发
//! - **故障恢复** - 自动故障转移
//!
//! ## 架构设计
//!
//! ```
//! Cluster (集群)
//! +-- Node A (Leader)
//! |   +-- Task Scheduler
//! |   +-- State Manager
//! +-- Node B (Follower)
//! |   +-- Worker Pool
//! +-- Node C (Follower)
//!     +-- Worker Pool
//!
//! Communication:
//! - gRPC for inter-node communication
//! - WebSocket for real-time updates
//! - HTTP/REST for external API
//! ```

pub mod config;
pub mod node;
pub mod cluster;
pub mod election;
pub mod sync;
pub mod load_balancer;
pub mod service;
pub mod cli;
pub mod integration;
pub mod metrics;
pub mod dashboard_api;
pub mod grpc_comm;

#[cfg(test)]
mod integration_tests;

pub use config::ClusterConfig;
pub use node::ClusterNode;
pub use cluster::ClusterManager;
pub use election::ElectionService;
pub use sync::{StateSync, CrdtType};
pub use load_balancer::LoadBalancer;
pub use service::ClusterService;
pub use cli::{ClusterArgs, execute_cluster_command};
pub use integration::{
    init_cluster_service,
    shutdown_cluster_service,
    is_cluster_enabled,
    is_local_node_leader,
    get_cluster_status,
    ClusterStatusInfo,
    execute_if_leader,
};
pub use metrics::{get_metrics, structured_log};
pub use dashboard_api::{DashboardServer, DashboardConfig};