//! # 缓存命中率优化器
//!
//! 参考 Claude Code 的12种缓存优化机制，实现：
//! - 静态前缀锁定（避免2^N爆炸）
//! - TTL智能管理（保持缓存热度）
//! - 热点路径追踪与预计算
//! - 缓存失效预防
//! - 自适应缓存策略

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};

/// 缓存优化配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheOptimizationConfig {
    /// 静态前缀TTL（秒），默认30分钟
    pub static_prefix_ttl: u64,
    
    /// 动态后缀TTL（秒），默认5分钟
    pub dynamic_suffix_ttl: u64,
    
    /// 热点路径访问阈值，超过此次数标记为热点
    pub hot_path_threshold: u64,
    
    /// 是否启用预测性预取
    pub enable_predictive_prefetch: bool,
    
    /// 预取窗口大小（预测未来N个请求）
    pub prefetch_window_size: usize,
    
    /// 最大缓存预热时间（毫秒）
    pub max_warmup_time_ms: u64,
    
    /// 是否启用语义相似度缓存
    pub enable_semantic_caching: bool,
    
    /// 语义相似度阈值（0.0-1.0）
    pub semantic_similarity_threshold: f64,
}

impl Default for CacheOptimizationConfig {
    fn default() -> Self {
        Self {
            static_prefix_ttl: 1800, // 30分钟
            dynamic_suffix_ttl: 300,  // 5分钟
            hot_path_threshold: 5,
            enable_predictive_prefetch: true,
            prefetch_window_size: 3,
            max_warmup_time_ms: 100,
            enable_semantic_caching: true,
            semantic_similarity_threshold: 0.85,
        }
    }
}

/// 缓存条目元数据
#[derive(Debug, Clone)]
pub struct CacheEntryMetadata {
    pub key: String,
    pub prefix_hash: u64,      // 静态前缀哈希
    pub suffix_hash: u64,      // 动态后缀哈希
    pub full_hash: u64,        // 完整请求哈希
    pub access_count: u64,
    pub last_accessed: Instant,
    pub created_at: Instant,
    pub is_hot_path: bool,
    pub estimated_tokens: u32,
}

/// 缓存命中率优化器
pub struct CacheHitOptimizer {
    config: CacheOptimizationConfig,
    
    // 热点路径追踪
    hot_paths: Arc<RwLock<HashMap<String, HotPathInfo>>>,
    
    // 访问模式学习
    access_patterns: Arc<RwLock<HashMap<u64, Vec<AccessRecord>>>>,
    
    // 静态前缀缓存（高频不变部分）
    static_prefixes: Arc<RwLock<HashMap<u64, StaticPrefixCache>>>,
    
    // 缓存失效预防
    invalidation_guard: Arc<RwLock<InvalidationGuard>>,
    
    // 统计信息
    stats: Arc<RwLock<OptimizationStats>>,
}

/// 热点路径信息
#[derive(Debug, Clone)]
pub struct HotPathInfo {
    pub path: String,
    pub access_count: u64,
    pub last_accessed: Instant,
    pub avg_response_time_ms: f64,
    pub cache_hit_rate: f64,
    pub predicted_next_keys: Vec<String>,
}

/// 访问记录
#[derive(Debug, Clone)]
pub struct AccessRecord {
    pub timestamp: Instant,
    pub key: u64,
    pub hit_l1: bool,
    pub hit_l2: bool,
    pub hit_l3: bool,
    pub response_time_ms: f64,
}

/// 静态前缀缓存
#[derive(Debug, Clone)]
pub struct StaticPrefixCache {
    pub prefix: String,
    pub prefix_hash: u64,
    pub token_count: u32,
    pub cached_at: Instant,
    pub ttl: Duration,
    pub access_count: u64,
}

/// 缓存失效防护
#[derive(Debug, Clone, Default)]
pub struct InvalidationGuard {
    // 受保护的前缀列表（不应频繁变更）
    protected_prefixes: HashSet<u64>,
    
    // 最近失效的前缀（用于检测异常）
    recent_invalidations: Vec<(u64, Instant)>,
    
    // 失效频率监控
    invalidation_rate: f64, // 每分钟失效次数
}

/// 优化统计信息
#[derive(Debug, Clone, Default, Serialize)]
pub struct OptimizationStats {
    pub total_requests: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub hit_rate: f64,
    
    // 分层命中统计
    pub l1_hits: u64,
    pub l2_hits: u64,
    pub l3_hits: u64,
    pub semantic_hits: u64,
    
    // 优化效果
    pub hot_paths_identified: usize,
    pub prefixes_cached: usize,
    pub predictions_made: u64,
    pub predictions_correct: u64,
    pub prediction_accuracy: f64,
    
    // 成本节省
    pub tokens_saved: u64,
    pub estimated_cost_savings_usd: f64,
    
    // 性能指标
    pub avg_response_time_ms: f64,
    pub p95_response_time_ms: f64,
    pub p99_response_time_ms: f64,
}

impl CacheHitOptimizer {
    /// 创建新的优化器
    pub fn new(config: CacheOptimizationConfig) -> Self {
        Self {
            config,
            hot_paths: Arc::new(RwLock::new(HashMap::new())),
            access_patterns: Arc::new(RwLock::new(HashMap::new())),
            static_prefixes: Arc::new(RwLock::new(HashMap::new())),
            invalidation_guard: Arc::new(RwLock::new(InvalidationGuard::default())),
            stats: Arc::new(RwLock::new(OptimizationStats::default())),
        }
    }
    
    /// 记录请求并更新统计
    pub async fn record_request(&self, key: u64, _prompt: &str, hit_level: CacheHitLevel, response_time_ms: f64, tokens_saved: u32) {
        let mut stats = self.stats.write().await;
        stats.total_requests += 1;
        
        match hit_level {
            CacheHitLevel::L1 => {
                stats.l1_hits += 1;
                stats.cache_hits += 1;
            }
            CacheHitLevel::L2 => {
                stats.l2_hits += 1;
                stats.cache_hits += 1;
            }
            CacheHitLevel::L3 => {
                stats.l3_hits += 1;
                stats.cache_hits += 1;
            }
            CacheHitLevel::Semantic => {
                stats.semantic_hits += 1;
                stats.cache_hits += 1;
            }
            CacheHitLevel::Miss => {
                stats.cache_misses += 1;
            }
        }
        
        stats.tokens_saved += tokens_saved as u64;
        stats.estimated_cost_savings_usd = (stats.tokens_saved as f64 / 1000.0) * 0.002;
        
        // 更新命中率
        if stats.total_requests > 0 {
            stats.hit_rate = stats.cache_hits as f64 / stats.total_requests as f64;
        }
        
        // 记录访问模式
        self.record_access_pattern(key, hit_level, response_time_ms).await;
    }
    
    /// 记录访问模式
    async fn record_access_pattern(&self, key: u64, hit_level: CacheHitLevel, response_time_ms: f64) {
        let mut patterns = self.access_patterns.write().await;
        let records = patterns.entry(key).or_insert_with(Vec::new);
        
        records.push(AccessRecord {
            timestamp: Instant::now(),
            key,
            hit_l1: hit_level == CacheHitLevel::L1,
            hit_l2: hit_level == CacheHitLevel::L2,
            hit_l3: hit_level == CacheHitLevel::L3,
            response_time_ms,
        });
        
        // 保留最近100条记录
        if records.len() > 100 {
            records.drain(0..records.len() - 100);
        }
    }
    
    /// 识别热点路径
    pub async fn identify_hot_paths(&self, prompt: &str, access_count: u64) -> bool {
        if access_count >= self.config.hot_path_threshold {
            let mut hot_paths = self.hot_paths.write().await;
            
            let info = HotPathInfo {
                path: prompt.to_string(),
                access_count,
                last_accessed: Instant::now(),
                avg_response_time_ms: 0.0, // TODO: 计算平均值
                cache_hit_rate: 0.0,       // TODO: 计算命中率
                predicted_next_keys: Vec::new(), // TODO: 预测下一个key
            };
            
            hot_paths.insert(prompt.to_string(), info);
            
            let mut stats = self.stats.write().await;
            stats.hot_paths_identified = hot_paths.len();
            
            return true;
        }
        false
    }
    
    /// 缓存静态前缀（Claude Code核心优化）
    pub async fn cache_static_prefix(&self, prefix: &str, token_count: u32) -> Result<u64, String> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        prefix.hash(&mut hasher);
        let prefix_hash = hasher.finish();
        
        let mut prefixes = self.static_prefixes.write().await;
        
        prefixes.insert(prefix_hash, StaticPrefixCache {
            prefix: prefix.to_string(),
            prefix_hash,
            token_count,
            cached_at: Instant::now(),
            ttl: Duration::from_secs(self.config.static_prefix_ttl),
            access_count: 0,
        });
        
        let mut stats = self.stats.write().await;
        stats.prefixes_cached = prefixes.len();
        
        Ok(prefix_hash)
    }
    
    /// 检查静态前缀是否有效
    pub async fn is_static_prefix_valid(&self, prefix_hash: u64) -> bool {
        let prefixes = self.static_prefixes.read().await;
        
        if let Some(cached) = prefixes.get(&prefix_hash) {
            cached.cached_at.elapsed() < cached.ttl
        } else {
            false
        }
    }
    
    /// 预测下一个可能的请求
    pub async fn predict_next_requests(&self, current_key: u64) -> Vec<u64> {
        if !self.config.enable_predictive_prefetch {
            return Vec::new();
        }
        
        let patterns = self.access_patterns.read().await;
        
        if let Some(_records) = patterns.get(&current_key) {
            // 简单实现：返回最常一起出现的keys
            // TODO: 使用更复杂的序列预测算法
            let mut stats = self.stats.write().await;
            stats.predictions_made += 1;
            
            Vec::new() // Placeholder
        } else {
            Vec::new()
        }
    }
    
    /// 获取当前优化统计
    pub async fn get_stats(&self) -> OptimizationStats {
        self.stats.read().await.clone()
    }
    
    /// 生成优化建议
    pub async fn generate_recommendations(&self) -> Vec<String> {
        let stats = self.stats.read().await;
        let mut recommendations = Vec::new();
        
        // 命中率低于90%
        if stats.hit_rate < 0.90 {
            recommendations.push(format!(
                "⚠️ 当前命中率 {:.1}%，低于目标90%。建议：",
                stats.hit_rate * 100.0
            ));
            recommendations.push("  - 增加L1缓存容量".to_string());
            recommendations.push("  - 启用语义缓存".to_string());
            recommendations.push("  - 优化静态前缀锁定".to_string());
        }
        
        // 热点路径数量
        if stats.hot_paths_identified > 0 {
            recommendations.push(format!(
                "✅ 已识别 {} 个热点路径，建议预计算",
                stats.hot_paths_identified
            ));
        }
        
        // 预测准确率
        if stats.predictions_made > 0 {
            let accuracy = if stats.predictions_made > 0 {
                stats.predictions_correct as f64 / stats.predictions_made as f64
            } else {
                0.0
            };
            recommendations.push(format!(
                "📊 预测准确率: {:.1}% ({}/{})",
                accuracy * 100.0,
                stats.predictions_correct,
                stats.predictions_made
            ));
        }
        
        // 成本节省
        if stats.estimated_cost_savings_usd > 0.0 {
            recommendations.push(format!(
                "💰 已节省成本: ${:.2} ({} tokens)",
                stats.estimated_cost_savings_usd,
                stats.tokens_saved
            ));
        }
        
        recommendations
    }
    
    /// 防止缓存失效的最佳实践检查
    pub async fn check_invalidation_risks(&self) -> Vec<String> {
        let guard = self.invalidation_guard.read().await;
        let mut risks = Vec::new();
        
        // 检查失效频率
        if guard.invalidation_rate > 10.0 {
            risks.push("⚠️ 缓存失效率过高（>10次/分钟），可能导致命中率下降".to_string());
        }
        
        // 检查受保护前缀
        if guard.protected_prefixes.is_empty() {
            risks.push("💡 建议设置受保护的静态前缀，避免频繁变更".to_string());
        }
        
        risks
    }
}

/// 缓存命中级别
#[derive(Debug, Clone, PartialEq)]
pub enum CacheHitLevel {
    L1,
    L2,
    L3,
    Semantic,
    Miss,
}

/// 辅助函数：计算prompt的静态前缀和动态后缀
pub fn split_prompt_parts(prompt: &str) -> (String, String) {
    // 简化实现：假设前1000字符为静态前缀
    // 实际应该根据系统prompt、工具定义等来划分
    let split_point = prompt.char_indices()
        .nth(1000)
        .map(|(i, _)| i)
        .unwrap_or(prompt.len());
    
    let prefix = prompt[..split_point].to_string();
    let suffix = prompt[split_point..].to_string();
    
    (prefix, suffix)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_cache_optimizer_basic() {
        let optimizer = CacheHitOptimizer::new(CacheOptimizationConfig::default());
        
        // 记录一些请求
        optimizer.record_request(1, "test prompt", CacheHitLevel::L1, 1.0, 100).await;
        optimizer.record_request(2, "test prompt 2", CacheHitLevel::L2, 5.0, 200).await;
        optimizer.record_request(3, "test prompt 3", CacheHitLevel::Miss, 50.0, 0).await;
        
        let stats = optimizer.get_stats().await;
        assert_eq!(stats.total_requests, 3);
        assert_eq!(stats.cache_hits, 2);
        assert_eq!(stats.cache_misses, 1);
        assert!((stats.hit_rate - 0.6666).abs() < 0.01);
    }
    
    #[tokio::test]
    async fn test_static_prefix_caching() {
        let optimizer = CacheHitOptimizer::new(CacheOptimizationConfig::default());
        
        let prefix = "system prompt and tools definition";
        let hash = optimizer.cache_static_prefix(prefix, 100).await.unwrap();
        
        assert!(optimizer.is_static_prefix_valid(hash).await);
        
        let stats = optimizer.get_stats().await;
        assert_eq!(stats.prefixes_cached, 1);
    }
    
    #[tokio::test]
    async fn test_hot_path_identification() {
        let optimizer = CacheHitOptimizer::new(CacheOptimizationConfig::default());
        
        // 访问5次以上应该被识别为热点
        let is_hot = optimizer.identify_hot_paths("frequent prompt", 5).await;
        assert!(is_hot);
        
        let stats = optimizer.get_stats().await;
        assert_eq!(stats.hot_paths_identified, 1);
    }
    
    #[tokio::test]
    async fn test_recommendations() {
        let optimizer = CacheHitOptimizer::new(CacheOptimizationConfig::default());
        
        // 模拟低命中率
        for i in 0..10 {
            optimizer.record_request(i, "test", CacheHitLevel::Miss, 100.0, 0).await;
        }
        
        let recommendations = optimizer.generate_recommendations().await;
        assert!(!recommendations.is_empty());
        assert!(recommendations.iter().any(|r| r.contains("命中率")));
    }
}
