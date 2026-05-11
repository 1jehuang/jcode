//! Web IDE 请求处理器
//!
//! 所有 WebSocket 请求的路由和处理逻辑：
//! - editor: 编辑器操作（打开、关闭、保存、编辑）
//! - lsp: LSP 语言服务（补全、诊断、导航）
//! - fs: 文件系统操作（浏览、读写、监控）
//! - terminal: 终端会话管理
//! - git: Git 工作流集成
//! - ai: AI 助手交互
//! - collab: 协作编辑
//! - project: 项目构建与测试

pub mod editor;
pub mod lsp;
pub mod fs;
pub mod terminal;
pub mod git;
pub mod ai;
pub mod collab;
pub mod project;
pub mod system;
