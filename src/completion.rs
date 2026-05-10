//! # 补全引擎集成 — LSP + Qwen 3.6 + 记忆排序
//!
//! 在服务端将 jcode-completion crate 与 Qwen 3.6 Provider 和 LSP 服务器打通。

use crate::provider::Provider;
use jcode_completion::{
    CompletionEngine, CompletionProvider, LspAstProvider,
};
use async_trait::async_trait;
use std::sync::Arc;

/// Qwen 3.6 Provider 适配器 — 将 jcode 的 Provider trait 包装为 CompletionProvider
pub struct QwenProvider {
    inner: Arc<dyn Provider>,
}

impl QwenProvider {
    pub fn new(inner: Arc<dyn Provider>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl CompletionProvider for QwenProvider {
    /// 简单文本补全，无工具调用、无流式
    /// Qwen 3.6 利用 prompt cache 实现 < 50ms 感知延迟
    async fn complete_simple(&self, prompt: &str, system: &str) -> anyhow::Result<String> {
        self.inner.complete_simple(prompt, system).await
    }
}

/// 初始化补全引擎 — 配置 LSP 服务器 + Qwen 3.6 Provider
pub fn create_completion_engine(provider: Arc<dyn Provider>) -> CompletionEngine {
    // 配置 LSP 服务器 (根据已安装的工具链自动注册)
    let lsp = LspAstProvider::new();

    // 注册支持的 LSP 服务器
    register_lsp_if_available(&lsp, "rust", "rust-analyzer", &[]);
    register_lsp_if_available(&lsp, "typescript", "typescript-language-server", &["--stdio"]);
    register_lsp_if_available(&lsp, "python", "pyright-langserver", &["--stdio"]);

    // 创建引擎
    CompletionEngine::new(
        Box::new(QwenProvider::new(provider)),
        Some(Arc::new(lsp)),
    )
}

/// 检查 LSP 服务器是否可执行，是则注册
fn register_lsp_if_available(lsp: &LspAstProvider, language: &str, cmd: &str, args: &[&str]) {
    if which::which(cmd).is_ok() {
        lsp.register_server(language, cmd, args.iter().map(|s| s.to_string()).collect());
        tracing::info!("Registered LSP server: {} → {}", language, cmd);
    } else {
        tracing::debug!("LSP server '{}' not found, skipping", cmd);
    }
}
