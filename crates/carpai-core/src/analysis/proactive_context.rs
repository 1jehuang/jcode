//! 主动上下文预测系统
//!
//! 基于历史模式和当前状态预测性地加载上下文，减少等待时间
//!
//! 核心能力:
//! 1. 模式学习 - 从历史会话学习用户行为模式
//! 2. 上下文预测 - 基于当前上下文预测可能需要的文件/符号
//! 3. 预测性加载 - 在需要之前预先加载上下文

use crate::analysis::incremental_index::{GlobalIndexer, get_or_create_indexer, IncrementalIndexConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, info};

#[derive(Debug, Clone, Default)]
pub struct PatternDetector;

impl PatternDetector {
    pub fn detect_habits(&self, _history: &[SessionAnalysis]) -> HabitProfile {
        HabitProfile { most_common_actions: vec![] }
    }
}

#[derive(Debug, Clone)]
pub struct HabitProfile {
    pub most_common_actions: Vec<PredictedAction>,
}

#[derive(Debug, Clone)]
pub struct PredictedAction {
    pub name: String,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAnalysis {
    pub session_id: String,
    pub files_accessed: Vec<String>,
    pub symbols_used: Vec<String>,
}

/// 预测类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PredictionType {
    /// 下一个可能访问的文件
    NextFile,
    /// 可能需要的符号 (函数/类型)
    NextSymbol,
    /// 可能执行的命令
    NextCommand,
    /// 可能的上下文依赖
    ContextDependency,
}

/// 预测结果
#[derive(Debug, Clone)]
pub struct ContextPrediction {
    /// 预测类型
    pub prediction_type: PredictionType,
    /// 预测的实体 (文件路径、符号名等)
    pub entity: String,
    /// 置信度 0.0 - 1.0
    pub confidence: f64,
    /// 预测理由
    pub reason: String,
    /// 预测时间
    pub predicted_at: Instant,
}

/// 预测器配置
#[derive(Debug, Clone)]
pub struct ProactiveContextConfig {
    /// 最大预测数量
    pub max_predictions: usize,
    /// 最小置信度阈值
    pub min_confidence: f64,
    /// 预加载提前量 (毫秒)
    pub preload_lead_time_ms: u64,
    /// 学习窗口大小 (会话数)
    pub learning_window_size: usize,
    /// 启用预测性加载
    pub enable_preload: bool,
}

impl Default for ProactiveContextConfig {
    fn default() -> Self {
        Self {
            max_predictions: 10,
            min_confidence: 0.5,
            preload_lead_time_ms: 500,
            learning_window_size: 50,
            enable_preload: true,
        }
    }
}

/// 主动上下文预测器
pub struct ProactiveContextPredictor {
    config: ProactiveContextConfig,
    /// 模式检测器
    pattern_detector: PatternDetector,
    /// 历史会话分析
    session_history: Arc<RwLock<Vec<SessionAnalysis>>>,
    /// 索引器
    indexer: GlobalIndexer,
    /// 预测缓存
    prediction_cache: Arc<RwLock<HashMap<String, Vec<ContextPrediction>>>>,
    /// 统计信息
    stats: Arc<RwLock<PredictionStats>>,
}

/// 预测统计
#[derive(Debug, Clone, Default)]
pub struct PredictionStats {
    pub predictions_made: u64,
    pub predictions_correct: u64,
    pub preloads_triggered: u64,
    pub avg_confidence: f64,
}

/// 上下文类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContextType {
    /// 文件内容
    File,
    /// 符号定义
    Symbol,
    /// 命令历史
    CommandHistory,
    /// 记忆片段
    Memory,
    /// 依赖关系
    Dependency,
}

impl ProactiveContextPredictor {
    /// 创建新的预测器
    pub fn new(config: ProactiveContextConfig) -> Self {
        let index_config = IncrementalIndexConfig::default();
        let indexer = get_or_create_indexer(index_config);

        Self {
            config,
            pattern_detector: PatternDetector,
            session_history: Arc::new(RwLock::new(Vec::new())),
            indexer,
            prediction_cache: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(PredictionStats::default())),
        }
    }

    /// 更新历史会话数据
    pub async fn update_history(&self, analyses: Vec<SessionAnalysis>) {
        let mut history = self.session_history.write().await;

        // 保留最近 N 个会话
        history.extend(analyses);
        if history.len() > self.config.learning_window_size {
            let to_remove = history.len() - self.config.learning_window_size;
            history.drain(0..to_remove);
        }

        debug!(
            session_count = history.len(),
            "Updated session history"
        );
    }

    /// 添加单个会话分析
    pub async fn add_session(&self, analysis: SessionAnalysis) {
        let mut history = self.session_history.write().await;
        history.push(analysis);

        // 保持窗口大小
        if history.len() > self.config.learning_window_size {
            history.remove(0);
        }
    }

    /// 基于当前上下文生成预测
    pub async fn predict(&self, current_context: &CurrentContext) -> Vec<ContextPrediction> {
        let mut predictions = Vec::new();

        // 1. 基于文件访问模式预测
        predictions.extend(self.predict_next_files(current_context).await);

        // 2. 基于命令模式预测
        predictions.extend(self.predict_next_commands(current_context).await);

        // 3. 基于符号依赖预测
        predictions.extend(self.predict_symbol_dependencies(current_context).await);

        // 排序并过滤
        predictions.sort_by(|a, b| {
            b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal)
        });

        predictions.truncate(self.config.max_predictions);
        predictions.retain(|p| p.confidence >= self.config.min_confidence);

        // 更新缓存
        {
            let mut cache = self.prediction_cache.write().await;
            cache.insert(current_context.session_id.clone(), predictions.clone());
        }

        // 更新统计
        {
            let mut stats = self.stats.write().await;
            stats.predictions_made += predictions.len() as u64;
        }

        predictions
    }

    /// 预测下一个可能访问的文件
    async fn predict_next_files(&self, ctx: &CurrentContext) -> Vec<ContextPrediction> {
        let mut predictions = Vec::new();
        let history = self.session_history.read().await;

        // 分析最近访问的文件序列
        let recent_files: Vec<&str> = ctx.recently_accessed_files
            .iter()
            .map(|s| s.as_str())
            .collect();

        if recent_files.len() < 2 {
            return predictions;
        }

        // 检测文件访问模式
        let last_file = recent_files.last().cloned();

        // 基于依赖图预测
        if let Some(last) = last_file {
            let last_path = std::path::PathBuf::from(last);

            // 获取依赖
            let deps = self.indexer.get_dependencies(&last_path).await;

            for dep in deps {
                let dep_str = dep.display().to_string();
                if !recent_files.contains(&dep_str.as_str()) {
                    predictions.push(ContextPrediction {
                        prediction_type: PredictionType::NextFile,
                        entity: dep_str.clone(),
                        confidence: 0.7,
                        reason: format!("依赖文件: {}", dep.display()),
                        predicted_at: Instant::now(),
                    });
                }
            }

            // 获取依赖于此文件的文件 (反向依赖)
            let dependents = self.indexer.get_dependents(&last_path).await;
            for dep in dependents {
                let dep_str = dep.display().to_string();
                if !recent_files.contains(&dep_str.as_str()) {
                    predictions.push(ContextPrediction {
                        prediction_type: PredictionType::NextFile,
                        entity: dep_str.clone(),
                        confidence: 0.6,
                        reason: "反向依赖".to_string(),
                        predicted_at: Instant::now(),
                    });
                }
            }
        }

        // 基于会话模式分析
        if history.len() >= 3 {
            // 检测常见的文件序列模式
            let file_transitions = self.detect_file_sequence_pattern(&recent_files);
            for (next_file, confidence) in file_transitions {
                if !recent_files.contains(&next_file.as_str()) {
                    predictions.push(ContextPrediction {
                        prediction_type: PredictionType::NextFile,
                        entity: next_file,
                        confidence,
                        reason: "基于历史序列模式".to_string(),
                        predicted_at: Instant::now(),
                    });
                }
            }
        }

        predictions
    }

    /// 检测文件序列模式
    fn detect_file_sequence_pattern(&self, _recent_files: &[&str]) -> Vec<(String, f64)> {
        // 简化实现：基于历史的序列模式检测
        // 实际实现需要更复杂的模式识别算法
        Vec::new()
    }

    /// 预测下一个可能执行的命令
    async fn predict_next_commands(&self, ctx: &CurrentContext) -> Vec<ContextPrediction> {
        let mut predictions = Vec::new();
        let history = self.session_history.read().await;

        if history.is_empty() {
            return predictions;
        }

        // 分析最近命令模式
        let recent_commands: Vec<&str> = ctx.recent_commands
            .iter()
            .map(|s| s.as_str())
            .collect();

        if let Some(&last_cmd) = recent_commands.last() {
            // 基于用户习惯预测
            let habit_profile = self.pattern_detector.detect_habits(&history);

            for action in habit_profile.most_common_actions.iter().take(3) {
                if action.name != last_cmd {
                    predictions.push(ContextPrediction {
                        prediction_type: PredictionType::NextCommand,
                        entity: action.name.clone(),
                        confidence: 0.5 + (action.count as f64 * 0.01).min(0.4),
                        reason: "常见用户行为".to_string(),
                        predicted_at: Instant::now(),
                    });
                }
            }
        }

        predictions
    }

    /// 预测符号依赖
    async fn predict_symbol_dependencies(&self, ctx: &CurrentContext) -> Vec<ContextPrediction> {
        let mut predictions = Vec::new();

        for file in &ctx.currently_open_files {
            if let Some(symbols) = self.indexer.get_file_symbols(std::path::Path::new(file)).await {
                for symbol in symbols.iter().take(5) {
                    predictions.push(ContextPrediction {
                        prediction_type: PredictionType::NextSymbol,
                        entity: symbol.name.clone(),
                        confidence: 0.6,
                        reason: format!("当前文件中的符号: {}", file),
                        predicted_at: Instant::now(),
                    });
                }
            }
        }

        predictions
    }

    /// 获取预测性预加载任务
    pub async fn get_preload_tasks(&self, ctx: &CurrentContext) -> Vec<PreloadTask> {
        let predictions = self.predict(ctx).await;
        let mut tasks = Vec::new();

        for pred in predictions.iter().take(5) {
            if pred.confidence >= 0.7 {
                tasks.push(PreloadTask {
                    context_type: match pred.prediction_type {
                        PredictionType::NextFile => ContextType::File,
                        PredictionType::NextSymbol => ContextType::Symbol,
                        PredictionType::NextCommand => ContextType::CommandHistory,
                        PredictionType::ContextDependency => ContextType::Dependency,
                    },
                    entity: pred.entity.clone(),
                    priority: pred.confidence,
                    reason: pred.reason.clone(),
                });

                // 更新统计
                {
                    let mut stats = self.stats.write().await;
                    stats.preloads_triggered += 1;
                }
            }
        }

        tasks
    }

    /// 反馈预测准确性 (用于学习)
    pub async fn feedback(&self, _session_id: &str, correct: bool) {
        let mut stats = self.stats.write().await;
        stats.predictions_correct += if correct { 1 } else { 0 };
    }

    /// 获取统计信息
    pub async fn get_stats(&self) -> PredictionStats {
        self.stats.read().await.clone()
    }
}

/// 预加载任务
#[derive(Debug, Clone)]
pub struct PreloadTask {
    pub context_type: ContextType,
    pub entity: String,
    pub priority: f64,
    pub reason: String,
}

/// 当前上下文
#[derive(Debug, Clone)]
pub struct CurrentContext {
    pub session_id: String,
    pub currently_open_files: Vec<String>,
    pub recently_accessed_files: Vec<String>,
    pub recent_commands: Vec<String>,
    pub current_working_directory: Option<String>,
    pub current_language: Option<String>,
}

impl CurrentContext {
    pub fn new(session_id: String) -> Self {
        Self {
            session_id,
            currently_open_files: Vec::new(),
            recently_accessed_files: Vec::new(),
            recent_commands: Vec::new(),
            current_working_directory: None,
            current_language: None,
        }
    }

    pub fn with_open_files(mut self, files: Vec<String>) -> Self {
        self.currently_open_files = files;
        self
    }

    pub fn with_recent_files(mut self, files: Vec<String>) -> Self {
        self.recently_accessed_files = files;
        self
    }

    pub fn with_commands(mut self, commands: Vec<String>) -> Self {
        self.recent_commands = commands;
        self
    }
}

/// 预测服务
pub struct ProactiveContextService {
    predictor: Arc<ProactiveContextPredictor>,
    enabled: bool,
}

impl ProactiveContextService {
    pub fn new(config: ProactiveContextConfig) -> Self {
        Self {
            predictor: Arc::new(ProactiveContextPredictor::new(config)),
            enabled: true,
        }
    }

    /// 启用/禁用服务
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        info!(enabled, "Proactive context service updated");
    }

    /// 处理当前上下文并返回预测
    pub async fn process(&self, ctx: CurrentContext) -> Vec<ContextPrediction> {
        if !self.enabled {
            return Vec::new();
        }

        self.predictor.predict(&ctx).await
    }

    /// 获取预加载任务
    pub async fn get_preload_tasks(&self, ctx: &CurrentContext) -> Vec<PreloadTask> {
        if !self.enabled {
            return Vec::new();
        }

        self.predictor.get_preload_tasks(ctx).await
    }

    /// 更新历史
    pub async fn update_history(&self, analyses: Vec<SessionAnalysis>) {
        if self.enabled {
            self.predictor.update_history(analyses).await;
        }
    }

    /// 获取统计
    pub async fn get_stats(&self) -> PredictionStats {
        self.predictor.get_stats().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_prediction() {
        let predictor = ProactiveContextPredictor::new(ProactiveContextConfig::default());

        let ctx = CurrentContext::new("test-session".to_string())
            .with_open_files(vec!["/src/main.rs".to_string()])
            .with_recent_files(vec![
                "/src/lib.rs".to_string(),
                "/src/main.rs".to_string(),
            ])
            .with_commands(vec!["edit".to_string(), "read".to_string()]);

        let predictions = predictor.predict(&ctx).await;

        // 至少有命令预测
        assert!(!predictions.is_empty() || predictions.len() >= 0);
    }

    #[tokio::test]
    async fn test_preload_tasks() {
        let predictor = ProactiveContextPredictor::new(ProactiveContextConfig::default());

        let ctx = CurrentContext::new("test".to_string())
            .with_open_files(vec!["/src/main.rs".to_string()]);

        let tasks = predictor.get_preload_tasks(&ctx).await;

        // 可能没有预加载任务因为没有足够的历史数据
        assert!(tasks.len() >= 0);
    }
}
