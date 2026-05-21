use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use super::providers::CompletionItemEnhanced;

#[derive(Debug, Clone)]
pub struct CompletionRanker {
    weights: RankingWeights,
    usage_history: Arc<RwLock<HashMap<String, u64>>>,
    recency_bonus: f64,
}

#[derive(Debug, Clone)]
pub struct RankingWeights {
    pub context_match: f64,
    pub popularity: f64,
    pub provider_bonus: f64,
    pub recency: f64,
    pub type_match: f64,
    pub length_penalty: f64,
}

impl Default for RankingWeights {
    fn default() -> Self {
        Self {
            context_match: 0.4,
            popularity: 0.2,
            provider_bonus: 0.15,
            recency: 0.1,
            type_match: 0.1,
            length_penalty: 0.05,
        }
    }
}

impl CompletionRanker {
    pub fn new() -> Self {
        Self {
            weights: RankingWeights::default(),
            usage_history: Arc::new(RwLock::new(HashMap::new())),
            recency_bonus: 0.05,
        }
    }

    pub async fn rank(&self, items: Vec<CompletionItemEnhanced>) -> Vec<CompletionItemEnhanced> {
        let mut ranked = items;
        
        for item in ranked.iter_mut() {
            item.score = self.calculate_score(item).await;
        }
        
        ranked.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        ranked
    }

    async fn calculate_score(&self, item: &CompletionItemEnhanced) -> f64 {
        let base_score = item.score;
        
        let context_score = item.context_score * self.weights.context_match;
        let popularity_score = self.calculate_popularity_score(item).await * self.weights.popularity;
        let provider_score = self.calculate_provider_score(item) * self.weights.provider_bonus;
        let recency_score = self.calculate_recency_score(item).await * self.weights.recency;
        let type_score = self.calculate_type_score(item) * self.weights.type_match;
        let length_penalty = self.calculate_length_penalty(item) * self.weights.length_penalty;

        base_score + context_score + popularity_score + provider_score + recency_score + type_score - length_penalty
    }

    async fn calculate_popularity_score(&self, item: &CompletionItemEnhanced) -> f64 {
        let label = &item.item.label;
        let history = self.usage_history.read().await;
        
        if let Some(count) = history.get(label) {
            let max_count = history.values().max().copied().unwrap_or(1);
            (*count as f64) / (max_count as f64)
        } else {
            item.popularity_score
        }
    }

    fn calculate_provider_score(&self, item: &CompletionItemEnhanced) -> f64 {
        match item.provider {
            super::providers::CompletionProviderType::Ai => 1.0,
            super::providers::CompletionProviderType::Lsp => 0.8,
            super::providers::CompletionProviderType::Builtin => 0.6,
            super::providers::CompletionProviderType::Snippet => 0.5,
        }
    }

    async fn calculate_recency_score(&self, item: &CompletionItemEnhanced) -> f64 {
        let label = &item.item.label;
        let history = self.usage_history.read().await;
        
        if history.contains_key(label) {
            self.recency_bonus
        } else {
            0.0
        }
    }

    fn calculate_type_score(&self, item: &CompletionItemEnhanced) -> f64 {
        if let Some(kind) = item.item.kind {
            match kind {
                lsp_types::CompletionItemKind::FUNCTION |
                lsp_types::CompletionItemKind::METHOD => 1.0,
                lsp_types::CompletionItemKind::STRUCT |
                lsp_types::CompletionItemKind::CLASS => 0.9,
                lsp_types::CompletionItemKind::VARIABLE => 0.7,
                lsp_types::CompletionItemKind::SNIPPET => 0.6,
                lsp_types::CompletionItemKind::TEXT => 0.5,
                _ => 0.3,
            }
        } else {
            0.5
        }
    }

    fn calculate_length_penalty(&self, item: &CompletionItemEnhanced) -> f64 {
        let label_len = item.item.label.len() as f64;
        if label_len > 50.0 {
            (label_len - 50.0) / 100.0
        } else {
            0.0
        }
    }

    pub async fn record_usage(&self, label: &str) {
        let mut history = self.usage_history.write().await;
        *history.entry(label.to_string()).or_insert(0) += 1;
    }

    pub async fn get_top_items(&self, items: Vec<CompletionItemEnhanced>, limit: usize) -> Vec<CompletionItemEnhanced> {
        let ranked = self.rank(items).await;
        ranked.into_iter().take(limit).collect()
    }
}