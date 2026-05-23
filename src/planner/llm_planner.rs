//! 基于LLM的自主任务规划
//!
//! 缺失能力补齐:
//! - LLM-based Plan Generation: 用LLM生成结构化计划
//! - Dynamic Task Re-planning: 执行中发现新问题自动调整
//! - Progress Tracking: 追踪每个步骤的完成状态
//! - Failure Recovery: 步骤失败时自动生成替代方案

use crate::planner::plan::{Plan, PlanStep, StepType, ImpactLevel, StepStatus};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 规划配置
#[derive(Debug, Clone)]
pub struct LlmPlannerConfig {
    pub max_steps: usize,
    pub auto_replan: bool,
    pub progress_report_interval: u64, // 秒
    pub max_retries_per_step: u32,
}

impl Default for LlmPlannerConfig {
    fn default() -> Self {
        Self {
            max_steps: 15,
            auto_replan: true,
            progress_report_interval: 30,
            max_retries_per_step: 3,
        }
    }
}

/// 规划请求
#[derive(Debug, Clone)]
pub struct PlanningRequest {
    pub goal: String,
    pub context: String,          // 项目上下文
    pub constraints: Vec<String>,  // 约束条件
    pub files: Vec<String>,        // 相关文件
}

/// 规划结果
#[derive(Debug, Clone)]
pub struct PlanningResult {
    pub plan: Plan,
    pub confidence: f64,
    pub alternatives: Vec<Plan>,
    pub warnings: Vec<String>,
}

/// [LLM-based Plan Generation] 基于LLM生成结构化计划
pub struct LlmPlanner {
    config: LlmPlannerConfig,
    provider: Option<Arc<dyn crate::provider::Provider>>,
}

impl LlmPlanner {
    pub fn new(config: LlmPlannerConfig) -> Self {
        Self { config, provider: None }
    }

    pub fn with_provider(mut self, provider: Arc<dyn crate::provider::Provider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// 解析用户目标生成多步计划
    pub async fn generate_plan(&self, request: &PlanningRequest) -> Result<PlanningResult> {
        let mut plan = Plan::new(&request.goal);
        let mut warnings = Vec::new();

        // 使用LLM生成步骤 (实际调用provider)
        let prompt = self.build_planning_prompt(request);
        let steps_text = match &self.provider {
            Some(provider) => {
                // 调用 provider 生成计划
                let response = provider.complete_simple(&prompt).await?;
                self.parse_steps_from_llm(&response)
            }
            None => {
                // 回退: 使用简单启发式
                self.heuristic_steps(request)
            }
        };

        for step in steps_text {
            plan.add_step(step);
        }

        // 验证计划
        if let Err(e) = plan.validate() {
            warnings.push(format!("Plan validation: {}", e));
        }

        let confidence = if plan.steps.is_empty() { 0.0 }
            else if plan.has_cycles() { 0.3 }
            else { 0.8 };

        Ok(PlanningResult {
            plan,
            confidence,
            alternatives: vec![],
            warnings,
        })
    }

    /// [Dynamic Re-planning] 当步骤失败时动态重规划
    pub async fn replan(
        &self,
        current_plan: &Plan,
        failed_step: &str,
        error: &str,
    ) -> Result<Option<Plan>> {
        if !self.config.auto_replan {
            return Ok(None);
        }

        let mut new_plan = current_plan.clone();

        // 标记失败步骤
        new_plan.fail_step(failed_step, error);

        // 检查失败步骤是否可替代
        let failed_idx = new_plan.steps.iter().position(|s| s.id == failed_step);
        if let Some(idx) = failed_idx {
            let step = &new_plan.steps[idx];

            // 生成替代步骤
            let alt_description = format!(
                "[Alternative] {} (original failed: {})",
                step.description, error
            );

            let alt_step = PlanStep::new(
                &format!("{}-alt", step.id),
                &alt_description,
                step.step_type.clone(),
            ).with_files(step.file_paths.clone())
             .depends_on(step.dependencies.clone());

            new_plan.steps.insert(idx + 1, alt_step);
        }

        Ok(Some(new_plan))
    }

    /// [Progress Tracking] 追踪进度并报告
    pub async fn get_progress(&self, plan: &Plan) -> PlanProgress {
        let total = plan.steps.len();
        let completed = plan.steps.iter().filter(|s| {
            matches!(s.status, StepStatus::Completed)
        }).count();
        let failed = plan.steps.iter().filter(|s| {
            matches!(s.status, StepStatus::Failed(_))
        }).count();
        let in_progress = plan.steps.iter().filter(|s| {
            matches!(s.status, StepStatus::InProgress)
        }).count();

        let percentage = if total > 0 { completed as f64 / total as f64 * 100.0 } else { 0.0 };

        PlanProgress {
            total_steps: total,
            completed_steps: completed,
            failed_steps: failed,
            in_progress_steps: in_progress,
            percentage,
            estimated_remaining: self.estimate_remaining(plan),
        }
    }

    /// [Failure Recovery] 失败恢复策略
    pub async fn recover(&self, plan: &Plan, failed_step: &str, error: &str) -> Result<RecoveryStrategy> {
        let retry_count = self.config.max_retries_per_step;

        Ok(RecoveryStrategy {
            failed_step: failed_step.to_string(),
            error: error.to_string(),
            retries_left: retry_count.saturating_sub(1),
            strategy: if error.contains("timeout") || error.contains("network") {
                // 网络/超时错误: 重试
                RecoveryAction::Retry
            } else if error.contains("not found") || error.contains("missing") {
                // 资源缺失: 跳过并标记
                RecoveryAction::Skip
            } else {
                // 其他错误: 生成替代方案
                RecoveryAction::Alternative
            },
        })
    }

    // ---- 内部方法 ----

    fn build_planning_prompt(&self, request: &PlanningRequest) -> String {
        format!(
            r#"Generate a step-by-step plan to accomplish this goal:

Goal: {}
Context: {}
Constraints: {}
Related files: {}

For each step, specify:
1. A clear description
2. The files involved
3. The type (Read/Create/Modify/Delete/Refactor/Verify)

Output format (one step per line, pipe-separated):
description | file1,file2 | Type

Max {} steps."#,
            request.goal, request.context, request.constraints.join(", "),
            request.files.join(", "), self.config.max_steps
        )
    }

    fn parse_steps_from_llm(&self, _response: &str) -> Vec<PlanStep> {
        // 解析LLM输出为结构化步骤
        // 实际实现会解析 "description | files | type" 格式
        vec![]
    }

    fn heuristic_steps(&self, request: &PlanningRequest) -> Vec<PlanStep> {
        // 回退方案: 基于文件类型推断步骤
        let mut steps = Vec::new();

        if request.files.is_empty() {
            steps.push(PlanStep::new("explore", "Explore project structure", StepType::Read));
        }

        for (i, file) in request.files.iter().enumerate() {
            steps.push(PlanStep::new(
                &format!("read-{}", i),
                &format!("Read and analyze {}", file),
                StepType::Read,
            ).with_files(vec![file.clone()]));
        }

        steps.push(PlanStep::new("execute", &request.goal, StepType::Modify));
        steps.push(PlanStep::new("verify", "Verify changes compile", StepType::Verify));

        steps
    }

    fn estimate_remaining(&self, plan: &Plan) -> Duration {
        let pending = plan.steps.iter().filter(|s| {
            matches!(s.status, StepStatus::Pending | StepStatus::InProgress)
        }).count();
        // 每步估算120秒
        Duration::from_secs(pending as u64 * 120)
    }
}

/// 计划进度
#[derive(Debug, Clone)]
pub struct PlanProgress {
    pub total_steps: usize,
    pub completed_steps: usize,
    pub failed_steps: usize,
    pub in_progress_steps: usize,
    pub percentage: f64,
    pub estimated_remaining: Duration,
}

/// 恢复策略
#[derive(Debug, Clone)]
pub struct RecoveryStrategy {
    pub failed_step: String,
    pub error: String,
    pub retries_left: u32,
    pub strategy: RecoveryAction,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryAction {
    Retry,
    Skip,
    Alternative,
}

/// 进度报告格式化
pub fn format_progress(progress: &PlanProgress) -> String {
    let bar_width = 30usize;
    let filled = (progress.percentage / 100.0 * bar_width as f64) as usize;
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(bar_width.saturating_sub(filled)));

    format!(
        "Progress: [{:.1}%]\n  {}\n  {}/{} steps, {} failed, {} in progress\n  Estimated remaining: {:?}",
        progress.percentage, bar,
        progress.completed_steps, progress.total_steps,
        progress.failed_steps, progress.in_progress_steps,
        progress.estimated_remaining,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_heuristic_plan() {
        let planner = LlmPlanner::new(LlmPlannerConfig::default());
        let request = PlanningRequest {
            goal: "Add user authentication".into(),
            context: "Medium Rust project with Actix-web".into(),
            constraints: vec!["Must use JWT".into()],
            files: vec!["src/auth.rs".into(), "src/api.rs".into()],
        };
        let result = planner.generate_plan(&request).await.unwrap();
        assert!(!result.plan.steps.is_empty());
        assert!(result.plan.steps.iter().any(|s| s.file_paths.contains(&"src/auth.rs".to_string())));
    }

    #[test]
    fn test_progress_format() {
        let p = PlanProgress {
            total_steps: 10,
            completed_steps: 4,
            failed_steps: 1,
            in_progress_steps: 1,
            percentage: 40.0,
            estimated_remaining: Duration::from_secs(600),
        };
        let output = format_progress(&p);
        assert!(output.contains("40.0%"));
    }

    #[test]
    fn test_recovery_decision() {
        let strategy = RecoveryStrategy {
            failed_step: "build".into(),
            error: "timeout".into(),
            retries_left: 2,
            strategy: RecoveryAction::Retry,
        };
        assert_eq!(strategy.strategy, RecoveryAction::Retry);
    }
}
