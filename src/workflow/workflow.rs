use std::collections::HashMap;

use super::step::{WorkflowStep, StepStatus};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkflowId(pub String);

#[derive(Debug, Clone, PartialEq)]
pub enum WorkflowStatus {
    Idle,
    Running,
    Paused,
    Completed,
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct WorkflowConfig {
    pub id: WorkflowId,
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    pub tags: Vec<String>,
    pub steps: Vec<WorkflowStep>,
    pub variables: HashMap<String, String>,
    pub env_vars: HashMap<String, String>,
    pub timeout_secs: u64,
    pub max_retries: u32,
    pub parallel_steps: bool,
    pub requires_confirmation: bool,
    pub on_success: Option<String>,
    pub on_failure: Option<String>,
}

impl WorkflowConfig {
    pub fn new(name: &str) -> Self {
        WorkflowConfig {
            id: WorkflowId(format!("wf-{}", chrono::Utc::now().timestamp())),
            name: name.to_string(),
            description: String::new(),
            version: "1.0.0".to_string(),
            author: "user".to_string(),
            tags: vec![],
            steps: vec![],
            variables: HashMap::new(),
            env_vars: HashMap::new(),
            timeout_secs: 3600,
            max_retries: 0,
            parallel_steps: false,
            requires_confirmation: false,
            on_success: None,
            on_failure: None,
        }
    }

    pub fn with_step(mut self, step: WorkflowStep) -> Self {
        self.steps.push(step);
        self
    }

    pub fn with_var(mut self, key: &str, value: &str) -> Self {
        self.variables.insert(key.to_string(), value.to_string());
        self
    }
}

#[derive(Debug, Clone)]
pub struct Workflow {
    pub config: WorkflowConfig,
    pub status: WorkflowStatus,
    pub current_step: usize,
    pub step_results: Vec<StepResult>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub log: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StepResult {
    pub step_name: String,
    pub status: StepStatus,
    pub output: Option<String>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

impl Workflow {
    pub fn new(config: WorkflowConfig) -> Self {
        Workflow {
            config,
            status: WorkflowStatus::Idle,
            current_step: 0,
            step_results: vec![],
            started_at: None,
            completed_at: None,
            log: vec![],
        }
    }
}