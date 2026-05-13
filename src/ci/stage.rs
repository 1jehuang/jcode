use std::collections::HashMap;
use std::path::PathBuf;

/// Status of a single pipeline stage
#[derive(Debug, Clone, PartialEq)]
pub enum StageStatus {
    Pending,
    Running,
    Succeeded,
    Failed(String),
    Cancelled,
    Skipped,
}

impl StageStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, StageStatus::Succeeded | StageStatus::Failed(_) | StageStatus::Cancelled | StageStatus::Skipped)
    }

    pub fn is_success(&self) -> bool {
        matches!(self, StageStatus::Succeeded)
    }

    pub fn label(&self) -> &str {
        match self {
            StageStatus::Pending => "pending",
            StageStatus::Running => "running",
            StageStatus::Succeeded => "succeeded",
            StageStatus::Failed(_) => "failed",
            StageStatus::Cancelled => "cancelled",
            StageStatus::Skipped => "skipped",
        }
    }
}

/// Stage execution step
#[derive(Debug, Clone)]
pub struct StageStep {
    pub name: String,
    pub command: String,
    pub working_dir: Option<PathBuf>,
    pub timeout_secs: Option<u64>,
    pub env_vars: HashMap<String, String>,
    pub status: StageStatus,
    pub output: Vec<String>,
    pub exit_code: Option<i32>,
}

impl StageStep {
    pub fn new(name: &str, command: &str) -> Self {
        StageStep {
            name: name.to_string(),
            command: command.to_string(),
            working_dir: None,
            timeout_secs: None,
            env_vars: HashMap::new(),
            status: StageStatus::Pending,
            output: vec![],
            exit_code: None,
        }
    }
}

/// Configuration for a pipeline stage
#[derive(Debug, Clone)]
pub struct StageConfig {
    pub name: String,
    pub description: String,
    pub depends_on: Vec<String>,
    pub parallel: bool,
    pub allow_failure: bool,
    pub steps: Vec<StageStep>,
    pub cache_keys: Vec<String>,
    pub artifact_paths: Vec<PathBuf>,
    pub timeout_secs: u64,
    pub retry_count: u32,
    pub required_approval: bool,
}

impl StageConfig {
    pub fn new(name: &str) -> Self {
        StageConfig {
            name: name.to_string(),
            description: String::new(),
            depends_on: vec![],
            parallel: false,
            allow_failure: false,
            steps: vec![],
            cache_keys: vec![],
            artifact_paths: vec![],
            timeout_secs: 3600,
            retry_count: 0,
            required_approval: false,
        }
    }

    pub fn with_step(mut self, step: StageStep) -> Self {
        self.steps.push(step);
        self
    }

    pub fn with_dependency(mut self, name: &str) -> Self {
        self.depends_on.push(name.to_string());
        self
    }
}

/// A running pipeline stage
#[derive(Debug, Clone)]
pub struct PipelineStage {
    pub index: usize,
    pub config: StageConfig,
    pub status: StageStatus,
    pub step_results: Vec<StageStatus>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub attempt: u32,
}

impl PipelineStage {
    pub fn new(index: usize, config: StageConfig) -> Self {
        PipelineStage {
            index,
            config,
            status: StageStatus::Pending,
            step_results: vec![],
            started_at: None,
            completed_at: None,
            attempt: 0,
        }
    }
}