// jcode-lsp
// ════════════════════════════════════════════════════════════════
// LSP (Language Server Protocol) 统一实现
//
// ## 整合成果
// 将 jcode 中 4 套独立且未完成的 LSP 实现整合为统一的工业级系统：
//
//   ✅ **transport.rs** — JSON-RPC 2.0 传输层 (来自 completion/lsp_provider)
//   ✅ **client.rs** — 工业级 LSP Client (整合 ide-integration + lsp_provider)
//   ✅ **server_manager.rs** — 多语言 Server 管理 (12 种语言支持)
//
// ## 核心能力 (对标 Claude Code LSPClient.ts)
//   1. JSON-RPC over stdio 持久连接
//   2. 符号定义跳转 (Go to Definition)
//   3. 引用查找 (Find All References)
//   4. 悬停文档 (Hover)
//   5. 诊断/错误/警告 (Diagnostics)
//   6. 补全建议 (Completion)
//   7. 重命名重构 (Rename)
//   8. 文档符号 (Document Symbols)
//   9. 工作区符号 (Workspace Symbols)
//  10. 多语言 Server 自动路由
//  11. 进程生命周期管理
//  12. 崩溃恢复与优雅关闭
//
// ## 架构
//
// ┌──────────────────────────────┐
// │        Tool Layer            │ ← src/tool/lsp.rs (AI Agent 入口)
// └──────────────┬───────────────┘
//                │
// ┌──────────────▼───────────────┐
// │      LspServerManager        │ ← server_manager.rs (多实例管理)
// │  ┌────────┬────────┐        │
// │  │rust-analyzer│tsserver│    │ ← 每个 LSP Server 进程
// │  └────────┴────────┘        │
// └──────────────┬───────────────┘
//                │
// ┌──────────────▼───────────────┐
// │       LspClient              │ ← client.rs (JSON-RPC 通信)
// │  · send_request / response   │
// │  · send_notification         │
// │  · process lifecycle         │
// └──────────────┬───────────────┘
//                │
// ┌──────────────▼───────────────┐
// │     Transport Layer          │ ← transport.rs (协议编解码)
// │  · Content-Length parsing    │
// │  · Async I/O (tokio)        │
// │  · Request ID routing        │
// └──────────────────────────────┘
// ════════════════════════════════════════════════════════════════

mod transport;
mod client;
mod server_manager;
mod cache;
mod document_sync;
mod diagnostics;
mod completion;
mod performance;

pub use transport::{build_request, build_notification, parse_response, JsonRpcError};
pub use client::{LspClient, LspError, LspResult};
pub use server_manager::{
    LspServerManager, 
    ServerConfig, 
    LanguageId,
};
pub use cache::{
    LspResultCache,
    CacheStats,
};
pub use document_sync::DocumentSyncManager;
pub use diagnostics::{DiagnosticsManager, DiagnosticEvent, DiagnosticsConfig, FileDiagnosticSummary};
pub use completion::{CompletionManager, CompletionConfig, EnhancedCompletionItem};
pub use performance::{
    PerformanceMonitor,
    OperationMetrics,
    PerformanceStats,
    ServerHealthInfo,
    AdaptiveConfig,
};

/// 便捷的 LSP 操作 trait — 统一的高层 API
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

    // ─── Advanced operations (Phase 2) ──────────────────

    /// 获取文档符号列表 (函数、类、变量等)
    async fn document_symbol(&self, file: &str) -> LspResult<Vec<lsp_types::DocumentSymbol>>;

    /// 工作区符号搜索
    async fn workspace_symbol(&self, query: &str) -> LspResult<Vec<lsp_types::SymbolInformation>>;

    /// 跳转到实现 (接口/trait)
    async fn goto_implementation(&self, file: &str, line: u32, character: u32) -> LspResult<Vec<lsp_types::Location>>;

    /// 准备调用层次 (获取调用树根节点)
    async fn prepare_call_hierarchy(&self, file: &str, line: u32, character: u32) -> LspResult<Vec<lsp_types::CallHierarchyItem>>;
}
