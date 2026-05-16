// jcode-agent-advanced
// ════════════════════════════════════════════════════════════════
// 高级 Agent 循环引擎 - 移植自 Claude Code query.ts (1700+行)
//
// 核心能力:
//   1. ReAct 循环增强版 — Thought->Action->Observation 多轮迭代
//   2. 并行工具调用 — 无依赖工具同时执行
//   3. 流式中断与取消 — CancellationToken + AbortController
//   4. 错误恢复策略 — 指数退避重试 + 模型降级 + 工具降级
//   5. 模型降级 Fallback — 主模型失败时自动切换备用模型
//   6. 结果验证与重格式化 — 输出截断/高亮/结构化解析
//
// 对应 Claude Code 源码:
//   - src/query.ts:219-1729  (queryLoop 核心循环)
//   - src/query.ts:650-955    (API 调用 + fallback)
//   - src/query.ts:1363-1409  (并行工具执行)
//   - src/query.ts:1015-1052  (Abort 处理)
// ════════════════════════════════════════════════════════════════

mod types;
mod agent_loop;
mod parallel_executor;
mod abort_controller;
mod error_recovery;
mod model_fallback;
mod result_handler;
mod streaming_tool_executor;

pub use types::*;
pub use agent_loop::{AgentLoop, AgentLoopConfig, LoopEvent, TerminalState};
pub use parallel_executor::{ParallelToolExecutor, DependencyGraph, ToolExecutionPlan};
pub use abort_controller::{AbortController, AbortSignal, AbortReason};
pub use error_recovery::{
    ErrorRecoveryStrategy, RetryPolicy, BackoffStrategy, 
    RecoveryAction, ErrorClassifier, ToolFallbackRegistry
};
pub use model_fallback::{ModelFallbackManager, FallbackChain, FallbackTrigger};
pub use result_handler::{ResultProcessor, OutputFormat, ValidationResult};
pub use streaming_tool_executor::{StreamingToolExecutor, ToolStreamEvent};

/// 最大重试次数 (Claude Code 默认值)
pub const MAX_RETRY_ATTEMPTS: u32 = 3;

/// 最大模型降级链深度
pub const MAX_FALLBACK_DEPTH: usize = 2;

/// 并行工具调用最大并发数
pub const MAX_PARALLEL_TOOLS: usize = 5;

/// Abort 超时时间 (等待正在执行的工具完成)
pub const ABORT_GRACE_PERIOD_MS: u64 = 5000;

/// 指数退避初始延迟 (ms)
pub const BACKOFF_INITIAL_MS: u64 = 500;

/// 指数退避最大延迟 (ms)
pub const BACKOFF_MAX_MS: u64 = 30000;

/// 指数退避抖动因子 (0.0-1.0)
pub const BACKOFF_JITTER_FACTOR: f64 = 0.2;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_loop_creation() {
        let config = AgentLoopConfig::default();
        let _loop = AgentLoop::new(config);
        // 基本构造测试通过即可，完整集成测试在 integration tests 中
    }

    #[test]
    fn test_abort_signal_propagation() {
        let controller = AbortController::new();
        assert!(!controller.signal().is_aborted());
        controller.abort(AbortReason::UserCancelled);
        assert!(controller.signal().is_aborted());
        assert_eq!(controller.reason(), Some(AbortReason::UserCancelled));
    }

    #[test]
    fn test_retry_policy_exponential_backoff() {
        let policy = RetryPolicy::exponential(MAX_RETRY_ATTEMPTS);
        
        // 第1次重试: ~500ms ± jitter
        let delay1 = policy.next_delay(1).unwrap();
        assert!(delay1 >= BACKOFF_INITIAL_MS as f64 * (1.0 - BACKOFF_JITTER_FACTOR));
        assert!(delay1 <= BACKOFF_INITIAL_MS as f64 * (1.0 + BACKOFF_JITTER_FACTOR));
        
        // 第3次重试: 应该更长
        let delay3 = policy.next_delay(3).unwrap();
        assert!(delay3 > delay1);
    }

    #[test]
    fn test_dependency_graph_resolution() {
        let mut graph = DependencyGraph::new();
        
        // tool_b 依赖 tool_a 的输出
        graph.add_node("tool_a".to_string());
        graph.add_node("tool_b".to_string());
        graph.add_edge("tool_a".to_string(), "tool_b".to_string()).unwrap();
        
        let plan = graph.resolve_execution_plan().unwrap();
        
        // tool_a 必须在 tool_a 之前执行
        let pos_a = plan.iter().position(|t| t == "tool_a").unwrap();
        let pos_b = plan.iter().position(|t| t == "tool_b").unwrap();
        assert!(pos_a < pos_b, "tool_a should execute before tool_b");
    }
}
