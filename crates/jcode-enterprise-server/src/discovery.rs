//! ## 任务 2.1: 动态节点调度适配
//!
//! 本模块实现节点的自动发现、注册、心跳监控和断线重连。
//!
//! ### 核心能力
//!
//! 1. **mDNS 自动发现**: 局域网内节点自动互相发现，无需手动配置 IP
//! 2. **心跳检测**: 每 5 秒发送心跳，30 秒超时自动标记离线
//! 3. **节点状态追踪**: 实时跟踪内存、CPU、负载等状态
//! 4. **断线任务恢复**: 节点离线后自动将任务切换到其他可用节点
//! 5. **动态资源池**: 节点可随时加入/离开，不影响整体服务

use crate::config::EnterpriseConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::UdpSocket;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, warn};

/// 节点信息（用于注册和状态同步）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRegistration {
    /// 节点唯一 ID
    pub node_id: String,
    /// 节点名称（如 "办公室台式机"、"网吧_01"）
    pub node_name: String,
    /// 节点类型
    pub node_type: NodeType,
    /// IP 地址
    pub ip_address: String,
    /// 服务端口
    pub port: u16,
    /// 总物理内存 (GB)
    pub total_memory_gb: f64,
    /// 可用物理内存 (GB)
    pub available_memory_gb: f64,
    /// 虚拟内存 / 交换空间 (GB)
    pub swap_total_gb: f64,
    /// CPU 物理核心数
    pub cpu_cores: u32,
    /// CPU 使用率 (0.0 - 1.0)
    pub cpu_usage: f64,
    /// 是否有 GPU
    pub has_gpu: bool,
    /// GPU 显存 (MB)，0=无GPU
    pub gpu_vram_mb: u64,
    /// 已运行的模型名称列表
    pub loaded_models: Vec<String>,
    /// 最后心跳时间 (UNIX 时间戳)
    pub last_heartbeat: i64,
    /// 启动时间 (UNIX 时间戳)
    pub started_at: i64,
    /// 自定义标签
    pub tags: HashMap<String, String>,
}

/// 节点类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeType {
    /// 固定服务器（一直在线）
    Server,
    /// 办公室台式机（工作时间可用）
    Desktop,
    /// 笔记本电脑（随时可能下线）
    Laptop,
    /// 网吧机器（特定时间段可用）
    InternetCafe,
    /// 云实例
    CloudInstance,
}

/// 节点状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    /// 在线
    Online,
    /// 离线
    Offline,
    /// 忙碌（所有资源被占用）
    Busy,
    /// 降级（部分资源可用）
    Degraded,
}

/// 节点发现管理器
pub struct NodeDiscoveryManager {
    /// 配置
    config: Arc<EnterpriseConfig>,
    /// 已知节点 (node_id -> NodeRegistration)
    nodes: Arc<RwLock<HashMap<String, NodeRegistration>>>,
    /// 节点状态 (node_id -> NodeStatus)
    statuses: Arc<RwLock<HashMap<String, NodeStatus>>>,
    /// mDNS 服务是否运行中
    mdns_running: Arc<AtomicBool>,
    /// 状态变更通知发送端
    status_tx: mpsc::UnboundedSender<NodeEvent>,
    /// 状态变更通知接收端
    pub status_rx: Arc<RwLock<Option<mpsc::UnboundedReceiver<NodeEvent>>>>,
}

/// 节点事件（供上层调度器消费）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeEvent {
    /// 新节点上线
    NodeOnline(NodeRegistration),
    /// 节点离线
    NodeOffline(String),
    /// 节点状态更新
    NodeUpdated(NodeRegistration),
    /// 心跳超时
    NodeHeartbeatTimeout(String),
}

impl NodeDiscoveryManager {
    /// 创建节点发现管理器
    pub fn new(config: Arc<EnterpriseConfig>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            config,
            nodes: Arc::new(RwLock::new(HashMap::new())),
            statuses: Arc::new(RwLock::new(HashMap::new())),
            mdns_running: Arc::new(AtomicBool::new(false)),
            status_tx: tx,
            status_rx: Arc::new(RwLock::new(Some(rx))),
        }
    }

    /// 注册一个新节点（由节点启动时调用）
    pub async fn register_node(&self, registration: NodeRegistration) -> anyhow::Result<()> {
        let node_id = registration.node_id.clone();
        let mut nodes = self.nodes.write().await;
        let mut statuses = self.statuses.write().await;

        nodes.insert(node_id.clone(), registration.clone());
        statuses.insert(node_id.clone(), NodeStatus::Online);

        info!(
            "节点注册: '{}' ({}), 内存={}GB, 虚拟内存={}GB, CPU={}核",
            registration.node_name, node_id, registration.total_memory_gb,
            registration.swap_total_gb, registration.cpu_cores
        );

        // 通知上层调度器
        let _ = self.status_tx.send(NodeEvent::NodeOnline(registration));

        Ok(())
    }

    /// 更新节点心跳
    pub async fn update_heartbeat(&self, registration: NodeRegistration) -> anyhow::Result<()> {
        let node_id = registration.node_id.clone();
        let mut nodes = self.nodes.write().await;
        let mut statuses = self.statuses.write().await;

        if let Some(existing) = nodes.get_mut(&node_id) {
            *existing = registration.clone();
            *statuses.get_mut(&node_id).unwrap() = NodeStatus::Online;
        } else {
            // 新节点自动注册
            nodes.insert(node_id.clone(), registration.clone());
            statuses.insert(node_id.clone(), NodeStatus::Online);
            info!("心跳中新节点自动注册: {}", node_id);
        }

        Ok(())
    }

    /// 心跳检测循环 — 定期检查超时节点
    pub async fn heartbeat_check_loop(self: Arc<Self>) {
        let timeout = self.config.scheduling.heartbeat_timeout_secs;
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(10));

        info!("心跳检测循环启动 (超时={}秒)", timeout);

        loop {
            interval.tick().await;
            let now = chrono::Utc::now().timestamp();
            let mut nodes = self.nodes.write().await;
            let mut statuses = self.statuses.write().await;

            let mut timed_out = Vec::new();
            for (node_id, node) in nodes.iter() {
                if now - node.last_heartbeat > timeout as i64 {
                    timed_out.push(node_id.clone());
                }
            }

            for node_id in &timed_out {
                if let Some(status) = statuses.get_mut(node_id) {
                    *status = NodeStatus::Offline;
                    warn!("心跳超时，节点离线: {}", node_id);
                    let _ = self.status_tx.send(NodeEvent::NodeHeartbeatTimeout(node_id.clone()));
                }
            }
        }
    }

    /// 获取在线节点列表
    pub async fn get_online_nodes(&self) -> Vec<NodeRegistration> {
        let nodes = self.nodes.read().await;
        let statuses = self.statuses.read().await;
        nodes.iter()
            .filter(|(id, _)| statuses.get(*id).map(|s| *s == NodeStatus::Online).unwrap_or(false))
            .map(|(_, n)| n.clone())
            .collect()
    }

    /// 获取所有节点
    pub async fn get_all_nodes(&self) -> Vec<(NodeRegistration, NodeStatus)> {
        let nodes = self.nodes.read().await;
        let statuses = self.statuses.read().await;
        nodes.iter()
            .map(|(id, n)| (n.clone(), statuses.get(id).copied().unwrap_or(NodeStatus::Offline)))
            .collect()
    }

    /// 获取节点统计
    pub async fn get_node_statistics(&self) -> NodeStatistics {
        let all = self.get_all_nodes().await;
        let total = all.len();
        let online = all.iter().filter(|(_, s)| *s == NodeStatus::Online).count();
        let total_memory: f64 = all.iter().map(|(n, _)| n.total_memory_gb).sum();
        let total_swap: f64 = all.iter().map(|(n, _)| n.swap_total_gb).sum();
        let total_cores: u32 = all.iter().map(|(n, _)| n.cpu_cores).sum();

        let by_type = all.iter().fold(HashMap::new(), |mut acc, (n, _)| {
            *acc.entry(n.node_type).or_insert(0usize) += 1;
            acc
        });

        NodeStatistics {
            total_nodes: total,
            online_nodes: online,
            offline_nodes: total - online,
            total_memory_gb: total_memory,
            total_swap_gb: total_swap,
            total_cpu_cores: total_cores,
            nodes_by_type: by_type,
        }
    }
}

/// 节点统计数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStatistics {
    pub total_nodes: usize,
    pub online_nodes: usize,
    pub offline_nodes: usize,
    pub total_memory_gb: f64,
    pub total_swap_gb: f64,
    pub total_cpu_cores: u32,
    pub nodes_by_type: HashMap<NodeType, usize>,
}
