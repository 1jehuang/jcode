use std::sync::{Arc, Mutex};
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq)]
pub enum PlanMode {
    Off,
    Planning,
    Executing,
}

#[derive(Debug, Clone)]
pub struct PlanStep {
    pub id: usize,
    pub description: String,
    pub status: StepStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StepStatus {
    Pending,
    Approved,
    Rejected,
    Completed,
    Skipped,
}

pub struct PlanModeState {
    mode: PlanMode,
    plan: Vec<PlanStep>,
    history: VecDeque<String>,
    max_history: usize,
}

impl PlanModeState {
    pub fn new() -> Self {
        PlanModeState {
            mode: PlanMode::Off,
            plan: vec![],
            history: VecDeque::new(),
            max_history: 100,
        }
    }

    pub fn enter_plan_mode(&mut self) -> Result<(), String> {
        if self.mode == PlanMode::Planning {
            return Err("Already in plan mode".to_string());
        }
        self.mode = PlanMode::Planning;
        self.plan.clear();
        self.add_log("Entered plan mode");
        Ok(())
    }

    pub fn exit_plan_mode(&mut self) -> Result<(), String> {
        if self.mode == PlanMode::Off {
            return Err("Not in plan mode".to_string());
        }
        self.mode = PlanMode::Off;
        self.plan.clear();
        self.add_log("Exited plan mode");
        Ok(())
    }

    pub fn is_planning(&self) -> bool { self.mode == PlanMode::Planning }
    pub fn is_executing(&self) -> bool { self.mode == PlanMode::Executing }

    pub fn add_step(&mut self, description: &str) -> Result<usize, String> {
        if !self.is_planning() {
            return Err("Not in planning mode. Use /plan on first.".to_string());
        }
        let step = PlanStep {
            id: self.plan.len() + 1,
            description: description.to_string(),
            status: StepStatus::Pending,
            created_at: chrono::Utc::now(),
        };
        let id = step.id;
        self.plan.push(step);
        Ok(id)
    }

    pub fn approve_step(&mut self, id: usize) -> Result<(), String> {
        let step_desc = {
            let step = self.plan.iter_mut().find(|s| s.id == id)
                .ok_or_else(|| format!("Step {} not found", id))?;
            step.status = StepStatus::Approved;
            step.description.clone()
        };
        self.add_log(&format!("Approved step {}: {}", id, step_desc));
        Ok(())
    }

    pub fn reject_step(&mut self, id: usize) -> Result<(), String> {
        let step_desc = {
            let step = self.plan.iter_mut().find(|s| s.id == id)
                .ok_or_else(|| format!("Step {} not found", id))?;
            step.status = StepStatus::Rejected;
            step.description.clone()
        };
        self.add_log(&format!("Rejected step {}: {}", id, step_desc));
        Ok(())
    }

    pub fn complete_step(&mut self, id: usize) -> Result<(), String> {
        let step_desc = {
            let step = self.plan.iter_mut().find(|s| s.id == id)
                .ok_or_else(|| format!("Step {} not found", id))?;
            step.status = StepStatus::Completed;
            step.description.clone()
        };
        self.add_log(&format!("Completed step {}: {}", id, step_desc));
        Ok(())
    }

    pub fn get_plan(&self) -> &[PlanStep] { &self.plan }
    pub fn get_mode(&self) -> &PlanMode { &self.mode }

    pub fn get_summary(&self) -> String {
        let mut summary = format!("Plan Mode: {:?}\n", self.mode);
        summary.push_str(&format!("Steps: {} total\n", self.plan.len()));

        let pending = self.plan.iter().filter(|s| s.status == StepStatus::Pending).count();
        let approved = self.plan.iter().filter(|s| s.status == StepStatus::Approved).count();
        let completed = self.plan.iter().filter(|s| s.status == StepStatus::Completed).count();

        summary.push_str(&format!("  Pending: {} | Approved: {} | Completed: {}\n", pending, approved, completed));

        for step in &self.plan {
            let status = match step.status {
                StepStatus::Pending => "⏳",
                StepStatus::Approved => "✅",
                StepStatus::Rejected => "❌",
                StepStatus::Completed => "✓",
                StepStatus::Skipped => "⏭️",
            };
            summary.push_str(&format!("  [{}] {}. {}\n", status, step.id, step.description));
        }

        summary
    }

    fn add_log(&mut self, message: &str) {
        let log_entry = format!("[{}] {}", chrono::Utc::now().format("%H:%M:%S"), message);
        if self.history.len() >= self.max_history {
            self.history.pop_front();
        }
        self.history.push_back(log_entry);
    }

    pub fn get_history(&self) -> Vec<&String> { self.history.iter().collect() }
}

pub struct EnterPlanModeTool;

impl EnterPlanModeTool {
    pub fn execute(state: Arc<Mutex<PlanModeState>>) -> Result<String, String> {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        s.enter_plan_mode()?;
        Ok("✓ Entered plan mode. All actions will require approval before execution.\nUse /plan to view and manage your plan.".to_string())
    }
}

pub struct ExitPlanModeV2Tool;

impl ExitPlanModeV2Tool {
    pub fn execute(state: Arc<Mutex<PlanModeState>>, force: bool) -> Result<String, String> {
        let mut s = state.lock().map_err(|e| e.to_string())?;

        if !force && s.is_planning() && !s.get_plan().is_empty() {
            let pending = s.get_plan().iter().filter(|step| matches!(step.status, StepStatus::Pending | StepStatus::Approved)).count();
            if pending > 0 {
                return Err(format!(
                    "Cannot exit plan mode with {} pending/approved steps.\nUse 'exit --force' to discard the plan.",
                    pending
                ));
            }
        }

        s.exit_plan_mode()?;
        Ok("✓ Exited plan mode. Normal execution resumed.".to_string())
    }
}
