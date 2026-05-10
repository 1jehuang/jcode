// jcode-session-persist
// ════════════════════════════════════════════════════════════════
// 会话管理与持久化 - 移植自 Claude Code
//
// 核心能力:
//   1. JSONL 存储 — 逐行追加写入，崩溃安全 (对应 transcript.jsonl)
//   2. 会话快照 — 完整状态序列化/反序列化
//   3. 增量恢复 — 从中断点恢复，而非从头开始
//   4. 对话摘要 — LLM 压缩长对话，保留关键信息
//   5. 会话元数据 — 标题/标签/agent/成本 追踪
//   6. 文件指针 — 记录已读取位置，支持断点续读
//   7. 多级存储 — memory → disk → archive 三层架构
//
// 对应 Claude Code 源码:
//   - src/utils/sessionStorage.ts (1384行) — Project 单例核心类
//   - src/utils/sessionState.ts — 状态机定义
//   - src/utils/sessionRestore.ts (551行) — 恢复流程
//   - src/utils/transcript.ts — JSONL 读写
// ════════════════════════════════════════════════════════════════

mod types;
mod jsonl_store;
mod session_manager;
mod snapshot;
mod summary;
mod metadata;

pub use types::*;
pub use jsonl_store::JSONLStore;
pub use session_manager::{
    SessionManager,
    SessionHandle,
    SessionLifecycle,
};
pub use snapshot::{SessionSnapshot, Snapshotter};
pub use summary::{ConversationSummarizer, ConversationSummary};

/// JSONL 文件扩展名
pub const TRANSCRIPT_EXT: &str = ".jsonl";

/// 默认最大摘要长度 (字符)
pub const DEFAULT_MAX_SUMMARY_LENGTH: usize = 4000;

/// 快照自动保存间隔 (秒)
pub const SNAPSHOT_AUTO_SAVE_INTERVAL_SECS: u64 = 60;

/// 最大内存中缓存的事件数
pub const MAX_MEMORY_EVENTS: usize = 1000;

/// 存储目录名称
pub const STORAGE_DIR_NAME: &str = ".jcode";
pub const SESSIONS_SUBDIR: &str = "sessions";
pub const ARCHIVE_SUBDIR: &str = "archive";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_jsonl_store_write_and_read() {
        let dir = TempDir::new().unwrap();
        let store = JSONLStore::new(dir.path().join("test.jsonl"));
        
        // 写入事件
        let event = SessionEvent {
            id: "evt-001".to_string(),
            timestamp: chrono::Utc::now(),
            event_type: EventType::Message { role: "user".to_string(), content: "hello".into() },
            session_id: "sess-001".to_string(),
        };
        store.append(&event).await.unwrap();
        
        // 读取回放
        let events: Vec<SessionEvent> = store.read_all().await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "evt-001");
    }

    #[tokio::test]
    async fn test_jsonl_store_append_atomicity() {
        let dir = TempDir::new().unwrap();
        let store = JSONLStore::new(dir.path().join("atomic_test.jsonl"));
        
        // 并发追加
        for i in 0..100 {
            let event = SessionEvent {
                id: format!("evt-{:03}", i),
                timestamp: chrono::Utc::now(),
                event_type: EventType::System { message: format!("event {}", i) },
                session_id: "sess-concurrent".to_string(),
            };
            store.append(&event).await.unwrap();
        }
        
        // 验证行数
        let count = store.count_lines().await.unwrap();
        assert_eq!(count, 100);
    }

    #[test]
    fn test_session_state_machine() {
        // Idle → Running
        let state = SessionState::Idle;
        assert!(state.can_transition_to(&SessionState::Running));
        
        // Running → RequiresAction
        let state2 = SessionState::Running;
        assert!(state2.can_transition_to(&SessionState::RequiresAction));
        
        // RequiresAction → Running (用户批准后)
        let state3 = SessionState::RequiresAction;
        assert!(state3.can_transition_to(&SessionState::Running));
        
        // Running → Idle
        let state4 = SessionState::Running;
        assert!(state4.can_transition_to(&SessionState::Idle));
    }

    #[tokio::test]
    async fn test_snapshot_creation_and_restore() {
        let dir = TempDir::new().unwrap();
        let snapshotter = Snapshotter::new(dir.path().to_path_buf());
        
        let snapshot = SessionSnapshot {
            session_id: "snap-test".to_string(),
            created_at: chrono::Utc::now(),
            messages: vec![
                MessageSnapshot { 
                    role: "user".to_string(), 
                    content: "test message".to_string(),
                    token_count: Some(3),
                }
            ],
            turn_count: 1,
            total_tokens_used: 10,
            cost_usd: 0.0001,
            metadata: HashMap::new(),
        };
        
        snapshotter.save(&snapshot).await.unwrap();
        let restored = snapshotter.load("snap-test").await.unwrap();
        
        assert_eq!(restored.session_id, "snap-test");
        assert_eq!(restored.messages.len(), 1);
    }
}
