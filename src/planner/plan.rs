//! 跨文件计划数据结构与执行逻辑
//!
//! 对标 Claude Code 的 Plan Mode，提供：
//! - 结构化计划步骤
//! - 依赖图拓扑排序
//! - 变更影响评估
//! - 安全验证

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

/// 计划步骤类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StepType {
    /// 读取/分析文件
    Read,
    /// 创建新文件
    Create,
    /// 修改现有文件
    Modify,
    /// 删除文件
    Delete,
    /// 重构 (rename, extract, etc.)
    Refactor,
    /// 验证 (lint, test, build)
    Verify,
}

/// 步骤状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
    Failed(String),
    Skipped,
}

/// 影响等级
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd)]
pub enum ImpactLevel {
    Low,       // 单文件，无依赖
    Medium,    // 多文件，无循环依赖
    High,      // 多文件含交叉依赖
    Critical,  // Breaking changes, API 变更
}

/// 单个计划步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub description: String,
    pub file_paths: Vec<String>,
    pub step_type: StepType,
    pub dependencies: Vec<String>,
    pub status: StepStatus,
    pub estimated_impact: ImpactLevel,
}

impl PlanStep {
    pub fn new(id: &str, description: &str, step_type: StepType) -> Self {
        Self {
            id: id.to_string(),
            description: description.to_string(),
            file_paths: Vec::new(),
            step_type,
            dependencies: Vec::new(),
            status: StepStatus::Pending,
            estimated_impact: ImpactLevel::Low,
        }
    }

    pub fn with_files(mut self, files: Vec<String>) -> Self {
        self.file_paths = files;
        self
    }

    pub fn depends_on(mut self, deps: Vec<String>) -> Self {
        self.dependencies = deps;
        self
    }
}

/// 完整跨文件计划
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub goal: String,
    pub steps: Vec<PlanStep>,
    pub affected_files: Vec<String>,
    pub dependency_graph: HashMap<String, Vec<String>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub estimated_risk: ImpactLevel,
}

impl Plan {
    pub fn new(goal: &str) -> Self {
        Self {
            goal: goal.to_string(),
            steps: Vec::new(),
            affected_files: Vec::new(),
            dependency_graph: HashMap::new(),
            created_at: chrono::Utc::now(),
            estimated_risk: ImpactLevel::Low,
        }
    }

    /// 添加步骤
    pub fn add_step(&mut self, step: PlanStep) {
        // 合并文件列表
        for f in &step.file_paths {
            if !self.affected_files.contains(f) {
                self.affected_files.push(f.clone());
            }
        }
        self.steps.push(step);
        self.estimated_risk = self.calculate_risk();
    }

    /// 验证计划一致性和依赖可解析性
    pub fn validate(&self) -> Result<Vec<String>> {
        let mut warnings = Vec::new();
        let step_ids: HashSet<&str> = self.steps.iter().map(|s| s.id.as_str()).collect();

        for step in &self.steps {
            for dep in &step.dependencies {
                if !step_ids.contains(dep.as_str()) {
                    warnings.push(format!(
                        "Step '{}' depends on '{}' which does not exist",
                        step.id, dep
                    ));
                }
            }
        }

        if self.has_cycles() {
            warnings.push("Plan contains circular dependencies".to_string());
        }

        Ok(warnings)
    }

    /// 检测循环依赖 (DFS)
    pub fn has_cycles(&self) -> bool {
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();

        fn dfs(
            step_id: &str,
            steps: &[PlanStep],
            visited: &mut HashSet<String>,
            rec_stack: &mut HashSet<String>,
        ) -> bool {
            visited.insert(step_id.to_string());
            rec_stack.insert(step_id.to_string());

            if let Some(step) = steps.iter().find(|s| s.id == step_id) {
                for dep in &step.dependencies {
                    if !visited.contains(dep) {
                        if dfs(dep, steps, visited, rec_stack) {
                            return true;
                        }
                    } else if rec_stack.contains(dep) {
                        return true;
                    }
                }
            }

            rec_stack.remove(step_id);
            false
        }

        for step in &self.steps {
            if !visited.contains(&step.id) {
                if dfs(&step.id, &self.steps, &mut visited, &mut rec_stack) {
                    return true;
                }
            }
        }
        false
    }

    /// 获取准备好执行的步骤（所有依赖已满足）
    pub fn ready_steps(&self) -> Vec<&PlanStep> {
        let completed: HashSet<&str> = self
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::Completed || s.status == StepStatus::Skipped)
            .map(|s| s.id.as_str())
            .collect();

        self.steps
            .iter()
            .filter(|s| {
                s.status == StepStatus::Pending
                    && s.dependencies.iter().all(|d| completed.contains(d.as_str()))
            })
            .collect()
    }

    /// 获取拓扑排序执行顺序
    pub fn execution_order(&self) -> Result<Vec<usize>> {
        if self.has_cycles() {
            anyhow::bail!("Cannot determine execution order: plan contains circular dependencies");
        }

        let mut in_degree: HashMap<usize, usize> = HashMap::new();
        let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();

        for (i, step) in self.steps.iter().enumerate() {
            in_degree.entry(i).or_insert(0);
            for dep in &step.dependencies {
                if let Some(j) = self.steps.iter().position(|s| s.id == *dep) {
                    adj.entry(j).or_default().push(i);
                    *in_degree.entry(i).or_insert(0) += 1;
                }
            }
        }

        // Kahn's algorithm
        let mut queue: VecDeque<usize> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(i, _)| *i)
            .collect();

        let mut order = Vec::new();
        while let Some(node) = queue.pop_front() {
            order.push(node);
            if let Some(neighbors) = adj.get(&node) {
                for &next in neighbors {
                    if let Some(deg) = in_degree.get_mut(&next) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(next);
                        }
                    }
                }
            }
        }

        if order.len() != self.steps.len() {
            anyhow::bail!("Could not resolve all dependencies in plan");
        }

        Ok(order)
    }

    /// 计算风险等级
    fn calculate_risk(&self) -> ImpactLevel {
        let max_impact = self
            .steps
            .iter()
            .map(|s| &s.estimated_impact)
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        match max_impact {
            Some(ImpactLevel::Critical) => ImpactLevel::Critical,
            Some(ImpactLevel::High) => ImpactLevel::High,
            Some(ImpactLevel::Medium) => ImpactLevel::Medium,
            _ => {
                if self.affected_files.len() > 5 {
                    ImpactLevel::Medium
                } else if self.affected_files.len() > 1 {
                    ImpactLevel::Low
                } else {
                    ImpactLevel::Low
                }
            }
        }
    }

    /// 标记步骤完成
    pub fn complete_step(&mut self, step_id: &str) {
        if let Some(step) = self.steps.iter_mut().find(|s| s.id == step_id) {
            step.status = StepStatus::Completed;
        }
    }

    /// 标记步骤失败
    pub fn fail_step(&mut self, step_id: &str, error: &str) {
        if let Some(step) = self.steps.iter_mut().find(|s| s.id == step_id) {
            step.status = StepStatus::Failed(error.to_string());
        }
    }

    /// 渲染为 Markdown
    pub fn to_markdown(&self) -> String {
        let mut md = format!("# Plan: {}\n\n", self.goal);
        md.push_str(&format!("**Created**: {}\n", self.created_at.format("%Y-%m-%d %H:%M:%S UTC")));
        md.push_str(&format!("**Affected files**: {}\n", self.affected_files.len()));
        md.push_str(&format!("**Estimated risk**: {:?}\n\n", self.estimated_risk));
        md.push_str("## Steps\n\n");

        for (i, step) in self.steps.iter().enumerate() {
            let status_icon = match step.status {
                StepStatus::Pending => "⏳",
                StepStatus::InProgress => "🔄",
                StepStatus::Completed => "✅",
                StepStatus::Failed(_) => "❌",
                StepStatus::Skipped => "⏭️",
            };
            md.push_str(&format!("### {}. {} {}\n", i + 1, status_icon, step.description));
            md.push_str(&format!("- **Type**: {:?}\n", step.step_type));
            md.push_str(&format!("- **Files**: {}\n", step.file_paths.join(", ")));
            if !step.dependencies.is_empty() {
                md.push_str(&format!("- **Depends on**: {}\n", step.dependencies.join(", ")));
            }
            md.push_str(&format!("- **Impact**: {:?}\n", step.estimated_impact));
            md.push('\n');
        }

        md
    }
}

/// 计划验证器
pub struct PlanValidator;

impl PlanValidator {
    /// 验证计划一致性和安全性
    pub fn validate(plan: &Plan, _workspace_root: &std::path::Path) -> Result<Vec<String>> {
        let mut warnings = plan.validate()?;

        // 检查步骤是否有描述
        for step in &plan.steps {
            if step.description.is_empty() {
                warnings.push(format!("Step '{}' has no description", step.id));
            }
            if step.file_paths.is_empty() {
                warnings.push(format!("Step '{}' has no files specified", step.id));
            }
        }

        Ok(warnings)
    }

    /// 检查引用的文件是否存在 (仅限 modify/delete)
    pub fn check_file_existence(plan: &Plan, workspace_root: &std::path::Path) -> Vec<String> {
        let mut warnings = Vec::new();
        for step in &plan.steps {
            match step.step_type {
                StepType::Modify | StepType::Delete | StepType::Refactor => {
                    for f in &step.file_paths {
                        let full_path = workspace_root.join(f);
                        if !full_path.exists() {
                            warnings.push(format!(
                                "Step '{}' references non-existent file: {}",
                                step.id, f
                            ));
                        }
                    }
                }
                StepType::Create => {
                    for f in &step.file_paths {
                        let full_path = workspace_root.join(f);
                        if full_path.exists() {
                            warnings.push(format!(
                                "Step '{}' creates already-existing file: {}",
                                step.id, f
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        warnings
    }
}

/// 步骤执行结果
#[derive(Debug, Clone)]
pub struct PlanExecutionResult {
    pub step_id: String,
    pub success: bool,
    pub diff: Option<String>,
    pub error: Option<String>,
    pub affected_files: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_creation() {
        let mut plan = Plan::new("Add user authentication");
        plan.add_step(PlanStep::new("1", "Add login route", StepType::Create)
            .with_files(vec!["src/routes/login.rs".to_string()]));
        plan.add_step(PlanStep::new("2", "Add auth middleware", StepType::Create)
            .with_files(vec!["src/middleware/auth.rs".to_string()])
            .depends_on(vec!["1".to_string()]));

        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.affected_files.len(), 2);
    }

    #[test]
    fn test_topological_sort() {
        let mut plan = Plan::new("Test sort");
        plan.add_step(PlanStep::new("a", "A", StepType::Create));
        plan.add_step(PlanStep::new("b", "B", StepType::Modify).depends_on(vec!["a".to_string()]));
        plan.add_step(PlanStep::new("c", "C", StepType::Modify).depends_on(vec!["b".to_string()]));

        let order = plan.execution_order().unwrap();
        let ids: Vec<&str> = order.iter().map(|&i| plan.steps[i].id.as_str()).collect();
        // a must come before b, b must come before c
        assert!(ids.iter().position(|&x| x == "a").unwrap() < ids.iter().position(|&x| x == "b").unwrap());
        assert!(ids.iter().position(|&x| x == "b").unwrap() < ids.iter().position(|&x| x == "c").unwrap());
    }

    #[test]
    fn test_cycle_detection() {
        let mut plan = Plan::new("Test cycles");
        plan.add_step(PlanStep::new("a", "A", StepType::Create).depends_on(vec!["b".to_string()]));
        plan.add_step(PlanStep::new("b", "B", StepType::Modify).depends_on(vec!["a".to_string()]));
        assert!(plan.has_cycles());
    }

    #[test]
    fn test_ready_steps() {
        let mut plan = Plan::new("Test ready");
        plan.add_step(PlanStep::new("a", "A", StepType::Create));
        plan.add_step(PlanStep::new("b", "B", StepType::Modify).depends_on(vec!["a".to_string()]));

        let ready = plan.ready_steps();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "a");

        plan.complete_step("a");
        let ready = plan.ready_steps();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "b");
    }
}
