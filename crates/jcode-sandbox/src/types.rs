// ════════════════════════════════════════════════════════════════
// 沙箱与权限核心类型
// ════════════════════════════════════════════════════════════════

use std::collections::HashSet;
use serde::{Deserialize, Serialize};

/// 权限模式 (对应 Claude Code PermissionMode 层次结构)
/// 
/// 严格度排序 (从低到高):
///   BypassPermissions < AcceptEdits < Auto < Default < Plan
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PermissionMode {
    /// 跳过所有权限检查 (最宽松)
    Bypass,
    
    /// 自动接受文件编辑
    AcceptEdits,
    
    /// AI 自动分类决策 (YOLO)
    Auto,
    
    /// 默认模式: 需要用户确认工具调用
    Default,
    
    /// 仅规划模式，不执行任何操作 (最严格)
    Plan,
}

impl Default for PermissionMode {
    fn default() -> Self {
        Self::Default
    }
}

impl std::fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bypass => write!(f, "bypass-permissions"),
            Self::AcceptEdits => write!(f, "accept-edits"),
            Self::Auto => write!(f, "auto"),
            Self::Default => write!(f, "default"),
            Self::Plan => write!(f, "plan"),
        }
    }
}

/// 权限行为三元组
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionBehavior {
    Allow,
    Deny { reason: String },
    Ask { reason: String },
}

/// 权限决定结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionDecision {
    pub behavior: DecisionBehavior,
    
    pub mode: PermissionMode,
    
    /// 是否为 bypass-immune 决定 (某些安全检查不可绕过)
    pub safety_check: bool,
    
    /// 规则来源追踪 (调试用)
    pub rule_source: Option<String>,
}

impl PermissionDecision {
    pub fn allow(mode: PermissionMode) -> Self {
        Self {
            behavior: DecisionBehavior::Allow,
            mode,
            safety_check: false,
            rule_source: None,
        }
    }
    
    pub fn deny(reason: impl Into<String>, mode: PermissionMode) -> Self {
        Self {
            behavior: DecisionBehavior::Deny { reason: reason.into() },
            mode,
            safety_check: false,
            rule_source: None,
        }
    }
    
    pub fn ask(reason: impl Into<String>, mode: PermissionMode) -> Self {
        Self {
            behavior: DecisionBehavior::Ask { reason: reason.into() },
            mode,
            safety_check: false,
            rule_source: None,
        }
    }
    
    pub fn is_allowed(&self) -> bool {
        matches!(self.behavior, DecisionBehavior::Allow)
    }
    
    pub fn needs_user_input(&self) -> bool {
        matches!(self.behavior, DecisionBehavior::Ask { .. })
    }

    pub fn is_safety_check(&self) -> bool {
        self.safety_check
    }
}

/// 权限规则定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    /// 工具名
    pub tool_name: String,
    
    /// 匹配模式
    pub pattern: RulePattern,
    
    /// 行为: Allow / Deny / Ask
    pub behavior: DecisionBehavior,
    
    /// 规则优先级 (数值越高越优先)
    pub priority: u32,
    
    /// 规则描述
    pub description: Option<String>,
}

/// 规则匹配类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleMatch {
    Exact,     // 精确匹配
    Prefix,    // 前缀匹配 (git status:*)
    Wildcard,  // 通配符匹配 (git*)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulePattern {
    pub content: String,
    pub match_type: RuleMatch,
}

// ════════════════════════════════════════════════════════════════

/// 命令危险等级
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CommandSeverity {
    /// 安全 (只读命令如 cat/ls/git status)
    Safe,
    
    /// 低风险 (创建/修改非关键文件)
    Low,
    
    /// 中风险 (修改系统配置、网络访问)
    Medium,
    
    /// 高风险 (删除、覆盖、包发布)
    High,
    
    /// 严重危险 (rm -rf /, 格式化磁盘, sudo)
    Critical,
}

/// 命令沙箱执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxResult {
    /// 是否允许执行
    pub allowed: bool,
    
    /// 危险等级
    pub severity: Option<CommandSeverity>,
    
    /// 阻止原因 (如果不允许)
    pub block_reason: Option<String>,
    
    /// 是否需要用户确认
    pub requires_approval: bool,
    
    /// 建议的替代命令 (如果原命令被阻止)
    pub suggestion: Option<String>,
}

/// 安全检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyCheckResult {
    /// 是否通过安全检查
    pub safe: bool,
    
    /// 检查失败的原因列表
    pub violations: Vec<SafetyViolation>,
    
    /// 是否为强制审批 (不可绕过)
    pub force_approval: bool,
}

/// 安全违规项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyViolation {
    /// 违规类型
    pub violation_type: SafetyViolationType,
    
    /// 违规路径/内容
    pub target: String,
    
    /// 描述
    pub description: String,
}

/// 安全违规类型
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SafetyViolationType {
    SensitiveDirectory,   // .git/, .vscode/, .claude/
    SensitiveFile,        // .env, credentials, SSH keys
    PathTraversal,        // ../ 路径穿越攻击
    DangerousCommand,     // rm -rf, mkfs 等
    NetworkAccess,        // 未授权的网络请求
    EnvironmentExposure,  // 泄露环境变量
    SymlinkAttack,        // 符号链接攻击
}

/// YOLO 分类器结果 (可选 AI 功能)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YoloClassificationResult {
    /// 是否应该阻止
    pub should_block: bool,
    
    /// 阻止原因
    pub reason: String,
    
    /// 置信度 (0.0 - 1.0)
    pub confidence: f64,
}
