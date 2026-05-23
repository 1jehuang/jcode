//! 规划器 ↔ 调度器 集成
//!
//! [I-09] 将跨文件规划转换为可调度任务，由 UnifiedScheduler 执行。
//!
//! 工作流:
//!   Plan → decompose_to_tasks() → scheduler.submit_plan()
//!   → 调度器按依赖顺序并行执行 → 收集结果 → 验证

use crate::planner::plan::{Plan, PlanStep, StepType, StepStatus};
use crate::scheduler::{Task, TaskPriority, ResourceRequirements, AgentRole};

/// 将 Plan 分解为调度器可执行的任务列表
pub async fn decompose_plan_to_tasks(plan: &Plan, session_id: &str) -> Vec<ScheduledTask> {
    let mut tasks = Vec::new();

    for (i, step) in plan.steps.iter().enumerate() {
        let task = ScheduledTask {
            id: format!("plan-{}-{}", session_id, i),
            plan_step_id: step.id.clone(),
            description: step.description.clone(),
            priority: map_impact_to_priority(&step.estimated_impact),
            file_paths: step.file_paths.clone(),
            dependencies: step.dependencies.clone(),
            step_type: step.step_type.clone(),
            session_id: session_id.to_string(),
        };
        tasks.push(task);
    }

    tasks
}

/// 将 PlanStep 转换为调度器的 Task
fn plan_step_to_task(step: &PlanStep, session_id: &str, index: usize) -> Task {
    let priority = match step.estimated_impact {
        crate::planner::plan::ImpactLevel::Critical => TaskPriority::Urgent,
        crate::planner::plan::ImpactLevel::High => TaskPriority::High,
        crate::planner::plan::ImpactLevel::Medium => TaskPriority::Medium,
        crate::planner::plan::ImpactLevel::Low => TaskPriority::Low,
    };

    let role = match step.step_type {
        StepType::Verify => AgentRole::Specialist("verifier".to_string()),
        StepType::Refactor => AgentRole::Specialist("refactorer".to_string()),
        _ => AgentRole::Worker,
    };

    let resources = ResourceRequirements {
        cpu: 1,
        gpu: 0,
        memory: 256,
        network: 0,
    };

    Task {
        id: uuid::Uuid::new_v4(),
        description: format!("[Plan {}] {}", index + 1, step.description),
        priority,
        dependencies: vec![],
        metadata: serde_json::json!({
            "plan_step_id": step.id,
            "file_paths": step.file_paths,
            "step_type": format!("{:?}", step.step_type),
            "session_id": session_id,
        }),
    }
}

/// 从调度器结果收集执行状态
pub fn collect_execution_results(
    step_id: &str,
    task: &crate::scheduler::TaskExecutionResult,
) -> PlanStepStatus {
    match task.status {
        crate::scheduler::TaskStatus::Completed => PlanStepStatus::Completed,
        crate::scheduler::TaskStatus::Failed => {
            PlanStepStatus::Failed(task.result.clone())
        }
        _ => PlanStepStatus::Pending,
    }
}

/// 将影响等级映射为调度器优先级
fn map_impact_to_priority(impact: &crate::planner::plan::ImpactLevel) -> TaskPriority {
    match impact {
        crate::planner::plan::ImpactLevel::Critical => TaskPriority::Urgent,
        crate::planner::plan::ImpactLevel::High => TaskPriority::High,
        crate::planner::plan::ImpactLevel::Medium => TaskPriority::Medium,
        crate::planner::plan::ImpactLevel::Low => TaskPriority::Low,
    }
}

/// 可调度任务 (Plan → Scheduler 中间格式)
#[derive(Debug, Clone)]
pub struct ScheduledTask {
    pub id: String,
    pub plan_step_id: String,
    pub description: String,
    pub priority: TaskPriority,
    pub file_paths: Vec<String>,
    pub dependencies: Vec<String>,
    pub step_type: StepType,
    pub session_id: String,
}

// 用于收集结果的枚举
#[derive(Debug, Clone)]
pub enum PlanStepStatus {
    Pending,
    Completed,
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::plan::{Plan, PlanStep, StepType};

    #[tokio::test]
    async fn test_decompose_empty_plan() {
        let plan = Plan::new("test");
        let tasks = decompose_plan_to_tasks(&plan, "session-1").await;
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn test_decompose_single_step() {
        let mut plan = Plan::new("test");
        plan.add_step(PlanStep::new("1", "Create file", StepType::Create)
            .with_files(vec!["src/test.rs".to_string()]));

        let tasks = decompose_plan_to_tasks(&plan, "session-1").await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].file_paths, vec!["src/test.rs"]);
    }

    #[tokio::test]
    async fn test_priority_mapping() {
        let plan = Plan::new("test");
        assert_eq!(
            map_impact_to_priority(&crate::planner::plan::ImpactLevel::Critical),
            TaskPriority::Urgent
        );
        assert_eq!(
            map_impact_to_priority(&crate::planner::plan::ImpactLevel::Low),
            TaskPriority::Low
        );
    }
}
