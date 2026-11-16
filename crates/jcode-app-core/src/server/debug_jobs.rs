use crate::agent::Agent;
use crate::id;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

#[cfg(test)]
mod serialization_ownership_tests {
    use super::*;

    #[test]
    fn debug_job_payload_preserves_legacy_fields_and_optional_values() {
        for status in [
            DebugJobStatus::Queued,
            DebugJobStatus::Running,
            DebugJobStatus::Completed,
            DebugJobStatus::Failed,
        ] {
            for populated in [false, true] {
                let created_at = Instant::now() - Duration::from_secs(3);
                let job = DebugJob {
                    id: "job-雪\"\\".into(),
                    status: status.clone(),
                    command: "message:hello\n世界".into(),
                    session_id: populated.then(|| "session-🦀".into()),
                    created_at,
                    started_at: populated.then_some(created_at + Duration::from_secs(1)),
                    finished_at: populated.then_some(created_at + Duration::from_secs(2)),
                    output: populated.then(|| "output\n雪".into()),
                    error: populated.then(String::new),
                };
                let payload = job.status_payload();
                // Keep wall-clock values from this invocation while independently
                // reproducing the legacy owned-field serialization contract.
                let expected = serde_json::json!({
                    "id": job.id.clone(),
                    "status": job.status.as_str(),
                    "command": job.command.clone(),
                    "session_id": job.session_id.clone(),
                    "elapsed_secs": payload["elapsed_secs"].clone(),
                    "run_secs": payload["run_secs"].clone(),
                    "total_secs": populated.then_some(2.0),
                    "output": job.output.clone(),
                    "error": job.error.clone(),
                });
                assert_eq!(payload, expected);
                assert_eq!(
                    serde_json::to_vec(&payload).unwrap(),
                    serde_json::to_vec(&expected).unwrap()
                );
                assert!(payload["elapsed_secs"].as_f64().unwrap() >= 3.0);
                assert_eq!(payload["run_secs"].is_null(), !populated);
                if populated {
                    assert!(payload["run_secs"].as_f64().unwrap() >= 2.0);
                }
                assert_eq!(job.id, "job-雪\"\\");
                assert_eq!(job.command, "message:hello\n世界");
            }
        }
    }

    #[test]
    fn debug_job_large_output_keeps_source_and_result_independent() {
        let output = "large-output-雪\n".repeat(100_000);
        let source = output.as_ptr();
        let job = DebugJob {
            id: "large".into(),
            status: DebugJobStatus::Completed,
            command: "message:fixture".into(),
            session_id: None,
            created_at: Instant::now(),
            started_at: None,
            finished_at: None,
            output: Some(output),
            error: None,
        };
        let mut payload = job.status_payload();
        assert_eq!(payload["output"].as_str(), job.output.as_deref());
        assert_eq!(job.output.as_ref().unwrap().as_ptr(), source);
        payload["output"] = Value::String("modified diagnostic".into());
        assert_eq!(
            job.output.as_ref().unwrap().len(),
            "large-output-雪\n".len() * 100_000
        );
        assert!(
            job.output
                .as_ref()
                .unwrap()
                .starts_with("large-output-雪\n")
        );
        assert!(job.summary_payload().get("output").is_none());
    }
}

#[derive(Clone, Debug)]
pub(super) enum DebugJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

impl DebugJobStatus {
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            DebugJobStatus::Queued => "queued",
            DebugJobStatus::Running => "running",
            DebugJobStatus::Completed => "completed",
            DebugJobStatus::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct DebugJob {
    pub(super) id: String,
    pub(super) status: DebugJobStatus,
    pub(super) command: String,
    pub(super) session_id: Option<String>,
    pub(super) created_at: Instant,
    pub(super) started_at: Option<Instant>,
    pub(super) finished_at: Option<Instant>,
    pub(super) output: Option<String>,
    pub(super) error: Option<String>,
}

impl DebugJob {
    pub(super) fn summary_payload(&self) -> Value {
        let now = Instant::now();
        let elapsed_secs = now.duration_since(self.created_at).as_secs_f64();
        let run_secs = self.started_at.map(|s| now.duration_since(s).as_secs_f64());
        let total_secs = self
            .finished_at
            .map(|f| f.duration_since(self.created_at).as_secs_f64());

        serde_json::json!({
            "id": self.id,
            "status": self.status.as_str(),
            "command": self.command,
            "session_id": self.session_id,
            "elapsed_secs": elapsed_secs,
            "run_secs": run_secs,
            "total_secs": total_secs,
        })
    }

    pub(super) fn status_payload(&self) -> Value {
        let mut payload = self.summary_payload();
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("output".to_string(), serde_json::json!(self.output));
            obj.insert("error".to_string(), serde_json::json!(self.error));
        }
        payload
    }
}

pub(super) async fn maybe_start_async_debug_job(
    agent: Arc<Mutex<Agent>>,
    trimmed: &str,
    debug_jobs: Arc<RwLock<HashMap<String, DebugJob>>>,
) -> Result<Option<String>> {
    if trimmed.starts_with("swarm_message_async:") {
        let msg = trimmed
            .strip_prefix("swarm_message_async:")
            .unwrap_or("")
            .trim();
        if msg.is_empty() {
            return Err(anyhow::anyhow!("swarm_message_async: requires content"));
        }

        let job_id = create_job(&agent, &debug_jobs, format!("swarm_message:{}", msg)).await;

        let jobs = Arc::clone(&debug_jobs);
        let agent = Arc::clone(&agent);
        let msg = msg.to_string();
        let job_id_inner = job_id.clone();
        crate::runtime_activity::spawn(async move {
            mark_job_running(&jobs, &job_id_inner).await;

            let result = super::run_swarm_message(agent.clone(), &msg).await;
            let partial_output = if result.is_err() {
                let agent = agent.lock().await;
                agent.last_assistant_text()
            } else {
                None
            };

            finish_job(jobs, &job_id_inner, result, partial_output).await;
        });

        return Ok(Some(serde_json::json!({ "job_id": job_id }).to_string()));
    }

    if trimmed.starts_with("message_async:") {
        let msg = trimmed.strip_prefix("message_async:").unwrap_or("").trim();
        if msg.is_empty() {
            return Err(anyhow::anyhow!("message_async: requires content"));
        }

        let job_id = create_job(&agent, &debug_jobs, format!("message:{}", msg)).await;

        let jobs = Arc::clone(&debug_jobs);
        let agent = Arc::clone(&agent);
        let msg = msg.to_string();
        let job_id_inner = job_id.clone();
        crate::runtime_activity::spawn(async move {
            mark_job_running(&jobs, &job_id_inner).await;

            let result = {
                let mut agent = agent.lock().await;
                agent.run_once_capture(&msg).await
            };
            let partial_output = if result.is_err() {
                let agent = agent.lock().await;
                agent.last_assistant_text()
            } else {
                None
            };

            finish_job(jobs, &job_id_inner, result, partial_output).await;
        });

        return Ok(Some(serde_json::json!({ "job_id": job_id }).to_string()));
    }

    Ok(None)
}

pub(super) async fn maybe_handle_job_command(
    cmd: &str,
    debug_jobs: &Arc<RwLock<HashMap<String, DebugJob>>>,
) -> Result<Option<String>> {
    if cmd == "jobs" {
        let jobs_guard = debug_jobs.read().await;
        let payload: Vec<Value> = jobs_guard
            .values()
            .map(|job| job.summary_payload())
            .collect();
        return Ok(Some(
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "[]".to_string()),
        ));
    }

    if cmd.starts_with("job_status:") {
        let job_id = cmd.strip_prefix("job_status:").unwrap_or("").trim();
        if job_id.is_empty() {
            return Err(anyhow::anyhow!("job_status: requires a job id"));
        }
        let jobs_guard = debug_jobs.read().await;
        let output = jobs_guard
            .get(job_id)
            .map(|job| {
                serde_json::to_string_pretty(&job.status_payload())
                    .unwrap_or_else(|_| "{}".to_string())
            })
            .ok_or_else(|| anyhow::anyhow!("Unknown job id '{}'", job_id))?;
        return Ok(Some(output));
    }

    if cmd.starts_with("job_cancel:") {
        let job_id = cmd.strip_prefix("job_cancel:").unwrap_or("").trim();
        if job_id.is_empty() {
            return Err(anyhow::anyhow!("job_cancel: requires a job id"));
        }
        let mut jobs_guard = debug_jobs.write().await;
        let output = if let Some(job) = jobs_guard.get_mut(job_id) {
            if matches!(job.status, DebugJobStatus::Running | DebugJobStatus::Queued) {
                job.status = DebugJobStatus::Failed;
                job.output = Some("[CANCELLED]".to_string());
                serde_json::json!({
                    "status": "cancelled",
                    "job_id": job_id,
                })
                .to_string()
            } else {
                return Err(anyhow::anyhow!("Job '{}' is not running", job_id));
            }
        } else {
            return Err(anyhow::anyhow!("Unknown job id '{}'", job_id));
        };
        return Ok(Some(output));
    }

    if cmd == "jobs:purge" {
        let mut jobs_guard = debug_jobs.write().await;
        let before = jobs_guard.len();
        jobs_guard.retain(|_, job| {
            matches!(job.status, DebugJobStatus::Running | DebugJobStatus::Queued)
        });
        let removed = before - jobs_guard.len();
        return Ok(Some(
            serde_json::json!({
                "status": "purged",
                "removed": removed,
                "remaining": jobs_guard.len(),
            })
            .to_string(),
        ));
    }

    if cmd.starts_with("jobs:session:") {
        let sess_id = cmd.strip_prefix("jobs:session:").unwrap_or("").trim();
        let jobs_guard = debug_jobs.read().await;
        let payload: Vec<Value> = jobs_guard
            .values()
            .filter(|job| job.session_id.as_deref() == Some(sess_id))
            .map(|job| job.summary_payload())
            .collect();
        return Ok(Some(
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "[]".to_string()),
        ));
    }

    if cmd.starts_with("job_wait:") {
        let job_id = cmd.strip_prefix("job_wait:").unwrap_or("").trim();
        if job_id.is_empty() {
            return Err(anyhow::anyhow!("job_wait: requires a job id"));
        }
        let timeout = Duration::from_secs(900);
        let start = Instant::now();
        loop {
            {
                let jobs_guard = debug_jobs.read().await;
                if let Some(job) = jobs_guard.get(job_id) {
                    if matches!(
                        job.status,
                        DebugJobStatus::Completed | DebugJobStatus::Failed
                    ) {
                        return Ok(Some(
                            serde_json::to_string_pretty(&job.status_payload())
                                .unwrap_or_else(|_| "{}".to_string()),
                        ));
                    }
                } else {
                    return Err(anyhow::anyhow!("Unknown job id '{}'", job_id));
                }
            }
            if start.elapsed() > timeout {
                return Err(anyhow::anyhow!("Timeout waiting for job '{}'", job_id));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    Ok(None)
}

async fn create_job(
    agent: &Arc<Mutex<Agent>>,
    debug_jobs: &Arc<RwLock<HashMap<String, DebugJob>>>,
    command: String,
) -> String {
    let session = {
        let agent = agent.lock().await;
        agent.session_id().to_string()
    };
    let job_id = id::new_id("job");
    {
        let mut jobs = debug_jobs.write().await;
        jobs.insert(
            job_id.clone(),
            DebugJob {
                id: job_id.clone(),
                status: DebugJobStatus::Queued,
                command,
                session_id: Some(session),
                created_at: Instant::now(),
                started_at: None,
                finished_at: None,
                output: None,
                error: None,
            },
        );
    }
    job_id
}

async fn mark_job_running(debug_jobs: &Arc<RwLock<HashMap<String, DebugJob>>>, job_id: &str) {
    let mut jobs = debug_jobs.write().await;
    if let Some(job) = jobs.get_mut(job_id) {
        job.status = DebugJobStatus::Running;
        job.started_at = Some(Instant::now());
    }
}

async fn finish_job(
    debug_jobs: Arc<RwLock<HashMap<String, DebugJob>>>,
    job_id: &str,
    result: Result<String>,
    partial_output: Option<String>,
) {
    let mut jobs = debug_jobs.write().await;
    if let Some(job) = jobs.get_mut(job_id) {
        job.finished_at = Some(Instant::now());
        match result {
            Ok(output) => {
                job.status = DebugJobStatus::Completed;
                job.output = Some(output);
            }
            Err(error) => {
                job.status = DebugJobStatus::Failed;
                job.error = Some(error.to_string());
                if let Some(output) = partial_output {
                    job.output = Some(output);
                }
            }
        }
    }
}
