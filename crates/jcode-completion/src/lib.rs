//! # jcode-completion
//! 三层架构自动补全引擎：
//!
//! ```text
//! 光标位置
//!    ↓
//! ┌──────────────── Layer 1: AST 精准定位 ────────────────┐
//! │  解析当前文件 → 确定光标处期望的类型/符号/结构          │
//! │  输出: CompletionContext { expected_type, scope, ... } │
//! └──────────────────────────────────────────────────────┘
//!    ↓
//! ┌──────────────── Layer 2: LLM 创造力 ───────────────────┐
//! │  根据 Context 生成候选代码片段                          │
//! │  输出: Vec<CompletionCandidate>                        │
//! └──────────────────────────────────────────────────────┘
//!    ↓
//! ┌──────────────── Layer 3: 记忆个性化 ───────────────────┐
//! │  从用户历史编辑中提取模式 → 排序候选 → 过滤低质量       │
//! │  输出: 最终排序的完成项列表                             │
//! └──────────────────────────────────────────────────────┘
//!    ↓
//! 最终输出到编辑器

mod ast_context;
mod llm_candidate;
mod memory_ranker;
mod lsp_provider;
mod treesitter_provider;
mod unified_provider;

pub use ast_context::{AstContextProvider, CompletionContext, ScopeKind, RegexAstProvider};
pub use llm_candidate::{CandidateGenerator, CompletionCandidate, CandidateKind, CompletionProvider, ProviderCandidateGenerator};
pub use memory_ranker::{MemoryRanker, RankedCandidate, UsageTracker};
pub use lsp_provider::{LspAstProvider, LspConnection};
pub use treesitter_provider::TreeSitterAstProvider;
pub use unified_provider::UnifiedContextProvider;

use std::sync::Arc;

/// 补全引擎 — LSP 精准定位 + Qwen 3.6 原生生成 + 记忆个性化
///
/// 不再依赖本地小模型。利用服务端 Qwen 3.6 的 prompt cache 能力，
/// 补全延迟可控制在 < 50ms 感知延迟。
pub struct CompletionEngine {
    /// Layer 1: LSP/AST 精准定位 (LspAstProvider → TreeSitter → Regex)
    ast: Arc<dyn AstContextProvider>,
    /// Layer 2: Qwen 3.6 原生生成 (通过 Provider API)
    provider: Box<dyn CompletionProvider>,
    /// Layer 3: 用户习惯排序
    memory: Arc<dyn MemoryRanker>,
}

impl CompletionEngine {
    /// 创建生产级引擎 — LSP + Qwen 3.6 + 记忆
    pub fn new(
        provider: Box<dyn CompletionProvider>,
        lsp: Option<Arc<LspAstProvider>>,
    ) -> Self {
        let ast: Arc<dyn AstContextProvider> = match lsp {
            Some(l) => Arc::new(UnifiedContextProvider::new().with_lsp(l)),
            None => Arc::new(UnifiedContextProvider::new()),
        };
        Self {
            ast,
            provider,
            memory: Arc::new(crate::memory_ranker::DefaultMemoryRanker::new()),
        }
    }

    /// 在光标位置生成补全 — LSP → Qwen 3.6 → 记忆排序
    pub async fn complete(
        &self,
        file_path: &str,
        content: &str,
        cursor_line: usize,
        cursor_column: usize,
    ) -> Vec<RankedCandidate> {
        // Layer 1: LSP 获取精准上下文 (0.1-50ms)
        let context = match self.ast.resolve_context(content, cursor_line, cursor_column).await {
            Some(ctx) => CompletionContext { file_path: file_path.to_string(), ..ctx },
            None => return vec![],
        };

        // Layer 2: Qwen 3.6 直接生成 (利用 prompt cache, ~50ms 感知)
        let prompt = format!(
            "Complete the code at cursor:\n\
             File: {file}\n\
             Expected type: {type_:?}\n\
             Scope: {scope:?}\n\
             Current line: {line}\n\
             Cursor prefix: '{prefix}'\n\
             \n\
             Provide the single most likely completion:",
            file = context.file_path,
            type_ = context.expected_type,
            scope = context.scope,
            line = content.lines().nth(cursor_line).unwrap_or(""),
            prefix = context.prefix,
        );

        let candidates = match self.provider.complete_simple(&prompt, "You are a code completion engine. Output ONLY the completion text.").await {
            Ok(text) => {
                let cleaned = text.trim().to_string();
                vec![llm_candidate::CompletionCandidate {
                    label: cleaned.clone(), text: cleaned,
                    detail: context.expected_type.clone(),
                    kind: llm_candidate::CandidateKind::Snippet,
                    score: 0.95,
                }]
            }
            Err(_) => vec![],
        };

        // Layer 3: 记忆排序
        self.memory.rank_and_filter(candidates, &context).await
    }
}
