//! Feature Flag 服务 - 动态功能开关
//!
//! 移植自 Claude Code:
//! - GrowthBook 集成的 Feature Flag 系统
//! - `useDynamicConfig(configName, defaultValue)` React Hook
//! - 远程 API 下发的运行时开关

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Feature Flag 值类型 (多态)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FeatureFlagValue {
    Bool(bool),
    String(String),
    Number(f64),
    Json(serde_json::Value),
}

impl Default for FeatureFlagValue {
    fn default() -> Self {
        Self::Bool(false)
    }
}

impl PartialEq for FeatureFlagValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Number(a), Self::Number(b)) => a.total_cmp(b).is_eq(),
            (Self::Json(a), Self::Json(b)) => a == b,
            _ => false,
        }
    }
}

/// Feature Flag 服务配置
#[derive(Debug, Clone)]
pub struct FeatureFlagConfig {
    /// API 端点 URL (GrowthBook 或自托管)
    pub api_endpoint: Option<String>,
    
    /// 缓存 TTL (秒), 默认 300s
    pub cache_ttl_secs: u64,
    
    /// 是否启用 API 获取 (离线模式下全部使用默认值)
    pub enable_remote_fetch: bool,
}

impl Default for FeatureFlagConfig {
    fn default() -> Self {
        Self {
            api_endpoint: None,
            cache_ttl_secs: 300,
            enable_remote_fetch: false, // 默认关闭, 需显式启用
        }
    }
}

/// Feature Flag 缓存条目
struct CachedFlag {
    value: FeatureFlagValue,
    fetched_at: chrono::DateTime<chrono::Utc>,
    ttl_secs: u64,
}

impl CachedFlag {
    fn is_expired(&self) -> bool {
        let elapsed = chrono::Utc::now()
            .signed_duration_since(self.fetched_at)
            .num_seconds();
        elapsed > self.ttl_secs as i64
    }
}

/// 动态 Feature Flag 服务
///
/// 提供运行时动态切换功能开关的能力:
/// ```ignore
/// let flags = FeatureFlagService::new(config);
/// 
/// // 检查某个 feature flag
/// if flags.get("ai_code_completion").await?.is_bool_true() {
///     // 启用代码补全
/// }
///
/// // 强制刷新所有 flags
/// flags.refresh_all().await?;
/// ```
pub struct FeatureFlagService {
    flags: Arc<RwLock<HashMap<String, CachedFlag>>>,
    config: FeatureFlagConfig,
}

impl FeatureFlagService {
    /// 创建新的 Feature Flag 服务
    pub fn new(config: FeatureFlagConfig) -> Self {
        Self {
            flags: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults() -> Self {
        Self::new(FeatureFlagConfig::default())
    }

    /// 获取 Feature Flag 值 (带缓存)
    ///
    /// # Arguments
    /// * `name` - flag 名称
    /// * `default` - 默认值 (缓存未命中或获取失败时返回)
    pub async fn get(
        &self,
        name: &str,
        default: FeatureFlagValue,
    ) -> Result<FeatureFlagValue> {
        // 1. 检查内存缓存
        {
            let guard = self.flags.read().await;
            if let Some(cached) = guard.get(name) {
                if !cached.is_expired() {
                    debug!("Feature flag '{}' = {:?} (from cache)", name, cached.value);
                    return Ok(cached.value.clone());
                }
                debug!("Feature flag '{}' expired", name);
            }
        }

        // 2. 缓存未命中或过期 → 尝试从 API 获取
        if self.config.enable_remote_fetch {
            if let Some(value) = self.fetch_from_api(name).await? {
                // 写入缓存
                let mut guard = self.flags.write().await;
                guard.insert(name.to_string(), CachedFlag {
                    value: value.clone(),
                    fetched_at: chrono::Utc::now(),
                    ttl_secs: self.config.cache_ttl_secs,
                });
                
                return Ok(value);
            }
        }

        // 3. 使用默认值
        debug!("Feature flag '{}' using default: {:?}", name, default);
        
        // 将默认值也写入缓存 (避免重复请求)
        let mut guard = self.flags.write().await;
        guard.insert(name.to_string(), CachedFlag {
            value: default.clone(),
            fetched_at: chrono::Utc::now(),
            ttl_secs: self.config.cache_ttl_secs,
        });

        Ok(default)
    }

    /// 从远程 API 获取单个 flag (内部方法)
    async fn fetch_from_api(&self, _name: &str) -> Result<Option<FeatureFlagValue>> {
        // TODO: 实现 GrowthBook 或自定义 API 调用
        // Claude Code 中通过 `getDynamicConfig_BLOCKS_ON_INIT()` 实现
        
        match &self.config.api_endpoint {
            Some(url) => {
                info!("Fetching feature flags from {}", url);
                // 实际 HTTP 请求...
                Ok(None) // TODO: 实现后返回真实值
            }
            None => Ok(None),
        }
    }

    /// 强制刷新所有 flags (从 API)
    pub async fn refresh_all(&self) -> Result<usize> {
        if !self.config.enable_remote_fetch {
            return Ok(0); // 未启用远程, 无操作
        }

        info!("Force refreshing all feature flags...");

        // TODO: 批量从 API 获取所有 flags
        let count = {
            let guard = self.flags.read().await;
            guard.len()
        };

        info!("Refreshed {} feature flags (placeholder)", count);
        Ok(count)
    }

    /// 手动设置一个 flag 的值 (用于测试或本地覆盖)
    pub async fn set(&self, name: impl Into<String>, value: FeatureFlagValue) {
        let mut guard = self.flags.write().await;
        guard.insert(name.into(), CachedFlag {
            value,
            fetched_at: chrono::Utc::now(),
            ttl_secs: u64::MAX, // 手动设置的永不过期
        });
    }

    /// 清除指定 flag (回到使用默认值/远程值)
    pub async fn clear(&self, name: &str) {
        let mut guard = self.flags.write().await;
        guard.remove(name);
    }

    /// 获取当前缓存的 flag 数量
    pub async fn cached_count(&self) -> usize {
        self.flags.read().await.len()
    }

    /// 预定义的 JCode Feature Flags
    ///
    /// 这些是 JCode 内置的功能开关, 可以在运行时动态控制
    pub fn builtin_flags() -> HashMap<String, FeatureFlagValue> {
        let mut flags = HashMap::new();
        
        // === AI 功能 ===
        flags.insert("ai_code_completion".to_string(), FeatureFlagValue::Bool(true));
        flags.insert("ai_auto_fix".to_string(), FeatureFlagValue::Bool(true));
        flags.insert("ai_refactor".to_string(), FeatureFlagValue::Bool(false)); // 实验性
        flags.insert("ai_batch_operations".to_string(), FeatureFlagValue::Bool(false)); // 实验性

        // === IDE 集成 ===
        flags.insert("ide_integration".to_string(), FeatureFlagValue::Bool(true));
        flags.insert("ide_lsp_diagnostics".to_string(), FeatureFlagValue::Bool(true));
        flags.insert("ide_diff_in_editor".to_string(), FeatureFlagValue::Bool(true));

        // === 远程调试 ===
        flags.insert("remote_debugging".to_string(), FeatureFlagValue::Bool(true));
        flags.insert("remote_session_resume".to_string(), FeatureFlagValue::Bool(true));
        flags.insert("jwt_auto_refresh".to_string(), FeatureFlagValue::Bool(true));

        // === 性能 ===
        flags.insert("performance_monitoring".to_string(), FeatureFlagValue::Bool(false));
        flags.insert("memory_profiling".to_string(), FeatureFlagValue::Bool(false));
        flags.insert("swarm_mode".to_string(), FeatureFlagValue::Bool(true));

        // === UI ===
        flags.insert("tui_animations".to_string(), FeatureFlagValue::Bool(true));
        flags.insert("markdown_rendering".to_string(), FeatureFlagValue::Bool(true));
        flags.insert("mermaid_diagrams".to_string(), FeatureFlagValue::Bool(true));

        flags
    }
}
