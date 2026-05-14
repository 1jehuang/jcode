//! # 学习系统
//!
//! 实现基于模式匹配的在线学习机制：
//! - **决策记录** - 记录每次用户审批/拒绝
//! - **模式识别** - 自动提取操作模式
//! - **置信度调整** - 基于反馈动态调整
//! - **持久化支持** - 导出/导入学习数据
//!
//! ## 学习算法
//!
//! 1. **初始状态**: 置信度 = 0.5 (不确定)
//! 2. **正面反馈** (+1): confidence += learning_rate * (1 - confidence)
//! 3. **负面反馈** (-1): confidence -= learning_rate * confidence
//! 4. **时间衰减**: confidence *= decay_factor (每天)
//!
//! 学习率默认0.1，衰减因子0.995

use crate::auto_mode::{ActionType, AutoApprovalDecision};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// 学习系统核心结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningSystem {
    /// 已学习的模式列表
    patterns: Vec<LearnedPattern>,
    
    /// 决策历史记录
    decision_history: Vec<DecisionRecord>,
    
    /// 配置参数
    config: LearningConfig,
    
    /// 全局统计
    stats: LearningStats,
}

/// 学习配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningConfig {
    /// 学习率 (0.0-1.0)
    pub learning_rate: f64,
    
    /// 时间衰减因子 (每天)
    pub daily_decay_factor: f64,
    
    /// 最小置信度阈值
    pub min_confidence: f64,
    
    /// 最大存储的模式数
    pub max_patterns: usize,
    
    /// 最大历史记录数
    pub max_history_size: usize,
    
    /// 模式匹配阈值 (相似度)
    pub pattern_similarity_threshold: f64,
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            learning_rate: 0.1,
            daily_decay_factor: 0.995,
            min_confidence: 0.3,
            max_patterns: 10000,
            max_history_size: 50000,
            pattern_similarity_threshold: 0.7,
        }
    }
}

/// 已学习的模式
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedPattern {
    /// 模式ID
    pub id: String,
    
    /// 操作类型
    pub action_type: ActionType,
    
    /// 模式正则表达式
    #[serde(with = "serde_regex")]
    pub pattern_regex: Regex,
    
    /// 模式的规范化描述
    pub normalized_description: String,
    
    /// 批准次数
    pub approval_count: u64,
    
    /// 拒绝次数
    pub rejection_count: u64,
    
    /// 当前置信度 (0.0-1.0)
    pub confidence: f64,
    
    /// 平均处理时间 (ms)
    pub avg_processing_time_ms: f64,
    
    /// 首次学习时间
    pub first_seen: chrono::DateTime<chrono::Utc>,
    
    /// 最后更新时间
    pub last_updated: chrono::DateTime<chrono::Utc>,
    
    /// 用户标签（可选）
    pub tags: Vec<String>,
}

/// 决策记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    /// 记录ID
    pub id: String,
    
    /// 时间戳
    pub timestamp: chrono::DateTime<chrono::Utc>,
    
    /// 操作类型
    pub action_type: ActionType,
    
    /// 操作描述
    pub description: String,
    
    /// 系统决策
    pub system_decision: AutoApprovalDecision,
    
    /// 用户最终决定
    pub user_decision: UserDecision,
    
    /// 处理耗时 (ms)
    pub processing_time_ms: u64,
    
    /// 匹配到的模式ID (如果有)
    pub matched_pattern_id: Option<String>,
    
    /// 上下文信息
    pub context_info: HashMap<String, String>,
}

/// 用户决策枚举
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserDecision {
    Approved,
    Rejected,
    Modified,
    Skipped,
}

/// 反馈类型
#[derive(Debug, Clone)]
pub enum Feedback {
    Positive,   // 👍 正确的自动批准
    Negative,   // 👎 错误的自动批准（应该拒绝）
    FalseNegative, // ❌ 错误的拒绝（应该批准）
    Neutral,    // ➖ 中性
}

/// 学习统计信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LearningStats {
    /// 总决策数
    pub total_decisions: u64,
    
    /// 总模式数
    pub total_patterns: usize,
    
    /// 高置信度模式数 (>0.85)
    pub high_confidence_patterns: usize,
    
    /// 低置信度模式数 (<0.5)
    pub low_confidence_patterns: usize,
    
    /// 正面反馈数
    pub positive_feedback_count: u64,
    
    /// 负面反馈数
    pub negative_feedback_count: u64,
    
    /// 平均准确率 (估算)
    pub estimated_accuracy: f64,
    
    /// 学习天数
    pub days_of_learning: u64,
}

mod serde_regex {
    use regex::Regex;
    use serde::{self, Deserialize, Serializer};
    
    pub fn serialize<S>(regex: &Regex, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(regex.as_str())
    }
    
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Regex, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Regex::new(&s).map_err(serde::de::Error::custom)
    }
}

impl Default for LearningSystem {
    fn default() -> Self {
        Self {
            patterns: Vec::new(),
            decision_history: Vec::new(),
            config: LearningConfig::default(),
            stats: LearningStats::default(),
        }
    }
}

impl LearningSystem {
    /// 创建新的学习系统
    pub fn new(config: LearningConfig) -> Self {
        Self {
            patterns: Vec::new(),
            decision_history: Vec::new(),
            config,
            stats: LearningStats::default(),
        }
    }

    /// 记录决策用于学习
    pub fn record_decision(
        &mut self,
        action_type: &ActionType,
        description: &str,
        decision: &AutoApprovalDecision,
    ) {
        let record_id = Uuid::new_v4().to_string();
        
        // 尝试匹配已有模式
        let matched_pattern_id = self.find_or_create_pattern(action_type, description);
        
        let record = DecisionRecord {
            id: record_id,
            timestamp: chrono::Utc::now(),
            action_type: action_type.clone(),
            description: description.to_string(),
            system_decision: decision.clone(),
            user_decision: UserDecision::Skipped, // 默认，等待用户反馈
            processing_time_ms: 0,
            matched_pattern_id,
            context_info: HashMap::new(),
        };

        self.decision_history.push(record);

        // 限制历史记录大小
        if self.decision_history.len() > self.config.max_history_size {
            let remove_count = self.decision_history.len() - self.config.max_history_size;
            self.decision_history.drain(0..remove_count);
        }

        // 更新统计
        self.stats.total_decisions += 1;
    }

    /// 提供用户反馈
    pub fn provide_feedback(
        &mut self,
        action_type: &ActionType,
        description: &str,
        was_correct: bool,
    ) {
        // 找到最近的匹配记录
        if let Some(record) = self.decision_history.iter_mut().rev()
            .find(|r| r.action_type == *action_type && r.description == *description)
        {
            record.user_decision = if was_correct {
                UserDecision::Approved
            } else {
                UserDecision::Rejected
            };

            // 更新匹配模式的置信度
            if let Some(pattern_id) = &record.matched_pattern_id {
                if let Some(pattern) = self.patterns.iter_mut()
                    .find(|p| p.id == *pattern_id)
                {
                    self.update_confidence(pattern, was_correct);
                }
            }

            // 更新统计
            if was_correct {
                self.stats.positive_feedback_count += 1;
            } else {
                self.stats.negative_feedback_count += 1;
            }
        }
    }

    /// 匹配已学习的模式
    pub fn match_pattern(
        &self,
        action_type: &ActionType,
        description: &str,
    ) -> Option<&LearnedPattern> {
        self.patterns.iter()
            .filter(|p| p.action_type == *action_type && p.confidence >= self.config.min_confidence)
            .find(|p| p.pattern_regex.is_match(description))
    }

    /// 获取所有高置信度模式
    pub fn get_high_confidence_patterns(&self, threshold: f64) -> Vec<&LearnedPattern> {
        self.patterns.iter()
            .filter(|p| p.confidence >= threshold)
            .collect()
    }

    /// 获取操作类型的所有模式
    pub fn get_patterns_for_action(&self, action_type: &ActionType) -> Vec<&LearnedPattern> {
        self.patterns.iter()
            .filter(|p| p.action_type == *action_type)
            .collect()
    }

    /// 应用时间衰减（应该定期调用，如每天一次）
    pub fn apply_time_decay(&mut self) {
        let now = chrono::Utc::now();
        
        for pattern in &mut self.patterns {
            let days_since_update = (now - pattern.last_updated).num_days() as f64;
            
            if days_since_update > 0.0 {
                let decay = self.config.daily_decay_factor.powi(days_since_update as i32);
                pattern.confidence *= decay;
                
                // 确保不低于最小值
                if pattern.confidence < self.config.min_confidence {
                    pattern.confidence = self.config.min_confidence;
                }
                
                pattern.last_updated = now;
            }
        }

        self.stats.days_of_learning += 1;
        
        // 清理低置信度的旧模式
        self.cleanup_low_confidence_patterns();
    }

    /// 清理低置信度模式
    fn cleanup_low_confidence_patterns(&mut self) {
        let before = self.patterns.len();
        
        self.patterns.retain(|p| {
            // 保留条件：
            // 1. 置信度足够高
            // 2. 或者有足够的样本数据
            p.confidence >= self.config.min_confidence 
                || (p.approval_count + p.rejection_count) >= 10
        });

        let removed = before - self.patterns.len();
        if removed > 0 {
            tracing::debug!("Cleaned up {} low-confidence patterns", removed);
        }
    }

    /// 更新模式置信度
    fn update_confidence(&mut self, pattern: &mut LearnedPattern, positive: bool) {
        let lr = self.config.learning_rate;
        
        if positive {
            // 正面反馈：向1.0移动
            pattern.confidence += lr * (1.0 - pattern.confidence);
            pattern.approval_count += 1;
        } else {
            // 负面反馈：向0.0移动
            pattern.confidence -= lr * pattern.confidence;
            pattern.rejection_count += 1;
        }

        // 确保在有效范围内
        pattern.confidence = pattern.confidence.max(0.0).min(1.0);
        pattern.last_updated = chrono::Utc::now();
    }

    /// 查找或创建模式
    fn find_or_create_pattern(
        &mut self,
        action_type: &ActionType,
        description: &str,
    ) -> Option<String> {
        // 先尝试精确匹配
        if let Some(pos) = self.patterns.iter()
            .position(|p| p.action_type == *action_type && p.pattern_regex.is_match(description))
        {
            return Some(self.patterns[pos].id.clone());
        }

        // 创建新模式
        if self.patterns.len() < self.config.max_patterns {
            let normalized = self.normalize_description(description);
            
            // 尝试创建正则（简化版）
            let pattern_str = self.create_pattern_from_description(&normalized);
            
            if let Ok(regex) = Regex::new(&pattern_str) {
                let now = chrono::Utc::now();
                let pattern = LearnedPattern {
                    id: Uuid::new_v4().to_string(),
                    action_type: action_type.clone(),
                    pattern_regex: regex,
                    normalized_description: normalized,
                    approval_count: 0,
                    rejection_count: 0,
                    confidence: 0.5, // 初始不确定性
                    avg_processing_time_ms: 0.0,
                    first_seen: now,
                    last_updated: now,
                    tags: vec![],
                };

                let id = pattern.id.clone();
                self.patterns.push(pattern);
                self.stats.total_patterns = self.patterns.len();
                
                return Some(id);
            }
        }

        None
    }

    /// 规范化描述文本
    fn normalize_description(&self, desc: &str) -> String {
        desc.to_lowercase()
            // 替换空格和特殊字符为通配符
            .replace([' ', '-', '_', '/', '\\', '.'], ".")
            // 移除括号
            .replace(['(', ')'], "")
            // 替换数字为占位符
            .replace(|c: char| c.is_numeric(), "\\d")
            // 替换引号内容为通配符
            .replace(|c: char| c == '\'' || c == '"', ".+")
            // 清理多余的点号
            .replace("..+", ".+")
            .trim()
            .to_string()
    }

    /// 从描述创建正则模式
    fn create_pattern_from_description(&self, normalized: &str) -> String {
        format!(r"(?i){}", normalized)
    }

    /// 获取统计信息
    pub fn get_statistics(&self) -> LearningStats {
        // 计算高/低置信度模式数量
        let high_conf = self.patterns.iter()
            .filter(|p| p.confidence > 0.85)
            .count();
        
        let low_conf = self.patterns.iter()
            .filter(|p| p.confidence < 0.5)
            .count();

        // 估算准确率
        let total_feedback = self.stats.positive_feedback_count + self.stats.negative_feedback_count;
        let accuracy = if total_feedback > 0 {
            self.stats.positive_feedback_count as f64 / total_feedback as f64
        } else {
            0.0
        };

        LearningStats {
            total_patterns: self.patterns.len(),
            high_confidence_patterns: high_conf,
            low_confidence_patterns: low_conf,
            estimated_accuracy: accuracy,
            ..self.stats.clone()
        }
    }

    /// 获取最近N条决策记录
    pub fn get_recent_decisions(&self, limit: usize) -> Vec<&DecisionRecord> {
        self.decision_history.iter()
            .rev()
            .take(limit)
            .collect()
    }

    /// 导出为JSON字符串
    pub fn export_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// 从JSON导入
    pub fn import_json(data: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(data)
    }

    /// 合并另一个学习系统的数据
    pub fn merge_with(&mut self, other: &LearningSystem) {
        // 合并模式
        for other_pattern in &other.patterns {
            if let Some(existing) = self.patterns.iter_mut()
                .find(|p| p.normalized_description == other_pattern.normalized_description 
                    && p.action_type == other_pattern.action_type)
            {
                // 合并统计数据
                existing.approval_count += other_pattern.approval_count;
                existing.rejection_count += other_pattern.rejection_count;
                existing.confidence = (existing.confidence + other_pattern.confidence) / 2.0;
            } else if self.patterns.len() < self.config.max_patterns {
                self.patterns.push(other_pattern.clone());
            }
        }

        // 合并历史记录
        self.decision_history.extend(other.decision_history.iter().cloned());

        // 限制大小
        if self.decision_history.len() > self.config.max_history_size {
            let remove_count = self.decision_history.len() - self.config.max_history_size;
            self.decision_history.drain(0..remove_count);
        }

        // 更新统计
        self.stats.total_decisions = self.decision_history.len() as u64;
        self.stats.total_patterns = self.patterns.len();
    }

    /// 重置所有学习数据
    pub fn reset(&mut self) {
        self.patterns.clear();
        self.decision_history.clear();
        self.stats = LearningStats::default();
    }
}

// ==========================================
// 单元测试
// ==========================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_system() -> LearningSystem {
        LearningSystem::default()
    }

    #[test]
    fn test_record_decision() {
        let mut system = create_test_system();
        
        system.record_decision(
            &ActionType::GitCommit,
            "commit changes",
            &AutoApprovalDecision::AutoApprove("test".to_string()),
        );

        assert_eq!(system.get_statistics().total_decisions, 1);
        assert_eq!(system.patterns.len(), 1); // 应该自动创建模式
    }

    #[test]
    fn test_positive_feedback_increases_confidence() {
        let mut system = create_test_system();
        
        // 记录决策
        system.record_decision(
            &ActionType::FileRead,
            "read config file",
            &AutoApprovalDecision::AutoApprove("safe".to_string()),
        );

        // 提供正面反馈
        system.provide_feedback(
            &ActionType::FileRead,
            "read config file",
            true,
        );

        // 检查置信度是否增加
        let pattern = system.match_pattern(&ActionType::FileRead, "read config file");
        assert!(pattern.is_some());
        assert!(pattern.unwrap().confidence > 0.5); // 应该大于初始值0.5
    }

    #[test]
    fn test_negative_feedback_decreases_confidence() {
        let mut system = create_test_system();
        
        system.record_decision(
            &ActionType::BashCommand,
            "rm -rf temp",
            &AutoApprovalDecision::AutoApprove("wrong".to_string()),
        );

        // 提供负面反馈（错误的自动批准）
        system.provide_feedback(
            &ActionType::BashCommand,
            "rm -rf temp",
            false,
        );

        let pattern = system.match_pattern(&ActionType::BashCommand, "rm -rf temp");
        assert!(pattern.is_some());
        assert!(pattern.unwrap().confidence < 0.5); // 应该小于初始值0.5
    }

    #[test]
    fn test_pattern_matching() {
        let mut system = create_test_system();
        
        // 训练几个相似的操作
        for cmd in &["git commit -m 'feat'", "git commit -m 'fix'", "git commit -m 'docs'"] {
            system.record_decision(
                &ActionType::GitCommit,
                cmd,
                &AutoApprovalDecision::AutoApprove("safe".to_string()),
            );
            system.provide_feedback(&ActionType::GitCommit, cmd, true);
        }

        // 新的类似操作应该匹配到已学习的模式
        let matched = system.match_pattern(&ActionType::GitCommit, "git commit -m 'refactor'");
        assert!(matched.is_some(), "Should match learned git commit pattern");
    }

    #[test]
    fn test_statistics_tracking() {
        let mut system = create_test_system();
        
        // 模拟多次决策
        for i in 0..20 {
            let approved = i % 4 != 0; // 75% 批准率
            
            system.record_decision(
                &ActionType::FileEdit,
                &format!("edit file {}", i),
                &AutoApprovalDecision::AutoApprove("test".to_string()),
            );
            
            system.provide_feedback(&ActionType::FileEdit, &format!("edit file {}", i), approved);
        }

        let stats = system.get_statistics();
        assert_eq!(stats.total_decisions, 20);
        assert!(stats.positive_feedback_count > 0);
        assert!(stats.estimated_accuracy > 0.6); // 应该接近75%
    }

    #[test]
    fn test_export_import_roundtrip() {
        let mut system = create_test_system();
        
        // 添加一些数据
        system.record_decision(&ActionType::BashCommand, "ls", &AutoApprovalDecision::AutoApprove("ok".to_string()));
        system.provide_feedback(&ActionType::BashCommand, "ls", true);
        
        // 导出
        let json = system.export_json().expect("Export should succeed");
        assert!(!json.is_empty());
        
        // 导入
        let imported = LearningSystem::import_json(&json).expect("Import should succeed");
        
        // 验证数据一致
        assert_eq!(imported.get_statistics().total_decisions, system.get_statistics().total_decisions);
        assert_eq!(imported.patterns.len(), system.patterns.len());
    }

    #[test]
    fn test_time_decay() {
        let mut system = create_test_system();
        
        // 创建一个高置信度模式
        system.record_decision(&ActionType::FileRead, "read", &AutoApprovalDecision::AutoApprove("ok".to_string()));
        
        // 多次正面反馈提升置信度
        for _ in 0..10 {
            system.provide_feedback(&ActionType::FileRead, "read", true);
        }
        
        let initial_conf = system.match_pattern(&ActionType::FileRead, "read").unwrap().confidence;
        assert!(initial_conf > 0.8);
        
        // 应用时间衰减
        system.apply_time_decay();
        
        let decayed_conf = system.match_pattern(&ActionType::FileRead, "read").unwrap().confidence;
        assert!(decayed_conf < initial_conf, "Confidence should decrease after decay");
    }

    #[test]
    fn test_merge_systems() {
        let mut system1 = create_test_system();
        let mut system2 = create_test_system();
        
        // 系统1的数据
        system1.record_decision(&ActionType::GitPush, "push code", &AutoApprovalDecision::RequiresConfirmation("warn".to_string()));
        
        // 系统2的数据
        system2.record_decision(&ActionType::GitCommit, "commit msg", &AutoApprovalDecision::AutoApprove("ok".to_string()));
        
        // 合并
        system1.merge_with(&system2);
        
        assert_eq!(system1.get_statistics().total_decisions, 2);
        assert!(system1.patterns.len() >= 2);
    }

    #[test]
    fn test_reset_clears_data() {
        let mut system = create_test_system();
        
        // 添加数据
        system.record_decision(&ActionType::FileRead, "file", &AutoApprovalDecision::AutoApprove("ok".to_string()));
        assert!(!system.patterns.is_empty());
        
        // 重置
        system.reset();
        
        assert!(system.patterns.is_empty());
        assert!(system.decision_history.is_empty());
        assert_eq!(system.get_statistics().total_decisions, 0);
    }

    #[test]
    fn test_high_confidence_filtering() {
        let mut system = create_test_system();
        
        // 创建高低不同置信度的模式
        system.record_decision(&ActionType::FileRead, "high conf", &AutoApprovalDecision::AutoApprove("ok".to_string()));
        for _ in 0..15 {
            system.provide_feedback(&ActionType::FileRead, "high conf", true);
        }
        
        system.record_decision(&ActionType::FileRead, "low conf", &AutoApprovalDecision::AutoApprove("ok".to_string()));
        for _ in 0..2 {
            system.provide_feedback(&ActionType::FileRead, "low conf", false);
        }
        
        let high_conf = system.get_high_confidence_patterns(0.7);
        assert!(high_conf.len() >= 1, "Should have at least one high-confidence pattern");
        
        let all_read = system.get_patterns_for_action(&ActionType::FileRead);
        assert!(all_read.len() >= 2, "Should have multiple patterns for FileRead");
    }
}
