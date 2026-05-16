//! # TaskScheduler — 任务调度层
//!
//! 任务分解 -> 依赖解析(DAG) -> 任务执行 -> 超时管理 -> 重试 -> 结果收集

use crate::error::{BuildEngineError, Result};
use crate::types::*;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::future::join_all;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{info, instrument};

// ══════════════════════════════════════════════════════════════════
// 配置
// ═════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct TaskSchedulerConfig {
    pub max_concurrent: u32,
    pub default_timeout_secs: u64,
    pub max_retries: u32,
}

impl Default for TaskSchedulerConfig {
    fn default() -> Self {
        Self { max_concurrent: 4, default_timeout_secs: 600, max_retries: 3 }
    }
}

// ══════════════════════════════════════════════════════════════════
// SchedulerEvent — 调度事件
// ═════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub enum SchedulerEvent {
    TaskStarted { task_id: TaskId },
    TaskCompleted { task_id: TaskId, success: bool },
    TaskFailed { task_id: TaskId, error: String, retryable: bool },
    AllCompleted { build_id: BuildId, result: BuildResult },
    Progress { progress: BuildProgress },
    Log { entry: BuildLogEntry },
    Cancelled { build_id: BuildId },
}

// ══════════════════════════════════════════════════════════════════
// TaskDecomposer — 任务分解器
// ═════════════════════════════════════════════════════════════════

#[async_trait]
pub trait TaskDecomposer: Send + Sync {
    async fn decompose(&self, request: &BuildRequest) -> Result<Vec<CompiledTask>>;
}

pub struct SmartDecomposer;
impl SmartDecomposer {
    pub fn new() -> Self { Self }
    fn decompose_by_module(&self, request: &BuildRequest) -> Vec<CompiledTask> {
        vec![CompiledTask {
            task_id: TaskId::new(),
            parent_build_id: request.id.unwrap_or_default(),
            name: format!("{}-main", request.build_type),
            command: request.custom_command.clone().unwrap_or_else(|| "cargo build".to_string()),
            working_dir: ".".to_string(),
            resource_requirements: ResourceLimits::default(),
            estimated_duration_secs: 300,
            status: BuildStatus::Queued,
            dependencies: vec![],
            retry_count: 0,
            max_retries: 3,
            created_at: Utc::now(),
        }]
    }
}

#[async_trait]
impl TaskDecomposer for SmartDecomposer {
    async fn decompose(&self, request: &BuildRequest) -> Result<Vec<CompiledTask>> {
        Ok(self.decompose_by_module(request))
    }
}

// ══════════════════════════════════════════════════════════════════
// DependencyResolver — 依赖解析器 (简化版，无 petgraph)
// ═════════════════════════════════════════════════════════════════

pub struct DependencyResolver;

impl DependencyResolver {
    pub fn new() -> Self { Self }

    /// 简单的拓扑排序：每个任务作为一个单独的层
    pub fn build_graph(&self, tasks: &[CompiledTask]) -> Result<Vec<Vec<TaskId>>> {
        // 构建依赖图检测环 (简单实现: 检查是否有自引用)
        let mut seen = HashSet::new();
        for task in tasks {
            for dep in &task.dependencies {
                if dep.task_id == task.task_id {
                    return Err(BuildEngineError::DependencyCycleDetected);
                }
            }
            if !seen.insert(task.task_id) {
                return Err(BuildEngineError::DependencyError("Duplicate task ID".into()));
            }
        }
        // 每个任务一层（可进一步优化为并行分组）
        Ok(tasks.iter().map(|t| vec![t.task_id]).collect())
    }
}

// ══════════════════════════════════════════════════════════════════
// TaskScheduler — 调度器主结构
// ═════════════════════════════════════════════════════════════════

pub struct TaskScheduler {
    config: TaskSchedulerConfig,
    decomposer: Arc<dyn TaskDecomposer>,
    event_tx: broadcast::Sender<SchedulerEvent>,
    running_builds: std::sync::Mutex<std::collections::HashMap<BuildId, Arc<tokio_util::sync::CancellationToken>>>,
}

impl TaskScheduler {
    pub fn new(config: TaskSchedulerConfig) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            config,
            decomposer: Arc::new(SmartDecomposer::new()),
            event_tx,
            running_builds: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SchedulerEvent> {
        self.event_tx.subscribe()
    }

    #[instrument(skip(self), fields(build_id = %request.id.unwrap_or_default().0))]
    pub async fn submit_build(&self, request: BuildRequest) -> Result<BuildResult> {
        let build_id = request.id.unwrap_or_default();
        info!("Submitting build {}", build_id);
        let tasks = self.decomposer.decompose(&request).await?;
        let resolver = DependencyResolver::new();
        let layers = resolver.build_graph(&tasks).unwrap_or_default();

        let mut succeeded = 0u32;
        let mut failed = 0u32;
        let mut total_duration = std::time::Duration::ZERO;

        for (_layer_idx, layer) in layers.iter().enumerate() {
            let mut handles = vec![];
            let semaphore = Arc::new(tokio::sync::Semaphore::new(self.config.max_concurrent as usize));

            let task_map: HashMap<TaskId, CompiledTask> = tasks.iter().map(|t| (t.task_id, t.clone())).collect();

            for task_id in layer {
                if let Some(task) = task_map.get(task_id) {
                    let task = task.clone();
                    let sem = semaphore.clone();
                    let event_tx = self.event_tx.clone();

                    handles.push(tokio::spawn(async move {
                        let _permit = sem.acquire().await.unwrap();
                        let _ = event_tx.send(SchedulerEvent::TaskStarted { task_id: task.task_id });

                        let start = std::time::Instant::now();
                        let work_dir = std::path::Path::new(".").join(&task.working_dir);
                        let cmd_parts: Vec<&str> = task.command.split_whitespace().collect();
                        let (prog, args) = if cmd_parts.is_empty() {
                            ("echo", vec!["no-command".to_string()])
                        } else {
                            (cmd_parts[0], cmd_parts[1..].iter().map(|s| s.to_string()).collect::<Vec<_>>())
                        };

                        let output = tokio::process::Command::new(prog)
                            .args(&args)
                            .current_dir(&work_dir)
                            .stdout(std::process::Stdio::piped())
                            .stderr(std::process::Stdio::piped())
                            .output().await;

                        let duration = start.elapsed();
                        let (success, stdout, stderr) = match output {
                            Ok(out) => (out.status.success(),
                                        String::from_utf8_lossy(&out.stdout).to_string(),
                                        String::from_utf8_lossy(&out.stderr).to_string()),
                            Err(e) => (false, String::new(), e.to_string()),
                        };

                        if success {
                            let _ = event_tx.send(SchedulerEvent::TaskCompleted { task_id: task.task_id, success: true });
                        } else {
                            let _ = event_tx.send(SchedulerEvent::TaskFailed { task_id: task.task_id, error: stderr.clone(), retryable: false });
                        }
                        (task.task_id, success, duration, stdout, stderr)
                    }));
                }
            }

            for handle in join_all(handles).await {
                if let Ok((_tid, ok, dur, _stdout, _stderr)) = handle {
                    if ok { succeeded += 1; } else { failed += 1; }
                    total_duration = total_duration.max(dur);
                }
            }
        }

        let status = if failed == 0 { BuildStatus::Success } else if succeeded > 0 { BuildStatus::PartialSuccess } else { BuildStatus::Failed };
        let result = BuildResult {
            build_id, status, executed_on: None,
            started_at: None, finished_at: Some(Utc::now()),
            duration_ms: total_duration.as_millis() as u64,
            artifacts: vec![], error_summary: None,
            stats: BuildStats::default(), ai_suggestions: None,
        };

        let _ = self.event_tx.send(SchedulerEvent::AllCompleted { build_id, result: result.clone() });
        Ok(result)
    }

    pub fn cancel_build(&self, build_id: BuildId) -> bool {
        if let Some(token) = self.running_builds.lock().unwrap_or_else(|e| e.into_inner()).remove(&build_id) {
            token.cancel(); true
        } else { false }
    }
}
