//! # NodeScheduler — 节点调度层

use crate::error::{BuildEngineError, Result};
use crate::types::*;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use std::collections::HashMap;

// ResourceMonitor — 资源监控器
pub struct ResourceMonitor {
    current: Mutex<ComputeResource>,
}
impl ResourceMonitor {
    pub fn new() -> Self {
        Self {
            current: Mutex::new(ComputeResource {
                cpu_usage: 0.0, available_memory_mb: 0, total_memory_mb: 0,
                available_disk_mb: 0, total_disk_mb: 0, gpus: Vec::new(), load_factor: 1.0,
            }),
        }
    }
    pub fn update(&self, resource: ComputeResource) { *self.current.lock() = resource; }
    pub fn current(&self) -> ComputeResource { self.current.lock().clone() }
    pub fn available_pct(&self) -> f64 { (1.0 - self.current.lock().cpu_usage) * 100.0 }
    pub fn has_sufficient(&self, _requirements: &ResourceLimits) -> bool { self.available_pct() > 10.0 }
}

// TaskAllocator — 任务分配器
pub struct TaskAllocator {
    assignments: Mutex<HashMap<NodeId, Vec<TaskId>>>,
    max_concurrent: usize,
}
impl TaskAllocator {
    pub fn new(max_concurrent: usize) -> Self { Self { assignments: Mutex::new(HashMap::new()), max_concurrent } }
    pub fn assign(&self, node_id: NodeId, task_id: TaskId) -> Result<()> {
        let mut map = self.assignments.lock();
        let tasks = map.entry(node_id).or_default();
        if tasks.len() >= self.max_concurrent {
            return Err(BuildEngineError::NoAvailableNodes(format!("Node {} at capacity", node_id)));
        }
        tasks.push(task_id); Ok(())
    }
    pub fn release(&self, node_id: NodeId, task_id: TaskId) {
        if let Some(tasks) = self.assignments.lock().get_mut(&node_id) { tasks.retain(|t| *t != task_id); }
    }
    pub fn node_load(&self, node_id: NodeId) -> usize {
        self.assignments.lock().get(&node_id).map(|t| t.len()).unwrap_or(0)
    }
}

// PriorityScheduler
pub struct PriorityScheduler {
    queues: Mutex<Vec<(u8, Vec<TaskId>)>>,
}
impl PriorityScheduler {
    pub fn new() -> Self { Self { queues: Mutex::new(Vec::new()) } }
    pub fn enqueue(&self, task_id: TaskId, priority: u8) {
        let mut q = self.queues.lock();
        let entry = q.iter_mut().find(|(p, _)| *p == priority);
        if let Some((_, tasks)) = entry { tasks.push(task_id); }
        else { q.push((priority, vec![task_id])); }
    }
    pub fn dequeue_highest(&self) -> Option<(TaskId, u8)> {
        let mut q = self.queues.lock();
        q.sort_by(|a, b| b.0.cmp(&a.0));
        for i in 0..q.len() {
            if !q[i].1.is_empty() { let id = q[i].1.remove(0); return Some((id, q[i].0)); }
        }
        None
    }
}

// HealthChecker
pub struct HealthChecker {
    heartbeats: Mutex<HashMap<NodeId, DateTime<Utc>>>,
    timeout_secs: i64,
}
impl HealthChecker {
    pub fn new(timeout_secs: u64) -> Self { Self { heartbeats: Mutex::new(HashMap::new()), timeout_secs: timeout_secs as i64 } }
    pub fn record(&self, node_id: NodeId) { self.heartbeats.lock().insert(node_id, Utc::now()); }
    pub fn is_healthy(&self, node_id: NodeId) -> bool {
        self.heartbeats.lock().get(&node_id).map_or(false, |t| {
            (Utc::now() - *t).num_seconds() < self.timeout_secs
        })
    }
}

// NodeScheduler 主结构
pub struct NodeScheduler {
    pub resource_monitor: ResourceMonitor,
    pub task_allocator: TaskAllocator,
    pub priority_scheduler: PriorityScheduler,
    pub health_checker: HealthChecker,
}
impl NodeScheduler {
    pub fn new(max_tasks: usize) -> Self {
        Self {
            resource_monitor: ResourceMonitor::new(),
            task_allocator: TaskAllocator::new(max_tasks),
            priority_scheduler: PriorityScheduler::new(),
            health_checker: HealthChecker::new(30),
        }
    }
}
