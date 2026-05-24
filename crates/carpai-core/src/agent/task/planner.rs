//! Task Planner - Generates structured plans for complex goals
//!
//! Provides complete task lifecycle management with:
//! - Enhanced tasks with full metadata (priority, category, dependencies)
//! - Task plans containing multiple related tasks
//! - Workflow generation and visualization
//! - Status tracking and milestone management

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Enhanced task with full metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedTask {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub category: TaskCategory,
    pub dependencies: Vec<String>,
    pub subtasks: Vec<String>,
    pub assigned_to: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub due_date: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
    pub estimated_hours: Option<f64>,
    pub actual_hours: Option<f64>,
    pub notes: Vec<TaskNote>,
    pub plan_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TaskStatus {
    Backlog,
    Ready,
    InProgress,
    Blocked,
    InReview,
    Completed,
    Cancelled,
}

impl TaskStatus {
    pub fn label(&self) -> &str {
        match self {
            Self::Backlog => "backlog",
            Self::Ready => "ready",
            Self::InProgress => "in-progress",
            Self::Blocked => "blocked",
            Self::InReview => "in-review",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TaskPriority {
    Critical,
    High,
    Medium,
    Low,
    None,
}

impl TaskPriority {
    pub fn label(&self) -> &str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TaskCategory {
    Feature,
    Bug,
    Refactor,
    Test,
    Documentation,
    Build,
    Deployment,
    Research,
    Other,
}

impl TaskCategory {
    pub fn label(&self) -> &str {
        match self {
            Self::Feature => "feature",
            Self::Bug => "bug",
            Self::Refactor => "refactor",
            Self::Test => "test",
            Self::Documentation => "docs",
            Self::Build => "build",
            Self::Deployment => "deploy",
            Self::Research => "research",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNote {
    pub author: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

/// A plan containing multiple tasks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlan {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tasks: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: PlanStatus,
    pub goal: String,
    pub constraints: Vec<String>,
    pub milestones: Vec<Milestone>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlanStatus {
    Draft,
    Active,
    Completed,
    Abandoned,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Milestone {
    pub name: String,
    pub description: String,
    pub due_date: Option<DateTime<Utc>>,
    pub tasks: Vec<String>,
    pub completed: bool,
}

/// Task planner that creates execution plans from high-level goals
pub struct TaskPlanner {
    plans: HashMap<String, TaskPlan>,
    tasks: HashMap<String, EnhancedTask>,
}

impl TaskPlanner {
    pub fn new() -> Self {
        Self {
            plans: HashMap::new(),
            tasks: HashMap::new(),
        }
    }

    pub fn create_plan(&mut self, name: &str, description: &str, goal: &str) -> String {
        let id = format!("plan-{}", Utc::now().timestamp());
        let plan = TaskPlan {
            id: id.clone(),
            name: name.to_string(),
            description: description.to_string(),
            tasks: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            status: PlanStatus::Draft,
            goal: goal.to_string(),
            constraints: vec![],
            milestones: vec![],
        };
        self.plans.insert(id.clone(), plan);
        id
    }

    pub fn add_task(&mut self, plan_id: &str, task: EnhancedTask) -> Result<(), String> {
        let plan = self.plans.get_mut(plan_id)
            .ok_or_else(|| format!("Plan '{}' not found", plan_id))?;
        let task_id = task.id.clone();
        plan.tasks.push(task_id.clone());
        plan.updated_at = Utc::now();
        self.tasks.insert(task_id, task);
        Ok(())
    }

    pub fn get_plan(&self, id: &str) -> Option<&TaskPlan> {
        self.plans.get(id)
    }

    pub fn get_task(&self, id: &str) -> Option<&EnhancedTask> {
        self.tasks.get(id)
    }

    pub fn list_plans(&self) -> Vec<&TaskPlan> {
        self.plans.values().collect()
    }

    pub fn list_tasks(&self, plan_id: &str) -> Vec<&EnhancedTask> {
        self.plans.get(plan_id)
            .map(|plan| {
                plan.tasks.iter()
                    .filter_map(|tid| self.tasks.get(tid))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn list_all_tasks(&self) -> Vec<&EnhancedTask> {
        self.tasks.values().collect()
    }

    pub fn update_task_status(&mut self, task_id: &str, status: TaskStatus) -> Result<(), String> {
        let task = self.tasks.get_mut(task_id)
            .ok_or_else(|| format!("Task '{}' not found", task_id))?;
        task.status = status;
        task.updated_at = Utc::now();
        Ok(())
    }

    pub fn generate_workflow(&self, plan_id: &str) -> Option<Vec<String>> {
        let plan = self.plans.get(plan_id)?;
        let tasks: Vec<&EnhancedTask> = plan.tasks.iter()
            .filter_map(|tid| self.tasks.get(tid))
            .collect();

        let mut workflow = vec![];
        workflow.push(format!("Plan: {}", plan.name));
        workflow.push(format!("Goal: {}", plan.goal));
        workflow.push(String::new());

        for task in &tasks {
            workflow.push(format!("  [{:?}] {} - {}", task.priority, task.title, task.status.label()));
            if !task.dependencies.is_empty() {
                workflow.push(format!("       depends on: {}", task.dependencies.join(", ")));
            }
        }

        Some(workflow)
    }

    /// Find the plan ID that contains a given task
    pub fn find_plan_for_task(&self, task_id: &str) -> Option<String> {
        for (plan_id, plan) in &self.plans {
            if plan.tasks.contains(&task_id.to_string()) {
                return Some(plan_id.clone());
            }
        }
        None
    }
}

impl EnhancedTask {
    /// Create a new task with a description
    pub fn new(description: &str) -> Self {
        let now = Utc::now();
        let id = format!("task-{}", now.timestamp());
        Self {
            id,
            title: description.to_string(),
            description: description.to_string(),
            status: TaskStatus::Backlog,
            priority: TaskPriority::Medium,
            category: TaskCategory::Other,
            dependencies: vec![],
            subtasks: vec![],
            assigned_to: None,
            created_at: now,
            updated_at: now,
            due_date: None,
            tags: vec![],
            estimated_hours: None,
            actual_hours: None,
            notes: vec![],
            plan_id: None,
        }
    }
}

impl Default for TaskPlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_plan_and_task() {
        let mut planner = TaskPlanner::new();
        let plan_id = planner.create_plan("Test Plan", "A test plan", "Achieve something");
        
        let task = EnhancedTask::new("Do something important");
        planner.add_task(&plan_id, task).unwrap();
        
        assert_eq!(planner.list_plans().len(), 1);
        assert_eq!(planner.list_tasks(&plan_id).len(), 1);
    }

    #[test]
    fn test_generate_workflow() {
        let mut planner = TaskPlanner::new();
        let plan_id = planner.create_plan("Workflow Test", "Test workflow generation", "Goal");
        
        let task1 = EnhancedTask::new("First task");
        let mut task2 = EnhancedTask::new("Second task");
        task2.dependencies.push(task1.id.clone());
        
        planner.add_task(&plan_id, task1).unwrap();
        planner.add_task(&plan_id, task2).unwrap();
        
        let workflow = planner.generate_workflow(&plan_id);
        assert!(workflow.is_some());
        let wf = workflow.unwrap();
        assert!(wf.len() >= 3);
    }
}
