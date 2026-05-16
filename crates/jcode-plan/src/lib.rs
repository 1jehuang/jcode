// jcode-plan
// ════════════════════════════════════════════════════════════════
// 计划模式 (Plan Mode) — 移植自 Claude Code EnterPlanModeTool/ExitPlanModeV2Tool
//
// 双状态机:
//
//   +--------------+  用户审批通过   +--------------+
//   |   Plan Mode  | --------------->| Execute Mode |
//   |  (只规划)     |                |  (执行中)     |
//   +------+-------+ <---------------+------+-------+
//          | 用户修改计划              | 执行完成/取消
//          ▼                            ▼
//   +--------------+              +--------------+
//   |  Plan 编辑    |              |  结果展示     |
//   +--------------+              +--------------+
//
// 核心数据结构:
//   - Plan: 包含多个 Step 的有序列表
//   - PlanStep: 单个操作步骤, 可独立 approve/reject/modify
//   - PlanState: 记录当前模式 + 审批状态
// ════════════════════════════════════════════════════════════════

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 计划 ID
pub type PlanId = String;

/// 步骤 ID
pub type StepId = String;

/// 计划模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlanMode {
    /// 规划模式: 只生成计划, 不做任何修改
    Planning,
    /// 执行模式: 按计划逐步执行
    Executing,
}

/// 步骤状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
    Approved,
    Rejected { reason: String },
    Executing,
    Completed { output_summary: Option<String> },
    Failed { error: String },
    Skipped,
}

impl StepStatus {
    pub fn as_str(&self) -> &str {
        match self {
            StepStatus::Pending => "pending",
            StepStatus::Approved => "approved",
            StepStatus::Rejected { .. } => "rejected",
            StepStatus::Executing => "executing",
            StepStatus::Completed { .. } => "completed",
            StepStatus::Failed { .. } => "failed",
            StepStatus::Skipped => "skipped",
        }
    }
    
    pub fn from_status_str(s: &str) -> Self {
        match s {
            "approved" => StepStatus::Approved,
            "rejected" => StepStatus::Rejected { reason: "".to_string() },
            "executing" => StepStatus::Executing,
            "completed" => StepStatus::Completed { output_summary: None },
            "failed" => StepStatus::Failed { error: "".to_string() },
            "skipped" => StepStatus::Skipped,
            _ => StepStatus::Pending,
        }
    }
}

/// 单个计划步骤
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanStep {
    pub id: String,
    
    /// 序号 (从1开始)
    pub sequence: u32,
    
    /// 简短描述 (一行)
    pub title: String,
    
    /// 详细说明 (可选)
    pub description: Option<String>,
    
    /// 要执行的工具名
    pub tool_name: Option<String>, // None = 说明性步骤
    
    /// 工具参数 (JSON)
    pub tool_input: Option<serde_json::Value>,
    
    /// 预期影响 (文件变更/命令等)
    pub expected_impact: Option<String>,
    
    /// 当前状态
    pub status: String,
    
    /// 创建时间
    pub created_at: DateTime<Utc>,
    
    /// 完成时间
    pub completed_at: Option<DateTime<Utc>>,
    
    /// 用户备注 (审批时添加)
    pub user_note: Option<String>,
    
    /// 内容描述
    pub content: String,
    
    /// 分配给的会话
    pub assigned_to: Option<String>,
    
    /// 优先级
    pub priority: Option<u8>,
    
    /// 依赖的步骤 ID 列表
    pub blocked_by: Vec<String>,
}

impl PlanStep {
    pub fn new(sequence: u32, title: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            sequence,
            title: title.into(),
            description: None,
            tool_name: None,
            tool_input: None,
            expected_impact: None,
            status: "pending".to_string(),
            created_at: Utc::now(),
            completed_at: None,
            user_note: None,
            content: String::new(),
            assigned_to: None,
            priority: None,
            blocked_by: Vec::new(),
        }
    }

    pub fn with_tool(mut self, name: &str, input: serde_json::Value) -> Self {
        self.tool_name = Some(name.to_string());
        self.tool_input = Some(input);
        self
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn with_impact(mut self, impact: impl Into<String>) -> Self {
        self.expected_impact = Some(impact.into());
        self
    }

    pub fn is_executable(&self) -> bool {
        self.tool_name.is_some() && (self.status == "approved" || self.status == "pending")
    }

    pub fn is_completed(&self) -> bool {
        self.status == "completed" || self.status == "skipped"
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self.status.as_str(), "completed" | "failed" | "skipped" | "rejected")
    }
}

/// 计划主体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: PlanId,
    
    /// 计划标题
    pub title: String,
    
    /// 用户请求 / 目标描述
    pub goal: String,
    
    /// 有序步骤列表 (保持插入顺序)
    pub steps: IndexMap<StepId, PlanStep>,
    
    /// 当前模式
    pub mode: PlanMode,
    
    /// 创建时间
    pub created_at: DateTime<Utc>,
    
    /// 最后更新时间
    pub updated_at: DateTime<Utc>,
    
    /// 当前正在执行的步骤
    pub current_step_index: Option<usize>,
    
    /// 统计信息
    pub stats: PlanStats,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanStats {
    pub total_steps: u32,
    pub approved_count: u32,
    pub rejected_count: u32,
    pub completed_count: u32,
    pub failed_count: u32,
    pub skipped_count: u32,
}

impl Plan {
    pub fn new(title: impl Into<String>, goal: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            title: title.into(),
            goal: goal.into(),
            steps: IndexMap::new(),
            mode: PlanMode::Planning,
            created_at: now,
            updated_at: now,
            current_step_index: None,
            stats: Default::default(),
        }
    }

    /// 添加步骤
    pub fn add_step(&mut self, step: PlanStep) -> &mut Self {
        self.stats.total_steps += 1;
        let id = step.id.clone();
        self.steps.insert(id, step);
        self.updated_at = Utc::now();
        self
    }

    /// 批量添加步骤
    pub fn add_steps(&mut self, steps: Vec<PlanStep>) -> &mut Self {
        for step in steps {
            self.stats.total_steps += 1;
            let id = step.id.clone();
            self.steps.insert(id, step);
        }
        self.updated_at = Utc::now();
        self
    }

    /// 获取步骤的有序列表
    pub fn ordered_steps(&self) -> Vec<&PlanStep> {
        let mut steps: Vec<_> = self.steps.values().collect();
        steps.sort_by_key(|s| s.sequence);
        steps
    }

    /// 获取待执行的下一个步骤
    pub fn next_pending_step(&self) -> Option<(usize, &PlanStep)> {
        for (idx, step) in self.ordered_steps().iter().enumerate() {
            if step.status == "pending" || step.status == "approved" {
                return Some((idx, step));
            }
        }
        None
    }

    // --- 审批操作 ---------------------------

    /// 审批通过单个步骤
    pub fn approve_step(&mut self, step_id: &StepId) -> Result<(), String> {
        let step = self.steps.get_mut(step_id)
            .ok_or("Step not found")?;
        
        if step.status == "pending" {
            step.status = "approved".to_string();
            self.stats.approved_count += 1;
            self.updated_at = Utc::now();
            Ok(())
        } else {
            Err(format!("Cannot approve step in state {}", step.status))
        }
    }

    /// 拒绝单个步骤
    pub fn reject_step(&mut self, step_id: &StepId, _reason: impl Into<String>) -> Result<(), String> {
        let step = self.steps.get_mut(step_id)
            .ok_or("Step not found")?;
        
        if step.status == "pending" || step.status == "approved" {
            step.status = "rejected".to_string();
            self.stats.rejected_count += 1;
            self.updated_at = Utc::now();
            Ok(())
        } else {
            Err(format!("Cannot reject step in state {}", step.status))
        }
    }

    /// 全部批准
    pub fn approve_all(&mut self) {
        for step in self.steps.values_mut() {
            if step.status == "pending" {
                step.status = "approved".to_string();
                self.stats.approved_count += 1;
            }
        }
        self.updated_at = Utc::now();
    }

    /// 跳过步骤
    pub fn skip_step(&mut self, step_id: &StepId) -> Result<(), String> {
        let step = self.steps.get_mut(step_id)
            .ok_or("Step not found")?;
        
        if !step.is_terminal() {
            step.status = "skipped".to_string();
            self.stats.skipped_count += 1;
            self.updated_at = Utc::now();
            Ok(())
        } else {
            Err("Cannot skip a terminal step".into())
        }
    }

    // --- 模式切换 ---------------------------

    /// 切换到 Execute Mode
    pub fn enter_execute_mode(&mut self) -> Result<(), String> {
        if self.mode == PlanMode::Executing {
            return Err("Already in execute mode".into());
        }

        // 至少需要有一个已批准的可执行步骤
        let has_approved = self.steps.values()
            .any(|s| s.is_executable());

        if !has_approved && !self.steps.is_empty() {
            return Err("No approved executable steps".into());
        }

        self.mode = PlanMode::Executing;
        self.current_step_index = None;
        self.updated_at = Utc::now();

        tracing::info!(
            plan_id = %self.id,
            steps = self.stats.total_steps,
            "Entered execute mode"
        );

        Ok(())
    }

    /// 返回 Planning Mode
    pub fn enter_plan_mode(&mut self) {
        self.mode = PlanMode::Planning;
        self.current_step_index = None;
        self.updated_at = Utc::now();
    }

    /// 标记步骤开始执行
    pub fn start_step(&mut self, step_id: &StepId) -> Result<(), String> {
        if self.mode != PlanMode::Executing {
            return Err("Not in execute mode".into());
        }

        let step = self.steps.get_mut(step_id)
            .ok_or("Step not found")?;

        if !step.is_executable() {
            return Err(format!("Step '{}' is not executable", step.title));
        }

        step.status = "executing".to_string();
        self.updated_at = Utc::now();
        Ok(())
    }

    pub fn complete_step(&mut self, step_id: &StepId, _summary: Option<String>) -> Result<(), String> {
        let step = self.steps.get_mut(step_id)
            .ok_or("Step not found")?;

        if step.status == "executing" {
            step.status = "completed".to_string();
            step.completed_at = Some(Utc::now());
            self.stats.completed_count += 1;
            self.updated_at = Utc::now();
            Ok(())
        } else {
            Err(format!("Step is not executing (status: {})", step.status))
        }
    }

    pub fn fail_step(&mut self, step_id: &StepId, _error: impl Into<String>) -> Result<(), String> {
        let step = self.steps.get_mut(step_id)
            .ok_or("Step not found")?;

        if step.status == "executing" {
            step.status = "failed".to_string();
            step.completed_at = Some(Utc::now());
            self.stats.failed_count += 1;
            self.updated_at = Utc::now();
            Ok(())
        } else {
            Err(format!("Step is not executing (status: {})", step.status))
        }
    }

    /// 检查计划是否全部完成
    pub fn is_complete(&self) -> bool {
        self.steps.values().all(|s| s.is_terminal())
    }

    /// 获取计划完成百分比
    pub fn progress_percent(&self) -> f64 {
        if self.stats.total_steps == 0 { return 100.0; }      
        let done = self.stats.completed_count + self.stats.skipped_count + self.stats.rejected_count;
        (done as f64 / self.stats.total_steps as f64) * 100.0
    }

    /// 生成计划摘要文本
    pub fn summary_text(&self) -> String {
        let mut lines = vec![
            format!("## 计划: {}", self.title),
            format!("目标: {}", self.goal),
            format!("模式: {:?}", self.mode),
            format!("进度: {:.0}% ({}/{})",
                self.progress_percent(),
                self.stats.completed_count + self.stats.skipped_count,
                self.stats.total_steps
            ),
            String::from(""),
        ];

        for step in self.ordered_steps().iter() {
            let status_icon = match step.status.as_str() {
                "pending" => "⏳",
                "approved" => "✅",
                "rejected" => "❌",
                "executing" => "▶️",
                "completed" => "🟢",
                "failed" => "🔴",
                "skipped" => "⏭️",
                _ => "❓",
            };

            lines.push(format!(
                "{} {}. {}{}",
                status_icon,
                step.sequence,
                step.title,
                step.user_note.as_ref().map(|n| format!(" ({})", n)).unwrap_or_default()
            ));
        }

        lines.join("\n")
    }
}

// ════════════════════════════════════════════════════════════════
// Protocol compatibility layer — types/functions used by jcode-protocol
// ════════════════════════════════════════════════════════════════

/// Alias for protocol layer (jcode-protocol uses PlanItem internally)
pub type PlanItem = PlanStep;

/// Task progress tracking for swarm coordination
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SwarmTaskProgress {
    pub assigned_session_id: Option<String>,
    pub assignment_summary: Option<String>,
    pub assigned_at_unix_ms: Option<u64>,
    pub started_at_unix_ms: Option<u64>,
    pub last_heartbeat_unix_ms: Option<u64>,
    pub heartbeat_count: u32,
    pub last_detail: Option<String>,
    pub last_checkpoint_unix_ms: Option<u64>,
    pub checkpoint_count: u32,
    pub checkpoint_summary: Option<String>,
    pub stale_since_unix_ms: Option<u64>,
    pub completed_at_unix_ms: Option<u64>,
}

/// Versioned plan for swarm coordination (used by jcode-protocol)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionedPlan {
    pub version: u64,
    pub items: Vec<PlanStep>,
    pub task_progress: std::collections::HashMap<String, SwarmTaskProgress>,
    pub participants: std::collections::HashSet<String>,
}

impl Default for VersionedPlan {
    fn default() -> Self {
        Self::new()
    }
}

impl VersionedPlan {
    pub fn new() -> Self {
        Self {
            version: 0,
            items: Vec::new(),
            task_progress: std::collections::HashMap::new(),
            participants: std::collections::HashSet::new(),
        }
    }

    pub fn plan_definition(&self) -> serde_json::Value {
        serde_json::json!({
            "version": self.version,
            "item_count": self.items.len(),
            "participants_count": self.participants.len(),
        })
    }

    pub fn execution_state(&self) -> serde_json::Value {
        let active = self.items.iter().filter(|i| i.status == "executing").count();
        let completed = self.items.iter().filter(|i| i.status == "completed").count();
        let failed = self.items.iter().filter(|i| i.status == "failed").count();
        let pending = self.items.iter().filter(|i| i.status == "pending" || i.status == "approved").count();
        
        serde_json::json!({
            "active": active,
            "completed": completed,
            "failed": failed,
            "pending": pending,
            "total": self.items.len(),
        })
    }
}

/// Graph summary for plan visualization
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanGraphSummary {
    pub ready_ids: Vec<String>,
    pub blocked_ids: Vec<String>,
    pub active_ids: Vec<String>,
    pub completed_ids: Vec<String>,
    pub cycle_ids: Vec<String>,
    pub unresolved_dependency_ids: Vec<String>,
}

/// Summarize plan items into a graph status structure.
/// Compatible with jcode-protocol's PlanGraphStatus::from_versioned_plan.
pub fn summarize_plan_graph(items: &[PlanStep]) -> PlanGraphSummary {
    let mut ready = Vec::new();
    let mut blocked = Vec::new();
    let mut active = Vec::new();
    let mut completed = Vec::new();

    for item in items {
        match item.status.as_str() {
            "pending" | "approved" => {
                ready.push(item.id.to_string());
            }
            "rejected" | "failed" => {
                blocked.push(item.id.to_string());
            }
            "executing" => {
                active.push(item.id.to_string());
            }
            "completed" | "skipped" => {
                completed.push(item.id.to_string());
            }
            _ => {}
        }
    }

    PlanGraphSummary {
        ready_ids: ready,
        blocked_ids: blocked,
        active_ids: active,
        completed_ids: completed,
        cycle_ids: Vec::new(),
        unresolved_dependency_ids: Vec::new(),
    }
}

/// Get IDs of next runnable items, up to the given limit.
pub fn next_runnable_item_ids(items: &[PlanStep], limit: Option<usize>) -> Vec<String> {
    let runnable: Vec<String> = items
        .iter()
        .filter(|item| item.status == "pending" || item.status == "approved")
        .map(|item| item.id.to_string())
        .collect();

    match limit {
        Some(limit) => runnable.into_iter().take(limit).collect(),
        None => runnable,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskControlAction {
    Start,
    Wake,
    Resume,
    Retry,
    Reassign,
    Replace,
    Salvage,
}

impl TaskControlAction {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "start" | "s" => Some(TaskControlAction::Start),
            "wake" | "w" => Some(TaskControlAction::Wake),
            "resume" | "r" => Some(TaskControlAction::Resume),
            "retry" => Some(TaskControlAction::Retry),
            "reassign" => Some(TaskControlAction::Reassign),
            "replace" => Some(TaskControlAction::Replace),
            "salvage" => Some(TaskControlAction::Salvage),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            TaskControlAction::Start => "start",
            TaskControlAction::Wake => "wake",
            TaskControlAction::Resume => "resume",
            TaskControlAction::Retry => "retry",
            TaskControlAction::Reassign => "reassign",
            TaskControlAction::Replace => "replace",
            TaskControlAction::Salvage => "salvage",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssignmentAffinityResult {
    pub dependency_carryover: Vec<(String, f64)>,
    pub metadata_carryover: Vec<(String, f64)>,
    pub loads: std::collections::HashMap<String, u32>,
}

pub fn assignment_affinities_for_task(_plan: &VersionedPlan, _task_id: &str) -> Result<AssignmentAffinityResult, String> {
    Ok(AssignmentAffinityResult::default())
}

pub fn assignment_loads(_plan: &VersionedPlan) -> std::collections::HashMap<String, u32> {
    std::collections::HashMap::new()
}

pub fn build_control_assignment_text(content: &str, _message: Option<&str>) -> String {
    content.to_string()
}

pub fn combine_assignment_text(content: &str, message: Option<&str>) -> String {
    match message {
        Some(msg) if !msg.is_empty() => format!("{} | {}", content, msg),
        _ => content.to_string(),
    }
}

pub fn explicit_task_blocked_reason(plan: &VersionedPlan, task_id: &str) -> Option<String> {
    plan.items.iter()
        .find(|i| i.id == task_id)
        .and_then(|item| {
            match item.status.as_str() {
                "completed" | "failed" | "skipped" | "rejected" => Some(format!("Task is {}", item.status)),
                _ => None,
            }
        })
}

pub fn next_unassigned_runnable_item_id(plan: &VersionedPlan) -> Option<String> {
    plan.items.iter()
        .find(|i| (i.status == "pending" || i.status == "approved") && i.assigned_to.is_none())
        .map(|i| i.id.clone())
}

pub fn newly_ready_item_ids(before: &[PlanStep], after: &[PlanStep]) -> Vec<String> {
    let before_ids: std::collections::HashSet<_> = before.iter()
        .filter(|i| i.status == "pending" || i.status == "approved")
        .map(|i| i.id.clone())
        .collect();
    
    after.iter()
        .filter(|i| (i.status == "pending" || i.status == "approved") && !before_ids.contains(&i.id))
        .map(|i| i.id.clone())
        .collect()
}

pub fn task_control_action_allows_status(action: &TaskControlAction, status: &str) -> bool {
    match action {
        TaskControlAction::Start | TaskControlAction::Wake | TaskControlAction::Resume => {
            matches!(status, "queued" | "running_stale" | "paused")
        }
        TaskControlAction::Retry => {
            matches!(status, "failed" | "cancelled")
        }
        TaskControlAction::Reassign | TaskControlAction::Replace => true,
        TaskControlAction::Salvage => {
            matches!(status, "running_stale" | "failed" | "cancelled")
        }
    }
}

pub fn task_control_status_error(action: &TaskControlAction, status: &str, task_id: &str) -> String {
    format!(
        "Cannot {} task '{}' (current status: '{}')",
        action.as_str(),
        task_id,
        status
    )
}

pub fn task_control_target_item_id(items: &[PlanStep], target_session: &str, action: &TaskControlAction) -> Option<String> {
    match action {
        TaskControlAction::Reassign | TaskControlAction::Replace => {
            items.iter().find(|i| i.assigned_to.as_deref() == Some(target_session)).map(|i| i.id.clone())
        }
        _ => next_unassigned_runnable_item_id_internal(items),
    }
}

fn next_unassigned_runnable_item_id_internal(items: &[PlanStep]) -> Option<String> {
    items.iter()
        .find(|i| (i.status == "pending" || i.status == "approved") && i.assigned_to.is_none())
        .map(|i| i.id.clone())
}
