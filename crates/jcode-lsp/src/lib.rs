// jcode-lsp
// ════════════════════════════════════════════════════════════════
// LSP (Language Server Protocol) 集成 — 移植自 Claude Code tools/LSPTool/
//
// 核心能力:
//
//   1. JSON-RPC over stdio 通信
//   2. 符号定义跳转 (Go to Definition)
//   3. 引用查找 (Find All References)
//   4. 诊断/错误/警告 (Diagnostics)
//   5. 补全建议 (Completion)
//   6. 重命名重构 (Rename)
//   7. 悬停文档 (Hover)
//   8. 多语言 Server 管理 (TypeScript/Rust/Python/Go...)
//   9. 文档同步 (textDocument/didOpen/didChange/didSave)
//   10. Workspace 扫描和自动 Server 启动
//
// 架构:
//
// ┌──────────────────────────────┐
// │        LspClientManager      │
// │                              │
// │  ┌──────────┬──────────┐    │
// │  │ TS Server│ Rust     │    │ ← 每个 LSP Server 进程
// │  │ Analyzer │ Analyzer │    │   通过 stdio JSON-RPC 通信
// │  └──────────┴──────────┘    │
// │         ↕ JSON-RPC          │
// │  ┌────────────────────┐     │
// │  │  Unified LSP API   │     │ ← 统一接口, 屏蔽语言差异
// │  └────────────────────┘     │
// └──────────────────────────────┘
// ════════════════════════════════════════════════════════════════

mod client;
mod server_manager;
mod types_ext;

pub use client::{LspClient, LspResult};
pub use server_manager::{LspServerManager, ServerConfig, LanguageId};

/// 便捷的 LSP 操作 trait
#[async_trait::async_trait]
pub trait LspOperations: Send + Sync {
    /// 跳转到定义
    async fn goto_definition(&self, file: &str, line: u32, character: u32) -> LspResult<Vec<lsp_types::Location>>;

    /// 查找所有引用
    async fn find_references(&self, file: &str, line: u32, character: u32) -> LspResult<Vec<lsp_types::Location>>;

    /// 获取诊断信息 (错误/警告)
    async fn get_diagnostics(&self, file: &str) -> LspResult<Vec<lsp_types::Diagnostic>>;

    /// 获取补全建议
    async fn get_completion(&self, file: &str, line: u32, character: u32) -> LspResult<Vec<lsp_types::CompletionItem>>;

    /// 获取悬停文档
    async fn hover(&self, file: &str, line: u32, character: u32) -> LspResult<Option<lsp_types::Hover>>;
}
