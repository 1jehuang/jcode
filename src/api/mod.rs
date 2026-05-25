//! # CarpAI API Layer
//!
//! Unified API layer providing gRPC, REST, and WebSocket endpoints.

pub mod middleware;
pub mod rest_api;

// Re-export auto_mode and completion for backward compatibility
pub use crate::auto_mode;
pub use crate::completion;

pub use middleware::{TenantContext, tenant_middleware, get_tenant_context};

// ══════════════════════════════
// Auto Mode 公共API
// ══════════════════════════════

pub use crate::auto_mode::engine::AutoModeEngine;
pub use crate::auto_mode::{
    AutoModeConfig,
    AutoApprovalDecision,
    ActionType,
    ToolContext,
    AutoModeStats,
};
pub use crate::auto_mode::safety::{
    SafetyGuardrail,
    RiskLevel,
    SecurityCategory,
    SafetyAdvice,
    SafetyRecommendation,
};
pub use crate::auto_mode::learning::{
    LearningSystem,
    LearnedPattern,
    DecisionRecord,
    UserDecision,
    LearningStats,
    Feedback,
};
pub use crate::auto_mode::confidence::{
    ConfidenceModel,
    ConfidenceResult,
    ModelConfig,
};

// ══════════════════════════════
// Completion 公共API
// ══════════════════════════════

pub use crate::completion::bash::{
    BashParser,
    CommandRegistry,
    CompletionSuggestion,
    CompletionKind,
    CompletionContext,
    CompletionResult,
    CommandSpec,
    SubcommandSpec,
    ArgSpec,
    ArgType,
    CommandCategory,
};

// ══════════════════════════════
// 工具函数
// ══════════════════════════════

/// 创建默认配置的Auto Mode引擎（推荐用于大多数场景）
pub fn create_default_auto_engine() -> AutoModeEngine {
    AutoModeEngine::with_defaults()
}

/// 创建高安全级别的Auto Mode引擎（适用于生产环境）
pub fn create_secure_auto_engine() -> AutoModeEngine {
    let mut config = AutoModeConfig::default();
    config.enabled = true;
    config.approval_threshold = 0.95;      // 更高的阈值
    config.auto_accept_safe = false;       // 禁用自动接受
    config.max_auto_actions = 10;          // 限制连续自动操作
    
    AutoModeEngine::new(config)
}

/// 创建开发环境友好的Auto Mode引擎
pub fn create_dev_friendly_auto_engine() -> AutoModeEngine {
    let mut config = AutoModeConfig::default();
    config.enabled = true;
    config.approval_threshold = 0.7;       // 较低阈值，更宽松
    config.auto_accept_safe = true;
    config.max_auto_actions = 100;
    
    AutoModeEngine::new(config)
}

/// 创建Bash补全解析器实例
pub fn create_bash_completer() -> (BashParser, CommandRegistry) {
    (BashParser::new(), CommandRegistry::new())
}

/// 获取命令补全建议（便捷方法）
pub fn get_completion_suggestions(
    input: &str,
    cursor_position: usize,
) -> Vec<CompletionSuggestion> {
    let (parser, _) = create_bash_completer();
    let ctx = CompletionContext::new(input, cursor_position);
    parser.get_suggestions(&ctx)
}

// ══════════════════════════════
// 使用示例文档
// ══════════════════════════════

#[cfg(doctest)]
doc_comment! {
    /// # 示例1：基本Auto Mode使用
    ///
    /// ```rust,no_run
    /// use carpai::api::{create_default_auto_engine, ActionType, ToolContext};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = create_default_auto_engine();
    ///
    ///     // 安全操作应该自动批准
    ///     let ctx = ToolContext::new(ActionType::FileRead, "read README.md")
    ///         .with_file("README.md");
    ///
    ///     if let AutoApprovalDecision::AutoApprove(reason) =
    ///         engine.should_auto_approve(&ActionType::FileRead, "read docs", &ctx).await
    ///     {
    ///         println!("✅ {}", reason);
    ///         // 执行读取操作
    ///     }
    /// }
    /// ```
    fn example1_basic_usage() {}

    /// # 示例2：安全护栏集成
    ///
    /// ```rust,no_run
    /// use carpai::api::{SafetyGuardrail, RiskLevel, AutoModeConfig};
    ///
    /// let guardrail = SafetyGuardrail::new(&AutoModeConfig::default());
    ///
    /// // 评估各种操作的风险
    /// let operations = vec![
    ///     ("ls -la", RiskLevel::Low),
    ///     ("rm -rf /tmp/data", RiskLevel::High),
    ///     ("DROP TABLE users", RiskLevel::Critical),
    /// ];
    ///
    /// for (op, expected_risk) in operations {
    ///     let risk = guardrail.assess_risk(op);
    ///     assert_eq!(risk, expected_risk);
    ///
    ///     if risk == RiskLevel::Critical {
    ///         eprintln!("🛑 操作被阻止: {}", op);
    ///     } else if risk >= RiskLevel::High {
    ///         println!("⚠️ 需要确认: {} ({})", op, risk);
    ///     } else {
    ///         println!("✅ 允许执行: {}", op);
    ///     }
    /// }
    /// ```
    fn example2_safety_integration() {}

    /// # 示例3：Bash补全集成到REPL/IDE
    ///
    /// ```rust,no_run
    /// use carpai::api::{BashParser, CommandRegistry, CompletionContext};
    ///
    /// struct ShellRepl {
    ///     parser: BashParser,
    ///     registry: CommandRegistry,
    ///     history: Vec<String>,
    /// }
    ///
    /// impl ShellRepl {
    ///     pub fn new() -> Self {
    ///         Self {
    ///             parser: BashParser::new(),
    ///             registry: CommandRegistry::new(),
    ///             history: vec![],
    ///         }
    ///     }
    ///
    ///     /// 处理Tab键补全请求
    ///     pub fn handle_tab_completion(
    ///         &mut self,
    ///         line: &str,
    ///         cursor_pos: usize,
    ///     ) -> Vec<String> {
    ///         let ctx = CompletionContext::new(line, cursor_pos)
    ///             .with_working_dir(std::env::current_dir().unwrap_or_default().as_path());
    ///
    ///         // 从AST解析器获取建议
    ///         let mut suggestions = self.parser.get_suggestions(&ctx);
    ///
    ///         // 如果有命令上下文，从注册表获取更详细的子命令信息
    ///         if let Some(cmd) = &ctx.cursor_context.current_command {
    ///             let subcmd_suggestions = self.registry
    ///                 .get_subcommand_suggestions(cmd, ctx.get_word_before_cursor());
    ///             suggestions.extend(subcmd_suggestions);
    ///         }
    ///
    ///         // 排序并去重
    ///         suggestions.sort_by(|a, b| b.priority.cmp(&a.priority));
    ///         suggestions.dedup_by(|s| s.text.clone());
    ///
    ///         // 返回补全文本列表
    ///         suggestions.into_iter()
    ///             .map(|s| s.text)
    ///             .collect()
    ///     }
    /// }
    /// ```
    fn example3_ide_integration() {}

    /// # 示例4：学习系统训练与反馈循环
    ///
    /// ```rust,no_run
    /// use carpai::api::{create_dev_friendly_auto_engine, ActionType};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = create_dev_friendly_auto_engine();
    ///
    ///     // 模拟用户工作流程
    ///     let workflow = vec![
    ///         (ActionType::FileEdit, "update src/main.rs"),
    ///         (ActionType::GitCommit, "commit changes"),
    ///         (ActionType::BashCommand, "cargo test"),
    ///         (ActionType::BashCommand, "cargo build --release"),
    ///     ];
    ///
    ///     for (action_type, description) in workflow.iter() {
    ///         let ctx = carpai::ToolContext::new(action_type.clone(), description);
    ///
    ///         // 获取决策
    ///         let decision = engine
    ///             .should_auto_approve(action_type, description, &ctx)
    ///             .await;
    ///
    ///         // 模拟用户反馈（实际应用中来自UI交互）
    ///         let was_correct = matches!(
    ///             decision,
    ///             AutoApprovalDecision::AutoApprove(_)
    ///         );
    ///
    ///         // 提供反馈以改进模型
    ///         engine.provide_feedback(action_type, description, was_correct).await;
    ///     }
    ///
    ///     // 导出学习数据以便持久化
    ///     let learning_data = engine.export_learning_data().await;
    ///     std::fs::write("carpai_learning.json", &learning_data)
    ///         .expect("Failed to save learning data");
    ///
    ///     // 打印统计信息
    ///     let stats = engine.get_statistics();
    ///     println!("📊 决策统计:");
    ///     println!("   总计: {}", stats.total_decisions);
    ///     println!("   自动批准: {}", stats.auto_approved);
    ///     println!("   平均置信度: {:.1}%", stats.avg_confidence * 100.0);
    /// }
    /// ```
    fn example4_learning_loop() {}
}

#[cfg(test)]
mod api_tests {
    use super::*;

    #[test]
    fn test_create_default_engine() {
        let engine = create_default_auto_engine();
        let config = engine.get_config();
        assert!(!config.enabled); // 默认禁用
    }

    #[test]
    fn test_create_secure_engine() {
        let engine = create_secure_auto_engine();
        let config = engine.get_config();
        assert!(config.enabled);
        assert!((config.approval_threshold - 0.95).abs() < f64::EPSILON);
        assert!(!config.auto_accept_safe);
        assert_eq!(config.max_auto_actions, 10);
    }

    #[test]
    fn test_create_dev_engine() {
        let engine = create_dev_friendly_auto_engine();
        let config = engine.get_config();
        assert!(config.enabled);
        assert!((config.approval_threshold - 0.7).abs() < f64::EPSILON);
        assert!(config.auto_accept_safe);
    }

    #[tokio::test]
    async fn test_safe_operation_auto_approval() {
        let engine = create_dev_friendly_auto_engine();
        
        let decision = engine.should_auto_approve(
            &ActionType::FileRead,
            "read file",
            &ToolContext::new(ActionType::FileRead, "read file").with_file("README.md"),
        ).await;

        assert!(decision.is_auto_approved());
    }

    #[test]
    fn test_bash_completer_creation() {
        let (parser, registry) = create_bash_completer();
        
        // 应该能正常创建
        let suggestions = parser.get_suggestions(&CompletionContext::new("git ", 4));
        assert!(!suggestions.is_empty());
        
        // 注册表应包含50+命令
        assert!(registry.list_commands().len() >= 50);
    }

    #[test]
    fn test_convenience_function() {
        let suggestions = get_completion_suggestions("docker ", 7);
        assert!(!suggestions.is_empty());
        
        // 应该包含Docker子命令
        let has_ps = suggestions.iter().any(|s| s.text.contains("ps"));
        let has_run = suggestions.iter().any(|s| s.text.contains("run"));
        assert!(has_ps && has_run);
    }
}
