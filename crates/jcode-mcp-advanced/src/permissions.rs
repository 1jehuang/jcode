// ════════════════════════════════════════════════════════════════
// MCP 权限协商 — 工具调用前的权限检查
//
// 对应 Claude Code:
//   - channelPermissions.ts
//   - channelNotification.ts
//   - channelAllowlist.ts
// ════════════════════════════════════════════════════════════════

use serde::{Deserialize, Serialize};

/// 权限级别
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PermissionLevel {
    /// 无限制 — 自动允许所有操作
    None,
    /// 只读操作自动允许，修改需确认
    ConfirmWrites,
    /// 所有操作都需要确认
    ConfirmAll,
    /// 禁止所有工具调用
    BlockAll,
}

impl Default for PermissionLevel {
    fn default() -> Self {
        Self::ConfirmWrites // 最安全的默认值
    }
}

/// 单个工具的权限规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPermissionRule {
    pub tool_name: String,
    pub pattern: Option<String>,  // 参数匹配模式
    pub level: PermissionLevel,
}

/// MCP 连接的权限配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConnectionPermissionConfig {
    /// 全局默认权限级别
    pub global_level: PermissionLevel,

    /// 每个工具的特定规则
    pub tool_rules: Vec<ToolPermissionRule>,

    /// 白名单: 这些工具/参数组合始终被允许
    pub allowlist: Vec<ToolPermissionRule>,

    /// 黑名单: 这些工具/参数组合始终被阻止
    pub blocklist: Vec<ToolPermissionRule>,

    /// 是否记录所有权限决策到日志
    pub audit_logging: bool,
}

impl Default for McpConnectionPermissionConfig {
    fn default() -> Self {
        Self {
            global_level: Default::default(),
            tool_rules: vec![],
            allowlist: vec![],
            blocklist: vec![],
            audit_logging: true,
        }
    }
}

/// 权限检查结果
#[derive(Debug, Clone)]
pub struct PermissionCheckResult {
    pub allowed: bool,
    pub reason: String,
    pub level: PermissionLevel,
    pub rule_source: Option<String>,
}

impl McpConnectionPermissionConfig {
    /// 检查工具调用是否需要用户确认
    pub fn check_tool_call(&self, tool_name: &str, _arguments: &serde_json::Value) -> PermissionCheckResult {
        // 1. Check blocklist first (highest priority)
        for rule in &self.blocklist {
            if rule.tool_name == tool_name || rule.tool_name == "*" {
                return PermissionCheckResult {
                    allowed: false,
                    reason: format!("Tool '{}' is in blocklist", tool_name),
                    level: PermissionLevel::BlockAll,
                    rule_source: Some("blocklist".into()),
                };
            }
        }

        // 2. Check allowlist
        for rule in &self.allowlist {
            if rule.tool_name == tool_name || rule.tool_name == "*" {
                return PermissionCheckResult {
                    allowed: true,
                    reason: "In allowlist".into(),
                    level: PermissionLevel::None,
                    rule_source: Some("allowlist".into()),
                };
            }
        }

        // 3. Check specific tool rules
        for rule in &self.tool_rules {
            if rule.tool_name == tool_name {
                return match rule.level {
                    PermissionLevel::None => PermissionCheckResult { allowed: true, reason: "Explicitly allowed".into(), level: rule.level.clone(), rule_source: Some("tool_rule".into()) },
                    PermissionLevel::BlockAll => PermissionCheckResult { allowed: false, reason: "Explicitly blocked by rule".into(), level: rule.level.clone(), rule_source: Some("tool_rule".into()) },
                    _ => PermissionCheckResult { 
                        allowed: false,  // needs confirmation
                        reason: "Requires confirmation per rule".into(),
                        level: rule.level.clone(), 
                        rule_source: Some("tool_rule".into()) 
                    },
                };
            }
        }

        // 4. Fall back to global level
        match &self.global_level {
            PermissionLevel::None => PermissionCheckResult { allowed: true, reason: "Default: no restriction".into(), level: self.global_level.clone(), rule_source: None },
            PermissionLevel::ConfirmWrites => {
                let is_write = Self::is_write_operation(tool_name);
                PermissionCheckResult {
                    allowed: !is_write,
                    reason: if is_write { "Write operation requires confirmation" } else { "Read operation auto-allowed" }.into(),
                    level: self.global_level.clone(),
                    rule_source: Some("global_default".into()),
                }
            },
            _ => PermissionCheckResult { allowed: false, reason: format!("Global policy requires confirmation ({:?})", self.global_level), level: self.global_level.clone(), rule_source: Some("global_default".into()) },
        }
    }

    fn is_write_operation(tool_name: &str) -> bool {
        matches!(tool_name.to_lowercase().as_str(),
            "write" | "edit" | "create" | "delete" | "update" | "modify" | "file_write"
                | "file_edit" | "directory_create"
        )
    }
}
