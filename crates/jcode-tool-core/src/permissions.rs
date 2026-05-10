//! # 工具权限上下文
//!
//! 译自 Claude Code CLI 的 `ToolPermissionContext` 和 `PermissionResult` (Tool.ts)。
//!
//! 提供细粒度的工具权限控制：
//! - 权限模式（默认/绕过/自动）
//! - 允许/拒绝/询问规则
//! - 工具级别的权限检查

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// 权限模式 — 译自 `PermissionMode`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    /// 默认模式：每次工具调用都询问用户
    Default,
    /// 绕过权限：自动允许所有工具调用
    Bypass,
    /// 自动模式：根据规则自动决定
    Auto,
}

impl Default for PermissionMode {
    fn default() -> Self { Self::Default }
}

/// 权限规则条目 — 译自规则匹配部分
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    /// 匹配的模式（支持 glob）
    pub pattern: String,
    /// 规则行为
    pub behavior: PermissionBehavior,
}

/// 权限行为
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionBehavior {
    /// 允许
    Allow,
    /// 拒绝
    Deny,
    /// 询问用户
    Ask,
}

/// 工具权限上下文 — 译自 Claude Code 的 `ToolPermissionContext`
///
/// 在每次工具调用前检查，决定是否允许执行。
#[derive(Debug, Clone)]
pub struct ToolPermissionContext {
    /// 当前权限模式
    pub mode: PermissionMode,
    /// 始终允许的规则（按来源分组）
    pub always_allow_rules: HashMap<String, Vec<PermissionRule>>,
    /// 始终拒绝的规则
    pub always_deny_rules: HashMap<String, Vec<PermissionRule>>,
    /// 始终询问的规则
    pub always_ask_rules: HashMap<String, Vec<PermissionRule>>,
    /// 是否可用绕过模式
    pub can_bypass: bool,
    /// 是否可用自动模式
    pub auto_mode_available: bool,
}

impl Default for ToolPermissionContext {
    fn default() -> Self {
        Self {
            mode: PermissionMode::Default,
            always_allow_rules: HashMap::new(),
            always_deny_rules: HashMap::new(),
            always_ask_rules: HashMap::new(),
            can_bypass: false,
            auto_mode_available: false,
        }
    }
}

/// 权限检查结果 — 译自 `PermissionResult`
#[derive(Debug, Clone)]
pub struct PermissionResult {
    /// 权限行为：允许/拒绝/需要询问
    pub behavior: PermissionBehavior,
    /// 可选的更新后的输入（在允许时修改）
    pub updated_input: Option<serde_json::Value>,
    /// 拒绝原因（仅在拒绝时有用）
    pub reason: Option<String>,
}

impl PermissionResult {
    /// 允许（默认实现）
    pub fn allow() -> Self {
        Self {
            behavior: PermissionBehavior::Allow,
            updated_input: None,
            reason: None,
        }
    }

    /// 允许并更新输入
    pub fn allow_with_input(input: serde_json::Value) -> Self {
        Self {
            behavior: PermissionBehavior::Allow,
            updated_input: Some(input),
            reason: None,
        }
    }

    /// 拒绝
    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            behavior: PermissionBehavior::Deny,
            updated_input: None,
            reason: Some(reason.into()),
        }
    }

    /// 需要询问
    pub fn ask() -> Self {
        Self {
            behavior: PermissionBehavior::Ask,
            updated_input: None,
            reason: None,
        }
    }
}

/// 工具过滤上下文 — 用于 `getAllTools()` -> `getTools(permissionContext)` 模式
///
/// 译自 Claude Code 的权限上下文过滤。
#[derive(Debug, Clone, Default)]
pub struct ToolFilterContext {
    /// 是否在简单模式（仅 Bash/Read/Edit）
    pub simple_mode: bool,
    /// 是否在协调器模式
    pub coordinator_mode: bool,
    /// 是否在 REPL 模式
    pub repl_mode: bool,
    /// 是否在远程模式
    pub remote_mode: bool,
    /// 明确允许的工具名称集合（None = 全部允许）
    pub allowed_tool_names: Option<HashSet<String>>,
    /// 明确拒绝的工具名称集合
    pub denied_tool_names: HashSet<String>,
}

impl ToolFilterContext {
    /// 检查工具是否应该被包含
    pub fn should_include(&self, tool_name: &str) -> bool {
        // 先检查拒绝列表
        if self.denied_tool_names.contains(tool_name) {
            return false;
        }
        // 再检查允许列表
        if let Some(ref allowed) = self.allowed_tool_names {
            return allowed.contains(tool_name);
        }
        true
    }
}

/// 工具安全性白名单 — 译自 `REMOTE_SAFE_COMMANDS` / `BRIDGE_SAFE_COMMANDS`
#[derive(Debug, Clone, Default)]
pub struct ToolSafetyContext {
    /// 在远程模式下安全的工具
    pub remote_safe: HashSet<String>,
    /// 在桥接模式下安全的工具
    pub bridge_safe: HashSet<String>,
}

impl ToolSafetyContext {
    pub fn new() -> Self { Self::default() }

    /// 标记工具为远程安全
    pub fn with_remote_safe(mut self, names: &[&str]) -> Self {
        for name in names {
            self.remote_safe.insert(name.to_string());
        }
        self
    }

    /// 标记工具为桥接安全
    pub fn with_bridge_safe(mut self, names: &[&str]) -> Self {
        for name in names {
            self.bridge_safe.insert(name.to_string());
        }
        self
    }
}
