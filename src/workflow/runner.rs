use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::workflow::{Workflow, WorkflowConfig, WorkflowId, WorkflowStatus, StepResult};
use super::step::{StepType, StepStatus};

/// Workflow execution engine
pub struct WorkflowRunner {
    active_workflows: Arc<RwLock<HashMap<WorkflowId, Workflow>>>,
    history: Arc<RwLock<Vec<WorkflowId>>>,
}

impl WorkflowRunner {
    pub fn new() -> Self {
        WorkflowRunner {
            active_workflows: Arc::new(RwLock::new(HashMap::new())),
            history: Arc::new(RwLock::new(vec![])),
        }
    }

    pub async fn register(&self, config: WorkflowConfig) -> WorkflowId {
        let id = config.id.clone();
        let workflow = Workflow::new(config);
        self.active_workflows.write().await.insert(id.clone(), workflow);
        id
    }

    pub async fn execute(&self, id: &WorkflowId) -> Result<(), String> {
        let mut guard = self.active_workflows.write().await;
        let workflow = guard.get_mut(id)
            .ok_or_else(|| "Workflow not found".to_string())?;

        workflow.status = WorkflowStatus::Running;
        workflow.started_at = Some(chrono::Utc::now());
        workflow.log.push("Workflow started".to_string());

        for step_index in 0..workflow.config.steps.len() {
            workflow.current_step = step_index;
            let step_name = workflow.config.steps[step_index].name.clone();
            let step_type = workflow.config.steps[step_index].step_type.clone();
            let allow_failure = workflow.config.steps[step_index].allow_failure;

            workflow.log.push(format!("Running step: {}", step_name));

            let result = match &step_type {
                StepType::Command { command, args } => {
                    let output = tokio::process::Command::new("cmd")
                        .args(&["/C", command])
                        .args(args)
                        .output().await;

                    match output {
                        Ok(out) => {
                            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                            if out.status.success() {
                                StepResult {
                                    step_name: step_name.clone(),
                                    status: StepStatus::Completed,
                                    output: Some(stdout),
                                    error: None,
                                    duration_ms: 0,
                                }
                            } else {
                                StepResult {
                                    step_name: step_name.clone(),
                                    status: StepStatus::Failed(
                                        String::from_utf8_lossy(&out.stderr).to_string()
                                    ),
                                    output: None,
                                    error: Some(String::from_utf8_lossy(&out.stderr).to_string()),
                                    duration_ms: 0,
                                }
                            }
                        }
                        Err(e) => StepResult {
                            step_name: step_name.clone(),
                            status: StepStatus::Failed(e.to_string()),
                            output: None,
                            error: Some(e.to_string()),
                            duration_ms: 0,
                        }
                    }
                }
                StepType::Script { content, interpreter } => {
                    let output = tokio::process::Command::new(interpreter)
                        .arg("-Command")
                        .arg(content)
                        .output().await;

                    match output {
                        Ok(out) => StepResult {
                            step_name: step_name.clone(),
                            status: if out.status.success() { StepStatus::Completed } else { StepStatus::Failed(String::from_utf8_lossy(&out.stderr).to_string()) },
                            output: Some(String::from_utf8_lossy(&out.stdout).to_string()),
                            error: if out.status.success() { None } else { Some(String::from_utf8_lossy(&out.stderr).to_string()) },
                            duration_ms: 0,
                        },
                        Err(e) => StepResult {
                            step_name: step_name.clone(),
                            status: StepStatus::Failed(e.to_string()),
                            output: None,
                            error: Some(e.to_string()),
                            duration_ms: 0,
                        }
                    }
                }
                StepType::Approval { .. } => {
                    StepResult {
                        step_name: step_name.clone(),
                        status: StepStatus::Completed,
                        output: Some("Approval step (auto-approved)".to_string()),
                        error: None,
                        duration_ms: 0,
                    }
                }
                _ => StepResult {
                    step_name: step_name.clone(),
                    status: StepStatus::Skipped,
                    output: Some("Step type not implemented".to_string()),
                    error: None,
                    duration_ms: 0,
                }
            };

            let is_failure = matches!(&result.status, StepStatus::Failed(_));
            workflow.step_results.push(result);

            if is_failure && !allow_failure {
                workflow.status = WorkflowStatus::Failed(format!("Step '{}' failed", step_name));
                workflow.log.push(format!("Workflow failed at step: {}", step_name));
                return Err(format!("Workflow '{}' failed at step '{}'", id.0, step_name));
            }
        }

        workflow.status = WorkflowStatus::Completed;
        workflow.completed_at = Some(chrono::Utc::now());
        workflow.log.push("Workflow completed".to_string());
        self.history.write().await.push(id.clone());
        Ok(())
    }

    pub async fn get(&self, id: &WorkflowId) -> Option<Workflow> {
        self.active_workflows.read().await.get(id).cloned()
    }

    pub async fn list(&self) -> Vec<(WorkflowId, String, String)> {
        self.active_workflows.read().await.iter()
            .map(|(id, wf)| (id.clone(), wf.config.name.clone(), format!("{:?}", wf.status)))
            .collect()
    }

    pub async fn abort(&self, id: &WorkflowId) -> bool {
        let mut workflows = self.active_workflows.write().await;
        if let Some(wf) = workflows.get_mut(id) {
            if wf.status == WorkflowStatus::Running {
                wf.status = WorkflowStatus::Cancelled;
                wf.log.push("Workflow cancelled by user".to_string());
                true
            } else {
                false
            }
        } else {
            false
        }
    }
}

impl Default for WorkflowRunner {
    fn default() -> Self {
        Self::new()
    }
}