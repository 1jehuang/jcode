//! # Session Export - 会话导出模块
//!
//! 提供多格式的会话数据导出能力，支持：
//! - **JSON格式** - 结构化数据，适合程序处理
//! - **Markdown格式** - 人类可读，适合文档归档
//! - **元数据提取** - 统计信息和会话属性
//!
//! ## 支持的导出格式
//!
//! ### JSON (结构化)
//! ```json
//! {
//!   "session_id": "abc-123",
//!   "export_time": "2026-05-14T12:00:00Z",
//!   "messages": [
//!     {"role": "User", "content": "Hello", "timestamp": "..."}
//!   ],
//!   "metadata": {
//!     "model": "gpt-4",
//!     "total_tokens": 1500,
//!     "file_edits": 3,
//!     "commands_run": 5
//!   },
//!   "stats": {
//!     "message_count": 10,
//!     "user_messages": 5,
//!     "assistant_messages": 4,
//!     "tool_calls": 1,
//!     "estimated_cost_usd": 0.50
//!   }
//! }
//! ```
//!
//! ### Markdown (人类可读)
//! ```markdown
//! # Session Export: abc-123
//!
//! Exported: 2026-05-14 12:00:00 UTC
//!
//! ## 👤 User
//!
//! Hello, can you help me debug this issue?
//!
//! ## 🤖 Assistant
//!
//! Sure! Let me look at the error logs...
//!
//! ## 🔧 Tool
//!
//! ```
//! Running tests...
//! ```
//! ```
//!
//! ## 使用示例
//!
//! ```rust,no_run
//! use carpai::session_export::{SessionExporter, MessageRole};
//!
//! // 准备消息数据
//! let messages = vec![
//!     (MessageRole::User, "Help me fix bug #123".to_string()),
//!     (MessageRole::Assistant, "I'll analyze the code...".to_string()),
//!     (MessageRole::Tool, "Running linter...".to_string()),
//! ];
//!
//! // 导出为JSON
//! SessionExporter::export_to_json(
//!     "debug-session",
//!     messages.clone(),
//!     &PathBuf::from("session.json")
//! ).unwrap();
//!
//! // 导出为Markdown（适合文档）
//! SessionExporter::export_to_markdown(
//!     "debug-session",
//!     messages,
//!     &PathBuf::from("session.md")
//! ).unwrap();
//!
//! // 列出所有已导出的会话
//! let sessions = SessionExporter::list_sessions(&PathBuf::from("./sessions")).unwrap();
//! for session in sessions {
//!     println!("{} - {} bytes", session.id, session.size_bytes);
//! }
//! ```

use serde::{Serialize, Deserialize};