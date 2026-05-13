use std::sync::Arc;
use tokio::sync::RwLock;

use super::pipeline::{PipelineId, PipelineRunner, PipelineStatus};
use super::stage::StageStatus;

/// Pipeline execution engine
pub struct PipelineExecutor {
    runner: Arc<PipelineRunner>,
    abort_signals: Arc<RwLock<Vec<PipelineId>>>,
}

impl PipelineExecutor {
    pub fn new(runner: Arc<PipelineRunner>) -> Self {
        PipelineExecutor {
            runner,
            abort_signals: Arc::new(RwLock::new(vec![])),
        }
    }

    pub async fn execute(&self, pipeline_id: &PipelineId) -> Result<(), String> {
        let mut pipeline = self.runner.get(pipeline_id).await
            .ok_or_else(|| "Pipeline not found".to_string())?;

        pipeline.status = PipelineStatus::Running;
        pipeline.started_at = Some(chrono::Utc::now());

        for stage_index in 0..pipeline.stages.len() {
            if self.is_aborted(pipeline_id).await {
                pipeline.status = PipelineStatus::Cancelled;
                return Err("Pipeline cancelled".to_string());
            }

            let stage = &mut pipeline.stages[stage_index];
            stage.status = StageStatus::Running;
            stage.started_at = Some(chrono::Utc::now());

            for step in &stage.config.steps {
                let exit_code = self.run_step(&step.command).await;
                let success = exit_code == 0;

                if success {
                    stage.step_results.push(StageStatus::Succeeded);
                } else {
                    stage.step_results.push(StageStatus::Failed(
                        format!("Exit code: {}", exit_code)
                    ));
                    if !stage.config.allow_failure {
                        stage.status = StageStatus::Failed(format!("Step '{}' failed", step.name));
                        pipeline.status = PipelineStatus::Failed(
                            format!("Stage '{}' failed at step '{}'", stage.config.name, step.name)
                        );
                        return Err(pipeline.status.clone().to_string_lossy());
                    }
                }
            }

            if stage.status == StageStatus::Running {
                stage.status = StageStatus::Succeeded;
            }
            stage.completed_at = Some(chrono::Utc::now());
        }

        if !pipeline.any_stage_failed() {
            pipeline.status = PipelineStatus::Succeeded;
        }
        pipeline.completed_at = Some(chrono::Utc::now());

        Ok(())
    }

    async fn run_step(&self, _command: &str) -> i32 {
        tokio::process::Command::new("cmd")
            .args(&["/C", _command])
            .output()
            .await
            .map(|output| output.status.code().unwrap_or(-1))
            .unwrap_or(-1)
    }

    async fn is_aborted(&self, pipeline_id: &PipelineId) -> bool {
        self.abort_signals.read().await.contains(pipeline_id)
    }

    pub async fn abort(&self, pipeline_id: &PipelineId) {
        self.abort_signals.write().await.push(pipeline_id.clone());
    }
}

impl PipelineStatus {
    fn to_string_lossy(&self) -> String {
        match self {
            PipelineStatus::Failed(msg) => msg.clone(),
            PipelineStatus::Cancelled => "Pipeline was cancelled".to_string(),
            _ => "Unknown error".to_string(),
        }
    }
}