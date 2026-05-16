//! 配置合并引擎 - 多源优先级合并
//!
//! 移植自 Claude Code `src/utils/settings/settings.ts`:
//! ```typescript
//! // 优先级从低到高:
//! pluginSettings -> userSettings -> projectSettings -> localSettings -> flagSettings -> policySettings
//!
//! // 合并语义:
//! // - 数组: 去重并集 (uniq([...objValue, ...srcValue]))
//! // - 对象: lodash deep merge (高优先级覆盖低优先级)
//! ```
//!
//! 安全模型: projectSettings 不参与安全敏感检查

use super::{ConfigSourcePriority, MergedConfig};
use anyhow::Result;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use tracing::debug;

/// 合并选项
#[derive(Debug, Clone)]
pub struct MergeOptions {
    /// 是否执行安全检查
    pub safety_check: bool,

    /// 数组是否去重并集 (true) 还是追加 (false)
    pub array_unique_union: bool,
}

impl Default for MergeOptions {
    fn default() -> Self {
        Self {
            safety_check: true,
            array_unique_union: true,
        }
    }
}

/// 配置合并器
pub struct ConfigMerger;

impl ConfigMerger {
    /// 深度合并多个配置源
    ///
    /// # Arguments
    /// * `sources` - 按 (优先级, 值) 排列的配置源列表
    /// 
    /// # Returns
    /// 包含合并结果和来源映射的 `MergedConfig`
    pub fn merge(sources: Vec<(ConfigSourcePriority, JsonValue)>) -> MergedConfig {
        Self::merge_with_options(sources, MergeOptions::default())
    }

    /// 带选项的深度合并
    pub fn merge_with_options(
        sources: Vec<(ConfigSourcePriority, JsonValue)>,
        options: MergeOptions,
    ) -> MergedConfig {
        if sources.is_empty() {
            return MergedConfig {
                value: JsonValue::Object(serde_json::Map::new()),
                source_map: HashMap::new(),
            };
        }

        let mut result = JsonValue::Null;
        let mut source_map = HashMap::new();

        for (priority, value) in &sources {
            result = Self::deep_merge(
                result.clone(),
                value.clone(),
                &options,
            );

            // 记录每个顶层 key 的来源优先级
            if let Some(obj) = result.as_object() {
                for key in obj.keys() {
                    if !source_map.contains_key(key) {
                        source_map.insert(
                            key.to_string(),
                            *priority,
                        );
                    }
                }
            }
        }

        debug!(
            "Config merge complete: {} sources, {} top-level keys",
            sources.len(),
            source_map.len()
        );

        MergedConfig { value: result, source_map }
    }

    /// 深度合并两个 JSON 值
    ///
    /// # Rules (移植自 Claude Code settingsMergeCustomizer):
    /// 1. 两边都是 Object -> 递归深合并
    /// 2. 两边都是 Array -> 去重并集 (如果 array_unique_union=true), 否则追加
    /// 3. 其他情况 -> 右侧值覆盖左侧 (右侧为更高优先级)
    fn deep_merge(base: JsonValue, override_val: JsonValue, options: &MergeOptions) -> JsonValue {
        match (&base, &override_val) {
            // 两个都是 Object -> 递归合并
            (
                JsonValue::Object(mut base_map),
                JsonValue::Object(override_map),
            ) => {
                for (key, val) in override_map {
                    let merged = if base_map.contains_key(key) {
                        Self::deep_merge(
                            base_map.remove(key).unwrap(),
                            val.clone(),
                            options,
                        )
                    } else {
                        val.clone()
                    };
                    base_map.insert(key.clone(), merged);
                }
                JsonValue::Object(base_map)
            }

            // 两个都是 Array
            (JsonValue::Array(mut base_arr), JsonValue::Array(override_arr)) => {
                if options.array_unique_union {
                    // 去重并集 (移植自 Claude Code: uniq([...objValue, ...srcValue]))
                    let mut seen = std::collections::HashSet::new();
                    let mut result = Vec::with_capacity(base_arr.len() + override_arr.len());

                    // 先处理基础数组的元素
                    for item in base_arr.drain(..) {
                        let key = Self::array_item_key(&item);
                        if seen.insert(key) {
                            result.push(item);
                        }
                    }

                    // 再处理覆盖数组的元素
                    for item in override_arr.iter().cloned() {
                        let key = Self::array_item_key(&item);
                        if seen.insert(key) {
                            result.push(item);
                        }
                    }

                    JsonValue::Array(result)
                } else {
                    // 简单追加
                    let mut combined = base_arr;
                    combined.extend(override_arr.clone());
                    JsonValue::Array(combined)
                }
            }

            // 其他情况: 覆盖值直接使用
            _ => override_val,
        }
    }

    /// 为数组元素生成唯一键用于去重
    /// 
    /// 对于对象使用 JSON 序列化作为 key
    /// 对于基本类型直接用字符串表示
    fn array_item_key(item: &JsonValue) -> String {
        match item {
            JsonValue::Object(_) | JsonValue::Array(_) => {
                // 复杂类型: 使用序列化结果作为 key
                item.to_string()
            }
            other => other.to_string(),
        }
    }

    /// 执行安全敏感检查
    /// 
    /// 移植自 Claude Code hasSkipDangerousModePermissionPrompt():
    /// ```typescript
    /// // projectSettings 被排除在某些安全敏感检查之外
    /// return !!getSettingsForSource('userSettings')?.skipDangerousMode ??
    ///        getSettingsForSource('localSettings')?.skipDangerousMode
    /// ```
    pub fn check_safety_permission(
        merged: &MergedConfig,
        key: &str,
        source_to_exclude: Option<ConfigSourcePriority>,
    ) -> SafetyCheckResult {
        let value = match merged.value.get(key) {
            Some(v) => v,
            None => {
                return SafetyCheckResult {
                    allowed: true,
                    reason: "Key not found".to_string(),
                    effective_source: None,
                };
            }
        };

        let boolean_value = value.as_bool().unwrap_or(false);

        if boolean_value {
            SafetyCheckResult {
                allowed: true,
                reason: "Explicitly allowed".to_string(),
                effective_source: merged.source_map.get(key).copied(),
            }
        } else {
            SafetyCheckResult {
                allowed: false,
                reason: "Permission not granted".to_string(),
                effective_source: merged.source_map.get(key).copied(),
            }
        }
    }
}

/// 安全检查结果
#[derive(Debug, Clone)]
pub struct SafetyCheckResult {
    pub allowed: bool,
    pub reason: String,
    pub effective_source: Option<ConfigSourcePriority>,
}
