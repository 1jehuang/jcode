use std::collections::HashMap;
use chrono::{DateTime, Utc};

/// Enhanced task with full metadata
#[derive(Debug, Clone)]
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

#[derive(Debug, Clone, PartialEq)]
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
            TaskStatus::Backlog => "backlog",
            TaskStatus::Ready => "ready",
            TaskStatus::InProgress => "in-progress",
            TaskStatus::Blocked => "blocked",
            TaskStatus::InReview => "in-review",
            TaskStatus::Completed => "completed",
            TaskStatus::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
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
            TaskPriority::Critical => "critical",
            TaskPriority::High => "high",
            TaskPriority::Medium => "medium",
            TaskPriority::Low => "low",
            TaskPriority::None => "none",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
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
            TaskCategory::Feature => "feature",
            TaskCategory::Bug => "bug",
            TaskCategory::Refactor => "refactor",
            TaskCategory::Test => "test",
            TaskCategory::Documentation => "docs",
            TaskCategory::Build => "build",
            TaskCategory::Deployment => "deploy",
            TaskCategory::Research => "research",
            TaskCategory::Other => "other",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskNote {
    pub author: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

/// A plan containing multiple tasks
#[derive(Debug, Clone)]
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

#[derive(Debug, Clone, PartialEq)]
pub enum PlanStatus {
    Draft,
    Active,
    Completed,
    Abandoned,
}

#[derive(Debug, Clone)]
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
        TaskPlanner {
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
}

impl Default for TaskPlanner {
    fn default() -> Self {
        Self::new()
    }
}