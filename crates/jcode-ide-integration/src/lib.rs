//! JCode IDE Deep Integration Module
//!
//! ## 来源
//! 移植自 Claude Code (Anthropic) 的优秀功能:
//! - `src/utils/ide.ts` (45KB) — IDE 检测、连接、扩展安装
//! - `src/services/lsp/LSPClient.ts` (14KB) — LSP 客户端封装
//! - `src/services/lsp/LSPServerManager.ts` (13KB) — 多服务器实例管理
//! - `src/services/lsp/LSPServerInstance.ts` (16KB) — 单服务器生命周期
//! - `src/tools/LSPTool/LSPTool.ts` (25KB) — LSP 工具暴露给 AI
//! - `src/hooks/useIDEIntegration.tsx` (10KB) — React Hook: IDE 集成生命周期
//! - `src/components/IdeAutoConnectDialog.tsx` — 自动连接对话框
//! - `src/services/mcp/client.ts` — MCP 客户端 IDE RPC 调用
//!
//! ## 功能概览
//! 1. **IDE Lockfile 发现协议**: 扫描 `~/.jcode/ide/*.lock` 自动发现运行中的 IDE
//! 2. **双协议支持**: WebSocket (实时双向) + SSE (服务端推送)
//! 3. **LSP 语言服务客户端**: 基于 tower-lsp，支持多服务器路由
//! 4. **MCP IDE 桥接**: IDE 注册为特殊 MCP Server，Agent 统一调用
//! 5. **WSL/Windows 路径转换**: 跨平台兼容
//! 6. **JetBrains + VSCode 系列**: 支持 22 种主流 IDE
//!
//! ## 架构设计原则 (继承自 Claude Code)
//! - **非侵入式扩展**: 所有代码通过外部 Lockfile 协议, 不修改 IDE 核心
//! - **渐进式加载**: IDE/LSP 按需初始化, 不影响启动速度
//! - **可插拔架构**: Provider/IDE/LSP 均可热插拔
//! - **隐私安全**: 代码不出本机 (除非用户明确授权远程会话)

pub mod types;
pub mod ide_detector;
pub mod lsp_client;
pub mod mcp_ide_bridge;

// Re-export main types for convenience
pub use types::{
    IdeType, IdeTransport, IdeLockfileContent, DetectedIdeInfo,
    McpIdeConfig, LspStartOptions, LspDiagnostic, LspSeverity, LspReference,
    IdeConnectionStatus,
};

// Re-export IDE detection & connection
pub use ide_detector::{
    IdeDetector, IdeDetectorConfig, IdeConnectionManager, IdeConnectionCallbacks,
};

// Re-export LSP client
pub use lsp_client::{
    LspClient, StdioLspClient, LspServerManager, LspServerEntry,
};

// Re-export MCP bridge
pub use mcp_ide_bridge::{
    McpIdeBridge, DynamicMcpConfig, IdeRpcMethod, IdeRpcResponse,
    FileLocation, TextEditOperation, McpToolDefinition,
};

// ============================================================================
// 预配置的常用 LSP 服务器注册表
// ============================================================================

/// 获取常用编程语言的 LSP 服务器预配置
///
/// 覆盖 Rust, TypeScript, Python, Go, C/C++, Java 等主流语言
/// 对应 Claude Code 中的 LSP 服务器自动发现机制
pub fn get_builtin_lsp_servers() -> Vec<LspServerEntry> {
    vec![
        // === Rust: rust-analyzer ===
        LspServerEntry {
            id: "rust-analyzer".to_string(),
            name: "rust-analyzer (Rust)".to_string(),
            command: "rust-analyzer".to_string(),
            args_template: vec!["rust-analyzer".to_string()],
            extensions: vec![".rs".to_string()],
            lazy_start: true,
        },

        // === TypeScript/JavaScript: typescript-language-server ===
        LspServerEntry {
            id: "typescript".to_string(),
            name: "TypeScript Language Server".to_string(),
            command: "typescript-language-server".to_string(),
            args_template: vec!["typescript-language-server".to_string(), "--stdio".to_string()],
            extensions: vec![
                ".ts".to_string(), ".tsx".to_string(), ".js".to_string(), 
                ".jsx".to_string(), ".mjs".to_string(), ".cjs".to_string(),
            ],
            lazy_start: true,
        },

        // === Python: pylsp (python-lsp-server) ===
        LspServerEntry {
            id: "pylsp".to_string(),
            name: "PyLSP (Python)".to_string(),
            command: "pylsp".to_string(),
            args_template: vec!["pylsp".to_string()],
            extensions: vec![".py".to_string(), ".pyi".to_string()],
            lazy_start: true,
        },

        // === Go: gopls ===
        LspServerEntry {
            id: "gopls".to_string(),
            name: "gopls (Go)".to_string(),
            command: "gopls".to_string(),
            args_template: vec!["gopls".to_string(), "serve".to_string()],
            extensions: vec![".go".to_string()],
            lazy_start: true,
        },

        // === C/C++: clangd ===
        LspServerEntry {
            id: "clangd".to_string(),
            name: "clangd (C/C++)".to_string(),
            command: "clangd".to_string(),
            args_template: vec!["clangd".to_string(), "--background-index".to_string()],
            extensions: vec![
                ".c".to_string(), ".cpp".to_string(), ".cc".to_string(),
                ".cxx".to_string(), ".h".to_string(), ".hpp".to_string(), ".hxx".to_string(),
            ],
            lazy_start: true,
        },

        // === Java: jdtls ===
        LspServerEntry {
            id: "jdtls".to_string(),
            name: "JDTLS (Java)".to_string(),
            command: "jdtls".to_string(),
            args_template: vec!["jdtls".to_string()],
            extensions: vec![".java".to_string()],
            lazy_start: true,
        },

        // === HTML: html-languageserver ===
        LspServerEntry {
            id: "html".to_string(),
            name: "HTML Language Server".to_string(),
            command: "html-languageserver".to_string(),
            args_template: vec!["html-languageserver".to_string(), "--stdio".to_string()],
            extensions: vec![
                ".html".to_string(), ".htm".to_string(), 
                ".vue".to_string(), ".svelte".to_string(),
            ],
            lazy_start: false,
        },

        // === CSS: css-languageserver ===
        LspServerEntry {
            id: "css".to_string(),
            name: "CSS Language Server".to_string(),
            command: "css-languageserver".to_string(),
            args_template: vec!["css-languageserver".to_string(), "--stdio".to_string()],
            extensions: vec![".css".to_string(), ".scss".to_string(), ".less".to_string()],
            lazy_start: false,
        },

        // === JSON: json-languageserver ===
        LspServerEntry {
            id: "json".to_string(),
            name: "JSON Language Server".to_string(),
            command: "json-languageserver".to_string(),
            args_template: vec!["json-languageserver".to_string(), "--stdio".to_string()],
            extensions: vec![".json".to_string()],
            lazy_start: false,
        },

        // === YAML: yaml-language-server ===
        LspServerEntry {
            id: "yaml".to_string(),
            name: "YAML Language Server".to_string(),
            command: "yaml-language-server".to_string(),
            args_template: vec!["yaml-language-server".to_string(), "--stdio".to_string()],
            extensions: vec![".yml".to_string(), ".yaml".to_string()],
            lazy_start: false,
        },

        // === Markdown: marksman ===
        LspServerEntry {
            id: "markdown".to_string(),
            name: "Marksman (Markdown)".to_string(),
            command: "marksman".to_string(),
            args_template: vec!["marksman".to_string(), "server".to_string()],
            extensions: vec![".md".to_string(), ".markdown".to_string()],
            lazy_start: false,
        },

        // === TOML: taplo ===
        LspServerEntry {
            id: "toml".to_string(),
            name: "TAPLO (TOML)".to_string(),
            command: "taplo".to_string(),
            args_template: vec!["taplo".to_string(), "lsp".to_string()],
            extensions: vec![".toml".to_string()],
            lazy_start: false,
        },
    ]
}
