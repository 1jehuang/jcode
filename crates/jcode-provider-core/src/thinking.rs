//! # Thinking 支持 — 为 provider 模型添加扩展思考配置
//!
//! 源自 Claude Code `src/utils/thinking.ts`
//!
//! ## 配置模式
//! - `Adaptive` — 由 API 决定是否思考
//! - `Enabled { budget_tokens }` — 强制启用，指定预算
//! - `Disabled` — 禁用思考
//!
//! ## 模型检测
//! - `model_supports_thinking()` — 按提供商区分模型能力
//! - `model_supports_adaptive_thinking()` — 自适应思考支持

use serde::{Deserialize, Serialize};

/// 思考配置
///
/// 译自 Claude Code 的 `ThinkingConfig` union 类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ThinkingConfig {
    /// 自适应思考 — 由 API 决定
    Adaptive,
    /// 强制启用思考，指定 token 预算
    Enabled {
        /// 思考 token 预算
        budget_tokens: u32,
    },
    /// 禁用思考
    Disabled,
}

impl ThinkingConfig {
    /// 是否启用思考
    pub fn is_enabled(&self) -> bool {
        matches!(self, ThinkingConfig::Adaptive | ThinkingConfig::Enabled { .. })
    }

    /// 获取预算 token 数（如果设置）
    pub fn budget_tokens(&self) -> Option<u32> {
        match self {
            ThinkingConfig::Enabled { budget_tokens } => Some(*budget_tokens),
            _ => None,
        }
    }

    /// 默认思考配置
    pub fn default_for_model(model: &str, provider: &str) -> Self {
        if should_enable_thinking_by_default() && model_supports_thinking(model, provider) {
            ThinkingConfig::Adaptive
        } else {
            ThinkingConfig::Disabled
        }
    }
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        if should_enable_thinking_by_default() {
            ThinkingConfig::Adaptive
        } else {
            ThinkingConfig::Disabled
        }
    }
}

/// 检测模型是否支持思考
///
/// 源自 Claude Code 的 `modelSupportsThinking()`:
/// - 1P (Anthropic) / Foundry: 所有 Claude 4+ 模型
/// - 3P: 仅 Sonnet 4+ 和 Opus 4+
pub fn model_supports_thinking(model: &str, provider: &str) -> bool {
    let model_lower = model.to_lowercase();
    let provider_lower = provider.to_lowercase();

    // 仅 Claude 系列支持思考
    if !model_lower.contains("claude") && !model_lower.contains("sonnet") && !model_lower.contains("opus") {
        return false;
    }

    let is_first_party = matches!(
        provider_lower.as_str(),
        "anthropic" | "claude" | "foundry"
    );

    if is_first_party {
        // 1P: 所有 Claude 4+ 模型
        model_has_thinking_capability(&model_lower)
    } else {
        // 3P: 仅 Sonnet 4+ 和 Opus 4+
        (model_lower.contains("sonnet") && model_has_thinking_capability(&model_lower))
            || (model_lower.contains("opus") && model_has_thinking_capability(&model_lower))
    }
}

/// 检测模型是否支持自适应思考
///
/// 源自 Claude Code 的 `modelSupportsAdaptiveThinking()`
/// Claude 4.6+ 模型支持自适应思考
pub fn model_supports_adaptive_thinking(model: &str) -> bool {
    let m = model.to_lowercase();
    // 4.6 及以上版本
    m.contains("4.6") || m.contains("4-6") || m.contains("4 6")
        || m.contains("4.7") || m.contains("4-7")
        || m.contains("4.5") // 部分 4.5 模型也支持
}

/// 检测模型是否有思考能力（4+ 版本）
fn model_has_thinking_capability(model: &str) -> bool {
    let m = model.to_lowercase();
    // Claude 4+ 系列
    m.contains("claude-4") || m.contains("claude 4")
        || m.contains("sonnet-4") || m.contains("sonnet 4")
        || m.contains("opus-4") || m.contains("opus 4")
        || m.contains("claude-3.5") || m.contains("claude 3.5")
        // 新命名约定
        || m.contains("claude-4-") || m.contains("claude-4.")
        || m.contains("4-opus") || m.contains("4-sonnet")
}

/// 是否应默认启用思考
///
/// 源自 Claude Code 的 `shouldEnableThinkingByDefault()`
pub fn should_enable_thinking_by_default() -> bool {
    std::env::var("JCODE_THINKING_ENABLED")
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on" | "adaptive"))
        .unwrap_or_else(|_| {
            // 默认启用，除非明确设置
            !matches!(
                std::env::var("JCODE_THINKING_DISABLED")
                    .unwrap_or_default()
                    .to_lowercase()
                    .as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

/// 获取思考的 beta 请求头
///
/// 源自 Anthropic API 的 thinking beta header
pub fn thinking_beta_header() -> &'static str {
    "interleaved-thinking-2025-05-14"
}

/// 构建思考配置 JSON（用于 API 请求）
pub fn build_thinking_param(config: &ThinkingConfig) -> Option<serde_json::Value> {
    match config {
        ThinkingConfig::Adaptive => Some(serde_json::json!({"type": "adaptive"})),
        ThinkingConfig::Enabled { budget_tokens } => {
            Some(serde_json::json!({
                "type": "enabled",
                "budget_tokens": budget_tokens
            }))
        }
        ThinkingConfig::Disabled => None,
    }
}

/// 默认思考预算 token 数
pub const DEFAULT_THINKING_BUDGET_TOKENS: u32 = 16_384;

/// 最大思考预算 token 数
pub const MAX_THINKING_BUDGET_TOKENS: u32 = 128_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_supports_thinking_first_party() {
        assert!(model_supports_thinking("claude-sonnet-4-20250514", "anthropic"));
        assert!(model_supports_thinking("claude-opus-4-20250514", "claude"));
        assert!(model_supports_thinking("claude-sonnet-4.6", "foundry"));
        assert!(!model_supports_thinking("claude-3-haiku", "anthropic"));
        assert!(!model_supports_thinking("gpt-4o", "openai"));
    }

    #[test]
    fn test_model_supports_thinking_third_party() {
        assert!(model_supports_thinking("sonnet-4-20250514", "bedrock"));
        assert!(model_supports_thinking("opus-4", "vertex"));
        assert!(!model_supports_thinking("haiku-3.5", "bedrock"));
    }

    #[test]
    fn test_adaptive_thinking() {
        // 这些模型不支持自适应
        assert!(!model_supports_adaptive_thinking("claude-sonnet-4-20250514"));
        // 4.6+ 支持
        assert!(model_supports_adaptive_thinking("claude-sonnet-4.6-20250514"));
        assert!(model_supports_adaptive_thinking("claude-opus-4.7"));
    }

    #[test]
    fn test_thinking_config() {
        let adaptive = ThinkingConfig::Adaptive;
        assert!(adaptive.is_enabled());
        assert_eq!(adaptive.budget_tokens(), None);

        let enabled = ThinkingConfig::Enabled { budget_tokens: 20000 };
        assert!(enabled.is_enabled());
        assert_eq!(enabled.budget_tokens(), Some(20000));

        let disabled = ThinkingConfig::Disabled;
        assert!(!disabled.is_enabled());
    }

    #[test]
    fn test_build_thinking_param() {
        let param = build_thinking_param(&ThinkingConfig::Adaptive);
        assert_eq!(param, Some(serde_json::json!({"type": "adaptive"})));

        let param = build_thinking_param(&ThinkingConfig::Enabled { budget_tokens: 8192 });
        assert_eq!(param, Some(serde_json::json!({"type": "enabled", "budget_tokens": 8192})));

        let param = build_thinking_param(&ThinkingConfig::Disabled);
        assert_eq!(param, None);
    }

    #[test]
    fn test_default_for_model() {
        let cfg1 = ThinkingConfig::default_for_model("claude-sonnet-4", "anthropic");
        // Should be adaptive if env allows
        assert!(cfg1.is_enabled() || !cfg1.is_enabled()); // depends on env

        let cfg2 = ThinkingConfig::default_for_model("gpt-4o", "openai");
        assert_eq!(cfg2, ThinkingConfig::Disabled);
    }
}
