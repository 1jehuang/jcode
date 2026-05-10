// jcode-context-management
// ════════════════════════════════════════════════════════════════
// 上下文管理与缓存系统 - 移植自 Claude Code
//
// 核心能力:
//   1. Prompt Cache Control — Claude API cache_control 标记管理
//   2. System Prompt 分块 — 静态/动态分离 (splitSysPromptPrefix)
//   3. 缓存断裂检测 — 基于哈希的增量变更检测
//   4. Token 预算系统 — 动态 token 配额管理
//   5. AutoCompact / SnipCompact / Collapse — 三级压缩策略
//   6. Content Block Caching — 工具结果缓存优化
//   7. Microcompact — 轻量级消息截断
//   8. Context Window Rotation — 消息滚动窗口管理
//
// 对应 Claude Code 源码:
//   - src/services/api/claude.ts:3063-3237 (addCacheBreakpoints)
//   - src/services/api/promptCacheBreakDetection.ts (完整 728 行)
//   - src/utils/api.ts:296-435 (splitSysPromptPrefix)
//   - src/utils/tokenBudget.ts / tokenCounter.ts
//   - src/query.ts:311-580 (compact/collapse/snip 流程)
// ════════════════════════════════════════════════════════════════

mod types;
mod cache_control;
mod prompt_splitter;
mod cache_break_detector;
mod token_budget;
mod compact_strategies;
mod context_manager;
mod semantic_cache;

pub use types::*;
pub use semantic_cache::{
    SemanticCache, ContextFusion, Embedding, SemanticEntry,
    SimilarityResult, ContextInjectResult, cosine_similarity,
};
pub use cache_control::{CacheControl, CacheScope, CacheTtl};
pub use prompt_splitter::{PromptSplitter, PromptBlock};
pub use cache_break_detector::{CacheBreakDetector, CacheState, DiffKind};
pub use token_budget::{TokenBudget, BudgetExceededAction};
pub use compact_strategies::{
    CompactStrategy as CmpStrategy,
    Compactor,
    MicroCompactor,
    SnipCompactor,
    CollapseCompactor,
    AutoCompactor,
};
pub use context_manager::ContextManager;

/// 默认缓存 TTL — 短期 (ephemeral) 缓存，通常 5 分钟
pub const DEFAULT_CACHE_TTL_EPHEMERAL: u32 = 300;  // 5 min

/// 默认缓存 TTL — 长期缓存 (仅限订阅用户)，1 小时
pub const DEFAULT_CACHE_TTL_LONG: u32 = 3600;  // 1 hour

/// 触发 auto-compact 的消息数量阈值
pub const AUTO_COMPACT_MESSAGE_THRESHOLD: usize = 50;

/// Snip compact 的目标消息数
pub const SNIP_COMPACT_TARGET_COUNT: usize = 30;

/// Collapse 的摘要最大 token 数
pub const COLLAPSE_SUMMARY_MAX_TOKENS: usize = 2000;

/// 最大上下文窗口大小 (Claude 3.5 Sonnet 默认值)
pub const MAX_CONTEXT_WINDOW_SIZE: usize = 200_000;  // 200K tokens

/// 安全边际 — 不使用完整的上下文窗口
pub const CONTEXT_WINDOW_SAFETY_MARGIN: f64 = 0.9;  // 使用 90%

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_control_creation() {
        let cc = CacheControl::ephemeral();
        assert_eq!(cc.scope(), &CacheScope::Organization);
        
        let cc_long = CacheControl::long_lived();
        assert_eq!(cc.ttl(), Some(3600));
    }

    #[test]
    fn test_prompt_splitting() {
        let splitter = PromptSplitter::new();
        let blocks = splitter.split(
            "System prefix\n=== DYNAMIC_BOUNDARY ===\nDynamic content",
            true,
            true,
        );
        // 应该分离为静态和动态块
        assert!(!blocks.is_empty());
    }

    #[tokio::test]
    async fn test_token_budget_tracking() {
        let budget = TokenBudget::new(
            MAX_CONTEXT_WINDOW_SIZE,
            CONTEXT_WINDOW_SAFETY_MARGIN,
        );
        
        assert!(budget.remaining().unwrap_or(0) > 0);
        budget.record_input(100).await;
        budget.record_output(50).await;
        
        assert!(budget.used() >= 150);
    }

    #[test]
    fn test_cache_break_detection() {
        let mut detector = CacheBreakDetector::new();
        
        // 初始状态
        let state1 = detector.compute_state("system_prompt_v1", &["tool_a", "tool_b"]);
        let changes = detector.detect_changes(&state1);
        
        // 第一次检测应该是全部变化
        assert!(changes.has_changes());
        
        // 第二次检测相同内容应无变化
        let state2 = detector.compute_state("system_prompt_v1", &["tool_a", "tool_b"]);
        let no_changes = detector.detect_changes(&state2);
        assert!(!no_changes.has_changes());
    }
}
