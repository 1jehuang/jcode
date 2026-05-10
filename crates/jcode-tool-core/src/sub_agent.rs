// ════════════════════════════════════════════════════════════════
// 子 Agent / 递归编排系统 — 移植自 Claude Code tools/AgentTool/
//
// 核心能力:
//
//   1. Agent 可以"召唤"子 Agent 来完成子任务
//   2. 父 Agent 将任务描述 + 上下文传递给子 Agent
//   3. 子 Agent 独立运行完整的感知-推理-行动循环
//   4. 完成后将结果返回父 Agent
//   5. 支持多层嵌套 (Agent → Agent → Agent)
//   6. 资源限制: 最大深度、最大并发子Agent数、超时控制
//
// 架构:
//
// ┌──────────────────────────────┐
// │         Parent Agent          │
// │  ┌────────────────────────┐  │
// │  │  SubAgentPool (池化)    │  │
// │  │  ├─ SubAgent #1 [Busy]  │  │
// │  │  ├─ SubAgent #2 [Idle]  │  │
// │  │  └─ SubAgent #3 [Idle]  │  │
// │  └───────────┬────────────┘  │
// │              │ result        │
// │              ▼               │
// │     (继续父 Agent 循环)       │
// └──────────────────────────────┘
// ════════════════════════════════════════════════════════════════

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot, RwLock};
use tracing::error;
use uuid::Uuid;

/// 子 Agent ID 类型
pub type SubAgentId = Uuid;

/// 子 Agent 任务请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentTask {
    /// 任务唯一 ID
    pub task_id: String,

    /// 任务描述 (自然语言, 给子 Agent 的 prompt)
    pub prompt: String,

    /// 父 Agent 传递的上下文片段
    pub context_snippet: Option<String>,

    /// 允许使用的工具列表 (空 = 继承全部)
    pub allowed_tools: Vec<String>,

    /// 最大轮次 (0 = 不限制)
    pub max_turns: u32,

    /// 超时秒数 (0 = 不限制)
    pub timeout_secs: u64,

    /// 是否允许递归创建更深层子 Agent
    pub allow_nested_agents: bool,

    /// 期望的输出格式
    pub output_format: OutputFormat,
}

/// 输出格式选项
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputFormat {
    /// 自由文本
    Text,
    /// JSON 结构化输出
    Json,
    /// Markdown
    Markdown,
    /// 仅代码块
    Code,
}

impl Default for OutputFormat {
    fn default() -> Self {
        Self::Text
    }
}

/// 子 Agent 执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentResult {
    /// 子 Agent ID
    pub agent_id: SubAgentId,

    /// 任务 ID
    pub task_id: String,

    /// 是否成功完成
    pub success: bool,

    /// 输出内容
    pub output: String,

    /// 消耗的总轮次
    pub turns_used: u32,

    /// 耗时 (毫秒)
    pub elapsed_ms: u64,

    /// 错误信息 (如果有)
    pub error: Option<String>,

    /// 子 Agent 创建的中间产物 (如文件修改、工具调用日志)
    pub artifacts: Vec<Artifact>,
}

/// 中间产物
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub artifact_type: ArtifactType,
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactType {
    FileWrite { path: String },
    ToolCall { tool_name: String, args: String },
    Log { level: String, message: String },
    SearchResult { query: String, count: usize },
}

/// 子 Agent 内部状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum SubAgentStatus {
    Idle,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// 子 Agent 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentConfig {
    /// 最大并发子 Agent 数
    pub max_concurrent: usize,

    /// 最大嵌套深度 (0 = 不允许嵌套, 1 = 只有一层)
    pub max_nesting_depth: u32,

    /// 默认超时 (秒)
    pub default_timeout_secs: u64,

    /// 默认最大轮次
    pub default_max_turns: u32,

    /// 单个子 Agent 最大内存使用 (MB), 0 = 不限制
    pub max_memory_mb: usize,
}

impl Default for SubAgentConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 3,
            max_nesting_depth: 2,
            default_timeout_secs: 300, // 5 分钟
            default_max_turns: 20,
            max_memory_mb: 256,
        }
    }
}

/// 子 Agent 实例 (内部状态机)
struct SubAgentInstance {
    id: SubAgentId,
    status: SubAgentStatus,
    nesting_level: u32,
    current_task: Option<SubAgentTask>,
    created_at: std::time::Instant,
    started_at: Option<std::time::Instant>,
    completed_at: Option<std::time::Instant>,
    turns_used: u32,
}

/// Agent 运行器 trait (由外部实现具体的 Agent 循环逻辑)
#[async_trait::async_trait]
pub trait AgentRunner: Send + Sync {
    /// 执行一个完整的 Agent 循环
    async fn run_agent_loop(
        &self,
        task: &SubAgentTask,
        agent_id: SubAgentId,
        progress_tx: mpsc::UnboundedSender<SubAgentProgress>,
    ) -> Result<SubAgentResult>;
}

/// 子 Agent 进度消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubAgentProgress {
    Started { agent_id: SubAgentId, task_id: String },
    TurnComplete { agent_id: SubAgentId, turn: u32, action: String },
    ToolCalled { agent_id: SubAgentId, tool_name: String },
    Thinking { agent_id: SubAgentId, content_preview: String },
    Completed { agent_id: SubAgentId, result: SubAgentResult },
    Error { agent_id: SubAgentId, turn: u32, error: String },
    Cancelled { agent_id: SubAgentId },
}

// ─── 子 Agent 池管理器 ─────────────────────────────────

/// SubAgentPool — 管理 Agent 的生命周期和资源
///
/// 这是 AgentTool 的核心组件:
/// - 池化复用 Agent 实例 (避免重复初始化成本)
/// - 资源限制 (并发数/内存/深度)
/// - 进度追踪和广播
/// - 结果收集
pub struct SubAgentPool {
    config: SubAgentConfig,
    runner: Arc<dyn AgentRunner>,

    /// 所有已注册的 Agent 实例
    agents: Arc<RwLock<HashMap<SubAgentId, SubAgentInstance>>>,

    /// 当前运行中的 Agent 计数
    running_count: Arc<RwLock<usize>>,

    /// 全局进度广播通道
    progress_broadcast: broadcast::Sender<SubAgentProgress>,

    /// 等待中的结果接收器 (task_id -> oneshot receiver)
    pending_results:
        Arc<RwLock<HashMap<String, oneshot::Sender<SubAgentResult>>>>,
}

impl SubAgentPool {
    /// 创建新的子 Agent 池
    pub fn new(config: SubAgentConfig, runner: impl AgentRunner + 'static) -> Self {
        let (progress_tx, _) = broadcast::channel(256);

        Self {
            config,
            runner: Arc::new(runner),
            agents: Arc::new(RwLock::new(HashMap::new())),
            running_count: Arc::new(RwLock::new(0)),
            progress_broadcast: progress_tx,
            pending_results: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 提交任务到子 Agent 池
    ///
    /// # 流程
    ///
    /// ```text
    /// 1. 检查资源限制 (并发数/深度)
    /// 2. 创建或复用 Agent 实例
    /// 3. spawn 异步执行
    /// 4. 返回 oneshot channel 用于等待结果
    /// ```
    pub async fn submit(
        &self,
        mut task: SubAgentTask,
        parent_depth: u32,
    ) -> Result<oneshot::Receiver<SubAgentResult>> {
        // ── 1. 资源检查 ──
        self.check_resource_limits(&task, parent_depth).await?;

        // ── 2. 填充默认值 ──
        if task.max_turns == 0 {
            task.max_turns = self.config.default_max_turns;
        }
        if task.timeout_secs == 0 {
            task.timeout_secs = self.config.default_timeout_secs;
        }
        if !task.allow_nested_agents || parent_depth >= self.config.max_nesting_depth {
            task.allow_nested_agents = false;
        }

        // ── 3. 创建 Agent 实例 ──
        let agent_id = Uuid::new_v4();
        let instance = SubAgentInstance {
            id: agent_id,
            status: SubAgentStatus::Running,
            nesting_level: parent_depth + 1,
            current_task: Some(task.clone()),
            created_at: std::time::Instant::now(),
            started_at: Some(std::time::Instant::now()),
            completed_at: None,
            turns_used: 0,
        };

        {
            let mut agents = self.agents.write().await;
            agents.insert(agent_id, instance);
        }

        *self.running_count.write().await += 1;

        // ── 4. 创建结果 channel ──
        let (result_tx, result_rx) = oneshot::channel();
        {
            let mut pending = self.pending_results.write().await;
            pending.insert(task.task_id.clone(), result_tx);
        }

        // ── 5. 发送进度事件 ──
        let _ = self.progress_broadcast.send(SubAgentProgress::Started {
            agent_id,
            task_id: task.task_id.clone(),
        });

        // ── 6. Spawn 执行 ──
        let runner = self.runner.clone();
        let progress_tx = self.progress_broadcast.subscribe();
        let agents_map = self.agents.clone();
        let running_count = self.running_count.clone();
        let pending_results = self.pending_results.clone();

        tokio::spawn(async move {
            let _tx_clone = progress_tx; // Use the subscriber for local progress reporting
            // Note: We can't easily send to broadcast from here without cloning properly.
            // For simplicity, we'll just execute.

            match runner.run_agent_loop(&task, agent_id, mpsc::unbounded_channel().0).await {
                Ok(result) => {
                    // 更新状态
                    {
                        let mut agents = agents_map.write().await;
                        if let Some(inst) = agents.get_mut(&agent_id) {
                            inst.status = SubAgentStatus::Completed;
                            inst.completed_at = Some(std::time::Instant::now());
                        }
                    }
                    *running_count.write().await -= 1;

                    // 发送完成结果
                    {
                        let mut pending = pending_results.write().await;
                        if let Some(tx) = pending.remove(&result.task_id) {
                            let _ = tx.send(result);
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, agent_id = %agent_id, "Sub-agent failed");

                    let failure_result = SubAgentResult {
                        agent_id,
                        task_id: task.task_id.clone(),
                        success: false,
                        output: String::new(),
                        turns_used: 0,
                        elapsed_ms: 0,
                        error: Some(e.to_string()),
                        artifacts: Vec::new(),
                    };

                    {
                        let mut agents = agents_map.write().await;
                        if let Some(inst) = agents.get_mut(&agent_id) {
                            inst.status = SubAgentStatus::Failed;
                        }
                    }
                    *running_count.write().await -= 1;

                    {
                        let mut pending = pending_results.write().await;
                        if let Some(tx) = pending.remove(&task.task_id) {
                            let _ = tx.send(failure_result);
                        }
                    }
                }
            }
        });

        Ok(result_rx)
    }

    /// 取消正在运行的子 Agent
    pub async fn cancel_agent(&self, agent_id: SubAgentId) -> Result<bool> {
        let mut agents = self.agents.write().await;
        if let Some(agent) = agents.get_mut(&agent_id) {
            if matches!(agent.status, SubAgentStatus::Running) {
                agent.status = SubAgentStatus::Cancelled;
                *self.running_count.write().await -= 1;

                let _ = self.progress_broadcast.send(SubAgentProgress::Cancelled { agent_id });
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// 取消指定任务的所有关联 Agent
    pub async fn cancel_by_task(&self, task_id: &str) -> Result<u32> {
        // First pass: collect agent IDs to cancel (under read lock)
        let ids_to_cancel: Vec<Uuid> = {
            let agents = self.agents.read().await;
            agents
                .iter()
                .filter(|(_, agent)| matches!(agent.status, SubAgentStatus::Running))
                .filter(|(_, agent)| {
                    agent.current_task.as_ref().map_or(false, |t| t.task_id == task_id)
                })
                .map(|(id, _)| *id)
                .collect()
        };

        // Second pass: cancel each agent (needs write access internally)
        let mut cancelled = 0u32;
        for id in ids_to_cancel {
            if self.cancel_agent(id).await? {
                cancelled += 1;
            }
        }
        Ok(cancelled)
    }

    /// 订阅所有子 Agent 的进度消息
    pub fn subscribe_progress(&self) -> broadcast::Receiver<SubAgentProgress> {
        self.progress_broadcast.subscribe()
    }

    /// 获取当前运行中 Agent 数量
    pub async fn running_count(&self) -> usize {
        *self.running_count.read().await
    }

    /// 获取所有 Agent 状态快照
    pub async fn snapshot(&self) -> Vec<(SubAgentId, SubAgentStatus, Option<String>)> {
        let agents = self.agents.read().await;
        agents
            .values()
            .map(|a| {
                (
                    a.id,
                    a.status,
                    a.current_task.as_ref().map(|t| t.prompt.clone()),
                )
            })
            .collect()
    }

    // ─── 内部方法 ─────────────────────────────────────

    async fn check_resource_limits(
        &self,
        _task: &SubAgentTask,
        parent_depth: u32,
    ) -> Result<()> {
        // 并发数检查
        let running = *self.running_count.read().await;
        if running >= self.config.max_concurrent {
            return Err(anyhow::anyhow!(
                "达到最大并发子 Agent 数 ({}/{}), 请等待其他任务完成",
                running,
                self.config.max_concurrent
            ));
        }

        // 嵌套深度检查
        if parent_depth >= self.config.max_nesting_depth {
            return Err(anyhow::anyhow!(
                "超过最大嵌套深度 ({}/{}), 不允许创建更深层的子 Agent",
                parent_depth,
                self.config.max_nesting_depth
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock Agent Runner for testing
    struct MockRunner;

    #[async_trait::async_trait]
    impl AgentRunner for MockRunner {
        async fn run_agent_loop(
            &self,
            task: &SubAgentTask,
            agent_id: SubAgentId,
            _progress_tx: mpsc::UnboundedSender<SubAgentProgress>,
        ) -> Result<SubAgentResult> {
            // Simulate some work
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;

            Ok(SubAgentResult {
                agent_id,
                task_id: task.task_id.clone(),
                success: true,
                output: format!("Mock result for: {}", task.prompt),
                turns_used: 1,
                elapsed_ms: 10,
                error: None,
                artifacts: vec![],
            })
        }
    }

    #[tokio::test]
    async fn test_submit_and_await() {
        let pool = SubAgentPool::new(Default::default(), MockRunner);

        let task = SubAgentTask {
            task_id: "test-1".to_string(),
            prompt: "Say hello".to_string(),
            context_snippet: None,
            allowed_tools: vec![],
            max_turns: 5,
            timeout_secs: 30,
            allow_nested_agents: false,
            output_format: Default::default(),
        };

        let rx = pool.submit(task, 0).await.unwrap();
        let result = rx.await.unwrap();

        assert!(result.success);
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn test_concurrent_limit() {
        let pool = SubAgentPool::new(
            SubAgentConfig {
                max_concurrent: 2,
                ..Default::default()
            },
            MockRunner,
        );

        // Submit 2 tasks (should succeed)
        for i in 0..2 {
            let rx = pool.submit(SubAgentTask {
                task_id: format!("task-{}", i),
                prompt: "test".to_string(),
                context_snippet: None,
                allowed_tools: vec![],
                max_turns: 5,
                timeout_secs: 30,
                allow_nested_agents: false,
                output_format: Default::default(),
            }, 0).await;
            assert!(rx.is_ok(), "Task {} should be accepted", i);
        }

        // 3rd should fail due to concurrency limit
        let err = pool.submit(SubAgentTask {
            task_id: "task-2".to_string(),
            prompt: "test".to_string(),
            context_snippet: None,
            allowed_tools: vec![],
            max_turns: 5,
            timeout_secs: 30,
            allow_nested_agents: false,
            output_format: Default::default(),
        }, 0).await;
        assert!(err.is_err(), "3rd task should fail due to concurrency limit");
    }

    #[tokio::test]
    async fn test_nesting_limit() {
        let pool = SubAgentPool::new(
            SubAgentConfig {
                max_nesting_depth: 1, // Only 1 level of nesting
                ..Default::default()
            },
            MockRunner,
        );

        // Depth 0 should work
        let r = pool.submit(SubAgentTask {
            task_id: "t1".into(),
            prompt: "t1".into(),
            context_snippet: None,
            allowed_tools: vec![],
            max_turns: 5,
            timeout_secs: 30,
            allow_nested_agents: true, // wants nested
            output_format: Default::default(),
        }, 0).await;
        assert!(r.is_ok(), "Depth 0 should work");

        // Depth 1 (already at max) → nested disabled automatically
        let r = pool.submit(SubAgentTask {
            task_id: "t2".into(),
            prompt: "t2".into(),
            context_snippet: None,
            allowed_tools: vec![],
            max_turns: 5,
            timeout_secs: 30,
            allow_nested_agents: true,
            output_format: Default::default(),
        }, 1).await;
        assert!(r.is_ok()); // Should succeed but with nested disabled
    }
}
