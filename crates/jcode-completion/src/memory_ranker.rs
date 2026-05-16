use crate::ast_context::CompletionContext;
use crate::llm_candidate::CompletionCandidate;
use async_trait::async_trait;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// 经过记忆排序后的候选
#[derive(Debug, Clone)]
pub struct RankedCandidate {
    pub candidate: CompletionCandidate,
    pub rank_score: f64,
    pub reason: &'static str,
}

/// 记忆排序器 trait
#[async_trait]
pub trait MemoryRanker: Send + Sync {
    async fn rank_and_filter(
        &self,
        candidates: Vec<CompletionCandidate>,
        context: &CompletionContext,
    ) -> Vec<RankedCandidate>;
}

/// 用户使用模式追踪 (什么补全被接受过)
pub struct UsageTracker {
    /// (file_prefix, accepted_text) -> count
    patterns: RwLock<HashMap<String, u32>>,
    /// 总的完成次数
    total: RwLock<u64>,
}

impl UsageTracker {
    pub fn new() -> Self {
        Self {
            patterns: RwLock::new(HashMap::new()),
            total: RwLock::new(0),
        }
    }

    pub fn record_accepted(&self, file_path: &str, text: &str) {
        let key = format!("{}::{}", self.file_prefix(file_path), text);
        let mut patterns = self.patterns.write();
        *patterns.entry(key).or_insert(0) += 1;
        *self.total.write() += 1;
    }

    /// 获取用户对该文件的偏好分数
    pub fn preference_score(&self, file_path: &str, text: &str) -> f64 {
        let prefix = self.file_prefix(file_path);
        // 精确匹配
        let exact_key = format!("{}::{}", prefix, text);
        if let Some(count) = self.patterns.read().get(&exact_key) {
            return (*count as f64).ln_1p() / 5.0; // log-scaled
        }
        // 模糊匹配: 检查文件中的其他模式
        let total_patterns: u32 = self.patterns.read().values().sum();
        if total_patterns == 0 { return 0.0; }
        0.0
    }

    fn file_prefix(&self, path: &str) -> String {
        std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string()
    }
}

/// 默认记忆排序器
pub struct DefaultMemoryRanker {
    tracker: Arc<UsageTracker>,
    /// [item, field, ...] -> score multiplier
    field_preferences: RwLock<HashMap<String, f64>>,
}

impl DefaultMemoryRanker {
    pub fn new() -> Self {
        Self {
            tracker: Arc::new(UsageTracker::new()),
            field_preferences: RwLock::new(HashMap::new()),
        }
    }

    pub fn tracker(&self) -> Arc<UsageTracker> { self.tracker.clone() }
}

#[async_trait]
impl MemoryRanker for DefaultMemoryRanker {
    async fn rank_and_filter(
        &self,
        candidates: Vec<CompletionCandidate>,
        context: &CompletionContext,
    ) -> Vec<RankedCandidate> {
        let mut ranked: Vec<RankedCandidate> = candidates
            .into_iter()
            .map(|c| {
                // Layer 3a: 记忆偏好提升
                let pref = self.tracker.preference_score(&context.file_path, &c.label);

                // Layer 3b: 前缀匹配
                let prefix_match = if context.prefix.is_empty() {
                    0.0
                } else if c.label.starts_with(&context.prefix) {
                    0.2
                } else if c.label.to_lowercase().contains(&context.prefix.to_lowercase()) {
                    0.1
                } else {
                    -0.5 // 不匹配的降权
                };

                let rank_score = c.score + pref + prefix_match;

                let reason = if pref > 0.0 {
                    "remembered"
                } else if prefix_match > 0.0 {
                    "prefix_match"
                } else {
                    "default"
                };

                RankedCandidate { candidate: c, rank_score, reason }
            })
            .filter(|r| r.rank_score > 0.0)
            .collect();

        ranked.sort_by(|a, b| b.rank_score.partial_cmp(&a.rank_score).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(20);
        ranked
    }
}
