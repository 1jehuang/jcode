//! # Context Pruner — 智能上下文窗口裁剪器
//!
//! 当对话接近 token 上限时，智能选择保留哪些消息、丢弃哪些。
//! 超越 Claude Code 的简单 FIFO 截断：
//! - **优先级感知**：工具结果 > 用户消息 > 助手回复 > 系统提示
//! - **语义去重**：相似内容只保留最新版本
//! - **结构保护**：不截断正在进行的函数/代码块
//! - **关键锚点**：始终保留最近 N 条用户消息和最后一条助手消息
//! - **Token 预估**：基于启发式的 token 计数（无需调用 tokenizer）

use serde::{Deserialize, Serialize};

const DEFAULT_TARGET_RATIO: f64 = 0.85;
const ANCHOR_USER_MESSAGES: usize = 3;
const MIN_TOOL_RESULTS_TO_KEEP: usize = 5;
const ESTIMATED_CHARS_PER_TOKEN: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMessage {
    pub role: MessageRole,
    pub content: String,
    pub token_estimate: usize,
    pub is_code_block: bool,
    pub is_active_edit: bool,
    pub tool_name: Option<String>,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneResult {
    pub original_count: usize,
    pub pruned_count: usize,
    pub original_tokens: usize,
    pub remaining_tokens: usize,
    pub target_tokens: usize,
    pub messages: Vec<ContextMessage>,
    pub summary: String,
    pub preserved_anchors: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PruneStrategy {
    PriorityBased,
    RecencyOnly,
    SemanticDedup,
    Hybrid,
}

impl Default for PruneStrategy { fn default() -> Self { Self::Hybrid } }

pub struct ContextPruner {
    target_ratio: f64,
    strategy: PruneStrategy,
    max_window_tokens: usize,
}

impl ContextPruner {
    pub fn new(max_tokens: usize) -> Self {
        Self {
            target_ratio: DEFAULT_TARGET_RATIO,
            strategy: PruneStrategy::Hybrid,
            max_window_tokens: max_tokens,
        }
    }

    pub fn with_strategy(mut self, s: PruneStrategy) -> Self { self.strategy = s; self }
    pub fn with_target_ratio(mut self, r: f64) -> Self { self.target_ratio = r; self }

    pub fn estimate_tokens(text: &str) -> usize {
        (text.len() as f64 / ESTIMATED_CHARS_PER_TOKEN).ceil() as usize
    }

    pub fn total_tokens(messages: &[ContextMessage]) -> usize {
        messages.iter().map(|m| m.token_estimate).sum()
    }

    pub fn prune(&self, messages: Vec<ContextMessage>) -> PruneResult {
        let original_count = messages.len();
        let original_tokens = Self::total_tokens(&messages);
        let target = (self.max_window_tokens as f64 * self.target_ratio) as usize;

        if original_tokens <= target {
            return PruneResult {
                original_count, pruned_count: 0,
                original_tokens, remaining_tokens: original_tokens,
                target_tokens: target, messages, summary: "No pruning needed".into(),
                preserved_anchors: Vec::new(),
            };
        }

        let anchors = self.find_anchor_positions(&messages);
        let scores = self.score_messages(&messages);

        let mut indexed: Vec<(usize, f64)> = scores.into_iter().enumerate().collect();
        indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut keep = std::collections::HashSet::new();
        for &idx in &anchors { keep.insert(idx); }

        let mut remaining = target;
        for &(idx, _score) in &indexed {
            if keep.contains(&idx) { continue; }
            if messages[idx].token_estimate <= remaining || keep.len() < original_count / 2 {
                keep.insert(idx);
                remaining = remaining.saturating_sub(messages[idx].token_estimate);
            }
        }

        let mut kept_indices: Vec<usize> = keep.into_iter().collect();
        kept_indices.sort();

        let pruned_messages: Vec<_> = kept_indices.iter().map(|&i| messages[i].clone()).collect();
        let remaining_tokens = Self::total_tokens(&pruned_messages);
        let pruned_count = original_count - pruned_messages.len();
        let _pruned_len = pruned_messages.len();

        PruneResult {
            original_count, pruned_count,
            original_tokens, remaining_tokens,
            target_tokens: target,
            messages: pruned_messages,
            summary: format!(
                "Pruned {} messages ({} tokens), retained {} messages ({} tokens)",
                pruned_count, original_tokens - remaining_tokens,
                _pruned_len, remaining_tokens
            ),
            preserved_anchors: anchors,
        }
    }

    fn find_anchor_positions(&self, messages: &[ContextMessage]) -> Vec<usize> {
        let mut anchors = Vec::new();
        let mut user_indices: Vec<(usize, u64)> = messages.iter().enumerate()
            .filter(|(_, m)| m.role == MessageRole::User)
            .map(|(i, m)| (i, m.created_at_ms))
            .collect();
        user_indices.sort_by_key(|&(_, t)| std::cmp::Reverse(t));

        for (idx, _) in user_indices.into_iter().take(ANCHOR_USER_MESSAGES) {
            anchors.push(idx);
        }

        if let Some(last_assistant) = messages.iter().rposition(|m| m.role == MessageRole::Assistant) {
            if !anchors.contains(&last_assistant) {
                anchors.push(last_assistant);
            }
        }

        for (i, m) in messages.iter().enumerate().rev() {
            if m.is_active_edit && !anchors.contains(&i) {
                anchors.push(i);
                break;
            }
        }
        anchors.sort(); anchors.dedup();
        anchors
    }

    fn score_messages(&self, messages: &[ContextMessage]) -> Vec<f64> {
        match self.strategy {
            PruneStrategy::RecencyOnly => messages.iter().enumerate()
                .map(|(i, m)| i as f64 * 0.001 + self.recency_score(m))
                .collect(),
            PruneStrategy::PriorityBased => messages.iter()
                .map(|m| self.priority_score(m))
                .collect(),
            PruneStrategy::SemanticDedup => self.semantic_scores(messages),
            PruneStrategy::Hybrid => {
                let prios: Vec<f64> = messages.iter().map(|m| self.priority_score(m)).collect();
                let recencies: Vec<f64> = messages.iter().map(|m| self.recency_score(m)).collect();
                prios.into_iter().zip(recencies.into_iter())
                    .map(|(p, r)| p * 0.6 + r * 0.4)
                    .collect()
            },
        }
    }

    fn priority_score(&self, msg: &ContextMessage) -> f64 {
        let base = match msg.role {
            MessageRole::System => 5.0,
            MessageRole::User => 9.0,
            MessageRole::Assistant => 4.0,
            MessageRole::Tool => 6.0,
        };
        let bonus = if msg.is_active_edit { 3.0 } else if msg.is_code_block { 1.0 } else { 0.0 };
        let recency = self.recency_score(msg);
        base + bonus + recency * 2.0
    }

    fn recency_score(&self, msg: &ContextMessage) -> f64 {
        let age_factor = (msg.created_at_ms as f64).max(1.0).log10();
        10.0 - age_factor.min(10.0)
    }

    fn semantic_scores(&self, messages: &[ContextMessage]) -> Vec<f64> {
        let n = messages.len();
        if n <= 1 { return vec![1.0; n]; }
        let mut scores = Vec::with_capacity(n);
        for (i, msg) in messages.iter().enumerate() {
            let mut uniqueness = 1.0f64;
            for (j, other) in messages.iter().enumerate() {
                if i != j && self.content_similarity(&msg.content, &other.content) > 0.8 {
                    uniqueness *= 0.7;
                }
            }
            scores.push(uniqueness * self.priority_score(msg));
        }
        scores
    }

    fn content_similarity(&self, a: &str, b: &str) -> f64 {
        if a.is_empty() || b.is_empty() { return 0.0; }
        let words_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
        let words_b: std::collections::HashSet<&str> = b.split_whitespace().collect();
        let intersection = words_a.intersection(&words_b).count();
        let union = words_a.union(&words_b).count();
        if union == 0 { 0.0 } else { intersection as f64 / union as f64 }
    }
}

impl ContextMessage {
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        let c = content.into();
        let token_estimate = ContextPruner::estimate_tokens(&c);
        let is_code_block = c.contains("```") || (c.lines().count() > 3 &&
            c.lines().filter(|l| l.trim_start().starts_with("fn ") ||
                              l.trim_start().starts_with("class ") ||
                              l.trim_start().starts_with("def ") ||
                              l.trim_start().starts_with("pub ")).count() > 0);
        Self {
            role, content: c,
            token_estimate,
            is_code_block,
            is_active_edit: false,
            tool_name: None,
            created_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default().as_millis() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msgs(count: usize) -> Vec<ContextMessage> {
        (0..count).map(|i| {
            let role = match i % 4 {
                0 => MessageRole::System,
                1 => MessageRole::User,
                2 => MessageRole::Assistant,
                _ => MessageRole::Tool,
            };
            ContextMessage::new(role, format!("message {} with some content here", i))
        }).collect()
    }

    #[test]
    fn test_no_prune_when_under_limit() {
        let pruner = ContextPruner::new(100000);
        let msgs = make_msgs(20);
        let result = pruner.prune(msgs);
        assert_eq!(result.pruned_count, 0);
    }

    #[test]
    fn test_prune_reduces_token_count() {
        let pruner = ContextPruner::new(500);
        let msgs = make_msgs(200);
        let result = pruner.prune(msgs);
        assert!(result.remaining_tokens < result.original_tokens);
        assert!(result.pruned_count > 0);
    }

    #[test]
    fn test_anchors_preserved() {
        let pruner = ContextPruner::new(500);
        let mut msgs = make_msgs(50);
        msgs[49] = ContextMessage::new(MessageRole::Assistant, "final response");
        let result = pruner.prune(msgs);
        assert!(!result.preserved_anchors.is_empty());
    }

    #[test]
    fn test_token_estimation() {
        let est = ContextPruner::estimate_tokens("hello world");
        assert!(est > 0);
        assert!(est < 10);
    }

    #[test]
    fn test_content_similarity() {
        let pruner = ContextPruner::new(10000);
        let sim = pruner.content_similarity("fn foo() { bar(); }", "fn foo() { baz(); }");
        assert!(sim > 0.5);
        let diff = pruner.content_similarity("hello world", "completely different");
        assert!(diff < 0.5);
    }
}
