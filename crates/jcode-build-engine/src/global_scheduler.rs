//! # GlobalScheduler — 全局调度层

use crate::error::{BuildEngineError, Result};
use crate::types::*;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::info;

#[derive(Debug, Clone)]
pub enum GlobalSchedulerEvent {
    BuildScheduled(BuildId),
    NodeRemoved(NodeId),
    PricingUpdated(DynamicPrice),
    GlobalStatus,
}

// ══════════════════════════════════════════════════════════════════
// SupplyDemandEngine
// ═════════════════════════════════════════════════════════════════

pub struct SupplyDemandEngine {
    history: Mutex<HashMap<String, Vec<SupplyDemandEntry>>>,
}

impl SupplyDemandEngine {
    pub fn new() -> Self { Self { history: Mutex::new(HashMap::new()) } }
    pub fn report(&self, entry: SupplyDemandEntry) {
        self.history.lock().entry(entry.region.clone()).or_default().push(entry);
    }
    pub fn get_ratio(&self, region: &str) -> Option<f64> {
        self.history.lock().get(region).and_then(|v| v.last()).map(|e| e.ratio)
    }
}

// ══════════════════════════════════════════════════════════════════
// NodeStateManager
// ═════════════════════════════════════════════════════════════════

pub struct NodeStateManager {
    nodes: Mutex<HashMap<NodeId, NodeInfo>>,
}
#[derive(Debug, Clone)]
pub enum NodeStateEvent { Registered(NodeInfo), Offline(NodeId), StatusChanged(NodeId, NodeStatus) }

impl NodeStateManager {
    pub fn new() -> (Self, tokio::sync::broadcast::Receiver<NodeStateEvent>) {
        let (tx, rx) = tokio::sync::broadcast::channel(256);
        let _ = tx;
        (Self { nodes: Mutex::new(HashMap::new()) }, rx)
    }
    pub fn register(&self, info: NodeInfo) {
        self.nodes.lock().insert(info.node_id, info.clone());
    }
    pub fn get_online(&self) -> Vec<NodeInfo> {
        self.nodes.lock().values().filter(|n| n.status.is_available()).cloned().collect()
    }
    pub fn count(&self) -> usize { self.nodes.lock().len() }
}

// ══════════════════════════════════════════════════════════════════
// TaskQueueManager
// ═════════════════════════════════════════════════════════════════

pub struct TaskQueueManager {
    queue: parking_lot::Mutex<Vec<QueueItem>>,
    max_depth: usize,
}
impl TaskQueueManager {
    pub fn new(max_depth: usize) -> Self { Self { queue: parking_lot::Mutex::new(Vec::new()), max_depth } }
    pub fn enqueue(&self, request: BuildRequest) -> Result<usize> {
        let mut q = self.queue.lock();
        if q.len() >= self.max_depth { return Err(BuildEngineError::InvalidState("Queue full".to_string())); }
        q.push(QueueItem { request, queued_at: Utc::now(), retry_count: 0 });
        Ok(q.len() - 1)
    }
    pub fn dequeue(&self) -> Option<QueueItem> {
        let mut q = self.queue.lock();
        if q.is_empty() { return None; }
        q.sort_by(|a, b| b.request.priority.cmp(&a.request.priority));
        Some(q.remove(0))
    }
    pub fn depth(&self) -> usize { self.queue.lock().len() }
}

// ══════════════════════════════════════════════════════════════════
// DynamicPricingEngine
// ═════════════════════════════════════════════════════════════════

pub struct DynamicPricingEngine;
impl DynamicPricingEngine {
    pub fn new() -> Self { Self }
    pub fn calculate(&self, _region: &str, supply: f64, demand: f64) -> DynamicPrice {
        let factor = if demand > 0.0 && supply / demand < 1.0 { 1.5 } else { 1.0 };
        DynamicPrice {
            price_per_cpu_sec: 0.001 * factor,
            estimated_total_price: 3.6 * factor,
            currency: "USD".to_string(), rule_name: "dynamic".to_string(),
            valid_until: Utc::now() + chrono::Duration::seconds(60),
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// FailoverEngine
// ═════════════════════════════════════════════════════════════════

pub struct FailoverEngine {
    failures: Mutex<HashMap<NodeId, u32>>,
    threshold: u32,
}
impl FailoverEngine {
    pub fn new(threshold: u32) -> Self { Self { failures: Mutex::new(HashMap::new()), threshold } }
    pub fn record_failure(&self, node_id: NodeId) -> bool {
        let mut f = self.failures.lock();
        let c = f.entry(node_id).or_insert(0);
        *c += 1;
        *c >= self.threshold
    }
    pub fn record_success(&self, node_id: NodeId) { self.failures.lock().remove(&node_id); }
}

// ══════════════════════════════════════════════════════════════════
// LoadPredictor
// ═════════════════════════════════════════════════════════════════

pub struct LoadPredictor {
    history: Mutex<HashMap<NodeId, Vec<(DateTime<Utc>, f64)>>>,
}
impl LoadPredictor {
    pub fn new() -> Self { Self { history: Mutex::new(HashMap::new()) } }
    pub fn report(&self, node_id: NodeId, load: f64) {
        self.history.lock().entry(node_id).or_default().push((Utc::now(), load));
    }
}

// ══════════════════════════════════════════════════════════════════
// GlobalScheduler 主结构
// ═════════════════════════════════════════════════════════════════

pub struct GlobalScheduler {
    pub supply_demand: Arc<SupplyDemandEngine>,
    pub node_state: Arc<NodeStateManager>,
    pub task_queue: Arc<TaskQueueManager>,
    pub pricing: Arc<DynamicPricingEngine>,
    pub failover: Arc<FailoverEngine>,
    pub load_predictor: Arc<LoadPredictor>,
    event_tx: broadcast::Sender<GlobalSchedulerEvent>,
}

impl GlobalScheduler {
    pub fn new(queue_depth: usize, failover_threshold: u32) -> (Self, broadcast::Receiver<GlobalSchedulerEvent>) {
        let (event_tx, rx) = broadcast::channel(256);
        let (node_state, _) = NodeStateManager::new();
        (Self {
            supply_demand: Arc::new(SupplyDemandEngine::new()),
            node_state: Arc::new(node_state),
            task_queue: Arc::new(TaskQueueManager::new(queue_depth)),
            pricing: Arc::new(DynamicPricingEngine::new()),
            failover: Arc::new(FailoverEngine::new(failover_threshold)),
            load_predictor: Arc::new(LoadPredictor::new()),
            event_tx,
        }, rx)
    }

    pub fn schedule_once(&self) -> Option<QueueItem> { self.task_queue.dequeue() }

    pub fn engine_health(&self) -> crate::EngineHealth {
        crate::EngineHealth {
            version: crate::BUILD_ENGINE_VERSION,
            global_scheduler_ready: true,
            node_count: self.node_state.count(),
            pending_tasks: self.task_queue.depth(),
            cache_hit_rate: 0.0, uptime_seconds: 0,
        }
    }
}
