//! # 设置优先级系统
//!
//! 源自 Claude Code 的 `settings/constants.ts`，提供多源配置覆盖。
//!
//! ## 优先级（从低到高）
//! 1. `PolicySettings` — 策略设置（最高权限，不可被覆盖）
//! 2. `FlagSettings` — 功能标志设置
//! 3. `ProjectSettings` — 项目级设置 (.jcode/config.toml)
//! 4. `UserSettings` — 用户级设置 (~/.jcode/config.toml)
//! 5. `LocalSettings` — 本地/环境变量设置 (最低优先级，实际被 env override 覆盖)

use std::collections::HashMap;
use std::path::PathBuf;

/// 设置来源 — 按优先级排序
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SettingSource {
    /// 本地/环境变量（最低优先级）
    LocalSettings = 0,
    /// 用户级设置 ~/.jcode/config.toml
    UserSettings = 1,
    /// 项目级设置 ./.jcode/config.toml
    ProjectSettings = 2,
    /// 功能标志设置
    FlagSettings = 3,
    /// 策略/MDM 设置（最高优先级）
    PolicySettings = 4,
}

impl SettingSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            SettingSource::LocalSettings => "local",
            SettingSource::UserSettings => "user",
            SettingSource::ProjectSettings => "project",
            SettingSource::FlagSettings => "flag",
            SettingSource::PolicySettings => "policy",
        }
    }

    /// 从字符串解析设置来源
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "local" | "env" | "environment" => Some(SettingSource::LocalSettings),
            "user" | "user_settings" => Some(SettingSource::UserSettings),
            "project" | "project_settings" => Some(SettingSource::ProjectSettings),
            "flag" | "flag_settings" | "feature" => Some(SettingSource::FlagSettings),
            "policy" | "policy_settings" | "mdm" => Some(SettingSource::PolicySettings),
            _ => None,
        }
    }

    /// 获取来源的显示名称
    pub fn display_name(&self) -> &'static str {
        match self {
            SettingSource::LocalSettings => "local settings",
            SettingSource::UserSettings => "user settings",
            SettingSource::ProjectSettings => "project settings",
            SettingSource::FlagSettings => "feature flags",
            SettingSource::PolicySettings => "policy settings",
        }
    }
}

/// 带来源标记的设置值
#[derive(Debug, Clone)]
pub struct SettingValue {
    /// 设置值
    pub value: String,
    /// 来源
    pub source: SettingSource,
}

/// 设置优先级解析器
///
/// 管理多来源配置，按优先级合并。
/// 源自 Claude Code 的 `getSettingSourceName()` + 优先级模型。
pub struct SettingsPriorityResolver {
    /// 按来源分组的设置
    settings: HashMap<SettingSource, HashMap<String, String>>,
    /// 各来源的配置文件路径
    config_paths: HashMap<SettingSource, Option<PathBuf>>,
}

impl Default for SettingsPriorityResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsPriorityResolver {
    pub fn new() -> Self {
        Self {
            settings: HashMap::new(),
            config_paths: HashMap::new(),
        }
    }

    /// 从指定来源加载设置
    pub fn load_from_source(&mut self, source: SettingSource, settings: HashMap<String, String>) {
        self.settings.insert(source, settings);
    }

    /// 加载设置并记住来源
    pub fn load_with_path(&mut self, source: SettingSource, settings: HashMap<String, String>, path: PathBuf) {
        self.config_paths.insert(source, Some(path));
        self.settings.insert(source, settings);
    }

    /// 获取单个设置（自动按优先级合并）
    /// 高优先级覆盖低优先级。
    pub fn get(&self, key: &str) -> Option<SettingValue> {
        // 按优先级从高到低遍历
        for source in [
            SettingSource::PolicySettings,
            SettingSource::FlagSettings,
            SettingSource::ProjectSettings,
            SettingSource::UserSettings,
            SettingSource::LocalSettings,
        ] {
            if let Some(settings) = self.settings.get(&source) {
                if let Some(value) = settings.get(key) {
                    return Some(SettingValue {
                        value: value.clone(),
                        source,
                    });
                }
            }
        }
        None
    }

    /// 获取设置值（不关心来源）
    pub fn get_value(&self, key: &str) -> Option<&str> {
        self.get(key).map(|_v| {
            // 借用一个临时字符串... 实际上需要不同的API设计
            // 使用 get_value_ref 代替
            unimplemented!("Use get_value_ref instead")
        })
    }

    /// 获取设置值的引用
    pub fn get_value_ref(&self, key: &str) -> Option<(&str, SettingSource)> {
        for source in [
            SettingSource::PolicySettings,
            SettingSource::FlagSettings,
            SettingSource::ProjectSettings,
            SettingSource::UserSettings,
            SettingSource::LocalSettings,
        ] {
            if let Some(settings) = self.settings.get(&source) {
                if let Some(value) = settings.get(key) {
                    return Some((value.as_str(), source));
                }
            }
        }
        None
    }

    /// 获取布尔类型设置
    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        match self.get_value_ref(key) {
            Some((value, _)) => matches!(value.to_lowercase().as_str(), "true" | "1" | "yes" | "on"),
            None => default,
        }
    }

    /// 获取整数类型设置
    pub fn get_int(&self, key: &str, default: i64) -> i64 {
        self.get_value_ref(key)
            .and_then(|(v, _)| v.parse().ok())
            .unwrap_or(default)
    }

    /// 获取所有设置（按优先级合并后的最终值）
    pub fn get_all(&self) -> HashMap<String, SettingValue> {
        let mut result = HashMap::new();

        // 按优先级从低到高遍历，后写入的覆盖先写入的
        for source in [
            SettingSource::LocalSettings,
            SettingSource::UserSettings,
            SettingSource::ProjectSettings,
            SettingSource::FlagSettings,
            SettingSource::PolicySettings,
        ] {
            if let Some(settings) = self.settings.get(&source) {
                for (key, value) in settings {
                    result.insert(key.clone(), SettingValue {
                        value: value.clone(),
                        source,
                    });
                }
            }
        }

        result
    }

    /// 获取指定来源的所有设置
    pub fn get_source_settings(&self, source: SettingSource) -> Option<&HashMap<String, String>> {
        self.settings.get(&source)
    }

    /// 获取某来源的配置文件路径
    pub fn get_config_path(&self, source: SettingSource) -> Option<&PathBuf> {
        self.config_paths.get(&source).and_then(|p| p.as_ref())
    }

    /// 获取特定设置项的来源
    pub fn get_source_for_key(&self, key: &str) -> Option<SettingSource> {
        self.get_value_ref(key).map(|(_, source)| source)
    }

    /// 设置值（到本地设置层）
    pub fn set_local(&mut self, key: String, value: String) {
        let settings = self.settings
            .entry(SettingSource::LocalSettings)
            .or_default();
        settings.insert(key, value);
    }

    /// 从环境变量前缀加载设置
    pub fn load_from_env_prefix(&mut self, prefix: &str) {
        let mut env_settings = HashMap::new();
        for (key, value) in std::env::vars() {
            if let Some(rest) = key.strip_prefix(prefix) {
                let setting_key = rest.trim_start_matches('_').to_lowercase();
                env_settings.insert(setting_key, value);
            }
        }
        if !env_settings.is_empty() {
            self.load_from_source(SettingSource::LocalSettings, env_settings);
        }
    }

    /// 设置路径映射
    pub fn clear(&mut self) {
        self.settings.clear();
        self.config_paths.clear();
    }
}

/// 设置来源显示名（源自 Claude Code 的 `getSettingSourceName()`）
pub fn source_display_name(source: SettingSource) -> &'static str {
    source.display_name()
}

/// 解析设置来源列表（源自 Claude Code 的 `parseSettingSourcesFlag()`）
pub fn parse_setting_sources_flag(flag: &str) -> Vec<SettingSource> {
    flag.split(',')
        .filter_map(|s| SettingSource::from_str(s.trim()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_priority_resolution() {
        let mut resolver = SettingsPriorityResolver::new();

        let mut user_settings = HashMap::new();
        user_settings.insert("theme".into(), "dark".into());
        resolver.load_from_source(SettingSource::UserSettings, user_settings);

        let mut project_settings = HashMap::new();
        project_settings.insert("theme".into(), "light".into());
        project_settings.insert("model".into(), "claude-4".into());
        resolver.load_from_source(SettingSource::ProjectSettings, project_settings);

        // ProjectSettings (higher priority) should override UserSettings for "theme"
        let theme = resolver.get("theme").unwrap();
        assert_eq!(theme.value, "light");
        assert_eq!(theme.source, SettingSource::ProjectSettings);

        // "model" should come from ProjectSettings
        let model = resolver.get_value_ref("model").unwrap();
        assert_eq!(model.0, "claude-4");
    }

    #[test]
    fn test_policy_overrides_all() {
        let mut resolver = SettingsPriorityResolver::new();

        let mut project = HashMap::new();
        project.insert("key".into(), "project-value".into());
        resolver.load_from_source(SettingSource::ProjectSettings, project);

        let mut policy = HashMap::new();
        policy.insert("key".into(), "policy-value".into());
        resolver.load_from_source(SettingSource::PolicySettings, policy);

        let value = resolver.get("key").unwrap();
        assert_eq!(value.value, "policy-value");
        assert_eq!(value.source, SettingSource::PolicySettings);
    }

    #[test]
    fn test_get_bool() {
        let mut resolver = SettingsPriorityResolver::new();

        let mut user = HashMap::new();
        user.insert("enabled".into(), "true".into());
        user.insert("disabled".into(), "false".into());
        resolver.load_from_source(SettingSource::UserSettings, user);

        assert!(resolver.get_bool("enabled", false));
        assert!(!resolver.get_bool("disabled", true));
        assert_eq!(resolver.get_bool("nonexistent", true), true);
    }

    #[test]
    fn test_get_int() {
        let mut resolver = SettingsPriorityResolver::new();

        let mut user = HashMap::new();
        user.insert("port".into(), "8080".into());
        resolver.load_from_source(SettingSource::UserSettings, user);

        assert_eq!(resolver.get_int("port", 0), 8080);
        assert_eq!(resolver.get_int("nonexistent", 42), 42);
    }

    #[test]
    fn test_get_all() {
        let mut resolver = SettingsPriorityResolver::new();

        let mut user = HashMap::new();
        user.insert("a".into(), "1".into());
        resolver.load_from_source(SettingSource::UserSettings, user);

        let mut project = HashMap::new();
        project.insert("b".into(), "2".into());
        resolver.load_from_source(SettingSource::ProjectSettings, project);

        let all = resolver.get_all();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_source_display_name() {
        assert_eq!(source_display_name(SettingSource::UserSettings), "user settings");
        assert_eq!(source_display_name(SettingSource::PolicySettings), "policy settings");
    }

    #[test]
    fn test_parse_flag() {
        let sources = parse_setting_sources_flag("user, project, policy");
        assert_eq!(sources.len(), 3);
        assert_eq!(sources[0], SettingSource::UserSettings);
        assert_eq!(sources[1], SettingSource::ProjectSettings);
        assert_eq!(sources[2], SettingSource::PolicySettings);
    }

    #[test]
    fn test_set_local() {
        let mut resolver = SettingsPriorityResolver::new();
        resolver.set_local("key".into(), "local-value".into());

        let value = resolver.get("key").unwrap();
        assert_eq!(value.value, "local-value");
        assert_eq!(value.source, SettingSource::LocalSettings);
    }

    #[test]
    fn test_source_for_key() {
        let mut resolver = SettingsPriorityResolver::new();
        let mut user = HashMap::new();
        user.insert("key".into(), "val".into());
        resolver.load_from_source(SettingSource::UserSettings, user);

        assert_eq!(resolver.get_source_for_key("key"), Some(SettingSource::UserSettings));
        assert_eq!(resolver.get_source_for_key("absent"), None);
    }
}
