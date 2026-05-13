use std::collections::HashMap;
use chrono::{DateTime, Utc};

/// Pipeline execution report
#[derive(Debug, Clone)]
pub struct PipelineReport {
    pub stages: Vec<StageReport>,
    pub total_duration_ms: u64,
    pub success_count: u32,
    pub failure_count: u32,
    pub skipped_count: u32,
    pub created_at: DateTime<Utc>,
}

/// Single stage execution report
#[derive(Debug, Clone)]
pub struct StageReport {
    pub name: String,
    pub status: String,
    pub duration_ms: u64,
    pub log_summary: Vec<String>,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub artifacts: Vec<String>,
    pub output_vars: HashMap<String, String>,
}

impl PipelineReport {
    pub fn new() -> Self {
        PipelineReport {
            stages: vec![],
            total_duration_ms: 0,
            success_count: 0,
            failure_count: 0,
            skipped_count: 0,
            created_at: Utc::now(),
        }
    }

    pub fn add_stage(&mut self, report: StageReport) {
        match report.status.as_str() {
            "succeeded" => self.success_count += 1,
            "failed" => self.failure_count += 1,
            "skipped" => self.skipped_count += 1,
            _ => {}
        }
        self.stages.push(report);
    }

    pub fn summary(&self) -> String {
        format!(
            "Pipeline: {} succeeded, {} failed, {} skipped ({}ms)",
            self.success_count, self.failure_count, self.skipped_count, self.total_duration_ms
        )
    }

    pub fn has_failures(&self) -> bool {
        self.failure_count > 0
    }
}

impl StageReport {
    pub fn new(name: &str) -> Self {
        StageReport {
            name: name.to_string(),
            status: "pending".to_string(),
            duration_ms: 0,
            log_summary: vec![],
            error: None,
            warnings: vec![],
            artifacts: vec![],
            output_vars: HashMap::new(),
        }
    }
}