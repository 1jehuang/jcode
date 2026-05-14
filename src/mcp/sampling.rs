//! # MCP Sampling能力
//!
//! 实现MCP协议的sampling功能，允许通过MCP调用LLM：
//! - **LLM Provider抽象** - 支持多种后端模型
//! - **结果缓存** - 避免重复计算
//! - **速率限制** - 防止API滥用
//! - **Token追踪** - 监控用量和成本
//!
//! ## 使用示例
//!
//! ```rust
//! use carpai::mcp::sampling::{SamplingHandler, SamplingRequest, SamplingResponse};
//!
//! let handler = SamplingHandler::with_provider(MyProvider::new());
//!
//! let request = SamplingRequest {
//!     role: SamplingRole::User,
//!     content: "Explain Rust ownership".to_string(),
//!     model: Some("claude-3-5-sonnet-20241022".to_string()),
//!     max_tokens: Some(1024),
//!     ..Default::default()
//! };
//!
//! let response = handler.handle_request(request).await?;
//! println!("Generated: {}", response.content);
//! ```

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// LLM Provider trait - 抽象不同LLM后端
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// 生成文本
    async fn generate(&self, request: &SamplingRequest) -> Result<SamplingResponse, SamplingError>;
    
    /// 获取模型信息
    fn model_info(&self) -> ModelInfo;
    
    /// 检查是否支持特定功能
    fn supports_feature(&self, feature: &str) -> bool;
}

/// 模型信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub version: String,
    pub max_context_tokens: u32,
    pub max_output_tokens: u32,
}

/// Sampling请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplingRequest {
    /// 角色类型
    pub role: SamplingRole,
    
    /// 输入内容
    pub content: String,
    
    /// 模型名称（可选，使用默认）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    
    /// 系统提示词
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    
    /// 最大生成token数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    
    /// 温度参数 (0.0-2.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    
    /// Top-p采样 (0.0-1.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    
    /// 停止序列
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    
    /// 元数据
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// 采样角色
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SamplingRole {
    User,
    Assistant,
    System,
}

impl std::fmt::Display for SamplingRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SamplingRole::User => write!(f, "user"),
            SamplingRole::Assistant => write!(f, "assistant"),
            SamplingRole::System => write!(f, "system"),
        }
    }
}

/// 采样响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplingResponse {
    /// 生成的内容
    pub content: String,
    
    /// 使用的模型
    pub model: String,
    
    /// 停止原因
    pub stop_reason: StopReason,
    
    /// Token使用统计
    pub usage: TokenUsage,
    
    /// 生成耗时(ms)
    pub duration_ms: u64,
    
    /// 对应的请求ID（用于日志关联）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// 停止原因
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_CASE")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    Other(String),
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StopReason::EndTurn => write!(f, "end_turn"),
            StopReason::MaxTokens => write!(f, "max_tokens"),
            StopReason::StopSequence => write!(f, "stop_sequence"),
            StopReason::ToolUse => write!(f, "tool_use"),
            StopReason::Other(reason) => write!(f, "{}", reason),
        }
    }
}

/// Token使用统计
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

impl TokenUsage {
    pub fn new(input: u32, output: u32) -> Self {
        Self {
            input_tokens: input,
            output_tokens: output,
            total_tokens: input + output,
        }
    }
}

/// Sampling错误
#[derive(Debug, thiserror::Error)]
pub enum SamplingError {
    #[error("Provider error: {0}")]
    Provider(String),
    
    #[error("Rate limited: retry after {0}s")]
    RateLimited(u64),
    
    #[error("Context too long: {0} tokens > {1} limit")]
    ContextTooLong(u32, u32),
    
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    
    #[error("Timeout error")]
    Timeout,
    
    #[error("Authentication failed")]
    AuthenticationFailed,
    
    #[error("Model not found: {0}")]
    ModelNotFound(String),
}

// ════════════════════════════
// Sampling Handler 核心
// ════════════════════════════

/// 速率限制器
struct RateLimiter {
    requests_per_minute: u32,
    timestamps: Arc<RwLock<std::collections::VecDeque<std::time::Instant>>>,
}

impl RateLimiter {
    fn new(requests_per_minute: u32) -> Self {
        Self {
            requests_per_minute: requests_per_minute,
            timestamps: Arc::new(RwLock::new(std::collections::VecDeque::new())),
        }
    }

    async fn check_rate_limit(&self) -> Result<(), SamplingError> {
        let now = std::time::Instant::now();
        
        {
            let mut timestamps = self.timestamps.write().await;
            
            // 清理1分钟前的记录
            while let Some(front) = timestamps.front() {
                if now.duration_since(*front) > std::time::Duration::from_secs(60) {
                    timestamps.pop_front();
                } else {
                    break;
                }
            }

            // 检查是否超限
            if timestamps.len() as u32 >= self.requests_per_minute {
                return Err(SamplingError::RateLimited(60));
            }

            timestamps.push_back(now);
        }

        Ok(())
    }
}

/// 结果缓存条目
struct CacheEntry {
    response: SamplingResponse,
    created_at: std::time::Instant,
    ttl: std::time::Duration,
}

/// Sampling处理器核心
pub struct SamplingHandler {
    /// LLM Provider
    provider: Arc<dyn LlmProvider>,
    
    /// 结果缓存
    cache: Arc<RwLock<HashMap<String, CacheEntry>>>,
    
    /// 速率限制器
    rate_limiter: Arc<RateLimiter>,
    
    /// 统计信息
    stats: Arc<RwLock<SamplingStats>>,
    
    /// 默认配置
    default_config: SamplingConfig,
}

/// 采样配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplingConfig {
    /// 是否启用缓存
    pub enable_cache: bool,
    
    /// 缓存TTL (秒)
    pub cache_ttl_secs: u64,
    
    /// 最大缓存条目数
    pub max_cache_size: usize,
    
    /// 是否启用速率限制
    pub enable_rate_limit: bool,
    
    /// 每分钟最大请求数
    pub requests_per_minute: u32,
    
    /// 超时时间 (秒)
    pub timeout_secs: u64,
    
    /// 默认最大token数
    pub default_max_tokens: u32,
    
    /// 默认温度
    pub default_temperature: f64,
}

impl Default for SamplingConfig {
    fn default() -> Self {
        Self {
            enable_cache: true,
            cache_ttl_secs: 300, // 5分钟
            max_cache_size: 1000,
            enable_rate_limit: true,
            requests_per_minute: 20,
            timeout_secs: 120,
            default_max_tokens: 4096,
            default_temperature: 0.7,
        }
    }
}

/// 采样统计信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SamplingStats {
    pub total_requests: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub errors: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub avg_response_time_ms: f64,
}

impl SamplingStats {
    pub fn cache_hit_rate(&self) -> f64 {
        let total = self.cache_hits + self.cache_misses;
        if total == 0 {
            0.0
        } else {
            self.cache_hits as f64 / total as f64
        }
    }
}

impl SamplingHandler {
    /// 创建新的Sampling处理器
    pub fn new<P: LlmProvider + 'static>(provider: P) -> Self {
        Self::with_config(provider, SamplingConfig::default())
    }

    /// 使用自定义配置创建
    pub fn with_config<P: LlmProvider + 'static>(provider: P, config: SamplingConfig) -> Self {
        Self {
            provider: Arc::new(provider),
            cache: Arc::new(RwLock::new(HashMap::new())),
            rate_limiter: Arc::new(RateLimiter::new(config.requests_per_minute)),
            stats: Arc::new(RwLock::new(SamplingStats::default())),
            default_config: config,
        }
    }

    /// 处理单个sampling请求
    pub async fn handle_request(&self, mut request: SamplingRequest) -> Result<SamplingResponse, SamplingError> {
        let start_time = std::time::Instant::now();

        // 1. 应用默认配置
        self.apply_defaults(&mut request);

        // 2. 速率限制检查
        if self.default_config.enable_rate_limit {
            self.rate_limiter.check_rate_limit().await?;
        }

        // 3. 缓存查找
        if self.default_config.enable_cache {
            let cache_key = self.generate_cache_key(&request);
            
            {
                let cache = self.cache.read().await;
                if let Some(entry) = cache.get(&cache_key) {
                    if entry.created_at.elapsed() < entry.ttl {
                        // 缓存命中
                        let mut stats = self.stats.write().await;
                        stats.cache_hits += 1;
                        return Ok(entry.response.clone());
                    }
                }
            } // 读锁释放
        }

        // 4. 调用Provider生成
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(self.default_config.timeout_secs),
            self.provider.generate(&request)
        ).await
        .map_err(|_| SamplingError::Timeout)?
        .map_err(|e| e)?;

        // 5. 计算耗时
        let duration_ms = start_time.elapsed().as_millis() as u64;

        // 6. 更新统计
        {
            let mut stats = self.stats.write().await;
            stats.total_requests += 1;
            stats.total_input_tokens += result.usage.input_tokens as u64;
            stats.total_output_tokens += result.usage.output_tokens as u64;
            stats.errors = stats.errors; // 保持不变
            
            // 更新平均响应时间
            let total_time = stats.avg_response_time_ms * (stats.total_requests - 1) as f64;
            stats.avg_response_time_ms = (total_time + duration_ms as f64) / stats.total_requests as f64;

            if self.default_config.enable_cache {
                stats.cache_misses += 1;
            }
        }

        // 7. 存入缓存
        if self.default_config.enable_cache {
            let cache_key = self.generate_cache_key(&request);
            let mut cache = self.cache.write().await;
            
            // 淘汰过期/超额条目
            if cache.len() >= self.default_config.max_cache_size {
                Self::evict_old_entries(&mut cache);
            }

            cache.insert(cache_key, CacheEntry {
                response: result.clone(),
                created_at: std::time::Instant::now(),
                ttl: std::time::Duration::from_secs(self.default_config.cache_ttl_secs),
            });
        }

        Ok(result)
    }

    /// 批量处理多个请求
    pub async fn batch_handle(
        &self,
        requests: Vec<SamplingRequest>,
    ) -> Vec<Result<SamplingResponse, SamplingError>> {
        let mut results = Vec::with_capacity(requests.len());

        for request in requests {
            let result = self.handle_request(request).await;
            results.push(result);
        }

        results
    }

    /// 清除缓存
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
        
        let mut stats = self.stats.write().await;
        stats.cache_hits = 0;
        stats.cache_misses = 0;
    }

    /// 获取统计信息
    pub async fn get_statistics(&self) -> SamplingStats {
        self.stats.read().await.clone()
    }

    /// 获取缓存大小
    pub async fn cache_size(&self) -> usize {
        self.cache.read().await.len()
    }

    // ════════════════════════════
    // 内部方法
    // ════════════════════════════

    fn apply_defaults(&self, request: &mut SamplingRequest) {
        if request.max_tokens.is_none() {
            request.max_tokens = Some(self.default_config.default_max_tokens);
        }
        if request.temperature.is_none() {
            request.temperature = Some(self.default_config.default_temperature);
        }
        if request.model.is_none() {
            request.model = Some(self.provider.model_info().name.clone());
        }
    }

    fn generate_cache_key(&self, request: &SamplingRequest) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        request.role.hash(&mut hasher);
        request.content.hash(&mut hasher);
        if let Some(model) = &request.model {
            model.hash(&mut hasher);
        }
        if let Some(max_tokens) = request.max_tokens {
            max_tokens.hash(&mut hasher);
        }
        format!("{:x}", hasher.finish())
    }

    fn evict_old_entries(cache: &mut HashMap<String, CacheEntry>) {
        // 简单策略：删除最旧的25%条目
        let remove_count = cache.len() / 4;
        
        let mut entries: Vec<_> = cache.iter().collect();
        entries.sort_by(|a, b| a.1.created_at.cmp(&b.1.created_at));

        for (key, _) in entries.into_iter().take(remove_count) {
            cache.remove(key);
        }
    }
}

// ════════════════════════════
// Mock Provider (用于测试)
// ════════════════════════════

/// Mock LLM Provider用于测试
pub struct MockLlmProvider {
    model_name: String,
    delay_ms: u64,
}

impl MockLlmProvider {
    pub fn new(model_name: &str, delay_ms: u64) -> Self {
        Self {
            model_name: model_name.to_string(),
            delay_ms,
        }
    }
}

#[async_trait]
impl LlmProvider for MockLlmProvider {
    async fn generate(&self, request: &SamplingRequest) -> Result<SamplingResponse, SamplingError> {
        // 模拟延迟
        tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;

        // 根据内容长度模拟输出
        let output_length = (request.content.len() as f64 * 0.5).min(100.0) as u32;

        Ok(SamplingResponse {
            content: format!("[Mock Response] {}", request.content),
            model: self.model_name.clone(),
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage::new(request.content.len() as u32, output_length),
            duration_ms: self.delay_ms,
            request_id: None,
        })
    }

    fn model_info(&self) -> ModelInfo {
        ModelInfo {
            name: self.model_name.clone(),
            version: "mock-1.0".to_string(),
            max_context_tokens: 100000,
            max_output_tokens: 8192,
        }
    }

    fn supports_feature(&self, _feature: &str) -> bool {
        true // Mock支持所有功能
    }
}

// ════════════════════════════
// 单元测试
// ════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_sampling_request() {
        let mock = MockLlmProvider::new("mock-model", 10);
        let handler = SamplingHandler::new(mock);

        let request = SamplingRequest {
            role: SamplingRole::User,
            content: "Hello, world!".to_string(),
            ..Default::default()
        };

        let result = handler.handle_request(request).await;
        
        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(!response.content.is_empty());
        assert_eq!(response.model, "mock-model");
        assert_eq!(response.stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn test_caching_works() {
        let mock = MockLlmProvider::new("cached-model", 5);
        let handler = SamplingHandler::with_config(
            mock,
            SamplingConfig {
                enable_cache: true,
                cache_ttl_secs: 60,
                ..Default::default()
            }
        );

        let request = SamplingRequest {
            role: SamplingRole::User,
            content: "Cached content".to_string(),
            ..Default::default()
        };

        // 第一次请求 - 应该调用provider
        let result1 = handler.handle_request(request.clone()).await.unwrap();
        
        // 第二次请求 - 应该命中缓存
        let result2 = handler.handle_request(request).await.unwrap();

        // 内容应该相同
        assert_eq!(result1.content, result2.content);

        // 检查统计
        let stats = handler.get_statistics().await;
        assert_eq!(stats.cache_hits, 1);  // 第二次命中
        assert_eq!(stats.cache_misses, 1); // 第一次未命中
    }

    #[tokio::test]
    async fn test_batch_processing() {
        let mock = MockLlmProvider::new("batch-model", 5);
        let handler = SamplingHandler::new(mock);

        let requests = vec![
            SamplingRequest {
                role: SamplingRole::User,
                content: "First".to_string(),
                ..Default::default()
            },
            SamplingRequest {
                role: SamplingRole::User,
                content: "Second".to_string(),
                ..Default::default()
            },
        ];

        let results = handler.batch_handle(requests).await;
        
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.is_ok()));
    }

    #[tokio::test]
    async fn test_clear_cache() {
        let mock = MockLlmProvider::new("clear-cache-model", 5);
        let handler = SamplingHandler::new(mock);

        // 先填充缓存
        let _ = handler.handle_request(SamplingRequest {
            role: SamplingRole::User,
            content: "Test".to_string(),
            ..Default::default()
        }).await;

        assert!(handler.cache_size().await > 0);

        // 清除缓存
        handler.clear_cache().await;

        assert_eq!(handler.cache_size().await, 0);

        let stats = handler.get_statistics().await;
        assert_eq!(stats.cache_hits, 0);
        assert_eq!(stats.cache_misses, 0);
    }

    #[tokio::test]
    async fn test_statistics_tracking() {
        let mock = MockLlmProvider::new("stats-model", 1);
        let handler = SamplingHandler::new(mock);

        // 执行几次请求
        for i in 0..5 {
            let _ = handler.handle_request(SamplingRequest {
                role: SamplingRole::User,
                content: format!("Request {}", i),
                ..Default::default()
            }).await;
        }

        let stats = handler.get_statistics().await;
        assert_eq!(stats.total_requests, 5);
        assert!(stats.total_input_tokens > 0);
        assert!(stats.total_output_tokens > 0);
        assert!(stats.avg_response_time_ms >= 1.0);
    }

    #[test]
    fn test_token_usage_calculation() {
        let usage = TokenUsage::new(100, 50);
        
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
    }

    #[test]
    fn test_stop_reason_display() {
        assert_eq!(StopReason::EndTurn.to_string(), "end_turn");
        assert_eq!(StopReason::MaxTokens.to_string(), "max_tokens");
        assert_eq!(StopReason::Other("custom".to_string()).to_string(), "custom");
    }

    #[test]
    fn test_sampling_role_display() {
        assert_eq!(SamplingRole::User.to_string(), "user");
        assert_eq!(SamplingRole::Assistant.to_string(), "assistant");
        assert_eq!(SamplingRole::System.to_string(), "system");
    }

    #[test]
    fn test_default_config_values() {
        let config = SamplingConfig::default();
        
        assert!(config.enable_cache);
        assert_eq!(config.cache_ttl_secs, 300);
        assert_eq!(config.requests_per_minute, 20);
        assert_eq!(config.default_max_tokens, 4096);
        assert!((config.default_temperature - 0.7).abs() > f64::EPSILON);
    }

    #[test]
    fn test_mock_provider_info() {
        let mock = MockLlmProvider::new("test-model", 0);
        let info = mock.model_info();
        
        assert_eq!(info.name, "test-model");
        assert!(info.max_context_tokens > 0);
        assert!(info.max_output_tokens > 0);
    }
}
