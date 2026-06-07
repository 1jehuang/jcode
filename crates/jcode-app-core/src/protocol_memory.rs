use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStateSnapshot {
    Idle,
    Embedding,
    SidecarChecking { count: usize },
    FoundRelevant { count: usize },
    Extracting { reason: String },
    Maintaining { phase: String },
    ToolAction { action: String, detail: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStepStatusSnapshot {
    Pending,
    Running,
    Done,
    Error,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryStepResultSnapshot {
    pub summary: String,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryPipelineSnapshot {
    pub search: MemoryStepStatusSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_result: Option<MemoryStepResultSnapshot>,
    pub verify: MemoryStepStatusSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_result: Option<MemoryStepResultSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_progress: Option<(usize, usize)>,
    pub inject: MemoryStepStatusSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inject_result: Option<MemoryStepResultSnapshot>,
    pub maintain: MemoryStepStatusSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintain_result: Option<MemoryStepResultSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryActivitySnapshot {
    pub state: MemoryStateSnapshot,
    #[serde(default)]
    pub state_age_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<MemoryPipelineSnapshot>,
}
