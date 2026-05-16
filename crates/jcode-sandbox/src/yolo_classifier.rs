// ════════════════════════════════════════════════════════════════
// YOLO (You Only Look Once) AI 安全分类器 — 移植自 Claude Code
//   src/utils/permissions/yoloClassifier.ts (~51KB)
//
// 核心思路:
//   使用 Side Query 向 LLM 发送精炼的 prompt, 让 AI 判断当前操作的安全性。
//   比 hardcoded 规则更智能、更灵活。
//
// 特性:
//   1. LLM Side Query — 独立于主对话的 AI 分类请求
//   2. 缓存机制 — 相同操作不重复询问 (TTL 过期)
//   3. 统计追踪 — 允许/拒绝率、置信度分布
//   4. Fallback — LLM 调用失败时回退到规则引擎
// ════════════════════════════════════════════════════════════════

use crate::denial_tracker::DenialTracker;
use crate::permission_engine::PermissionRequest;
use crate::types::YoloClassificationResult;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// YOLO 分类器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YoloClassifierConfig {
    /// LLM API endpoint
    pub api_url: String,

    /// API Key
    pub api_key: String,

    /// 模型名称
    pub model: String,

    /// 最大 token 数 (用于分类响应)
    pub max_tokens: u32,

    /// 缓存 TTL (秒)
    pub cache_ttl_secs: u64,

    /// 温度参数 (低温度 = 更确定的分类)
    pub temperature: f64,

    /// 允许阈值 (置信度 > 此值则 auto allow)
    pub allow_threshold: f64,
}

impl Default for YoloClassifierConfig {
    fn default() -> Self {
        Self {
            api_url: "https://api.anthropic.com/v1/messages".to_string(),
            api_key: String::new(),
            model: "claude-haiku-4-5-20250414".to_string(), // 快速便宜的小模型
            max_tokens: 64,
            cache_ttl_secs: 300, // 5 分钟
            temperature: 0.1, // 低温度 = 更确定性输出
            allow_threshold: 0.85,
        }
    }
}

/// 缓存条目
struct CacheEntry {
    result: YoloClassificationResult,
    cached_at: Instant,
    hit_count: u32,
}

/// 统计数据
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct YoloStats {
    pub total_classifications: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub allowed_count: u64,
    pub blocked_count: u64,
    pub avg_confidence: f64,
    pub errors_count: u64,
}

/// LLM Provider trait (可替换实现)
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn classify(
        &self,
        system_prompt: &str,
        user_message: &str,
        config: &YoloClassifierConfig,
    ) -> Result<YoloClassifyResponse, String>;
}

/// LLM 原始响应
struct YoloClassifyResponse {
    raw_text: String,
    confidence: f64,
    should_block: bool,
    reason: String,
}

/// 默认的 Anthropic Claude Provider
pub struct AnthropicProvider {
    client: Client,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    fn parse_classification_response(raw: &str) -> YoloClassifyResponse {
        let lower = raw.to_lowercase().trim().to_string();

        // 解析结构化响应或自由文本
        let should_block = lower.contains("block")
            || lower.contains("deny")
            || lower.contains("dangerous")
            || lower.contains("unsafe")
            || lower.contains("reject");

        // 尝试提取置信度
        let confidence = if let Some(conf_str) = Self::extract_confidence(raw) {
            conf_str.parse::<f64>().unwrap_or(0.5)
        } else if should_block {
            0.9 // 阻断默认高置信度
        } else {
            0.9 // 允许也默认高置信度
        };

        let reason = if should_block {
            format!("AI 检测到潜在风险: {}", Self::extract_reason(raw).unwrap_or_else(|| "未说明".into()))
        } else {
            "AI 判断此操作安全".into()
        };

        YoloClassifyResponse {
            raw_text: raw.to_string(),
            confidence,
            should_block,
            reason,
        }
    }

    fn extract_confidence(text: &str) -> Option<String> {
        // 匹配 "confidence: 0.95" 或 "置信度: 95%" 等
        let re = regex::Regex::new(r"(?i)(?:confidence|置信度|certainty)\s*[:：]\s*([\d.]+%?)").ok()?;
        re.captures(text)?.get(1).map(|m| m.as_str().to_string())
    }

    fn extract_reason(text: &str) -> Option<String> {
        // 提取 "reason: ..." 后的内容
        let re = regex::Regex::new(r"(?i)reason\s*[:：]\s*(.+?)(?:\.|$|confidence)").ok()?;
        re.captures(text)?.get(1).map(|m| m.as_str().trim().to_string())
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn classify(
        &self,
        system_prompt: &str,
        user_message: &str,
        config: &YoloClassifierConfig,
    ) -> Result<YoloClassifyResponse, String> {
        let body = serde_json::json!({
            "model": config.model,
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "system": system_prompt,
            "messages": [
                { "role": "user", "content": user_message }
            ]
        });

        let resp = self
            .client
            .post(&config.api_url)
            .header("x-api-key", &config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("API error {}: {}", status, text));
        }

        let json: serde_json::Value = resp.json().await.map_err(|e| format!("JSON parse error: {}", e))?;

        let content = json["content"][0]["text"]
            .as_str()
            .ok_or("Missing content in response")?;

        Ok(Self::parse_classification_response(content))
    }
}

// --- YOLO 分类器主结构 --------------------------------

pub struct YoloClassifier {
    config: YoloClassifierConfig,
    provider: Box<dyn LlmProvider>,
    cache: Arc<RwLock<HashMap<u64, CacheEntry>>>,
    stats: Arc<RwLock<YoloStats>>,
    denial_tracker: Arc<RwLock<DenialTracker>>,
}

impl YoloClassifier {
    /// 创建新的 YOLO 分类器
    pub fn new(config: YoloClassifierConfig, provider: impl LlmProvider + 'static) -> Self {
        Self {
            config,
            provider: Box::new(provider),
            cache: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(YoloStats::default())),
            denial_tracker: Arc::new(RwLock::new(DenialTracker::new())),
        }
    }

    /// 使用 Anthropic API 的便捷构造
    pub fn with_anthropic(api_key: String, model: Option<String>) -> Self {
        let mut config = YoloClassifierConfig::default();
        config.api_key = api_key;
        if let Some(m) = model {
            config.model = m;
        }
        Self::new(config, AnthropicProvider::new())
    }

    /// 核心分类方法 — 对工具调用进行安全判断
    pub async fn classify(
        &self,
        request: &PermissionRequest,
    ) -> Result<YoloClassificationResult, String> {
        // 1. 计算请求 hash 用于缓存
        let hash = self.compute_request_hash(request);

        // 2. 检查缓存
        if let Some(cached) = self.check_cache(hash).await {
            self.record_cache_hit().await;
            return Ok(cached);
        }
        self.record_cache_miss().await;

        // 3. 构建 prompt
        let (system_prompt, user_message) = self.build_prompts(request);

        // 4. 调用 LLM
        match self.provider.classify(&system_prompt, &user_message, &self.config).await {
            Ok(response) => {
                let result = YoloClassificationResult {
                    should_block: response.should_block,
                    reason: response.reason,
                    confidence: response.confidence,
                };

                // 写入缓存
                self.write_to_cache(hash, result.clone()).await;

                // 更新统计
                self.record_classification(&result).await;

                // 追踪 YOLO 成功
                {
                    let mut dt = self.denial_tracker.write().await;
                    dt.record_yolo_success();
                }

                Ok(result)
            }
            Err(e) => {
                tracing::warn!(error = %e, "YOLO classification failed");
                self.record_error().await;

                // 追踪 YOLO 错误
                {
                    let mut dt = self.denial_tracker.write().await;
                    let action = dt.record_yolo_error();
                    if !matches!(action, crate::denial_tracker::DegradationAction::None) {
                        tracing::warn!(action = ?e, "YOLO degradation triggered");
                    }
                }

                Err(format!("YOLO 分类失败: {}", e))
            }
        }
    }

    // --- Prompt 工程 -----------------------------------

    /// 构建系统提示和用户消息
    fn build_prompts(&self, req: &PermissionRequest) -> (String, String) {
        let system_prompt = r#"你是一个安全审查助手。你的任务是分析一个工具调用是否安全。

请以以下严格 JSON 格式回复（不要添加其他文字）:
{
  "should_block": true/false,
  "reason": "简短原因（中文）",
  "confidence": 0.0-1.0
}

判断标准:
- should_block=true: 当操作可能造成数据丢失、系统破坏、安全漏洞或未经授权的访问时
- should_block=false: 当操作是安全的只读/常规开发操作时
- confidence: 你的确定程度

保守策略: 如果不确定，优先设置 should_block=true"#.to_string();

        let input_preview = if req.tool_input.len() > 200 {
            format!("{}...", &req.tool_input[..200])
        } else {
            req.tool_input.clone()
        };

        let user_message = format!(
            r#"## 工具调用信息

**工具名称**: {}
**操作参数**: {}
**是否只读**: {}
**工作目录**: {:?}
**会话ID**: {}

请分析此操作的安全性。"#,
            req.tool_name,
            input_preview,
            req.is_readonly,
            req.working_dir,
            req.session_id
        );

        (system_prompt, user_message)
    }

    // --- 缓存管理 -------------------------------------

    fn compute_request_hash(&self, req: &PermissionRequest) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        req.tool_name.hash(&mut hasher);
        req.tool_input.hash(&mut hasher);
        req.is_readonly.hash(&mut hasher);
        hasher.finish()
    }

    async fn check_cache(&self, hash: u64) -> Option<YoloClassificationResult> {
        let cache = self.cache.read().await;
        if let Some(entry) = cache.get(&hash) {
            if entry.cached_at.elapsed() < Duration::from_secs(self.config.cache_ttl_secs) {
                return Some(entry.result.clone());
            }
        }
        None
    }

    async fn write_to_cache(&self, hash: u64, result: YoloClassificationResult) {
        let mut cache = self.cache.write().await;
        cache.insert(
            hash,
            CacheEntry {
                result,
                cached_at: Instant::now(),
                hit_count: 0,
            },
        );
    }

    /// 清理过期缓存条目
    pub async fn cleanup_cache(&self) -> usize {
        let mut cache = self.cache.write().await;
        let before = cache.len();
        cache.retain(|_, entry| {
            entry.cached_at.elapsed() < Duration::from_secs(self.config.cache_ttl_secs * 2)
        });
        before - cache.len()
    }

    // --- 统计 -----------------------------------------

    async fn record_cache_hit(&self) {
        let mut s = self.stats.write().await;
        s.cache_hits += 1;
    }

    async fn record_cache_miss(&self) {
        let mut s = self.stats.write().await;
        s.cache_misses += 1;
    }

    async fn record_classification(&self, result: &YoloClassificationResult) {
        let mut s = self.stats.write().await;
        s.total_classifications += 1;
        if result.should_block {
            s.blocked_count += 1;
        } else {
            s.allowed_count += 1;
        }
        // 移动平均置信度
        s.avg_confidence =
            (s.avg_confidence * (s.total_classifications - 1) as f64 + result.confidence)
                / s.total_classifications as f64;
    }

    async fn record_error(&self) {
        let mut s = self.stats.write().await;
        s.errors_count += 1;
    }

    /// 获取统计快照
    pub async fn get_stats(&self) -> YoloStats {
        self.stats.read().await.clone()
    }

    /// 重置统计
    pub async fn reset_stats(&self) {
        let mut s = self.stats.write().await;
        *s = YoloStats::default();
    }

    /// 清空全部缓存
    pub async fn clear_cache(&self) {
        self.cache.write().await.clear();
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "ai-classifier")]
    use super::*;

    // 注意: 以下测试需要真实的 API key, 在 CI 中应 skip
    #[cfg(feature = "ai-classifier")]
    #[tokio::test]
    #[ignore] // 需要 API key
    async fn test_yolo_safe_operation() {
        let classifier = YoloClassifier::with_anthropic(
            std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
            None,
        );
        let req = PermissionRequest {
            tool_name: "Read".to_string(),
            tool_input: "./src/main.rs".to_string(),
            session_id: "test".to_string(),
            user_id: None,
            working_dir: None,
            is_readonly: true,
        };
        let result = classifier.classify(&req).await.unwrap();
        assert!(!result.should_block, "读取文件应被判定为安全");
        assert!(result.confidence > 0.5);
    }

    #[test]
    fn test_provider_response_parsing() {
        let safe_resp = AnthropicProvider::parse_classification_response(
            r#"{ "should_block": false, "reason": "Safe read operation", "confidence": 0.95 }"#
        );
        assert!(!safe_resp.should_block);
        assert_eq!(safe_resp.confidence, 0.95);

        let danger_resp = AnthropicProvider::parse_classification_response(
            "This operation is dangerous and should be BLOCKED because it deletes data."
        );
        assert!(danger_resp.should_block);

        let free_text_resp = AnthropicProvider::parse_classification_response(
            "I think this is probably safe to allow."
        );
        assert!(!free_text_resp.should_block); // 包含 "safe" 和 "allow"
    }
}
