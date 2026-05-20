//! # jcode-completion
//! 三层架构自动补全引擎：
//!
//! ```text
//! 光标位置
//!    v
//! +---------------- Layer 1: AST 精准定位 ----------------+
//! |  解析当前文件 -> 确定光标处期望的类型/符号/结构          |
//! |  输出: CompletionContext { expected_type, scope, ... } |
//! +------------------------------------------------------+
//!    v
//! +---------------- Layer 2: LLM 创造力 -------------------+
//! |  根据 Context 生成候选代码片段                          |
//! |  输出: Vec<CompletionCandidate>                        |
//! +------------------------------------------------------+
//!    v
//! +---------------- Layer 3: 记忆个性化 -------------------+
//! |  从用户历史编辑中提取模式 -> 排序候选 -> 过滤低质量       |
//! |  输出: 最终排序的完成项列表                             |
//! +------------------------------------------------------+
//!    v
//! 最终输出到编辑器

mod ast_context;
mod llm_candidate;
mod memory_ranker;
mod lsp_provider;
mod treesitter_provider;
mod unified_provider;
mod streaming_prefetch;
mod incremental_index;
mod behavior_learner;

pub use ast_context::{AstContextProvider, CompletionContext, ScopeKind, RegexAstProvider};
pub use llm_candidate::{CandidateGenerator, CompletionCandidate, CandidateKind, CompletionProvider, ProviderCandidateGenerator};
pub use memory_ranker::{MemoryRanker, RankedCandidate, UsageTracker};
pub use lsp_provider::{LspAstProvider, LspConnection};
pub use treesitter_provider::TreeSitterAstProvider;
pub use unified_provider::UnifiedContextProvider;
pub use streaming_prefetch::{StreamingPrefetcher, PrefetchStatistics};
pub use incremental_index::{IncrementalIndex, SymbolEntry, SymbolKind, FileChangeEvent, ChangeType, IndexStatistics};
pub use behavior_learner::{BehaviorLearner, CompletionEvent, CompletionContextSnapshot, UserPreferences, LearningStatistics};

use std::sync::Arc;
use std::path::PathBuf;

/// 补全引擎 — LSP 精准定位 + Qwen 3.6 原生生成 + 记忆个性化 + 流式预取 + 行为学习
///
/// 不再依赖本地小模型。利用服务端 Qwen 3.6 的 prompt cache 能力，
/// 补全延迟可控制在 < 50ms 感知延迟。
pub struct CompletionEngine {
    /// Layer 1: LSP/AST 精准定位 (LspAstProvider -> TreeSitter -> Regex)
    ast: Arc<dyn AstContextProvider>,
    /// Layer 2: Qwen 3.6 原生生成 (通过 Provider API)
    provider: Box<dyn CompletionProvider>,
    /// Layer 3: 用户习惯排序
    memory: Arc<dyn MemoryRanker>,
    /// Layer 4: 流式预取缓存
    prefetcher: Arc<StreamingPrefetcher>,
    /// Layer 5: 用户行为学习
    behavior_learner: Arc<BehaviorLearner>,
}

impl CompletionEngine {
    /// 创建生产级引擎 — LSP + Qwen 3.6 + 记忆 + 预取 + 学习
    pub fn new(
        provider: Box<dyn CompletionProvider>,
        lsp: Option<Arc<LspAstProvider>>,
        storage_path: Option<PathBuf>,
    ) -> Self {
        let ast: Arc<dyn AstContextProvider> = match lsp {
            Some(l) => Arc::new(UnifiedContextProvider::new().with_lsp(l)),
            None => Arc::new(UnifiedContextProvider::new()),
        };
        Self {
            ast,
            provider,
            memory: Arc::new(crate::memory_ranker::DefaultMemoryRanker::new()),
            prefetcher: Arc::new(StreamingPrefetcher::new()),
            behavior_learner: Arc::new(BehaviorLearner::new(storage_path)),
        }
    }

    /// 在光标位置生成补全 — 预取检查 -> LSP -> Qwen 3.6 -> 记忆排序 -> 行为学习 -> 记录模式
    pub async fn complete(
        &self,
        file_path: &str,
        content: &str,
        cursor_line: usize,
        cursor_column: usize,
    ) -> Vec<RankedCandidate> {
        // Layer 0: 检查预取缓存 (0-5ms if hit)
        let temp_context = CompletionContext {
            file_path: file_path.to_string(),
            expected_type: None,
            scope: None,
            prefix: "".to_string(),
            suffix: "".to_string(),
            line_content: "".to_string(),
        };

        if let Some(cached) = self.prefetcher.get_cached(&temp_context).await {
            let context = match self.ast.resolve_context(content, cursor_line, cursor_column).await {
                Some(ctx) => CompletionContext { file_path: file_path.to_string(), ..ctx },
                None => return vec![],
            };
            return self.memory.rank_and_filter(cached, &context).await;
        }

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

        // Store in prefetch cache for future use
        self.prefetcher.store_completions(&context, candidates.clone()).await;

        // Request prefetch for predicted next contexts
        self.prefetcher.request_prefetch(&context).await;

        // Layer 3: 记忆排序
        let mut ranked = self.memory.rank_and_filter(candidates, &context).await;

        // Layer 5: 应用行为学习个性化分数
        for ranked_item in &mut ranked {
            let personalization_score = self.behavior_learner.get_personalization_score(
                &ranked_item.candidate.label,
                file_path,
            );
            // Blend personalization with existing score
            ranked_item.rank_score = ranked_item.rank_score * 0.8 + personalization_score * 0.2;
        }

        // Re-sort after personalization
        ranked.sort_by(|a, b| b.rank_score.partial_cmp(&a.rank_score).unwrap_or(std::cmp::Ordering::Equal));

        // Record interaction for learning (assume first candidate would be accepted)
        if let Some(first) = ranked.first() {
            self.prefetcher.record_completion_accepted(file_path, &first.candidate.label);

            // Record detailed event for behavior learning
            let event = CompletionEvent {
                timestamp: chrono::Utc::now().timestamp_millis() as u64,
                file_path: file_path.to_string(),
                context: CompletionContextSnapshot {
                    prefix: context.prefix.clone(),
                    suffix: context.suffix.clone(),
                    line_content: context.line_content.clone(),
                    scope: context.scope.clone(),
                    expected_type: context.expected_type.clone(),
                },
                offered_completions: ranked.iter().map(|r| r.candidate.label.clone()).collect(),
                accepted_index: Some(0), // Assume top choice accepted
                time_to_decision_ms: 500, // Placeholder
            };
            self.behavior_learner.record_completion_event(event).await;
        }

        ranked
    }

    /// Get prefetch statistics for monitoring
    pub fn get_prefetch_stats(&self) -> PrefetchStatistics {
        self.prefetcher.get_stats()
    }

    /// Get behavior learning statistics
    pub fn get_learning_stats(&self) -> LearningStatistics {
        self.behavior_learner.get_learning_stats()
    }
}
