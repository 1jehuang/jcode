//! # Ultraplan — 高级规划模式（借鉴 Claude Code ultraplan 65KB 实现）
//!
//! 比普通 plan_mode 更深度的规划系统。提供：
//! - 多维度影响分析（代码库、API、性能、安全）
//! - 任务分解 + 依赖图
//! - 工作量评估（Story Points）
//! - 实施计划（分步骤 + 检查点）
//! - 约束检测（前置条件、后置条件）

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// 规划阶段
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PlanPhase {
    /// 需求分析
    Analysis,
    /// 架构设计
    Architecture,
    /// 任务分解
    Decomposition,
    /// 风险识别
    RiskAssessment,
    /// 实施规划
    Implementation,
    /// 验证策略
    Verification,
}

/// 任务节点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNode {
    pub id: String,
    pub title: String,
    pub description: String,
    pub dependencies: Vec<String>,
    pub estimated_minutes: u32,
    pub risk: RiskLevel,
    pub status: TaskStatus,
    /// 影响的文件列表
    pub affected_files: Vec<String>,
}

/// 任务状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Blocked(String),
    Completed,
    Skipped,
}

/// 风险级别
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// 影响分析结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactAnalysis {
    pub files_to_modify: Vec<String>,
    pub files_to_create: Vec<String>,
    pub api_changes: Vec<String>,
    pub breaking_changes: Vec<String>,
    pub performance_impact: Option<String>,
    pub security_concerns: Vec<String>,
    pub test_files_needed: Vec<String>,
}

/// 实施计划
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplementationPlan {
    pub title: String,
    pub summary: String,
    pub analysis: ImpactAnalysis,
    pub tasks: Vec<TaskNode>,
    pub total_estimated_minutes: u32,
    pub phases: Vec<PlanPhase>,
    pub checkpoints: Vec<Checkpoint>,
    pub rollback_strategy: Option<String>,
}

/// 检查点（用于验证实施进度）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub description: String,
    pub verification_steps: Vec<String>,
}

/// Ultraplan 引擎
pub struct Ultraplan;

impl Ultraplan {
    /// 创建新的实施计划
    pub fn plan(title: &str, description: &str) -> ImplementationPlan {
        ImplementationPlan {
            title: title.to_string(),
            summary: description.to_string(),
            analysis: ImpactAnalysis::default(),
            tasks: Vec::new(),
            total_estimated_minutes: 0,
            phases: vec![
                PlanPhase::Analysis,
                PlanPhase::Architecture,
                PlanPhase::Decomposition,
                PlanPhase::RiskAssessment,
                PlanPhase::Implementation,
                PlanPhase::Verification,
            ],
            checkpoints: Vec::new(),
            rollback_strategy: None,
        }
    }

    /// 添加任务到计划
    pub fn add_task(
        plan: &mut ImplementationPlan,
        id: &str,
        title: &str,
        description: &str,
        deps: Vec<String>,
        minutes: u32,
        risk: RiskLevel,
        files: Vec<String>,
    ) {
        plan.tasks.push(TaskNode {
            id: id.to_string(),
            title: title.to_string(),
            description: description.to_string(),
            dependencies: deps,
            estimated_minutes: minutes,
            risk,
            status: TaskStatus::Pending,
            affected_files: files,
        });
        plan.total_estimated_minutes += minutes;
    }

    /// 添加检查点
    pub fn add_checkpoint(plan: &mut ImplementationPlan, id: &str, description: &str, steps: Vec<String>) {
        plan.checkpoints.push(Checkpoint {
            id: id.to_string(),
            description: description.to_string(),
            verification_steps: steps,
        });
    }

    /// 设置回滚策略
    pub fn set_rollback(plan: &mut ImplementationPlan, strategy: &str) {
        plan.rollback_strategy = Some(strategy.to_string());
    }

    /// 生成计划的 HTML/Markdown 报告
    pub fn format_plan(plan: &ImplementationPlan) -> String {
        let mut output = String::new();
        output.push_str(&format!("# 📋 {}\n\n", plan.title));
        output.push_str(&format!("{}\n\n", plan.summary));
        output.push_str(&format!("**预计总工时**: {} 分钟 ({:.1} 人小时)\n\n", 
            plan.total_estimated_minutes,
            plan.total_estimated_minutes as f64 / 60.0));

        // 任务列表
        output.push_str("## 任务分解\n\n");
        for task in &plan.tasks {
            let status_icon = match task.status {
                TaskStatus::Pending => "⬜",
                TaskStatus::InProgress => "🔄",
                TaskStatus::Blocked(_) => "🚫",
                TaskStatus::Completed => "✅",
                TaskStatus::Skipped => "⏭️",
            };
            let risk_icon = match task.risk {
                RiskLevel::Low => "🟢",
                RiskLevel::Medium => "🟡",
                RiskLevel::High => "🟠",
                RiskLevel::Critical => "🔴",
            };
            output.push_str(&format!("{} **{}** — {} {} ({}m)\n", 
                status_icon, task.title, risk_icon, task.description, task.estimated_minutes));

            if !task.dependencies.is_empty() {
                output.push_str(&format!("  依赖: {}\n", task.dependencies.join(", ")));
            }
            if !task.affected_files.is_empty() {
                output.push_str(&format!("  文件: {}\n", task.affected_files.join(", ")));
            }
            output.push('\n');
        }

        // 检查点
        if !plan.checkpoints.is_empty() {
            output.push_str("## 检查点\n\n");
            for cp in &plan.checkpoints {
                output.push_str(&format!("### ✅ {}\n", cp.description));
                for step in &cp.verification_steps {
                    output.push_str(&format!("- [ ] {}\n", step));
                }
                output.push('\n');
            }
        }

        // 回滚策略
        if let Some(ref rollback) = plan.rollback_strategy {
            output.push_str("## 回滚策略\n\n");
            output.push_str(rollback);
            output.push('\n');
        }

        output
    }

    /// 生成 JSON 格式计划供 LLM 消费
    pub fn to_json(plan: &ImplementationPlan) -> String {
        serde_json::to_string_pretty(plan).unwrap_or_default()
    }
}

impl Default for ImpactAnalysis {
    fn default() -> Self {
        Self {
            files_to_modify: Vec::new(),
            files_to_create: Vec::new(),
            api_changes: Vec::new(),
            breaking_changes: Vec::new(),
            performance_impact: None,
            security_concerns: Vec::new(),
            test_files_needed: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_creation() {
        let mut plan = Ultraplan::plan("重构认证模块", "将现有的 OAuth 认证重构为 JWT 方式");
        Ultraplan::add_task(&mut plan, "T1", "设计 JWT 结构",
            "定义 JWT payload 字段和过期策略", vec![],
            30, RiskLevel::Low, vec!["src/auth/jwt.rs".to_string()]);
        Ultraplan::add_task(&mut plan, "T2", "实现 Token 签发",
            "编写 JWT 签发和验证逻辑", vec!["T1".to_string()],
            60, RiskLevel::Medium, vec!["src/auth/token.rs".to_string()]);
        Ultraplan::add_checkpoint(&mut plan, "C1", "认证模块功能完整",
            vec!["JWT 签发测试通过".to_string(), "JWT 验证测试通过".to_string(), "过期 Token 处理".to_string()]);
        Ultraplan::set_rollback(&mut plan, "恢复所有修改的文件并切换回 OAuth 实现");

        let report = Ultraplan::format_plan(&plan);
        assert!(report.contains("重构认证模块"));
        assert!(report.contains("T1"));
        assert!(report.contains("T2"));
        assert!(report.contains("回滚策略"));
    }

    #[test]
    fn test_json_output() {
        let plan = Ultraplan::plan("测试", "测试计划");
        let json = Ultraplan::to_json(&plan);
        assert!(json.contains("title"));
        assert!(json.contains("summary"));
    }
}
