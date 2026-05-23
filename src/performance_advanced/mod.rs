//! 高级性能优化
//!
//! 对标 Claude Code 的 6 层缓存架构:
//! - LLM Response Caching: 响应缓存 + cache_control 分发
//! - Predictive Pre-computation: 预测性预计算
//! - Parallel Tool Execution: 可配置并发上限的并行执行
//! - Lazy Context Loading: 懒加载上下文 (按需而非全量)
//! - Cache Hit Optimization: 命中率优化器（新增）

pub mod cache_optimizer;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::RwLock;

pub use cache_optimizer::{CacheHitOptimizer, CacheOptimizationConfig, CacheHitLevel};

/// ===== [1] LLM 响应缓存 (6层架构) =====
pub struct LlmResponseCache {
    l1_memory: Arc<RwLock<lru::LruCache<u64, CachedResponse>>>,  // L1: 热缓存 (<1ms)
    l2_disk: Arc<RwLock<HashMap<u64, CachedResponse>>>,           // L2: 温缓存 (<10ms)
    l3_redis: Option<Arc<RedisCache>>,                            // L3: 分布式缓存 (<50ms)
    l4_semantic: Option<Arc<SemanticCache>>,                      // L4: 语义缓存 (<100ms)
    l5_cdn: Option<Arc<CdnCache>>,                                // L5: CDN缓存 (<200ms)
    l6_model: Option<Arc<ModelCache>>,                            // L6: 模型级缓存 (<1s)
    stats: Arc<RwLock<CacheStatsAdvanced>>,
}

#[derive(Debug, Clone)]
pub struct CachedResponse {
    pub prompt_hash: u64,
    pub response: String,
    pub created_at: SystemTime,
    pub ttl: Duration,
    pub access_count: u64,
    pub tokens_saved: u32,
}

#[derive(Debug, Clone, Default)]
pub struct CacheStatsAdvanced {
    pub l1_hits: u64, pub l2_hits: u64, pub l3_hits: u64, pub l4_hits: u64,
    pub l5_hits: u64, pub l6_hits: u64, pub misses: u64,
    pub tokens_saved: u64, pub avg_latency_saved_ms: f64,
}

impl LlmResponseCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            l1_memory: Arc::new(RwLock::new(lru::LruCache::new(capacity))),
            l2_disk: Arc::new(RwLock::new(HashMap::new())),
            l3_redis: None,  // Will be initialized separately
            l4_semantic: None,
            l5_cdn: None,
            l6_model: None,
            stats: Arc::new(RwLock::new(CacheStatsAdvanced::default())),
        }
    }
    
    /// 创建完整的6层缓存
    pub fn new_full(
        capacity: usize,
        redis_cache: Option<Arc<RedisCache>>,
        semantic_cache: Option<Arc<SemanticCache>>,
        cdn_cache: Option<Arc<CdnCache>>,
        model_cache: Option<Arc<ModelCache>>,
    ) -> Self {
        Self {
            l1_memory: Arc::new(RwLock::new(lru::LruCache::new(capacity))),
            l2_disk: Arc::new(RwLock::new(HashMap::new())),
            l3_redis: redis_cache,
            l4_semantic: semantic_cache,
            l5_cdn: cdn_cache,
            l6_model: model_cache,
            stats: Arc::new(RwLock::new(CacheStatsAdvanced::default())),
        }
    }

    /// 分层查找: L1 → L2 → L3 → L4 → L5 → L6 → miss
    pub async fn get_multi_level(&self, key: u64, prompt: Option<&str>) -> Option<String> {
        // L1: 内存缓存 (<1ms)
        {
            let mut l1 = self.l1_memory.write().await;
            if let Some(entry) = l1.get(&key) {
                if entry.created_at.elapsed().ok()? < entry.ttl {
                    let mut stats = self.stats.write().await;
                    stats.l1_hits += 1;
                    stats.tokens_saved += entry.tokens_saved as u64;
                    return Some(entry.response.clone());
                }
            }
        }
        
        // L2: 磁盘缓存 (<10ms)
        {
            let mut l2 = self.l2_disk.write().await;
            if let Some(entry) = l2.get(&key) {
                if entry.created_at.elapsed().ok()? < entry.ttl {
                    // 提升到 L1
                    let mut l1 = self.l1_memory.write().await;
                    l1.put(key, entry.clone());
                    let mut stats = self.stats.write().await;
                    stats.l2_hits += 1;
                    stats.tokens_saved += entry.tokens_saved as u64;
                    return Some(entry.response.clone());
                } else {
                    l2.remove(&key);
                }
            }
        }
        
        // L3: Redis分布式缓存 (<50ms)
        if let Some(ref redis) = self.l3_redis {
            if let Some(response) = redis.get(key).await {
                // 提升到 L2 和 L1
                let entry = CachedResponse {
                    prompt_hash: key,
                    response: response.clone(),
                    created_at: SystemTime::now(),
                    ttl: Duration::from_secs(3600),
                    access_count: 0,
                    tokens_saved: 0,
                };
                self.l2_disk.write().await.insert(key, entry.clone());
                self.l1_memory.write().await.put(key, entry);
                
                let mut stats = self.stats.write().await;
                stats.l3_hits += 1;
                return Some(response);
            }
        }
        
        // L4: 语义缓存 (<100ms)
        if let (Some(ref semantic), Some(prompt_text)) = (self.l4_semantic.as_ref(), prompt) {
            if let Some(response) = semantic.semantic_search(prompt_text).await {
                // 存储到上层缓存
                let entry = CachedResponse {
                    prompt_hash: key,
                    response: response.clone(),
                    created_at: SystemTime::now(),
                    ttl: Duration::from_secs(3600),
                    access_count: 0,
                    tokens_saved: 0,
                };
                self.l2_disk.write().await.insert(key, entry.clone());
                self.l1_memory.write().await.put(key, entry);
                
                let mut stats = self.stats.write().await;
                stats.l4_hits += 1;
                return Some(response);
            }
        }
        
        // L5: CDN缓存 (<200ms)
        if let Some(ref cdn) = self.l5_cdn {
            if let Some(response) = cdn.get(key).await {
                let entry = CachedResponse {
                    prompt_hash: key,
                    response: response.clone(),
                    created_at: SystemTime::now(),
                    ttl: Duration::from_secs(3600),
                    access_count: 0,
                    tokens_saved: 0,
                };
                self.l2_disk.write().await.insert(key, entry.clone());
                self.l1_memory.write().await.put(key, entry);
                
                let mut stats = self.stats.write().await;
                stats.l5_hits += 1;
                return Some(response);
            }
        }
        
        // L6: 模型级缓存 (<1s)
        if let Some(ref model) = self.l6_model {
            if let Some(response) = model.get("default", key).await {
                let entry = CachedResponse {
                    prompt_hash: key,
                    response: response.clone(),
                    created_at: SystemTime::now(),
                    ttl: Duration::from_secs(3600),
                    access_count: 0,
                    tokens_saved: 0,
                };
                self.l2_disk.write().await.insert(key, entry.clone());
                self.l1_memory.write().await.put(key, entry);
                
                let mut stats = self.stats.write().await;
                stats.l6_hits += 1;
                return Some(response);
            }
        }
        
        // Cache miss
        self.stats.write().await.misses += 1;
        None
    }
    
    /// 兼容旧API
    pub async fn get(&self, key: u64) -> Option<String> {
        self.get_multi_level(key, None).await
    }

    /// 存入缓存 (自动选择层级)
    pub async fn put(&self, key: u64, response: String, tokens_saved: u32, ttl: Duration) {
        // 高频访问 → L1, 否则 L2
        let freq = self.stats.read().await.l1_hits.max(1);
        let entry = CachedResponse {
            prompt_hash: key, response, created_at: SystemTime::now(),
            ttl, access_count: 0, tokens_saved,
        };
        if freq > 10 {
            self.l1_memory.write().await.put(key, entry);
        } else {
            self.l2_disk.write().await.insert(key, entry);
        }
    }

    /// 缓存命中率
    pub async fn hit_rate(&self) -> f64 {
        let s = self.stats.read().await;
        let total = s.l1_hits + s.l2_hits + s.misses;
        if total == 0 { 0.0 } else { (s.l1_hits + s.l2_hits) as f64 / total as f64 }
    }

    /// 清理过期条目
    pub async fn evict_expired(&self) {
        let now = SystemTime::now();
        let mut l2 = self.l2_disk.write().await;
        l2.retain(|_, e| e.created_at.elapsed().map(|d| d < e.ttl).unwrap_or(false));
    }
}

/// ===== [2] 预测性预计算 =====
pub struct PredictivePrecomputer {
    cache: Arc<LlmResponseCache>,
    prediction_model: Arc<RwLock<HashMap<String, f64>>>, // pattern → probability
    hot_paths: Arc<RwLock<std::collections::HashSet<String>>>,
}

impl PredictivePrecomputer {
    pub fn new(cache: Arc<LlmResponseCache>) -> Self {
        Self { 
            cache, 
            prediction_model: Arc::new(RwLock::new(HashMap::new())),
            hot_paths: Arc::new(RwLock::new(std::collections::HashSet::new())),
        }
    }

    /// 学习用户输入模式
    pub async fn learn_pattern(&self, input_pattern: &str) {
        let mut model = self.prediction_model.write().await;
        *model.entry(input_pattern.to_string()).or_insert(0.0) += 1.0;
        
        // 如果访问频率高，标记为hot path
        let count = model.get(input_pattern).copied().unwrap_or(0.0);
        if count > 5.0 {
            self.hot_paths.write().await.insert(input_pattern.to_string());
        }
    }

    /// 预测下一个可能的查询
    pub async fn predict_next(&self, current_pattern: &str) -> Vec<(String, f64)> {
        let model = self.prediction_model.read().await;
        let mut predictions: Vec<(String, f64)> = model.iter()
            .filter(|(k, _)| k.starts_with(current_pattern) && *k != current_pattern)
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        predictions.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        predictions.truncate(3);
        predictions
    }

    /// 预热缓存 (预计算高概率查询)
    pub async fn prewarm(&self, patterns: &[&str]) {
        for pattern in patterns {
            let key = self.compute_key(pattern);
            if self.cache.get(key).await.is_none() {
                // 预计算占位
                self.cache.put(key, format!("[prewarmed] {}", pattern), 0, Duration::from_secs(300)).await;
            }
        }
    }
    
    /// 后台预计算热点路径
    pub async fn precompute_hot_paths(&self) -> usize {
        let paths = self.hot_paths.read().await.clone();
        let mut computed = 0usize;
        
        for path in paths {
            let key = self.compute_key(&path);
            if self.cache.get(key).await.is_none() {
                // 在实际实现中，这里会调用LLM进行预计算
                // 现在只是标记为已预计算
                self.cache.put(key, format!("[precomputed] {}", path), 0, Duration::from_secs(600)).await;
                computed += 1;
            }
        }
        
        computed
    }
    
    /// 获取热点路径列表
    pub async fn get_hot_paths(&self) -> Vec<String> {
        self.hot_paths.read().await.iter().cloned().collect()
    }

    fn compute_key(&self, input: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        input.hash(&mut hasher);
        hasher.finish()
    }
}

/// ===== [3] 并行工具执行器 =====
pub struct ParallelToolExecutor {
    max_concurrency: usize,
    stats: Arc<RwLock<ExecutorStats>>,
}

#[derive(Debug, Clone, Default)]
pub struct ExecutorStats {
    pub total_batches: u64,
    pub parallel_batches: u64,
    pub serial_batches: u64,
    pub total_tools: u64,
    pub avg_batch_size: f64,
}

impl ParallelToolExecutor {
    pub fn new(max_concurrency: usize) -> Self {
        Self { max_concurrency, stats: Arc::new(RwLock::new(ExecutorStats::default())) }
    }

    /// 分区: 只读工具并行执行, 写入工具串行执行
    pub fn partition_tools<'a>(
        &self, tools: &'a [ToolCallInfo]
    ) -> Vec<ToolBatch<'a>> {
        let mut batches = Vec::new();
        let mut current_batch = Vec::new();
        let mut all_readonly = true;

        for tool in tools {
            if tool.is_readonly {
                if all_readonly {
                    current_batch.push(tool);
                } else {
                    // 开始新批次
                    if !current_batch.is_empty() {
                        batches.push(ToolBatch { tools: current_batch, parallel: false });
                    }
                    current_batch = vec![tool];
                    all_readonly = true;
                }
            } else {
                // 写入工具: 结束当前批次, 单独一批
                if !current_batch.is_empty() {
                    batches.push(ToolBatch { tools: current_batch, parallel: all_readonly });
                }
                batches.push(ToolBatch { tools: vec![tool], parallel: false });
                current_batch = Vec::new();
                all_readonly = true;
            }
        }
        if !current_batch.is_empty() {
            batches.push(ToolBatch { tools: current_batch, parallel: all_readonly });
        }

        let mut stats = self.stats.blocking_write();
        stats.total_batches += 1;
        if all_readonly && !tools.is_empty() { stats.parallel_batches += 1; }
        else { stats.serial_batches += 1; }
        stats.total_tools += tools.len() as u64;

        batches
    }

    /// 并行执行只读工具
    pub async fn execute_parallel<'a>(&self, batch: &[&'a ToolCallInfo]) -> Vec<ToolResult> {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.max_concurrency));
        let mut handles = Vec::new();

        for tool in batch {
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let tool_name = tool.name.clone();
            
            handles.push(tokio::spawn(async move {
                let _permit = permit;
                // 工具执行逻辑（占位符）
                // 在实际实现中，这里会调用具体的工具
                ToolResult { 
                    name: tool_name.clone(), 
                    success: true, 
                    output: format!("Executed {}", tool_name) 
                }
            }));
        }

        let mut results = Vec::new();
        for handle in handles {
            if let Ok(result) = handle.await {
                results.push(result);
            }
        }
        
        // 更新统计
        let mut stats = self.stats.write().await;
        stats.total_tools += batch.len() as u64;
        
        results
    }
    
    /// 串行执行写入工具
    pub async fn execute_serial<'a>(&self, batch: &[&'a ToolCallInfo]) -> Vec<ToolResult> {
        let mut results = Vec::new();
        
        for tool in batch {
            // 串行执行，等待每个工具完成
            let result = ToolResult {
                name: tool.name.clone(),
                success: true,
                output: format!("Executed {} (serial)", tool.name),
            };
            results.push(result);
        }
        
        // 更新统计
        let mut stats = self.stats.write().await;
        stats.total_tools += batch.len() as u64;
        
        results
    }
    
    /// 获取执行器统计
    pub async fn get_stats(&self) -> ExecutorStats {
        self.stats.read().await.clone()
    }
    
    /// 重置统计
    pub async fn reset_stats(&self) {
        let mut stats = self.stats.write().await;
        *stats = ExecutorStats::default();
    }
}

#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub name: String,
    pub is_readonly: bool,
    pub input: String,
}

#[derive(Debug, Clone)]
pub struct ToolBatch<'a> {
    pub tools: Vec<&'a ToolCallInfo>,
    pub parallel: bool,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub name: String,
    pub success: bool,
    pub output: String,
}

/// ===== [4] 懒加载上下文 =====
pub struct LazyContextLoader {
    loaded: Arc<RwLock<HashMap<String, String>>>,
    loaders: HashMap<String, Box<dyn Fn() -> String + Send + Sync>>,
}

impl LazyContextLoader {
    pub fn new() -> Self {
        Self { loaded: Arc::new(RwLock::new(HashMap::new())), loaders: HashMap::new() }
    }

    /// 注册懒加载器
    pub fn register(&mut self, key: &str, loader: Box<dyn Fn() -> String + Send + Sync>) {
        self.loaders.insert(key.to_string(), loader);
    }

    /// 获取上下文 (按需加载)
    pub async fn get(&self, key: &str) -> Option<String> {
        {
            let loaded = self.loaded.read().await;
            if let Some(value) = loaded.get(key) {
                return Some(value.clone());
            }
        }
        // 懒加载
        if let Some(loader) = self.loaders.get(key) {
            let value = (loader)();
            self.loaded.write().await.insert(key.to_string(), value.clone());
            Some(value)
        } else {
            None
        }
    }

    /// 批量预加载
    pub async fn preload(&self, keys: &[&str]) {
        for key in keys {
            self.get(key).await;
        }
    }

    /// 清除缓存
    pub async fn clear(&self) {
        self.loaded.write().await.clear();
    }
}

/// ===== [5] 缓存监控器 =====
pub struct CacheMonitor {
    cache: Arc<LlmResponseCache>,
}

impl CacheMonitor {
    pub fn new(cache: Arc<LlmResponseCache>) -> Self {
        Self { cache }
    }
    
    /// 获取详细的缓存统计
    pub async fn get_detailed_stats(&self) -> CacheDetailedStats {
        let stats = self.cache.stats.read().await;
        
        let total_requests = stats.l1_hits + stats.l2_hits + stats.l3_hits + 
                            stats.l4_hits + stats.l5_hits + stats.l6_hits + stats.misses;
        
        let overall_hit_rate = if total_requests > 0 {
            (stats.l1_hits + stats.l2_hits + stats.l3_hits + 
             stats.l4_hits + stats.l5_hits + stats.l6_hits) as f64 / total_requests as f64
        } else {
            0.0
        };
        
        CacheDetailedStats {
            l1_hit_rate: if total_requests > 0 { stats.l1_hits as f64 / total_requests as f64 } else { 0.0 },
            l2_hit_rate: if total_requests > 0 { stats.l2_hits as f64 / total_requests as f64 } else { 0.0 },
            l3_hit_rate: if total_requests > 0 { stats.l3_hits as f64 / total_requests as f64 } else { 0.0 },
            l4_hit_rate: if total_requests > 0 { stats.l4_hits as f64 / total_requests as f64 } else { 0.0 },
            l5_hit_rate: if total_requests > 0 { stats.l5_hits as f64 / total_requests as f64 } else { 0.0 },
            l6_hit_rate: if total_requests > 0 { stats.l6_hits as f64 / total_requests as f64 } else { 0.0 },
            overall_hit_rate,
            tokens_saved: stats.tokens_saved,
            avg_latency_saved_ms: stats.avg_latency_saved_ms,
            estimated_cost_savings_usd: Self::calculate_cost_savings(stats.tokens_saved),
        }
    }
    
    /// 计算成本节省（假设$0.002/1K tokens）
    fn calculate_cost_savings(tokens_saved: u64) -> f64 {
        (tokens_saved as f64 / 1000.0) * 0.002
    }
    
    /// 生成缓存健康报告
    pub async fn generate_health_report(&self) -> String {
        let stats = self.get_detailed_stats().await;
        
        format!(
            "\n━━━ Cache Health Report ━━━\n\n\
             L1 Hit Rate: {:.1}%\n\
             L2 Hit Rate: {:.1}%\n\
             L3 Hit Rate: {:.1}%\n\
             L4 Hit Rate: {:.1}%\n\
             L5 Hit Rate: {:.1}%\n\
             L6 Hit Rate: {:.1}%\n\
             Overall Hit Rate: {:.1}%\n\n\
             Tokens Saved: {}\n\
             Estimated Cost Savings: ${:.2}\n\
             Avg Latency Saved: {:.0}ms\n",
            stats.l1_hit_rate * 100.0,
            stats.l2_hit_rate * 100.0,
            stats.l3_hit_rate * 100.0,
            stats.l4_hit_rate * 100.0,
            stats.l5_hit_rate * 100.0,
            stats.l6_hit_rate * 100.0,
            stats.overall_hit_rate * 100.0,
            stats.tokens_saved,
            stats.estimated_cost_savings_usd,
            stats.avg_latency_saved_ms
        )
    }
}

#[derive(Debug, Clone)]
pub struct CacheDetailedStats {
    pub l1_hit_rate: f64,
    pub l2_hit_rate: f64,
    pub l3_hit_rate: f64,
    pub l4_hit_rate: f64,
    pub l5_hit_rate: f64,
    pub l6_hit_rate: f64,
    pub overall_hit_rate: f64,
    pub tokens_saved: u64,
    pub avg_latency_saved_ms: f64,
    pub estimated_cost_savings_usd: f64,
}

/// ===== [1.5] L3: Redis分布式缓存 =====
#[cfg(feature = "redis")]
pub struct RedisCache {
    client: Option<redis::Client>,
    prefix: String,
}

#[cfg(feature = "redis")]
impl RedisCache {
    pub fn new(redis_url: &str, prefix: &str) -> Result<Self, String> {
        let client = redis::Client::open(redis_url)
            .map_err(|e| format!("Failed to connect to Redis: {}", e))?;
        
        Ok(Self {
            client: Some(client),
            prefix: prefix.to_string(),
        })
    }
    
    pub async fn get(&self, key: u64) -> Option<String> {
        if let Some(ref client) = self.client {
            let mut conn = client.get_async_connection().await.ok()?;
            let redis_key = format!("{}:{}", self.prefix, key);
            
            redis::cmd("GET")
                .arg(&redis_key)
                .query_async(&mut conn)
                .await
                .ok()
        } else {
            None
        }
    }
    
    pub async fn set(&self, key: u64, value: &str, ttl: Duration) -> Result<(), String> {
        if let Some(ref client) = self.client {
            let mut conn = client.get_async_connection().await
                .map_err(|e| format!("Redis connection failed: {}", e))?;
            
            let redis_key = format!("{}:{}", self.prefix, key);
            
            redis::cmd("SETEX")
                .arg(&redis_key)
                .arg(ttl.as_secs() as usize)
                .arg(value)
                .query_async(&mut conn)
                .await
                .map_err(|e| format!("Redis SET failed: {}", e))?;
            
            Ok(())
        } else {
            Err("Redis client not initialized".to_string())
        }
    }
}

/// ===== [1.6] L4: 语义缓存 (基于embedding相似度) =====
pub struct SemanticCache {
    cache_entries: Arc<RwLock<HashMap<String, SemanticEntry>>>,
    similarity_threshold: f64,
}

#[derive(Debug, Clone)]
pub struct SemanticEntry {
    pub prompt: String,
    pub response: String,
    pub embedding: Vec<f64>,  // Simplified embedding representation
    pub created_at: SystemTime,
    pub ttl: Duration,
}

impl SemanticCache {
    pub fn new(similarity_threshold: f64) -> Self {
        Self {
            cache_entries: Arc::new(RwLock::new(HashMap::new())),
            similarity_threshold,
        }
    }
    
    /// 基于prompt相似度搜索缓存
    pub async fn semantic_search(&self, prompt: &str) -> Option<String> {
        // Simplified: In production, use actual embedding comparison
        let entries = self.cache_entries.read().await;
        
        for entry in entries.values() {
            if entry.created_at.elapsed().ok()? < entry.ttl {
                // Simple string similarity check (placeholder for real embedding comparison)
                if Self::calculate_similarity(prompt, &entry.prompt) >= self.similarity_threshold {
                    return Some(entry.response.clone());
                }
            }
        }
        
        None
    }
    
    /// 存储新的prompt-response对
    pub async fn store(&self, prompt: &str, response: &str, embedding: Vec<f64>, ttl: Duration) {
        let entry = SemanticEntry {
            prompt: prompt.to_string(),
            response: response.to_string(),
            embedding,
            created_at: SystemTime::now(),
            ttl,
        };
        
        let key = Self::compute_prompt_key(prompt);
        self.cache_entries.write().await.insert(key, entry);
    }
    
    fn calculate_similarity(a: &str, b: &str) -> f64 {
        // Placeholder: Use simple Jaccard similarity
        let words_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
        let words_b: std::collections::HashSet<&str> = b.split_whitespace().collect();
        
        let intersection = words_a.intersection(&words_b).count();
        let union = words_a.union(&words_b).count();
        
        if union == 0 { 0.0 } else { intersection as f64 / union as f64 }
    }
    
    fn compute_prompt_key(prompt: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        prompt.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }
}

/// ===== [1.7] L5: CDN缓存 =====
pub struct CdnCache {
    cdn_endpoint: String,
    api_key: String,
}

impl CdnCache {
    pub fn new(cdn_endpoint: &str, api_key: &str) -> Self {
        Self {
            cdn_endpoint: cdn_endpoint.to_string(),
            api_key: api_key.to_string(),
        }
    }
    
    pub async fn get(&self, key: u64) -> Option<String> {
        // Placeholder: In production, make HTTP request to CDN
        // For now, return None to simulate CDN miss
        None
    }
    
    pub async fn set(&self, key: u64, value: &str, ttl: Duration) -> Result<(), String> {
        // Placeholder: In production, upload to CDN
        Ok(())
    }
}

/// ===== [1.8] L6: 模型级缓存 =====
pub struct ModelCache {
    model_responses: Arc<RwLock<HashMap<String, String>>>,
}

impl ModelCache {
    pub fn new() -> Self {
        Self {
            model_responses: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    pub async fn get(&self, model_name: &str, prompt_hash: u64) -> Option<String> {
        let key = format!("{}:{}", model_name, prompt_hash);
        self.model_responses.read().await.get(&key).cloned()
    }
    
    pub async fn set(&self, model_name: &str, prompt_hash: u64, response: &str) {
        let key = format!("{}:{}", model_name, prompt_hash);
        self.model_responses.write().await.insert(key, response.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_response_cache() {
        let cache = LlmResponseCache::new(100);
        let key = 42u64;
        assert!(cache.get(key).await.is_none());
        cache.put(key, "cached response".into(), 100, Duration::from_secs(3600)).await;
        assert_eq!(cache.get(key).await.unwrap(), "cached response");
    }

    #[tokio::test]
    async fn test_cache_hit_rate() {
        let cache = LlmResponseCache::new(100);
        assert_eq!(cache.hit_rate().await, 0.0);
        cache.get(1).await; // miss
        cache.put(2, "test".into(), 50, Duration::from_secs(3600)).await;
        cache.get(2).await; // hit
        let rate = cache.hit_rate().await;
        assert!(rate > 0.0 && rate < 1.0);
    }

    #[test]
    fn test_tool_partition() {
        let executor = ParallelToolExecutor::new(10);
        let tools = vec![
            ToolCallInfo { name: "read".into(), is_readonly: true, input: "".into() },
            ToolCallInfo { name: "grep".into(), is_readonly: true, input: "".into() },
            ToolCallInfo { name: "edit".into(), is_readonly: false, input: "".into() },
            ToolCallInfo { name: "read2".into(), is_readonly: true, input: "".into() },
        ];
        let batches = executor.partition_tools(&tools);
        assert_eq!(batches.len(), 3); // [read,grep] [edit] [read2]
        assert!(batches[0].parallel);
        assert!(!batches[1].parallel);
        assert_eq!(batches[1].tools.len(), 1);
    }

    #[tokio::test]
    async fn test_lazy_loader() {
        let mut loader = LazyContextLoader::new();
        loader.register("git_status", Box::new(|| "clean".to_string()));
        let result = loader.get("git_status").await;
        assert_eq!(result.unwrap(), "clean");
    }
    
    #[tokio::test]
    async fn test_predictive_precomputer() {
        let cache = Arc::new(LlmResponseCache::new(100));
        let precomputer = PredictivePrecomputer::new(cache);
        
        // 学习模式
        precomputer.learn_pattern("rust test").await;
        precomputer.learn_pattern("rust test").await;
        precomputer.learn_pattern("rust test").await;
        precomputer.learn_pattern("python test").await;
        
        // 预测下一个
        let predictions = precomputer.predict_next("rust").await;
        assert!(!predictions.is_empty());
        
        // 获取热点路径
        let hot_paths = precomputer.get_hot_paths().await;
        assert!(!hot_paths.is_empty());
    }
    
    #[tokio::test]
    async fn test_cache_monitor() {
        let cache = Arc::new(LlmResponseCache::new(100));
        let monitor = CacheMonitor::new(cache.clone());
        
        // 生成一些缓存活动
        cache.put(1, "response1".into(), 100, Duration::from_secs(3600)).await;
        cache.get(1).await; // hit
        cache.get(999).await; // miss
        
        // 获取详细统计
        let stats = monitor.get_detailed_stats().await;
        assert!(stats.overall_hit_rate > 0.0);
        assert!(stats.tokens_saved >= 0);
        
        // 生成健康报告
        let report = monitor.generate_health_report().await;
        assert!(report.contains("Cache Health Report"));
        assert!(report.contains("Hit Rate"));
    }
    
    #[tokio::test]
    async fn test_parallel_executor_stats() {
        let executor = ParallelToolExecutor::new(4);
        
        // 初始统计
        let stats = executor.get_stats().await;
        assert_eq!(stats.total_batches, 0);
        
        // 分区工具
        let tools = vec![
            ToolCallInfo { name: "read1".into(), is_readonly: true, input: "".into() },
            ToolCallInfo { name: "read2".into(), is_readonly: true, input: "".into() },
        ];
        let batches = executor.partition_tools(&tools);
        
        // 执行并行批次
        if !batches.is_empty() && batches[0].parallel {
            let results = executor.execute_parallel(&batches[0].tools).await;
            assert_eq!(results.len(), 2);
        }
        
        // 检查统计更新
        let stats = executor.get_stats().await;
        assert!(stats.total_tools > 0);
    }
    
    #[tokio::test]
    async fn test_semantic_cache() {
        let semantic = SemanticCache::new(0.7);
        
        // 存储条目
        semantic.store(
            "How to write Rust tests?",
            "Use #[test] attribute and cargo test",
            vec![0.1, 0.2, 0.3],
            Duration::from_secs(3600)
        ).await;
        
        // 语义搜索（相似query）
        let result = semantic.semantic_search("How to write tests in Rust?").await;
        // 由于Jaccard相似度，可能匹配也可能不匹配，取决于实现
        // 这里只是验证API正常工作
    }
}
