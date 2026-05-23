//! # Tool 增强生命周期 — 借鉴 Claude Code 的 Tool 接口设计
//!
//! 在现有 `jcode-tool-core::Tool` trait 基础上补充：
//! - `validate_input()` — 输入预验证（Claude Code 的 validateInput）
//! - `prompt_description()` — 工具自身提示词描述（Claude Code 的 prompt()）
//! - `interrupt_behavior()` — 中断行为策略（Claude Code 的 interruptBehavior）
//! - `check_permissions()` — 权限检查（Claude Code 的 checkPermissions）
//! - `render_progress()` — 进度展示（Claude Code 的 renderToolUseProgressMessage）
//!
//! 这些 trait 作为可选补充，不破坏现有 Tool trait 的向后兼容性

use async_trait::async_trait;
use serde_json::Value;
use std::fmt;

/// 中断行为策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptBehavior {
    /// 取消当前执行（默认）
    Cancel,
    /// 阻塞等待完成
    Block,
}

/// 输入验证结果
#[derive(Debug)]
pub enum ValidationResult {
    /// 验证通过
    Valid,
    /// 警告（可继续执行）
    Warning(String),
    /// 错误（不可执行）
    Error(String),
}

/// 权限检查结果
#[derive(Debug)]
pub enum PermissionResult {
    /// 允许
    Allowed,
    /// 拒绝
    Denied(String),
    /// 需要用户确认
    NeedsConfirmation(String),
}

/// 工具进度信息（用于 UI 渲染）
#[derive(Debug, Clone)]
pub struct ToolProgress {
    /// 进度百分比 (0-100)
    pub percent: Option<f64>,
    /// 进度消息
    pub message: String,
    /// 当前完成量
    pub current: Option<u64>,
    /// 总量
    pub total: Option<u64>,
    /// 预估剩余秒数
    pub eta_seconds: Option<f64>,
}

/// 增强的工具生命周期
#[async_trait]
pub trait ToolLifecycle: Send + Sync {
    /// 验证输入参数 — 在 execute 之前调用
    /// 源自 Claude Code 的 `validateInput(input, context)`
    async fn validate_input(&self, input: &Value) -> ValidationResult {
        // 默认实现：检查 JSON schema 兼容性（由 Rust 类型系统保证）
        ValidationResult::Valid
    }

    /// 获取工具自身的提示词描述（注入 system prompt 用）
    /// 源自 Claude Code 的 `prompt(options)`
    fn prompt_description(&self) -> Option<String> {
        None
    }

    /// 中断行为策略
    /// 源自 Claude Code 的 `interruptBehavior()`
    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    /// 权限检查 — 在执行前调用
    /// 源自 Claude Code 的 `checkPermissions(input, context)`
    async fn check_permissions(&self, input: &Value) -> PermissionResult {
        PermissionResult::Allowed
    }

    /// 获取工具使用的摘要
    /// 源自 Claude Code 的 `getToolUseSummary(input)`
    fn tool_use_summary(&self, input: &Value) -> Option<String> {
        None
    }

    /// 获取工具的活跃描述
    /// 源自 Claude Code 的 `getActivityDescription(input)`
    fn activity_description(&self) -> String {
        format!("Using {}", self.tool_name())
    }

    /// 工具名称（用于错误消息等）
    fn tool_name(&self) -> &str;
}

/// 渲染工具执行进度
/// 源自 Claude Code 的 `renderToolUseProgressMessage(progress, options)`
pub fn render_tool_progress(tool_name: &str, progress: &ToolProgress) -> String {
    match progress.percent {
        Some(pct) => format!("[{}] {:.0}% — {}", tool_name, pct, progress.message),
        None => format!("[{}] {}", tool_name, progress.message),
    }
}

/// 渲染工具使用消息
/// 源自 Claude Code 的 `renderToolUseMessage(input, options)`
pub fn render_tool_use(tool_name: &str, input: &Value) -> String {
    let summary = match input.get("file_path").or_else(|| input.get("path")) {
        Some(path) => format!("{} on {}", tool_name, path.as_str().unwrap_or("?")),
        None => format!("{}", tool_name),
    };
    summary
}

/// 渲染工具结果消息
/// 源自 Claude Code 的 `renderToolResultMessage(content, progress, options)`
pub fn render_tool_result(tool_name: &str, success: bool, summary: &str) -> String {
    let icon = if success { "✅" } else { "❌" };
    format!("{} {} — {}", icon, tool_name, summary)
}
