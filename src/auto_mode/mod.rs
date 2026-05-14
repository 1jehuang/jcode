//! # Auto Mode - 智能自动模式引擎
//!
//! 提供基于机器学习的智能决策系统，支持：
//! - **置信度评估** - 基于历史数据计算操作安全性
//! - **模式学习** - 记录用户决策，动态调整策略
//! - **敏感词检测** - 自动识别危险操作
//! - **安全白名单** - 低风险操作自动批准
//! - **统计监控** - 追踪自动/手动决策比例
//!
//! ## 决策流程
//!
//! ```
//! 用户请求 → should_auto_approve()
//!     │
//!     ├─ 模式未启用 → ManualReview (完全人工)
//!     │
//!     ├─ 包含敏感词 → RequiresConfirmation (必须确认)
//!     │   └─ delete/rm/force/push/deploy
//!     │
//!     ├─ 匹配学习模式
//!     │   ├─ 置信度 ≥ 阈值 → AutoApprove (自动批准)
//!     │   └─ 置信度 < 阈值 → SuggestApprove (建议但需审核)
//!     │
//!     └─ 安全操作 + auto_accept_safe → AutoApprove
//!         └─ FileEdit / FileCreate
//! ```

pub mod aho_corasick;
pub mod confidence;
pub mod enhanced_confidence;
pub mod engine;
pub mod safety;
pub mod learning;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ==========================================
// 核心数据类型定义
// ==========================================

/// 操作类型分类
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    // 文件操作
    FileRead,
    FileWrite,
    FileEdit,
    FileCreate,
    FileDelete,
    FileMove,
    FileCopy,

    // Bash命令
    BashCommand,
    BashScript,

    // Git操作
    GitCommit,
    GitPush,
    GitPull,
    GitBranch,
    GitMerge,
    GitRebase,
    GitReset,
    GitCheckout,

    // 网络请求
    HttpRequest,
    ApiCall,

    // 数据库操作
    DatabaseQuery,
    DatabaseMigration,
    DatabaseBackup,

    // 部署操作
    Deploy,
    Rollback,

    // 包管理
    PackageInstall,
    PackageUpdate,
    PackageRemove,

    // 容器操作
    DockerBuild,
    DockerRun,
    DockerStop,
    DockerRemove,

    // 配置修改
    ConfigChange,
    EnvironmentVariable,

    // 自定义
    Custom(String),
}

impl std::fmt::Display for ActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionType::Custom(name) => write!(f, "{}", name),
            _ => {
                let s = serde_json::to_string(self).unwrap_or_default();
                write!(f, "{}", s.trim_matches('"'))
            }
        }
    }
}

/// 自动审批决策结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoApprovalDecision {
    /// 自动批准
    AutoApprove(String),
    /// 建议批准（需审核）
    SuggestApprove { reason: String, confidence: f64 },
    /// 需要确认
    RequiresConfirmation(String),
    /// 完全人工审核
    ManualReview,
    /// 拒绝执行
    Blocked(String),
}

impl AutoApprovalDecision {
    pub fn is_auto_approved(&self) -> bool {
        matches!(self, AutoApprovalDecision::AutoApprove(_))
    }

    pub fn is_blocked(&self) -> bool {
        matches!(self, AutoApprovalDecision::Blocked(_))
    }

    pub fn requires_confirmation(&self) -> bool {
        matches!(self, AutoApprovalDecision::RequiresConfirmation(_))
    }
}

/// Auto Mode配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoModeConfig {
    /// 是否启用Auto Mode
    pub enabled: bool,

    /// 置信度阈值 (0.0-1.0)，超过此值自动批准
    pub approval_threshold: f64,

    /// 是否自动接受安全操作
    pub auto_accept_safe: bool,

    /// 最大连续自动操作数
    pub max_auto_actions: u32,

    /// 当前已执行的自动操作数
    #[serde(skip)]
    pub current_auto_actions: u32,

    /// 需要确认的敏感词列表
    pub require_confirmation_for: Vec<String>,

    /// 完全阻止的命令模式
    pub blocked_patterns: Vec<String>,

    /// 自动批准的模式
    pub auto_approve_patterns: Vec<String>,

    /// 安全操作白名单（这些操作类型总是安全的）
    pub safe_action_types: Vec<ActionType>,

    /// 学习模式启用
    pub enable_learning: bool,

    /// 审计日志启用
    pub enable_audit_log: bool,

    /// 最大学习样本数
    pub max_learning_samples: usize,
}

impl Default for AutoModeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            approval_threshold: 0.85,
            auto_accept_safe: true,
            max_auto_actions: 50,
            current_auto_actions: 0,
            require_confirmation_for: vec![
                "delete".to_string(),
                "rm".to_string(),
                "rm -rf".to_string(),
                "deploy".to_string(),
                "push --force".to_string(),
                "drop".to_string(),
                "truncate".to_string(),
                "format".to_string(),
                "mkfs".to_string(),
                "dd if=".to_string(),
            ],
            blocked_patterns: vec![
                "rm -rf /".to_string(),
                "rm -rf /*".to_string(),
                ":(){ :|:& };:".to_string(),  // Fork bomb
                "> /dev/sda".to_string(),
                "mkfs".to_string(),
                "chmod -R 777 /".to_string(),
                "chown -R".to_string(),
                "wget.*| sh".to_string(),
                "curl.*| bash".to_string(),
                "curl.*| sh".to_string(),
            ],
            auto_approve_patterns: vec![
                "git status".to_string(),
                "git log".to_string(),
                "git diff".to_string(),
                "ls -la".to_string(),
                "pwd".to_string(),
                "echo ".to_string(),
                "cat ".to_string(),
                "which ".to_string(),
                "--help".to_string(),
                "-v".to_string(),
                "--version".to_string(),
            ],
            safe_action_types: vec![
                ActionType::FileRead,
                ActionType::BashCommand,  // 需要进一步检查命令内容
            ],
            enable_learning: true,
            enable_audit_log: true,
            max_learning_samples: 10000,
        }
    }
}

/// 工具上下文信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContext {
    /// 操作类型
    pub action_type: ActionType,

    /// 操作描述
    pub description: String,

    /// 文件路径（如果适用）
    pub file_path: Option<String>,

    /// 工具名称
    pub tool_name: Option<String>,

    /// 用户输入
    pub user_input: Option<String>,

    /// 项目路径
    pub project_path: Option<String>,

    /// 额外元数据
    pub metadata: HashMap<String, String>,
}

impl ToolContext {
    pub fn new(action_type: ActionType, description: &str) -> Self {
        Self {
            action_type,
            description: description.to_string(),
            file_path: None,
            tool_name: None,
            user_input: None,
            project_path: None,
            metadata: HashMap::new(),
        }
    }

    pub fn with_file(mut self, path: &str) -> Self {
        self.file_path = Some(path.to_string());
        self
    }

    pub fn with_tool(mut self, name: &str) -> Self {
        self.tool_name = Some(name.to_string());
        self
    }

    pub fn with_user_input(mut self, input: &str) -> Self {
        self.user_input = Some(input.to_string());
        self
    }

    pub fn with_project(mut self, path: &str) -> Self {
        self.project_path = Some(path.to_string());
        self
    }

    pub fn with_metadata(mut self, key: &str, value: &str) -> Self {
        self.metadata.insert(key.to_string(), value.to_string());
        self
    }
}

/// 统计信息
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AutoModeStats {
    /// 总决策次数
    pub total_decisions: u64,
    /// 自动批准次数
    pub auto_approved: u64,
    /// 需确认次数
    pub required_confirmation: u64,
    /// 人工审核次数
    pub manual_reviews: u64,
    /// 拒绝次数
    pub blocked: u64,
    /// 平均置信度
    pub avg_confidence: f64,
    /// 学习模式命中次数
    pub learning_pattern_hits: u64,
    /// 敏感词触发次数
    pub sensitive_word_triggers: u64,
}
