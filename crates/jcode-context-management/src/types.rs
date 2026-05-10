// ════════════════════════════════════════════════════════════════
// 上下文管理核心类型
// ════════════════════════════════════════════════════════════════

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// 缓存作用域
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CacheScope {
    /// 全局缓存 (跨会话共享)
    Global,
    /// 组织级别缓存
    Organization,
    /// 会话/请求级别缓存
    Ephemeral,
    /// 无缓存
    None,
}

impl std::fmt::Display for CacheScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Global => write!(f, "global"),
            Self::Organization => write!(f, "organization"),
            Self::Ephemeral => write!(f, "ephemeral"),
            Self::None => write!(f, "none"),
        }
    }
}

/// 缓存 TTL 类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheTtl {
    /// 短期缓存 (~5 min)
    Ephemeral,
    /// 长期缓存 (~1 hour)
    Long,
    /// 自定义秒数
    Custom(u32),
}

/// 缓存控制标记
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub cache_type: String,
    
    pub scope: Option<CacheScope>,
    
    pub ttl: Option<u32>,  // 秒
}

impl CacheControl {
    /// 创建 ephemeral (短期) 缓存控制
    pub fn ephemeral() -> Self {
        Self {
            cache_type: "ephemeral".to_string(),
            scope: Some(CacheScope::Ephemeral),
            ttl: Some(DEFAULT_CACHE_TTL_EPHEMERAL),
        }
    }
    
    /// 创建长期缓存控制
    pub fn long_lived() -> Self {
        Self {
            cache_type: "ephemeral".to_string(),
            scope: Some(CacheScope::Organization),
            ttl: Some(DEFAULT_CACHE_TTL_LONG),
        }
    }
    
    /// 无缓存
    pub fn none() -> Self {
        Self {
            cache_type: "none".to_string(),
            scope: Some(CacheScope::None),
            ttl: None,
        }
    }
    
    pub fn is_cached(&self) -> bool {
        self.cache_type != "none"
    }
    
    pub fn scope(&self) -> &Option<CacheScope> {
        &self.scope
    }
    
    pub fn ttl(&self) -> Option<u32> {
        self.ttl
    }

    /// 序列化为 API 期望的格式
    pub fn to_api_value(&self) -> serde_json::Value {
        serde_json::json!({
            "type": self.cache_type,
            "ttl": self.ttl
        })
    }
}

// ════════════════════════════════════════════════════════════════

/// Prompt 分块 — 用于 splitSysPromptPrefix
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptBlock {
    pub content: String,
    
    pub scope: CacheScope,
    
    pub block_type: BlockType,
    
    /// 是否为动态内容 (工具列表等可能变化的)
    pub is_dynamic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockType {
    AttributionHeader,   // 版权头信息
    SystemPrefix,         // 系统提示前缀 (静态部分)
    StaticContent,       // 静态系统提示主体
    DynamicContent,      // 动态内容 (工具/MCP 列表等)
    Rest,               // 剩余内容
}

// ════════════════════════════════════════════════════════════════

/// Token 预算状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetState {
    /// 最大可用 token 数
    pub max_tokens: usize,
    
    /// 已使用 token 数
    pub used_tokens: usize,
    
    /// 输入 token 数 (累计)
    pub input_tokens: u64,
    
    /// 输出 token 数 (累计)
    pub output_tokens: u64,
    
    /// 缓存命中 token 数
    pub cache_hit_tokens: u64,
    
    /// 缓存写入 token 数
    pub cache_write_tokens: u64,
    
    /// 上次更新时间
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

impl BudgetState {
    pub fn remaining(&self) -> Option<usize> {
        if self.used_tokens >= self.max_tokens {
            Some(0)
        } else {
            self.max_tokens.checked_sub(self.used_tokens)
        }
    }
    
    pub fn utilization(&self) -> f64 {
        if self.max_tokens == 0 { return 0.0; }
        self.used_tokens as f64 / self.max_tokens as f64
    }
    
    pub fn is_near_limit(&self, threshold: f64) -> bool {
        self.utilization() >= threshold
    }
    
    pub fn total_tracked(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// 压缩结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactResult {
    /// 使用的压缩策略
    pub strategy: CmpStrategy,
    
    /// 压缩前消息数
    pub before_count: usize,
    
    /// 压缩后消息数
    pub after_count: usize,
    
    /// 减少的消息数
    pub messages_removed: usize,
    
    /// 节省的预估 token 数
    pub estimated_tokens_saved: usize,
    
    /// 耗时 (ms)
    pub duration_ms: u64,
    
    /// 是否生成了摘要 (Collapse 策略)
    pub summary: Option<String>,
}

// ════════════════════════════════════════════════════════════════
/// 压缩策略 (与 agent-advanced 区分，用 CmpStrategy)
// ════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CmpStrategy {
    Micro,     // 截断旧消息文本
    Snip,      // 移除中间消息块
    Collapse,  // LLM 摘要替换
    Auto,      // 自动选择最佳
}
