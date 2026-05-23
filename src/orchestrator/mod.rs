//! 多Agent协作编排引擎
//!
//! 缺失能力补齐:
//! - Visual Swarm Dashboard: Agent状态可视化统计
//! - Auto Load Balancing: 自动在Agent间分配任务
//! - Conflict Detection: 检测文件编辑冲突
//! - Resource-aware Scheduling: CPU/内存感知的任务调度

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::RwLock;

/// Agent 工作负载
#[derive(Debug, Clone)]
pub struct AgentWorkload {
    pub agent_id: String,
    pub current_tasks: usize,
    pub cpu_usage: f32,
    pub memory_mb: u64,
    pub status: AgentStatus,
    pub last_heartbeat: SystemTime,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentStatus {
    Idle, Busy, Blocked, Error(String)
}

/// ===== [1] 可视化 Swarm Dashboard 数据 =====
pub struct SwarmDashboard {
    agents: Arc<RwLock<HashMap<String, AgentWorkload>>>,
    events: Arc<RwLock<Vec<SwarmEvent>>>,
    start_time: Instant,
}

#[derive(Debug, Clone)]
pub struct SwarmEvent {
    pub timestamp: SystemTime,
    pub agent_id: String,
    pub event_type: String,
    pub detail: String,
}

impl SwarmDashboard {
    pub fn new() -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
            events: Arc::new(RwLock::new(Vec::new())),
            start_time: Instant::now(),
        }
    }

    /// 注册Agent
    pub async fn register_agent(&self, agent_id: &str) {
        self.agents.write().await.insert(agent_id.to_string(), AgentWorkload {
            agent_id: agent_id.to_string(),
            current_tasks: 0,
            cpu_usage: 0.0,
            memory_mb: 0,
            status: AgentStatus::Idle,
            last_heartbeat: SystemTime::now(),
        });
        self.record_event(agent_id, "registered", "Agent joined swarm").await;
    }

    /// 更新Agent心跳
    pub async fn heartbeat(&self, agent_id: &str, cpu: f32, mem: u64, tasks: usize) {
        if let Some(agent) = self.agents.write().await.get_mut(agent_id) {
            agent.cpu_usage = cpu;
            agent.memory_mb = mem;
            agent.current_tasks = tasks;
            agent.last_heartbeat = SystemTime::now();
        }
    }

    /// 获取Dashboard JSON (供前端渲染)
    pub async fn dashboard_json(&self) -> String {
        let agents = self.agents.read().await;
        let uptime = self.start_time.elapsed().as_secs();
        let total_tasks: usize = agents.values().map(|a| a.current_tasks).sum();
        let active = agents.values().filter(|a| a.status == AgentStatus::Busy).count();

        serde_json::json!({
            "uptime_secs": uptime,
            "total_agents": agents.len(),
            "active_agents": active,
            "total_tasks": total_tasks,
            "agents": agents.values().map(|a| serde_json::json!({
                "id": a.agent_id,
                "status": format!("{:?}", a.status),
                "tasks": a.current_tasks,
                "cpu": a.cpu_usage,
                "memory_mb": a.memory_mb,
                "last_heartbeat": format!("{:?}", a.last_heartbeat),
            })).collect::<Vec<_>>(),
        }).to_string()
    }

    async fn record_event(&self, agent_id: &str, event_type: &str, detail: &str) {
        let mut events = self.events.write().await;
        events.push(SwarmEvent {
            timestamp: SystemTime::now(),
            agent_id: agent_id.to_string(),
            event_type: event_type.to_string(),
            detail: detail.to_string(),
        });
        if events.len() > 1000 { events.remove(0); }
    }
}

/// ===== [2] 自动负载均衡 =====
pub struct LoadBalancer {
    agents: Arc<RwLock<Vec<String>>>,
    task_queue: Arc<RwLock<Vec<ScheduledTask>>>,
}

#[derive(Debug, Clone)]
pub struct ScheduledTask {
    pub id: String,
    pub description: String,
    pub estimated_cpu: f32,
    pub estimated_memory_mb: u64,
}

impl LoadBalancer {
    pub fn new() -> Self {
        Self {
            agents: Arc::new(RwLock::new(Vec::new())),
            task_queue: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 注册Agent到负载均衡池
    pub async fn register(&self, agent_id: &str) {
        self.agents.write().await.push(agent_id.to_string());
    }

    /// 提交任务到队列
    pub async fn submit(&self, task: ScheduledTask) {
        self.task_queue.write().await.push(task);
    }

    /// [自动分配] 选择最空闲的Agent执行任务
    pub async fn assign_task(&self, dashboard: &SwarmDashboard) -> Option<(String, ScheduledTask)> {
        let task = self.task_queue.write().await.pop()?;
        let agents = self.agents.read().await;
        let workloads = dashboard.agents.read().await;

        // 选择负载最低的Agent
        let best_agent = agents.iter()
            .filter_map(|id| workloads.get(id))
            .min_by(|a, b| {
                let score_a = a.current_tasks as f64 + a.cpu_usage as f64;
                let score_b = b.current_tasks as f64 + b.cpu_usage as f64;
                score_a.partial_cmp(&score_b).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|w| w.agent_id.clone());

        best_agent.map(|id| (id, task))
    }
}

/// ===== [3] 冲突检测 =====
pub struct ConflictDetector {
    file_locks: Arc<RwLock<HashMap<String, String>>>, // file -> agent_id
}

impl ConflictDetector {
    pub fn new() -> Self {
        Self { file_locks: Arc::new(RwLock::new(HashMap::new())) }
    }

    /// 尝试获取文件锁
    pub async fn try_lock(&self, file: &str, agent_id: &str) -> Result<(), String> {
        let mut locks = self.file_locks.write().await;
        if let Some(existing) = locks.get(file) {
            if existing != agent_id {
                return Err(format!("File '{}' is locked by agent '{}'", file, existing));
            }
        }
        locks.insert(file.to_string(), agent_id.to_string());
        Ok(())
    }

    /// 释放文件锁
    pub async fn unlock(&self, file: &str, agent_id: &str) {
        let mut locks = self.file_locks.write().await;
        if locks.get(file).map(|s| s == agent_id).unwrap_or(false) {
            locks.remove(file);
        }
    }

    /// 检测冲突 (同一文件被多个Agent编辑)
    pub async fn detect_conflicts(&self) -> Vec<(String, Vec<String>)> {
        let mut conflicts = Vec::new();
        let mut file_agents: HashMap<String, Vec<String>> = HashMap::new();
        for (file, agent) in self.file_locks.read().await.iter() {
            file_agents.entry(file.clone()).or_default().push(agent.clone());
        }
        for (file, agents) in file_agents {
            if agents.len() > 1 {
                conflicts.push((file, agents));
            }
        }
        conflicts
    }
}

/// ===== [4] 资源感知调度 =====
pub struct ResourceScheduler {
    total_cpu: f32,
    total_memory_mb: u64,
    allocated_cpu: f32,
    allocated_memory_mb: u64,
}

impl ResourceScheduler {
    pub fn new(total_cpu: f32, total_memory_mb: u64) -> Self {
        Self { total_cpu, total_memory_mb, allocated_cpu: 0.0, allocated_memory_mb: 0 }
    }

    /// 检查是否有足够的资源
    pub fn can_schedule(&self, cpu: f32, memory_mb: u64) -> bool {
        self.allocated_cpu + cpu <= self.total_cpu
            && self.allocated_memory_mb + memory_mb <= self.total_memory_mb
    }

    /// 分配资源
    pub fn allocate(&mut self, cpu: f32, memory_mb: u64) -> bool {
        if !self.can_schedule(cpu, memory_mb) { return false; }
        self.allocated_cpu += cpu;
        self.allocated_memory_mb += memory_mb;
        true
    }

    /// 释放资源
    pub fn release(&mut self, cpu: f32, memory_mb: u64) {
        self.allocated_cpu = (self.allocated_cpu - cpu).max(0.0);
        self.allocated_memory_mb = self.allocated_memory_mb.saturating_sub(memory_mb);
    }

    pub fn utilization(&self) -> (f32, f64) {
        let cpu_pct = if self.total_cpu > 0.0 { self.allocated_cpu / self.total_cpu * 100.0 } else { 0.0 };
        let mem_pct = if self.total_memory_mb > 0 { self.allocated_memory_mb as f64 / self.total_memory_mb as f64 * 100.0 } else { 0.0 };
        (cpu_pct, mem_pct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_conflict_detection() {
        let detector = ConflictDetector::new();
        assert!(detector.try_lock("src/main.rs", "agent-1").await.is_ok());
        assert!(detector.try_lock("src/main.rs", "agent-2").await.is_err());

        detector.unlock("src/main.rs", "agent-1").await;
        assert!(detector.try_lock("src/main.rs", "agent-2").await.is_ok());
    }

    #[test]
    fn test_resource_scheduler() {
        let mut sched = ResourceScheduler::new(8.0, 16384);
        assert!(sched.can_schedule(2.0, 4096));
        assert!(sched.allocate(2.0, 4096));
        assert!(!sched.can_schedule(8.0, 4096)); // 超过CPU
    }

    #[tokio::test]
    async fn test_dashboard_register() {
        let db = SwarmDashboard::new();
        db.register_agent("agent-1").await;
        let json = db.dashboard_json().await;
        assert!(json.contains("agent-1"));
    }
}
