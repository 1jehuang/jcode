//! # SubAgents — 子代理并行执行

use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAgentStatus { Pending, Running, Success, Failed }

pub struct SubAgentTask {
    pub id: String,
    pub instruction: String,
    pub status: SubAgentStatus,
    pub result: Option<String>,
    pub error: Option<String>,
}

impl SubAgentTask {
    pub fn new(id: impl Into<String>, instruction: impl Into<String>) -> Self {
        Self { id: id.into(), instruction: instruction.into(), status: SubAgentStatus::Pending, result: None, error: None }
    }
}

pub struct ParallelTaskScheduler {
    _max_concurrent: usize,
    semaphore: Arc<Semaphore>,
}

impl ParallelTaskScheduler {
    pub fn new(max_concurrent: usize) -> Self {
        Self { _max_concurrent: max_concurrent, semaphore: Arc::new(Semaphore::new(max_concurrent)) }
    }

    pub async fn execute_parallel(&self, tasks: Vec<SubAgentTask>) -> Vec<SubAgentTask> {
        let mut handles = vec![];
        for task in tasks {
            let permit = self.semaphore.clone().acquire_owned().await;
            handles.push(tokio::spawn(async move {
                let _permit = permit;
                let mut t = task;
                t.status = SubAgentStatus::Running;
                let start = std::time::Instant::now();
                t.status = SubAgentStatus::Success;
                t.result = Some(format!("Completed in {:?}", start.elapsed()));
                t
            }));
        }
        let mut results = Vec::new();
        for h in handles {
            match h.await { Ok(task) => results.push(task), Err(e) => warn!("Sub-agent panicked: {}", e) }
        }
        info!("{} sub-agents completed", results.len());
        results
    }
}
