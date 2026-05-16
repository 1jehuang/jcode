// ════════════════════════════════════════════════════════════════
// 会话增量恢复 — 移植自 Claude Code session 管理
//
// 核心思路:
//
//   传统方式: 会话恢复 = 重放全部历史消息 -> O(n) 成本, 越长越慢
//   增量方式:
//     1. 定期 Checkpoint: 每N轮保存完整状态快照
//     2. 增量 Diff: 快照之间只保存变化部分
//     3. 断点续传: 重启时加载最近 checkpoint + replay 后续增量
//
// 数据结构:
//
//   SessionSnapshot {
//     id, created_at,
//     messages: [Message],           // 完整消息历史 (压缩后)
//     tool_states: {name: State},    // 工具内部状态
//     file_states: {path: Hash},      // 文件内容哈希
//     context_summary: String,        // LLM 生成的上下文摘要
//     turn_count: u32,
//   }
//
//   IncrementalDiff {
//     snapshot_id,
//     since_turn: u32,
//     added_messages: [Message],
//     changed_files: [(path, diff)],
//     state_deltas: {tool_name: Delta},
//   }
// ════════════════════════════════════════════════════════════════

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Session ID
pub type SessionId = Uuid;

/// Snapshot ID
pub type SnapshotId = Uuid;

/// 配置
#[derive(Debug, Clone)]
pub struct RecoveryConfig {
    /// 多少轮创建一次 checkpoint (0 = 不自动创建)
    pub checkpoint_interval_turns: u32,

    /// 最大保存的 checkpoint 数量
    pub max_snapshots: usize,

    /// 是否压缩消息内容
    pub compress_messages: bool,

    /// 是否追踪文件状态
    pub track_file_states: bool,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            checkpoint_interval_turns: 10,
            max_snapshots: 5,
            compress_messages: true,
            track_file_states: true,
        }
    }
}

/// 消息类型 (简化版)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: String, // "user" | "assistant" | "system" | "tool"
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub turn_number: Option<u32>,
    pub metadata: HashMap<String, String>,
}

/// 文件状态 (通过内容哈希检测变更)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileState {
    pub path: String,
    pub content_hash: String, // blake3 or sha256
    pub last_modified: DateTime<Utc>,
    pub size_bytes: u64,
}

/// 工具状态 (工具特定的序列化状态)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolState {
    pub tool_name: String,
    pub state_data: serde_json::Value,
    pub updated_at: DateTime<Utc>,
}

/// 完整会话快照
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: SnapshotId,
    pub session_id: SessionId,

    /// 创建此快照时的轮次号
    pub at_turn: u32,

    /// 完整消息历史 (到此时为止)
    pub messages: Vec<SessionMessage>,

    /// 工具状态快照
    pub tool_states: Vec<ToolState>,

    /// 文件状态快照 (用于变更检测)
    pub file_states: Vec<FileState>,

    /// 上下文摘要 (LLM 生成的, 用于减少重放消息数)
    pub context_summary: Option<String>,

    /// 创建时间
    pub created_at: DateTime<Utc>,

    /// 快照大小估算 (bytes)
    pub estimated_size_bytes: usize,
}

/// 增量差异 (两个快照之间的变化)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncrementalDiff {
    pub from_snapshot_id: SnapshotId,
    pub to_snapshot_id: Option<SnapshotId>, // None = 尚未提交新快照

    /// 起始轮次
    pub since_turn: u32,

    /// 新增的消息
    pub added_messages: Vec<SessionMessage>,

    /// 变更的文件 (路径 + 内容 diff)
    pub changed_files: Vec<FileDelta>,

    /// 工具状态增量
    pub tool_state_deltas: Vec<StateDelta>,

    /// 时间戳
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDelta {
    pub path: String,
    pub operation: FileOperation, // Added/Modified/Deleted
    pub diff_text: Option<String>, // unified diff format
    pub new_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileOperation {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDelta {
    pub tool_name: String,
    pub old_state: Option<serde_json::Value>,
    pub new_state: serde_json::Value,
}

/// 会话恢复管理器
pub struct SessionRecoveryManager {
    config: RecoveryConfig,

    /// session_id -> snapshots (有序, 最新的在末尾)
    snapshots: Arc<RwLock<HashMap<SessionId, Vec<SessionSnapshot>>>>,

    /// 未提交的增量 (自上次 snapshot 以来的变化)
    pending_diffs: Arc<RwLock<HashMap<SessionId, IncrementalDiff>>>,

    /// 当前各 session 的轮次计数
    turn_counts: Arc<RwLock<HashMap<SessionId, u32>>>,
}

impl Default for SessionRecoveryManager {
    fn default() -> Self {
        Self::new(RecoveryConfig::default())
    }
}

impl SessionRecoveryManager {
    pub fn new(config: RecoveryConfig) -> Self {
        Self {
            config,
            snapshots: Arc::new(RwLock::new(HashMap::new())),
            pending_diffs: Arc::new(RwLock::new(HashMap::new())),
            turn_counts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 开始一个新会话
    pub async fn start_session(&self, session_id: SessionId) {
        self.turn_counts.write().await.insert(session_id, 0);
        
        // 创建初始空快照
        let initial_snapshot = SessionSnapshot {
            id: Uuid::new_v4(),
            session_id,
            at_turn: 0,
            messages: vec![],
            tool_states: vec![],
            file_states: vec![],
            context_summary: Some("New session".into()),
            created_at: Utc::now(),
            estimated_size_bytes: 0,
        };

        self.snapshots.write().await
            .insert(session_id, vec![initial_snapshot]);

        tracing::info!(session = %session_id, "Session started with recovery enabled");
    }

    /// 记录一轮对话 (消息 + 状态变更)
    pub async fn record_turn(
        &self,
        session_id: &SessionId,
        messages: Vec<SessionMessage>,
        file_changes: Vec<FileDelta>,
        tool_state_changes: Vec<StateDelta>,
    ) -> Result<(), String> {
        // 更新轮次计数
        {
            let mut counts = self.turn_counts.write().await;
            *counts.entry(*session_id).or_insert(0) += 1;
        }

        let current_turn = *self.turn_counts.read().await
            .get(session_id).unwrap_or(&0);

        // 追加到 pending diff
        {
            let mut diffs = self.pending_diffs.write().await;
            let diff = diffs.entry(*session_id).or_insert_with(|| {
                // 初始化 diff (从最新 snapshot)
                let snap_id = self.get_latest_snapshot_id(session_id).await;
                IncrementalDiff {
                    from_snapshot_id: snap_id,
                    to_snapshot_id: None,
                    since_turn: current_turn - 1, // 上次 checkpoint 后的第一轮
                    added_messages: vec![],
                    changed_files: vec![],
                    tool_state_deltas: vec![],
                    created_at: Utc::now(),
                }
            });

            diff.added_messages.extend(messages);
            diff.changed_files.extend(file_changes);
            diff.tool_state_deltas.extend(tool_state_changes);
        }

        // 检查是否需要创建新的 checkpoint
        if self.config.checkpoint_interval_turns > 0 
            && current_turn % self.config.checkpoint_interval_turns == 0 
        {
            self.create_checkpoint(session_id).await?;
        }

        Ok(())
    }

    /// 手动创建检查点
    pub async fn create_checkpoint(&self, session_id: &SessionId) -> Result<SnapshotId, String> {
        let current_turn = *self.turn_counts.read().await
            .get(session_id).unwrap_or(&0);

        // 收集所有消息和状态
        // 在真实实现中这里会从实际的数据源获取当前状态
        let (all_messages, all_tool_states, all_file_states) = self.collect_current_state(session_id).await;

        // 可选: 生成上下文摘要
        let summary = if !all_messages.is_empty() {
            Some(self.generate_summary(&all_messages))
        } else {
            None
        };

        let snapshot = SessionSnapshot {
            id: Uuid::new_v4(),
            session_id: *session_id,
            at_turn: current_turn,
            messages: all_messages,
            tool_states: all_tool_states,
            file_states: all_file_states,
            context_summary: summary,
            created_at: Utc::now(),
            estimated_size_bytes: 0, // TODO: 计算
        };

        let snapshot_id = snapshot.id;

        // 存储快照
        {
            let mut snapshots = self.snapshots.write().await;
            let list = snapshots.entry(*session_id).or_default();
            list.push(snapshot);

            // 保持不超过 max_snapshots
            while list.len() > self.config.max_snapshots {
                list.remove(0); // 移除最旧的
            }
        }

        // 清空 pending diff, 设置新的 from_snapshot_id
        {
            let mut diffs = self.pending_diffs.write().await;
            diffs.insert(*session_id, IncrementalDiff {
                from_snapshot_id: snapshot_id,
                to_snapshot_id: None,
                since_turn: current_turn,
                added_messages: vec![],
                changed_files: vec![],
                tool_state_deltas: vec![],
                created_at: Utc::now(),
            });
        }

        tracing::info!(
            session = %session_id,
            snapshot = %snapshot_id,
            turn = current_turn,
            msg_count = ?snapshot.messages.len(),
            "Checkpoint created"
        );

        Ok(snapshot_id)
    }

    /// 恢复会话到指定状态
    ///
    /// # 流程
    ///
    /// ```text
    /// 1. 加载最近的 snapshot (包含完整状态)
    /// 2. 找到该 snapshot 之后的所有 incremental diffs
    /// 3. 按顺序回放这些 diffs (重建后续的状态变更)
    /// 4. 返回恢复后的完整状态
    /// ```
    pub async fn recover_session(
        &self,
        session_id: &SessionId,
    ) -> Result<RecoveredSession, String> {
        // 1. 获取最新的 snapshot
        let snapshot = self.get_latest_snapshot(session_id).await
            .ok_or("No snapshots found for session")?;

        tracing::info!(
            session = %session_id,
            snapshot = %snapshot.id,
            snapshot_turn = snapshot.at_turn,
            "Starting session recovery"
        );

        // 2. 收集所有后续的 diffs
        let mut recovered_messages = snapshot.messages.clone();
        let mut recovered_file_states: HashMap<String, FileState> = snapshot.file_states.iter()
            .map(|f| (f.path.clone(), f.clone()))
            .collect();
        let mut recovered_tool_states: HashMap<String, ToolState> = snapshot.tool_states.iter()
            .map(|t| (t.tool_name.clone(), t.clone()))
            .collect();

        // 3. 回放 diffs
        // Note: In a full implementation we'd have stored all historical diffs.
        // Here we use the pending diff as the only post-snapshot data.

        if let Some(diff) = self.pending_diffs.read().await.get(session_id) {
            recovered_messages.extend(diff.added_messages.clone());
            
            for fd in &diff.changed_files {
                match &fd.operation {
                    FileOperation::Added | FileOperation::Modified => {
                        if let Some(hash) = &fd.new_hash {
                            recovered_file_states.insert(fd.path.clone(), FileState {
                                path: fd.path.clone(),
                                content_hash: hash.clone(),
                                last_modified: Utc::now(),
                                size_bytes: 0,
                            });
                        }
                    }
                    FileOperation::Deleted => {
                        recovered_file_states.remove(&fd.path);
                    }
                }
            }

            for delta in &diff.tool_state_deltas {
                recovered_tool_states.insert(
                    delta.tool_name.clone(),
                    ToolState {
                        tool_name: delta.tool_name.clone(),
                        state_data: delta.new_state.clone(),
                        updated_at: Utc::now(),
                    },
                );
            }
        }

        // 4. 返回恢复结果
        Ok(RecoveredSession {
            session_id: *session_id,
            base_snapshot: snapshot,
            messages: recovered_messages,
            file_states: recovered_file_states.into_values().collect(),
            tool_states: recovered_tool_states.into_values().collect(),
            recovered_at: Utc::now(),
        })
    }

    /// 获取指定会话的所有快照列表
    pub async fn list_snapshots(&self, session_id: &SessionId) -> Vec<(SnapshotId, u32)> {
        match self.snapshots.read().await.get(session_id) {
            Some(list) => list.iter()
                .map(|s| (s.id, s.at_turn))
                .collect(),
            None => vec![],
        }
    }

    // --- 内部方法 -----------------------------

    async fn get_latest_snapshot(&self, session_id: &SessionId) -> Option<SessionSnapshot> {
        self.snapshots.read().await
            .get(session_id)?
            .last()
            .cloned()
    }

    async fn get_latest_snapshot_id(&self, session_id: &SessionId) -> SnapshotId {
        self.get_latest_snapshot(session_id).await
            .map(|s| s.id)
            .unwrap_or(Uuid::nil())
    }

    async fn collect_current_state(
        &self,
        _session_id: &SessionId,
    ) -> (Vec<SessionMessage>, Vec<ToolState>, Vec<FileState>) {
        // Placeholder: 实际实现从运行中的系统收集当前状态
        (vec![], vec![], vec![])
    }

    fn generate_summary(&self, messages: &[SessionMessage]) -> String {
        // 简单的启发式摘要 (生产环境应使用 LLM 生成)
        let user_msgs = messages.iter()
            .filter(|m| m.role == "user")
            .count();
        let assistant_msgs = messages.iter()
            .filter(|m| m.role == "assistant")
            .count();
        
        format!(
            "Session has {} turns ({} user messages, {} assistant responses)",
            messages.len() / 2, user_msgs, assistant_msgs
        )
    }
}

/// 恢复后的会话状态
#[derive(Debug, Clone)]
pub struct RecoveredSession {
    pub session_id: SessionId,
    pub base_snapshot: SessionSnapshot,
    pub messages: Vec<SessionMessage>,
    pub file_states: Vec<FileState>,
    pub tool_states: Vec<ToolState>,
    pub recovered_at: DateTime<Utc>,
}
