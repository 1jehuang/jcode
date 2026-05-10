//! 动态配置系统核心模块
//!
//! 来源: 移植自 Claude Code
//! - `src/utils/config.ts` (62KB) — 全局配置引擎
//! - `src/utils/settings/settings.ts` (31KB) — 多源合并引擎
//!
//! ## 功能
//! 1. **多源优先级合并**: plugin < user < project < local < flag < policy(远程/MDM/托管)
//! 2. **文件监听热更新**: watchFile + mtime 检测, 跨进程一致性
//! 3. **写入保护三重机制**: 文件锁 + Auth Guard + 自动备份
//! 4. **Feature Flag 服务**: GrowthBook 集成式动态开关

mod config_merger;
mod file_watcher;
mod safe_writer;
mod feature_flags;

// Re-export public types
pub use config_merger::{
    ConfigMerger, MergedConfig, ConfigSourcePriority, 
    MergeOptions, SecurityCheckResult,
};
pub use file_watcher::{ConfigFileWatcher, ConfigCache};
pub use safe_writer::{
    SafeConfigWriter, SafeWriteOptions, WriteProtectionError,
};
pub use feature_flags::{
    FeatureFlagService, FeatureFlagValue, FeatureFlagConfig,
};

// ============================================================================
// 配置来源类型定义 (扩展 jcode-config-types)
// ============================================================================

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::collections::HashMap;
use serde_json::Value as JsonValue;

/// 配置来源标识
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigSourcePriority {
    Plugin = 0,
    User = 100,
    Project = 200,
    Local = 300,
    Flag = 400,
    PolicyRemote = 500,
    PolicyMdm = 600,
    PolicyManaged = 700,
}

impl std::fmt::Display for ConfigSourcePriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// 合并结果 (带来源标注)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergedConfig {
    pub value: JsonValue,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub source_map: HashMap<String, ConfigSourcePriority>,
}

// ============================================================================
// 全局配置结构 (对应 Claude Code GlobalConfig)
// ============================================================================

/// 全局配置 (~/.jcode/config.json)
/// 
/// 移植自 Claude Code GlobalConfig 类型, 保留 JCode 特有字段
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalConfig {
    // === IDE 集成 ===
    /// 自动连接 IDE
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_connect_ide: Option<bool>,
    /// 自动安装 IDE 扩展
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_install_ide_extension: Option<bool>,

    // === 远程控制 ===
    /// 启动时自动开启远程控制
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_control_at_startup: Option<bool>,

    // === 缓存的动态配置 ===
    /// 缓存 GrowthBook feature flags
    #[serde(rename = "cachedStatsigGates", skip_serializing_if = "Option::is_none")]
    pub cached_statsig_gates: Option<HashMap<String, bool>>,
    /// 缓存的动态配置值
    #[serde(rename = "cachedDynamicConfigs", skip_serializing_if = "Option::is_none")]
    pub cached_dynamic_configs: Option<HashMap<String, JsonValue>>,
    /// 缓存的 GrowthBook features
    #[serde(rename = "cachedGrowthBookFeatures", skip_serializing_if = "Option::is_none")]
    pub cached_growthbook_features: Option<HashMap<String, String>>,

    // === UI/显示 ===
    /// 默认 diff 显示模式 (继承自 jcode-config-types DisplayConfig)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_diff_mode: Option<String>,

    // === 性能 ===
    /// 性能等级: auto / full / reduced / minimal
    #[serde(skip_serializing_if = "Option::is_none")]
    pub performance_tier: Option<String>,

    // === 安全 ===
    /// 危险模式跳过确认
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_dangerous_mode: Option<bool>,

    // === 更新 ===
    /// 更新通道: stable / main
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_channel: Option<String>,

    // === 调试 ===
    /// 启用调试 socket
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_socket: Option<bool>,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            auto_connect_ide: Some(true),
            auto_install_ide_extension: Some(true),
            remote_control_at_startup: Some(false),
            cached_statsig_gates: None,
            cached_dynamic_configs: None,
            cached_growthbook_features: None,
            default_diff_mode: None,
            performance_tier: Some("auto".to_string()),
            skip_dangerous_mode: Some(false),
            update_channel: Some("stable".to_string()),
            debug_socket: Some(false),
        }
    }
}

impl GlobalConfig {
    /// 获取全局配置文件路径
    pub fn config_file_path() -> PathBuf {
        dirs::home_dir()
            .map(|h| h.join(".jcode").join("config.json"))
            .unwrap_or_else(|| PathBuf::from("/tmp/.jcode/config.json"))
    }

    /// 备份目录路径
    pub fn backups_dir_path() -> PathBuf {
        dirs::home_dir()
            .map(|h| h.join(".jcode").join("backups"))
            .unwrap_or_else(|| PathBuf::from("/tmp/.jcode/backups"))
    }
}
