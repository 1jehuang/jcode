//! UltraPlan - Advanced hierarchical planning with multi-level decomposition
//!
//! Provides:
//! - Hierarchical goal decomposition (Goal -> Milestone -> Task -> Subtask)
//! - Multi-level planning with different granularity
//! - Adaptive replanning based on execution feedback
//! - Cross-plan dependency management

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level goal with strategic objectives
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UltraGoal {
    pub id: String,
    pub title: String,
    pub description: String,
    pub success_criteria: Vec<String>,
    pub constraints: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub status: GoalStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GoalStatus {
    Proposed,
    Active,
    Completed,
    Abandoned,
}

/// Milestone representing a major checkpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UltraMilestone {
    pub id: String,
    pub goal_id: String,
    pub title: String,
    pub description: String,
    pub tasks: Vec<String>,
    pub due_date: Option<DateTime<Utc>>,
    pub completed: bool,
    pub completion_criteria: Vec<String>,
}

/// Hierarchical task with parent-child relationships
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UltraTask {
    pub id: String,
    pub milestone_id: String,
    pub parent_task: Option<String>,
    pub children: Vec<String>,
    pub title: String,
    pub description: String,
    pub priority: u8, // 0-10, higher = more important
    pub estimated_hours: f64,
    pub actual_hours: Option<f64>,
    pub status: UltraTaskStatus,
    pub dependencies: Vec<String>,
    pub artifacts: Vec<String>, // Output files/results
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UltraTaskStatus {
    NotStarted,
    InProgress,
    Blocked,
    Review,
    Completed,
    Cancelled,
}

/// Complete ultra plan with hierarchical structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UltraPlan {
    pub id: String,
    pub goal: UltraGoal,
    pub milestones: Vec<UltraMilestone>,
    pub tasks: HashMap<String, UltraTask>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: u32,
}

impl UltraPlan {
    pub fn new(title: &str, description: &str) -> Self {
        let now = Utc::now();
        Self {
            id: format!("plan-{}", now.timestamp()),
            goal: UltraGoal {
                id: format!("goal-{}", now.timestamp()),
                title: title.to_string(),
                description: description.to_string(),
                success_criteria: vec![],
                constraints: vec![],
                created_at: now,
                status: GoalStatus::Proposed,
            },
            milestones: vec![],
            tasks: HashMap::new(),
            created_at: now,
            updated_at: now,
            version: 1,
        }
    }

    /// Add a milestone to the plan
    pub fn add_milestone(&mut self, title: &str, description: &str) -> String {
        let id = format!("milestone-{}", self.milestones.len() + 1);
        let milestone = UltraMilestone {
            id: id.clone(),
            goal_id: self.goal.id.clone(),
            title: title.to_string(),
            description: description.to_string(),
            tasks: vec![],
            due_date: None,
            completed: false,
            completion_criteria: vec![],
        };
        self.milestones.push(milestone);
        self.updated_at = Utc::now();
        id
    }

    /// Add a task to a milestone
    pub fn add_task(
        &mut self,
        milestone_id: &str,
        title: &str,
        description: &str,
        priority: u8,
    ) -> Result<String, String> {
        if !self.milestones.iter().any(|m| m.id == milestone_id) {
            return Err(format!("Milestone '{}' not found", milestone_id));
        }

        let id = format!("task-{}", self.tasks.len() + 1);
        let task = UltraTask {
            id: id.clone(),
            milestone_id: milestone_id.to_string(),
            parent_task: None,
            children: vec![],
            title: title.to_string(),
            description: description.to_string(),
            priority,
            estimated_hours: 0.0,
            actual_hours: None,
            status: UltraTaskStatus::NotStarted,
            dependencies: vec![],
            artifacts: vec![],
        };

        // Add task to milestone
        if let Some(milestone) = self.milestones.iter_mut().find(|m| m.id == milestone_id) {
            milestone.tasks.push(id.clone());
        }

        self.tasks.insert(id.clone(), task);
        self.updated_at = Utc::now();
        Ok(id)
    }

    /// Add a subtask to an existing task
    pub fn add_subtask(
        &mut self,
        parent_task_id: &str,
        title: &str,
        description: &str,
    ) -> Result<String, String> {
        if !self.tasks.contains_key(parent_task_id) {
            return Err(format!("Parent task '{}' not found", parent_task_id));
        }

        let id = format!("subtask-{}", self.tasks.len() + 1);
        let parent_milestone = self.tasks[parent_task_id].milestone_id.clone();

        let task = UltraTask {
            id: id.clone(),
            milestone_id: parent_milestone,
            parent_task: Some(parent_task_id.to_string()),
            children: vec![],
            title: title.to_string(),
            description: description.to_string(),
            priority: self.tasks[parent_task_id].priority,
            estimated_hours: 0.0,
            actual_hours: None,
            status: UltraTaskStatus::NotStarted,
            dependencies: vec![parent_task_id.to_string()],
            artifacts: vec![],
        };

        // Update parent's children list
        if let Some(parent) = self.tasks.get_mut(parent_task_id) {
            parent.children.push(id.clone());
        }

        self.tasks.insert(id.clone(), task);
        self.updated_at = Utc::now();
        Ok(id)
    }

    /// Get task hierarchy as indented text
    pub fn get_hierarchy_text(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("# {}\n\n", self.goal.title));
        output.push_str(&format!("**Description:** {}\n\n", self.goal.description));

        for milestone in &self.milestones {
            output.push_str(&format!("## {} - {}\n\n", 
                if milestone.completed { "✅" } else { "⬜" },
                milestone.title
            ));

            for task_id in &milestone.tasks {
                if let Some(task) = self.tasks.get(task_id) {
                    if task.parent_task.is_none() {
                        self.format_task_tree(task, &mut output, 0);
                    }
                }
            }
            output.push('\n');
        }

        output
    }

    fn format_task_tree(&self, task: &UltraTask, output: &mut String, depth: usize) {
        let indent = "  ".repeat(depth);
        let status_icon = match task.status {
            UltraTaskStatus::Completed => "✅",
            UltraTaskStatus::InProgress => "🔄",
            UltraTaskStatus::Blocked => "🚫",
            UltraTaskStatus::Review => "👀",
            _ => "⬜",
        };

        output.push_str(&format!(
            "{}{} [P{}] {}\n",
            indent,
            status_icon,
            task.priority,
            task.title
        ));

        for child_id in &task.children {
            if let Some(child) = self.tasks.get(child_id) {
                self.format_task_tree(child, output, depth + 1);
            }
        }
    }

    /// Calculate plan completion percentage
    pub fn completion_percentage(&self) -> f64 {
        if self.tasks.is_empty() {
            return 0.0;
        }

        let completed = self.tasks.values()
            .filter(|t| t.status == UltraTaskStatus::Completed)
            .count();
        
        (completed as f64 / self.tasks.len() as f64) * 100.0
    }

    /// Get plan statistics
    pub fn get_stats(&self) -> PlanStats {
        let total_tasks = self.tasks.len();
        let mut by_status: HashMap<String, usize> = HashMap::new();
        
        for task in self.tasks.values() {
            let status_str = format!("{:?}", task.status);
            *by_status.entry(status_str).or_insert(0) += 1;
        }

        let total_estimated_hours: f64 = self.tasks.values()
            .map(|t| t.estimated_hours)
            .sum();

        PlanStats {
            total_tasks,
            total_milestones: self.milestones.len(),
            completion_percentage: self.completion_percentage(),
            total_estimated_hours,
            by_status,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStats {
    pub total_tasks: usize,
    pub total_milestones: usize,
    pub completion_percentage: f64,
    pub total_estimated_hours: f64,
    pub by_status: HashMap<String, usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_ultra_plan() {
        let mut plan = UltraPlan::new("Test Goal", "A test goal");
        assert_eq!(plan.goal.title, "Test Goal");
        assert_eq!(plan.milestones.len(), 0);
        assert_eq!(plan.tasks.len(), 0);
    }

    #[test]
    fn test_add_milestone_and_task() {
        let mut plan = UltraPlan::new("Build Feature", "Implement new feature");
        
        let milestone_id = plan.add_milestone("Phase 1", "Initial implementation");
        let task_id = plan.add_task(&milestone_id, "Setup project", "Initialize repo", 8)
            .unwrap();
        
        assert_eq!(plan.milestones.len(), 1);
        assert_eq!(plan.tasks.len(), 1);
        assert!(plan.tasks.contains_key(&task_id));
    }

    #[test]
    fn test_add_subtask() {
        let mut plan = UltraPlan::new("Goal", "Description");
        let milestone_id = plan.add_milestone("M1", "First milestone");
        let parent_id = plan.add_task(&milestone_id, "Parent task", "Do something", 5)
            .unwrap();
        let subtask_id = plan.add_subtask(&parent_id, "Subtask", "Do part of it")
            .unwrap();
        
        assert_eq!(plan.tasks.len(), 2);
        assert!(plan.tasks[&parent_id].children.contains(&subtask_id));
        assert_eq!(plan.tasks[&subtask_id].parent_task, Some(parent_id));
    }

    #[test]
    fn test_hierarchy_text() {
        let mut plan = UltraPlan::new("My Goal", "Achieve greatness");
        let mid = plan.add_milestone("Milestone 1", "First step");
        plan.add_task(&mid, "Task A", "Do A", 7).unwrap();
        plan.add_task(&mid, "Task B", "Do B", 5).unwrap();
        
        let text = plan.get_hierarchy_text();
        assert!(text.contains("My Goal"));
        assert!(text.contains("Milestone 1"));
        assert!(text.contains("Task A"));
    }

    #[test]
    fn test_completion_percentage() {
        let mut plan = UltraPlan::new("Goal", "Desc");
        let mid = plan.add_milestone("M1", "Milestone");
        let tid1 = plan.add_task(&mid, "Task 1", "First", 5).unwrap();
        let tid2 = plan.add_task(&mid, "Task 2", "Second", 5).unwrap();
        
        // Initially 0%
        assert_eq!(plan.completion_percentage(), 0.0);
        
        // Complete one task
        plan.tasks.get_mut(&tid1).unwrap().status = UltraTaskStatus::Completed;
        assert!((plan.completion_percentage() - 50.0).abs() < f64::EPSILON);
        
        // Complete both
        plan.tasks.get_mut(&tid2).unwrap().status = UltraTaskStatus::Completed;
        assert!((plan.completion_percentage() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_plan_stats() {
        let mut plan = UltraPlan::new("Goal", "Desc");
        let mid = plan.add_milestone("M1", "Milestone");
        plan.add_task(&mid, "Task 1", "First", 5).unwrap();
        plan.add_task(&mid, "Task 2", "Second", 7).unwrap();
        
        let stats = plan.get_stats();
        assert_eq!(stats.total_tasks, 2);
        assert_eq!(stats.total_milestones, 1);
    }
}
