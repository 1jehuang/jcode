use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};

use super::stage::{PipelineStage, StageStatus, StageConfig};
use super::report::PipelineReport;
use super::artifact::ArtifactStore;
use super::cache::CacheManager;
use super::notification::NotificationService;

/// Unique pipeline identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PipelineId(pub String);

/// Pipeline status
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineStatus {
    Pending,
    Running,
    Succeeded,
    Failed(String),
    Cancelled,
    Skipped,
}

/// Pipeline configuration
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub id: PipelineId,
    pub name: String,
    pub description: String,
    pub stages: Vec<StageConfig>,
    pub working_directory: PathBuf,
    pub max_parallel_stages: usize,
    pub fail_fast: bool,
    pub notify_on_failure: bool,
    pub notify_on_success: bool,
    pub timeout_secs: u64,
    pub variables: HashMap<String, String>,
    pub cache_dirs: Vec<PathBuf>,
    pub artifact_paths: Vec<PathBuf>,
    pub triggers: Vec<PipelineTrigger>,
}

/// Pipeline trigger sources
#[derive(Debug, Clone)]
pub enum PipelineTrigger {
    Manual,
    GitPush { branch: String },
    GitTag { pattern: String },
    Schedule { cron: String },
    Webhook { url: String },
    Dependency { pipeline_id: PipelineId },
}

/// A running pipeline instance
#[derive(Debug, Clone)]
pub struct Pipeline {
    pub config: PipelineConfig,
    pub status: PipelineStatus,
    pub stages: Vec<PipelineStage>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub current_stage_index: usize,
    pub report: PipelineReport,
    pub artifacts: ArtifactStore,
    pub cache: CacheManager,
    pub notifications: NotificationService,
    pub variables: HashMap<String, String>,
    pub error_log: Vec<String>,
}

impl Pipeline {
    pub fn new(config: PipelineConfig) -> Self {
        let stages: Vec<PipelineStage> = config.stages.iter()
            .enumerate()
            .map(|(i, sc)| PipelineStage::new(i, sc.clone()))
            .collect();

        Pipeline {
            artifacts: ArtifactStore::new(config.artifact_paths.clone()),
            cache: CacheManager::new(config.cache_dirs.clone()),
            completed_at: None,
            config,
            current_stage_index: 0,
            error_log: vec![],
            notifications: NotificationService::new(),
            report: PipelineReport::new(),
            stages,
            started_at: None,
            status: PipelineStatus::Pending,
            variables: HashMap::new(),
        }
    }

    pub fn current_stage(&self) -> Option<&PipelineStage> {
        self.stages.get(self.current_stage_index)
    }

    pub fn current_stage_mut(&mut self) -> Option<&mut PipelineStage> {
        self.stages.get_mut(self.current_stage_index)
    }

    pub fn stage_by_name(&self, name: &str) -> Option<&PipelineStage> {
        self.stages.iter().find(|s| s.config.name == name)
    }

    pub fn stage_by_name_mut(&mut self, name: &str) -> Option<&mut PipelineStage> {
        self.stages.iter_mut().find(|s| s.config.name == name)
    }

    pub fn all_stages_completed(&self) -> bool {
        self.stages.iter().all(|s| s.status.is_terminal())
    }

    pub fn any_stage_failed(&self) -> bool {
        self.stages.iter().any(|s| matches!(s.status, StageStatus::Failed(_)))
    }

    pub fn duration_seconds(&self) -> Option<f64> {
        match (self.started_at, self.completed_at) {
            (Some(start), Some(end)) => Some((end - start).num_seconds() as f64),
            _ => None,
        }
    }
}

/// Pipeline runner that manages execution
pub struct PipelineRunner {
    active_pipelines: Arc<RwLock<HashMap<PipelineId, Pipeline>>>,
    max_concurrent: usize,
}

impl PipelineRunner {
    pub fn new(max_concurrent: usize) -> Self {
        PipelineRunner {
            active_pipelines: Arc::new(RwLock::new(HashMap::new())),
            max_concurrent,
        }
    }

    pub async fn register(&self, pipeline: Pipeline) -> PipelineId {
        let id = pipeline.config.id.clone();
        self.active_pipelines.write().await.insert(id.clone(), pipeline);
        id
    }

    pub async fn get(&self, id: &PipelineId) -> Option<Pipeline> {
        self.active_pipelines.read().await.get(id).cloned()
    }

    pub async fn list(&self) -> Vec<PipelineId> {
        self.active_pipelines.read().await.keys().cloned().collect()
    }

    pub async fn remove(&self, id: &PipelineId) {
        self.active_pipelines.write().await.remove(id);
    }
}