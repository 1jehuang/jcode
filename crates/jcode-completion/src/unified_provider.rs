//! UnifiedContextProvider — 三层融合上下文提供者
//!
//! 策略:
//!   1. 先尝试 LspAstProvider (在线, 高精度)
//!   2. LSP 不可用 -> TreeSitterAstProvider (离线, 快速)
//!   3. TreeSitter 也不可用 -> RegexAstProvider (保底)
//!
//! 每一层的输出都向后传递，直到获得足够丰富的信息。

use crate::ast_context::{AstContextProvider, CompletionContext};
use crate::lsp_provider::LspAstProvider;
use crate::treesitter_provider::TreeSitterAstProvider;
use crate::ast_context::RegexAstProvider;
use async_trait::async_trait;
use std::sync::Arc;

/// 三层融合上下文提供者
pub struct UnifiedContextProvider {
    lsp: Option<Arc<LspAstProvider>>,
    treesitter: Arc<TreeSitterAstProvider>,
    regex: Arc<RegexAstProvider>,
}

impl UnifiedContextProvider {
    pub fn new() -> Self {
        Self {
            lsp: None,
            treesitter: Arc::new(TreeSitterAstProvider::new()),
            regex: Arc::new(RegexAstProvider::new()),
        }
    }

    /// 设置 LSP 提供者 (可选)
    pub fn with_lsp(mut self, lsp: Arc<LspAstProvider>) -> Self {
        self.lsp = Some(lsp);
        self
    }

    /// 注册 LSP 服务器 (快捷方式)
    pub fn register_lsp_server(&mut self, language: &str, command: &str, args: Vec<String>) {
        let lsp = LspAstProvider::new();
        lsp.register_server(language, command, args);
        self.lsp = Some(Arc::new(lsp));
    }
}

#[async_trait]
impl AstContextProvider for UnifiedContextProvider {
    async fn resolve_context(
        &self,
        content: &str,
        line: usize,
        column: usize,
    ) -> Option<CompletionContext> {
        // Layer 1: LSP (在线, 高精度)
        if let Some(ref lsp) = self.lsp {
            if let Some(ctx) = lsp.resolve_context(content, line, column).await {
                if ctx.expected_type.is_some() {
                    return Some(ctx);
                }
                // LSP 返回了基础信息但没有类型 — 尝试用 TreeSitter 增强
                if let Some(ts_ctx) = self.treesitter.resolve_context(content, line, column).await {
                    return Some(CompletionContext {
                        parent_symbol: ts_ctx.parent_symbol.or(ctx.parent_symbol),
                        ..ctx
                    });
                }
                return Some(ctx);
            }
        }

        // Layer 2: TreeSitter (离线, 快速)
        if let Some(ctx) = self.treesitter.resolve_context(content, line, column).await {
            return Some(ctx);
        }

        // Layer 3: Regex (保底)
        self.regex.resolve_context(content, line, column).await
    }
}
