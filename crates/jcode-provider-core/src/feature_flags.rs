//! # 特性标志系统 — 集中式功能开关
//!
//! 源自 Claude Code `src/services/analytics/growthbook.ts`
//!
//! ## 架构
//! - `FeatureFlag` 枚举 — 所有特性标志的单一真相源
//! - `FlagValue` — 支持布尔/字符串/数值类型
//! - `FlagClient` — 缓存读取 + 环境覆盖 + 可选的远程评估
//! - 环境变量 `JCODE_FEATURE_*` 可以覆盖任何标志

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// 所有特性标志的枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FeatureFlag {
    /// MCP 高级功能
    McpAdvanced,
    /// Swarm 协调模式
    CoordinatorMode,
    /// 扩展思考
    ExtendedThinking,
    /// 工作流脚本
    WorkflowScripts,
    /// Agent Teams (群组)
    AgentTeams,
    /// 远程会话
    RemoteSessions,
    /// 后台代理
    BackgroundAgents,
    /// 语音模式
    VoiceMode,
    /// 桥接模式
    BridgeMode,
    /// 提交归因追踪
    CommitAttribution,
}

impl FeatureFlag {
    pub fn name(&self) -> &'static str {
        match self {
            FeatureFlag::McpAdvanced => "mcp_advanced",
            FeatureFlag::CoordinatorMode => "coordinator_mode",
            FeatureFlag::ExtendedThinking => "extended_thinking",
            FeatureFlag::WorkflowScripts => "workflow_scripts",
            FeatureFlag::AgentTeams => "agent_teams",
            FeatureFlag::RemoteSessions => "remote_sessions",
            FeatureFlag::BackgroundAgents => "background_agents",
            FeatureFlag::VoiceMode => "voice_mode",
            FeatureFlag::BridgeMode => "bridge_mode",
            FeatureFlag::CommitAttribution => "commit_attribution",
        }
    }

    /// 默认值（未配置时）
    pub fn default_value(&self) -> bool {
        match self {
            FeatureFlag::ExtendedThinking => true,
            FeatureFlag::McpAdvanced => true,
            FeatureFlag::CommitAttribution => true,
            FeatureFlag::RemoteSessions => true,
            // 默认关闭的特性
            FeatureFlag::CoordinatorMode => false,
            FeatureFlag::WorkflowScripts => false,
            FeatureFlag::AgentTeams => false,
            FeatureFlag::BackgroundAgents => false,
            FeatureFlag::VoiceMode => false,
            FeatureFlag::BridgeMode => false,
        }
    }

    /// 环境变量名称
    pub fn env_var(&self) -> String {
        format!("JCODE_FEATURE_{}", self.name().to_uppercase())
    }
}

/// 标志值（支持多类型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FlagValue {
    Bool(bool),
    String(String),
    Number(f64),
}

impl FlagValue {
    /// 获取布尔值
    pub fn as_bool(&self) -> bool {
        match self {
            FlagValue::Bool(b) => *b,
            FlagValue::String(s) => matches!(s.to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
            FlagValue::Number(n) => *n != 0.0,
        }
    }

    /// 获取字符串值
    pub fn as_string(&self) -> Option<&str> {
        match self {
            FlagValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// 获取数值
    pub fn as_number(&self) -> Option<f64> {
        match self {
            FlagValue::Number(n) => Some(*n),
            _ => None,
        }
    }
}

impl From<bool> for FlagValue {
    fn from(b: bool) -> Self { FlagValue::Bool(b) }
}

/// 标志配置覆盖
#[derive(Debug, Clone, Default)]
struct FlagOverrides {
    values: HashMap<String, FlagValue>,
    expiry: Option<Instant>,
}

/// 特性标志客户端
///
/// 源自 Claude Code 的 GrowthBook 客户端模式：
/// - 内存缓存 ± 环境变量覆盖
/// - 可选的远程评估器
/// - 定期刷新 (TTL)
pub struct FlagClient {
    /// 缓存标志值
    cache: Arc<RwLock<HashMap<String, FlagValue>>>,
    /// 覆盖值（来自环境变量或 Config）
    overrides: Arc<RwLock<FlagOverrides>>,
    /// 远程评估器（可选，从配置文件/API 加载）
    remote_evaluator: Option<Box<dyn FlagEvaluator + Send + Sync>>,
    /// 缓存 TTL
    cache_ttl: Duration,
    /// 上次刷新时间
    last_refresh: Arc<RwLock<Instant>>,
}

/// 远程标志评估器 trait
pub trait FlagEvaluator {
    fn evaluate(&self, flags: &[FeatureFlag]) -> HashMap<String, FlagValue>;
    fn name(&self) -> &str;
}

impl FlagClient {
    pub fn new() -> Self {
        let client = Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            overrides: Arc::new(RwLock::new(FlagOverrides::default())),
            remote_evaluator: None,
            cache_ttl: Duration::from_secs(3600), // 1 小时
            last_refresh: Arc::new(RwLock::new(Instant::now())),
        };
        client.load_env_overrides();
        client
    }

    /// 设置远程评估器
    pub fn with_remote_evaluator(mut self, evaluator: Box<dyn FlagEvaluator + Send + Sync>) -> Self {
        self.remote_evaluator = Some(evaluator);
        self
    }

    /// 设置缓存 TTL
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// 从环境变量加载覆盖
    fn load_env_overrides(&self) {
        let mut overrides = HashMap::new();
        for (key, value) in std::env::vars() {
            if let Some(flag_name) = key.strip_prefix("JCODE_FEATURE_") {
                let normalized = flag_name.to_lowercase().replace('_', "");
                let flag = FEATURE_FLAGS.iter().find(|f| {
                    let fn_lower = f.name().to_lowercase().replace('_', "");
                    fn_lower == normalized
                });
                if let Some(f) = flag {
                    overrides.insert(
                        f.name().to_string(),
                        match value.to_lowercase().as_str() {
                            "1" | "true" | "yes" | "on" => FlagValue::Bool(true),
                            "0" | "false" | "no" | "off" => FlagValue::Bool(false),
                            s if s.parse::<f64>().is_ok() => FlagValue::Number(s.parse().unwrap()),
                            _ => FlagValue::String(value),
                        },
                    );
                }
            }
        }
        if let Ok(mut o) = self.overrides.write() {
            o.values = overrides;
            o.expiry = None;
        }
    }

    /// 检查标志是否启用（首选方法，缓存容忍）
    /// 源自 Claude Code 的 `getFeatureValue_CACHED_MAY_BE_STALE()`
    pub fn is_enabled(&self, flag: FeatureFlag) -> bool {
        self.get_cached_may_be_stale(flag).as_bool()
    }

    /// 从缓存读取，容忍过期
    pub fn get_cached_may_be_stale(&self, flag: FeatureFlag) -> FlagValue {
        let name = flag.name();

        // 1. 检查环境覆盖（最高优先级）
        if let Ok(overrides) = self.overrides.read() {
            if let Some(value) = overrides.values.get(name) {
                return value.clone();
            }
        }

        // 2. 检查缓存
        if let Ok(cache) = self.cache.read() {
            if let Some(value) = cache.get(name) {
                return value.clone();
            }
        }

        // 3. 返回默认值
        FlagValue::Bool(flag.default_value())
    }

    /// 刷新远程标志（从远程评估器加载）
    pub fn refresh(&self) {
        // 检查是否需要刷新
        {
            let last = self.last_refresh.read().unwrap();
            if last.elapsed() < self.cache_ttl {
                return;
            }
        }

        if let Some(ref evaluator) = self.remote_evaluator {
            let values = evaluator.evaluate(&FEATURE_FLAGS);
            if let Ok(mut cache) = self.cache.write() {
                cache.extend(values);
            }
            if let Ok(mut last) = self.last_refresh.write() {
                *last = Instant::now();
            }
        }
    }

    /// 强制刷新
    pub fn force_refresh(&self) {
        if let Ok(mut last) = self.last_refresh.write() {
            *last = Instant::now() - self.cache_ttl - Duration::from_secs(1);
        }
        self.refresh();
    }

    /// 设置运行时覆盖
    pub fn set_override(&self, flag: FeatureFlag, value: FlagValue) {
        if let Ok(mut overrides) = self.overrides.write() {
            overrides.values.insert(flag.name().to_string(), value);
        }
    }

    /// 清除缓存
    pub fn clear_cache(&self) {
        if let Ok(mut cache) = self.cache.write() {
            cache.clear();
        }
    }

    /// 获取所有标志的状态
    pub fn all_flags_status(&self) -> HashMap<String, bool> {
        let mut status = HashMap::new();
        for flag in &FEATURE_FLAGS {
            status.insert(flag.name().to_string(), self.is_enabled(*flag));
        }
        status
    }
}

impl Default for FlagClient {
    fn default() -> Self { Self::new() }
}

/// 所有特性标志列表
pub const FEATURE_FLAGS: [FeatureFlag; 10] = [
    FeatureFlag::McpAdvanced,
    FeatureFlag::CoordinatorMode,
    FeatureFlag::ExtendedThinking,
    FeatureFlag::WorkflowScripts,
    FeatureFlag::AgentTeams,
    FeatureFlag::RemoteSessions,
    FeatureFlag::BackgroundAgents,
    FeatureFlag::VoiceMode,
    FeatureFlag::BridgeMode,
    FeatureFlag::CommitAttribution,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flag_names() {
        assert_eq!(FeatureFlag::McpAdvanced.name(), "mcp_advanced");
        assert_eq!(FeatureFlag::ExtendedThinking.name(), "extended_thinking");
    }

    #[test]
    fn test_flag_defaults() {
        assert!(FeatureFlag::ExtendedThinking.default_value());
        assert!(!FeatureFlag::CoordinatorMode.default_value());
    }

    #[test]
    fn test_flag_env_var() {
        let var = FeatureFlag::McpAdvanced.env_var();
        assert_eq!(var, "JCODE_FEATURE_MCP_ADVANCED");
    }

    #[test]
    fn test_flag_value_conversion() {
        let b: FlagValue = true.into();
        assert!(b.as_bool());

        assert!(FlagValue::String("true".into()).as_bool());
        assert!(!FlagValue::String("false".into()).as_bool());
        assert!(FlagValue::Number(1.0).as_bool());
        assert!(!FlagValue::Number(0.0).as_bool());
    }

    #[test]
    fn test_client_defaults() {
        let client = FlagClient::new();
        assert!(client.is_enabled(FeatureFlag::ExtendedThinking));
        assert!(!client.is_enabled(FeatureFlag::CoordinatorMode));
    }

    #[test]
    fn test_client_overrides() {
        let client = FlagClient::new();
        client.set_override(FeatureFlag::CoordinatorMode, true.into());
        assert!(client.is_enabled(FeatureFlag::CoordinatorMode));
    }

    #[test]
    fn test_all_flags_status() {
        let client = FlagClient::new();
        let status = client.all_flags_status();
        assert_eq!(status.len(), FEATURE_FLAGS.len());
    }
}
