//! # Auto Mode核心引擎
//!
//! 实现智能决策引擎，整合：
//! - YOLO分类器（规则+ML混合）
//! - 置信度模型（历史数据训练）
//! - 安全护栏（敏感词检测）
//! - 学习系统（模式识别）

use crate::auto_mode::{
    AutoApprovalDecision, AutoModeConfig, AutoModeStats, ActionType, ToolContext,
};
use crate::auto_mode::confidence::ConfidenceModel;
use crate::auto_mode::learning::LearningSystem;
use crate::auto_mode::safety::SafetyGuardrail;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;

/// Auto Mode核心引擎
pub struct AutoModeEngine {
    /// 配置
    config: Arc<RwLock<AutoModeConfig>>,

    /// 置信度模型
    confidence_model: Arc<Mutex<ConfidenceModel>>,

    /// 安全护栏
    safety_guard: SafetyGuardrail,

    /// 学习系统
    learning_system: Arc<Mutex<LearningSystem>>,

    /// 统计信息
    stats: Arc<RwLock<AutoModeStats>>,

    /// 审计日志
    #[cfg(feature = "audit")]
    audit_log: Arc<Mutex<Vec<AuditEntry>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuditEntry {
    timestamp: chrono::DateTime<chrono::Utc>,
    action_type: ActionType,
    description: String,
    decision: AutoApprovalDecision,
    confidence: Option<f64>,
    processing_time_ms: u64,
}

impl AutoModeEngine {
    /// 创建新的Auto Mode引擎
    pub fn new(config: AutoModeConfig) -> Self {
        let safety_guard = SafetyGuardrail::new(&config);

        Self {
            config: Arc::new(RwLock::new(config)),
            confidence_model: Arc::new(Mutex::new(ConfidenceModel::default())),
            safety_guard,
            learning_system: Arc::new(Mutex::new(LearningSystem::default())),
            stats: Arc::new(RwLock::new(AutoModeStats::default())),
            #[cfg(feature = "audit")]
            audit_log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 带默认配置创建引擎
    pub fn with_defaults() -> Self {
        Self::new(AutoModeConfig::default())
    }

    /// 核心决策函数 - 判断是否应该自动批准操作
    pub async fn should_auto_approve(
        &self,
        action_type: &ActionType,
        description: &str,
        context: &ToolContext,
    ) -> AutoApprovalDecision {
        let start = std::time::Instant::now();

        // 1. 检查模式是否启用
        {
            let cfg = self.config.read().unwrap_or_else(|e| e.into_inner());
            if !cfg.enabled {
                return AutoApprovalDecision::ManualReview;
            }

            // 检查是否超过最大自动操作数
            if cfg.current_auto_actions >= cfg.max_auto_actions {
                return AutoApprovalDecision::RequiresConfirmation(
                    "已达到最大自动操作限制".to_string(),
                );
            }
        }

        // 2. 敏感词检测（安全护栏）
        if let Some(block_reason) = self.safety_guard.contains_sensitive_word(description) {
            self.record_stats(&AutoApprovalDecision::RequiresConfirmation(
                block_reason.clone(),
            ));
            
            // 检查是否为完全阻止的操作
            if self.safety_guard.is_blocked(description) {
                let decision = AutoApprovalDecision::Blocked(format!(
                    "⛔ 操作被安全护栏阻止: {}",
                    block_reason
                ));
                self.log_audit(action_type, description, &decision, None, start);
                return decision;
            }

            let decision = AutoApprovalDecision::RequiresConfirmation(format!(
                "⚠️ 检测到敏感操作: {}",
                block_reason
            ));
            self.log_audit(action_type, description, &decision, None, start);
            return decision;
        }

        // 3. 评估风险等级
        let risk_level = self.safety_guard.assess_risk(description);
        match risk_level {
            crate::auto_mode::safety::RiskLevel::Critical => {
                let decision = AutoApprovalDecision::Blocked(
                    "🔴 操作风险等级: Critical (致命)".to_string()
                );
                self.log_audit(action_type, description, &decision, None, start);
                return decision;
            }
            crate::auto_mode::safety::RiskLevel::High => {
                let decision = AutoApprovalDecision::RequiresConfirmation(
                    "🔴 操作风险等级: High (高)".to_string()
                );
                self.log_audit(action_type, description, &decision, None, start);
                return decision;
            }
            _ => {}
        }

        // 4. 学习模式匹配
        {
            let mut learning = self.learning_system.lock().await;
            if let Some(pattern) = learning.match_pattern(action_type, description) {
                // 计算置信度
                let mut model = self.confidence_model.lock().await;
                let confidence = model.calculate(&pattern, context);

                drop(model); // 释放锁

                let cfg = self.config.read().unwrap_or_else(|e| e.into_inner());
                
                if confidence >= cfg.approval_threshold {
                    // 自动批准
                    if cfg.enable_learning {
                        learning.record_decision(action_type, description, 
                            &AutoApprovalDecision::AutoApprove(format!(
                                "✅ 置信度 {:.1}% ≥ 阈值 {:.1}%",
                                confidence * 100.0,
                                cfg.approval_threshold * 100.0
                            ))
                        );
                    }

                    self.increment_auto_actions();

                    let decision = AutoApprovalDecision::AutoApprove(format!(
                        "✅ 学习模式匹配 + 置信度 {:.1}%",
                        confidence * 100.0
                    ));
                    
                    {
                        let mut stats = self.stats.write().unwrap_or_else(|e| e.into_inner());
                        stats.learning_pattern_hits += 1;
                        stats.auto_approved += 1;
                        stats.total_decisions += 1;
                        stats.avg_confidence = (stats.avg_confidence * (stats.total_decisions - 1) as f64
                            + confidence) / stats.total_decisions as f64;
                    }

                    self.log_audit(action_type, description, &decision, Some(confidence), start);
                    return decision;
                } else {
                    // 置信度不足，建议但需审核
                    if cfg.enable_learning {
                        learning.record_decision(action_type, description,
                            &AutoApprovalDecision::SuggestApprove {
                                reason: format!("置信度 {:.1}% < 阈值 {:.1}%",
                                    confidence * 100.0, cfg.approval_threshold * 100.0),
                                confidence,
                            }
                        );
                    }

                    let decision = AutoApprovalDecision::SuggestApprove {
                        reason: format!(
                            "📊 置信度 {:.1}% < 阈值 {:.1}%, 建议审核",
                            confidence * 100.0,
                            cfg.approval_threshold * 100.0
                        ),
                        confidence,
                    };

                    {
                        let mut stats = self.stats.write().unwrap_or_else(|e| e.into_inner());
                        stats.required_confirmation += 1;
                        stats.total_decisions += 1;
                    }

                    self.log_audit(action_type, description, &decision, Some(confidence), start);
                    return decision;
                }
            }
        }

        // 5. 安全操作白名单检查
        {
            let cfg = self.config.read().unwrap_or_else(|e| e.into_inner());
            if cfg.auto_accept_safe && cfg.safe_action_types.contains(action_type) {
                // 对于BashCommand，需要额外检查命令内容
                if *action_type == ActionType::BashCommand {
                    if self.is_safe_bash_command(description) {
                        self.increment_auto_actions();

                        let decision = AutoApprovalDecision::AutoApprove(
                            "✅ 安全操作白名单".to_string()
                        );

                        {
                            let mut stats = self.stats.write().unwrap_or_else(|e| e.into_inner());
                            stats.auto_approved += 1;
                            stats.total_decisions += 1;
                        }

                        self.log_audit(action_type, description, &decision, Some(0.95), start);
                        return decision;
                    }
                } else {
                    self.increment_auto_actions();

                    let decision = AutoApprovalDecision::AutoApprove(
                        "✅ 安全操作类型".to_string()
                    );

                    {
                        let mut stats = self.stats.write().unwrap_or_else(|e| e.into_inner());
                        stats.auto_approved += 1;
                        stats.total_decisions += 1;
                    }

                    self.log_audit(action_type, description, &decision, Some(0.95), start);
                    return decision;
                }
            }
        }

        // 6. 默认：人工审核
        let decision = AutoApprovalDecision::ManualReview;

        {
            let mut stats = self.stats.write().unwrap_or_else(|e| e.into_inner());
            stats.manual_reviews += 1;
            stats.total_decisions += 1;
        }

        self.log_audit(action_type, description, &decision, None, start);
        decision
    }

    /// 判断是否为安全的bash命令
    fn is_safe_bash_command(&self, command: &str) -> bool {
        let cmd_lower = command.to_lowercase();

        // 检查自动批准模式
        let cfg = self.config.read().unwrap_or_else(|e| e.into_inner());
        for pattern in &cfg.auto_approve_patterns {
            if cmd_lower.contains(&pattern.to_lowercase()) {
                return true;
            }
        }

        // 只读命令白名单
        let safe_commands = [
            "ls", "pwd", "echo", "cat", "which", "whereis", "type",
            "date", "whoami", "id", "uname", "hostname", "env",
            "git status", "git log", "git diff", "git branch",
            "--help", "-h", "--version", "-v",
            "npm list", "npm --version",
            "docker ps", "docker images",
            "kubectl get", "kubectl describe",
        ];

        safe_commands.iter().any(|&safe| cmd_lower.starts_with(safe))
    }

    /// 增加自动操作计数
    fn increment_auto_actions(&self) {
        let mut cfg = self.config.write().unwrap_or_else(|e| e.into_inner());
        cfg.current_auto_actions += 1;
    }

    /// 重置自动操作计数（每次用户交互后调用）
    pub fn reset_auto_action_count(&self) {
        let mut cfg = self.config.write().unwrap_or_else(|e| e.into_inner());
        cfg.current_auto_actions = 0;
    }

    /// 记录统计信息
    fn record_stats(&self, decision: &AutoApprovalDecision) {
        let mut stats = self.stats.write().unwrap_or_else(|e| e.into_inner());
        stats.total_decisions += 1;

        match decision {
            AutoApprovalDecision::AutoApprove(_) => stats.auto_approved += 1,
            AutoApprovalDecision::RequiresConfirmation(_) => {
                stats.required_confirmation += 1;
                stats.sensitive_word_triggers += 1;
            }
            AutoApprovalDecision::ManualReview => stats.manual_reviews += 1,
            AutoApprovalDecision::Blocked(_) => stats.blocked += 1,
            _ => {}
        }
    }

    /// 记录审计日志
    fn log_audit(
        &self,
        _action_type: &ActionType,
        _description: &str,
        _decision: &AutoApprovalDecision,
        _confidence: Option<f64>,
        _start: std::time::Instant,
    ) {
        #[cfg(feature = "audit")]
        {
            let cfg = self.config.read().unwrap_or_else(|e| e.into_inner());
            if cfg.enable_audit_log {
                let entry = AuditEntry {
                    timestamp: chrono::Utc::now(),
                    action_type: action_type.clone(),
                    description: description.to_string(),
                    decision: decision.clone(),
                    confidence,
                    processing_time_ms: start.elapsed().as_millis() as u64,
                };

                let mut log = self.audit_log.try_lock();
                if let Ok(ref mut log) = log {
                    log.push(entry);
                }
            }
        }
    }

    /// 获取统计信息
    pub fn get_statistics(&self) -> AutoModeStats {
        self.stats.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 获取配置
    pub fn get_config(&self) -> AutoModeConfig {
        self.config.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 更新配置
    pub fn update_config<F>(&mut self, updater: F)
    where
        F: FnOnce(&mut AutoModeConfig),
    {
        let cfg = {
            let mut cfg = self.config.write().unwrap_or_else(|e| e.into_inner());
            updater(&mut cfg);
            cfg.clone()
        };
        
        drop(cfg);
        
        // 重新初始化安全护栏（如果配置变更）
        self.safety_guard.refresh_config(&self.get_config());
    }

    /// 启用/禁用Auto Mode
    pub fn set_enabled(&mut self, enabled: bool) {
        self.update_config(|cfg| cfg.enabled = enabled);
    }

    /// 训练置信度模型（提供反馈）
    pub async fn provide_feedback(
        &self,
        action_type: &ActionType,
        description: &str,
        was_correct: bool,
    ) {
        let mut learning = self.learning_system.lock().await;
        learning.provide_feedback(action_type, description, was_correct);

        // 如果开启学习，更新置信度模型
        {
            let cfg = self.config.read().unwrap_or_else(|e| e.into_inner());
            if cfg.enable_learning {
                let mut model = self.confidence_model.lock().await;
                model.record_decision(
                    action_type,
                    &ToolContext::new(action_type.clone(), description),
                    was_correct,
                );
            }
        }
    }

    /// 导出学习数据（用于持久化）
    pub async fn export_learning_data(&self) -> String {
        let learning = self.learning_system.lock().await;
        serde_json::to_string_pretty(&*learning).unwrap_or_default()
    }

    /// 导入学习数据
    pub async fn import_learning_data(&self, data: &str) -> Result<(), String> {
        match serde_json::from_str(data) {
            Ok(imported) => {
                let mut learning = self.learning_system.lock().await;
                *learning = imported;
                Ok(())
            }
            Err(e) => Err(format!("导入失败: {}", e))
        }
    }

    /// 获取审计日志
    #[cfg(feature = "audit")]
    pub async fn get_audit_log(&self, limit: usize) -> Vec<AuditEntry> {
        let log = self.audit_log.lock().await;
        log.iter().rev().take(limit).cloned().collect()
    }

    /// 清空审计日志
    #[cfg(feature = "audit")]
    pub fn clear_audit_log(&self) {
        let mut log = self.audit_log.blocking_lock();
        log.clear();
    }
}

// ==========================================
// 单元测试
// ==========================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_engine_disabled_mode() {
        let engine = AutoModeEngine::with_defaults();
        engine.set_enabled(false);

        let ctx = ToolContext::new(ActionType::FileRead, "read file");
        let decision = engine.should_auto_approve(
            &ActionType::FileRead,
            "read config.toml",
            &ctx,
        ).await;

        assert!(matches!(decision, AutoApprovalDecision::ManualReview));
    }

    #[tokio::test]
    async fn test_safe_operation_auto_approve() {
        let mut config = AutoModeConfig::default();
        config.enabled = true;
        config.auto_accept_safe = true;

        let engine = AutoModeEngine::new(config);

        let ctx = ToolContext::new(ActionType::FileRead, "read file");
        let decision = engine.should_auto_approve(
            &ActionType::FileRead,
            "read README.md",
            &ctx,
        ).await;

        assert!(decision.is_auto_approved());
    }

    #[tokio::test]
    async fn test_sensitive_word_detection() {
        let mut config = AutoModeConfig::default();
        config.enabled = true;

        let engine = AutoModeEngine::new(config);

        let ctx = ToolContext::new(ActionType::BashCommand, "delete files");
        let decision = engine.should_auto_approve(
            &ActionType::BashCommand,
            "rm -rf /tmp/data",
            &ctx,
        ).await;

        assert!(decision.requires_confirmation());
    }

    #[tokio::test]
    async fn test_blocked_command() {
        let mut config = AutoModeConfig::default();
        config.enabled = true;

        let engine = AutoModeEngine::new(config);

        let ctx = ToolContext::new(ActionType::BashCommand, "dangerous");
        let decision = engine.should_auto_approve(
            &ActionType::BashCommand,
            "rm -rf /",
            &ctx,
        ).await;

        assert!(decision.is_blocked());
    }

    #[tokio::test]
    async fn test_max_auto_actions_limit() {
        let mut config = AutoModeConfig::default();
        config.enabled = true;
        config.max_auto_actions = 2;

        let engine = AutoModeEngine::new(config);

        // 第一次应该通过
        let ctx = ToolContext::new(ActionType::FileRead, "read");
        let decision1 = engine.should_auto_approve(
            &ActionType::FileRead,
            "file1.txt",
            &ctx,
        ).await;
        assert!(decision1.is_auto_approved());

        // 第二次应该通过
        let decision2 = engine.should_auto_approve(
            &ActionType::FileRead,
            "file2.txt",
            &ctx,
        ).await;
        assert!(decision2.is_auto_approved());

        // 第三次应该被限制
        let decision3 = engine.should_auto_approve(
            &ActionType::FileRead,
            "file3.txt",
            &ctx,
        ).await;
        assert!(decision3.requires_confirmation());
    }

    #[tokio::test]
    async fn test_safe_bash_commands() {
        let mut config = AutoModeConfig::default();
        config.enabled = true;
        config.auto_accept_safe = true;

        let engine = AutoModeEngine::new(config);

        let safe_commands = vec![
            "git status",
            "ls -la",
            "pwd",
            "echo hello",
            "which python",
        ];

        for cmd in safe_commands {
            let ctx = ToolContext::new(ActionType::BashCommand, cmd);
            let decision = engine.should_auto_approve(
                &ActionType::BashCommand,
                cmd,
                &ctx,
            ).await;
            
            assert!(decision.is_auto_approved(), 
                "Safe command '{}' should be auto-approved", cmd);
        }
    }

    #[tokio::test]
    async fn test_statistics_tracking() {
        let mut config = AutoModeConfig::default();
        config.enabled = true;

        let engine = AutoModeEngine::new(config);

        // 执行多个决策
        for _ in 0..10 {
            let ctx = ToolContext::new(ActionType::FileRead, "read");
            let _ = engine.should_auto_approve(
                &ActionType::FileRead,
                "file.txt",
                &ctx,
            ).await;
        }

        let stats = engine.get_statistics();
        assert_eq!(stats.total_decisions, 10);
        assert!(stats.auto_approved > 0);
    }

    #[tokio::test]
    async fn test_config_update() {
        let engine = AutoModeEngine::with_defaults();

        // 初始状态：禁用
        assert!(!engine.get_config().enabled);

        // 更新配置
        engine.set_enabled(true);
        assert!(engine.get_config().enabled);

        // 自定义更新
        engine.update_config(|cfg| {
            cfg.approval_threshold = 0.9;
            cfg.max_auto_actions = 100;
        });

        let cfg = engine.get_config();
        assert!((cfg.approval_threshold - 0.9).abs() < f64::EPSILON);
        assert_eq!(cfg.max_auto_actions, 100);
    }

    #[tokio::test]
    async fn test_learning_feedback_loop() {
        let mut config = AutoModeConfig::default();
        config.enabled = true;
        config.enable_learning = true;

        let engine = AutoModeEngine::new(config);

        // 提供正面反馈
        for _ in 0..5 {
            engine.provide_feedback(
                &ActionType::GitCommit,
                "commit changes",
                true,
            ).await;
        }

        // 验证学习数据已记录
        let data = engine.export_learning_data().await;
        assert!(!data.is_empty(), "Learning data should not be empty");
    }

    #[tokio::test]
    async fn test_risk_assessment_integration() {
        let mut config = AutoModeConfig::default();
        config.enabled = true;

        let engine = AutoModeEngine::new(config);

        // 测试高风险操作
        let high_risk_operations = vec![
            "deploy --force production",
            "DROP TABLE users",
            "git push --force origin main",
        ];

        for op in high_risk_operations {
            let ctx = ToolContext::new(ActionType::BashCommand, op);
            let decision = engine.should_auto_approve(
                &ActionType::BashCommand,
                op,
                &ctx,
            ).await;

            assert!(decision.requires_confirmation() || decision.is_blocked(),
                "High risk operation '{}' should require confirmation or be blocked", op);
        }
    }
}
