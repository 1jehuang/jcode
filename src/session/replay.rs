use chrono::{DateTime, Utc, Duration};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;
use serde::{Serialize, Deserialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct RecordedSession {
    pub id: Uuid,
    pub recorded_at: DateTime<Utc>,
    pub metadata: SessionMetadata,
    pub events: Vec<RecordedEvent>,
    pub initial_state: ProjectStateSnapshot,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub project_name: String,
    pub project_path: PathBuf,
    pub git_branch: Option<String>,
    pub git_commit: Option<String>,
    pub user_id: Option<String>,
    pub provider_model: Option<String>,
    pub total_duration: Duration,
    pub token_usage: TokenUsageStats,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TokenUsageStats {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub estimated_cost_usd: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RecordedEvent {
    UserInput { text: String, timestamp: i64 },
    ToolCall { tool: String, input: serde_json::Value, timestamp: i64 },
    ToolResult { output: ToolOutput, duration_ms: u64, timestamp: i64 },
    SystemMessage { content: String, timestamp: i64 },
    StateChange { field: String, old_value: Value, new_value: Value, timestamp: i64 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Value {
    String(String),
    Number(f64),
    Bool(bool),
    Null,
    Object(Vec<(String, Value)>),
    Array(Vec<Value>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Null, Value::Null) => true,
            (Value::Object(a), Value::Object(b)) => a.len() == b.len() && a.iter().all(|(k, v)| b.iter().any(|(bk, bv)| k == bk && v == bv)),
            (Value::Array(a), Value::Array(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Value {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    pub truncated: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectStateSnapshot {
    pub files: Vec<FileSnapshot>,
    pub environment_vars: Vec<(String, String)>,
    pub working_directory: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub path: PathBuf,
    pub content_hash: String,
    pub last_modified: DateTime<Utc>,
    pub size_bytes: u64,
}

#[derive(Clone)]
pub struct ReplayBranch {
    pub id: Uuid,
    pub name: String,
    pub parent_branch: Option<Uuid>,
    pub divergence_point: usize,
    pub modified_events: Vec<EventModification>,
    pub created_at: DateTime<Utc>,
    pub description: Option<String>,
}

#[derive(Clone)]
pub struct EventModification {
    pub event_index: usize,
    pub original_event: RecordedEvent,
    pub modified_event: RecordedEvent,
    pub reason: ModificationReason,
    pub modified_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub enum ModificationReason {
    TryDifferentApproach,
    FixError,
    Experiment,
    OptimizePerformance,
    AlternativeSolution,
}

#[derive(Clone)]
pub struct PlaybackState {
    pub current_event_index: usize,
    pub speed: PlaybackSpeed,
    pub paused: bool,
    pub mode: ReplayMode,
    pub auto_play_interval_ms: Option<u64>,
}

#[derive(Clone)]
pub enum PlaybackSpeed {
    RealTime,
    Fast { factor: u8 },
    StepByStep,
    Auto,
}

impl PlaybackSpeed {
    /// Get interval between events in ms based on speed
    pub fn interval_ms(&self) -> Option<u64> {
        match self {
            PlaybackSpeed::RealTime => Some(1000),
            PlaybackSpeed::Fast { factor } => Some((1000 / *factor as u64).max(50)),
            PlaybackSpeed::StepByStep => None, // Manual step
            PlaybackSpeed::Auto => Some(100),
        }
    }

    /// Cycle to next speed level
    pub fn next(&self) -> Self {
        match self {
            PlaybackSpeed::RealTime => PlaybackSpeed::Fast { factor: 2 },
            PlaybackSpeed::Fast { factor } if *factor < 8 => PlaybackSpeed::Fast { factor: factor * 2 },
            PlaybackSpeed::Fast { .. } => PlaybackSpeed::StepByStep,
            PlaybackSpeed::StepByStep => PlaybackSpeed::Auto,
            PlaybackSpeed::Auto => PlaybackSpeed::RealTime,
        }
    }
}

#[derive(Clone)]
pub enum ReplayMode {
    ViewOnly,
    Interactive,
    Compare,
}

impl ReplayMode {
    pub fn allows_modification(&self) -> bool {
        matches!(self, ReplayMode::Interactive)
    }

    pub fn next(&self) -> Self {
        match self {
            ReplayMode::ViewOnly => ReplayMode::Interactive,
            ReplayMode::Interactive => ReplayMode::Compare,
            ReplayMode::Compare => ReplayMode::ViewOnly,
        }
    }
}

pub struct SessionReplayer {
    original_session: RecordedSession,
    current_branch: ReplayBranch,
    branches: Vec<ReplayBranch>,
    playback_state: PlaybackState,
}

pub struct ReplayStepResult {
    pub event: Option<RecordedEvent>,
    pub index: usize,
    pub is_last: bool,
}

pub struct BranchInfo {
    pub id: Uuid,
    pub name: String,
    pub parent: Option<Uuid>,
    pub modification_count: usize,
    pub created_at: DateTime<Utc>,
}

pub struct BranchDiff {
    pub common_events: usize,
    pub divergent_events_a: Vec<EventModification>,
    pub divergent_events_b: Vec<EventModification>,
    pub similarity_score: f32,
}

pub struct MergeResult {
    pub success: bool,
    pub conflicts: Vec<MergeConflict>,
    pub merged_event_count: usize,
}

/// 合并冲突
#[derive(Debug, Clone)]
pub struct MergeConflict {
    pub event_index: usize,
    pub version_a: RecordedEvent,
    pub version_b: RecordedEvent,
}

pub enum ReplayExportFormat {
    InteractiveHtml,
    VideoGif,
    MarkdownTranscript,
    JsonLog,
}

#[derive(Debug)]
pub enum ReplayError {
    InvalidIndex,
    BranchNotFound,
    CannotModifyViewOnlyMode,
    MergeConflict(Vec<MergeConflict>),
    SerializationError(String),
    IoError(std::io::Error),
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayError::InvalidIndex => write!(f, "Invalid event index"),
            ReplayError::BranchNotFound => write!(f, "Branch not found"),
            ReplayError::CannotModifyViewOnlyMode => write!(f, "Cannot modify in view-only mode"),
            ReplayError::MergeConflict(_) => write!(f, "Merge conflict detected"),
            ReplayError::SerializationError(e) => write!(f, "Serialization error: {}", e),
            ReplayError::IoError(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for ReplayError {}

#[derive(Debug, Clone)]
pub enum MergeError {
    Conflicts(Vec<MergeConflict>),
    BranchNotFound,
    CircularDependency,
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeError::Conflicts(_) => write!(f, "Merge conflicts detected"),
            MergeError::BranchNotFound => write!(f, "Branch not found for merge"),
            MergeError::CircularDependency => write!(f, "Circular dependency detected"),
        }
    }
}

impl std::error::Error for MergeError {}

impl SessionReplayer {
    pub fn new(session: RecordedSession) -> Self {
        let main_branch = ReplayBranch {
            id: Uuid::new_v4(),
            name: "main".to_string(),
            parent_branch: None,
            divergence_point: 0,
            modified_events: vec![],
            created_at: Utc::now(),
            description: Some("Original session branch".to_string()),
        };
        SessionReplayer {
            original_session: session.clone(),
            current_branch: main_branch,
            branches: vec![],
            playback_state: PlaybackState {
                current_event_index: 0,
                speed: PlaybackSpeed::RealTime,
                paused: true,
                mode: ReplayMode::ViewOnly,
                auto_play_interval_ms: None,
            },
        }
    }

    pub fn from_file(path: &Path) -> Result<Self, ReplayError> {
        let data = fs::read(path).map_err(ReplayError::IoError)?;
        let session: RecordedSession =
            serde_json::from_slice(&data).map_err(|e| ReplayError::SerializationError(e.to_string()))?;
        Ok(Self::new(session))
    }

    pub fn save_to_file(&self, path: &Path) -> Result<(), ReplayError> {
        let data = serde_json::to_vec_pretty(&self.original_session)
            .map_err(|e| ReplayError::SerializationError(e.to_string()))?;
        fs::write(path, data).map_err(ReplayError::IoError)
    }

    pub fn play(&mut self) {
        self.playback_state.paused = false;
        self.playback_state.auto_play_interval_ms = self.playback_state.speed.interval_ms();
    }

    pub fn pause(&mut self) {
        self.playback_state.paused = true;
        self.playback_state.auto_play_interval_ms = None;
    }

    pub fn stop(&mut self) {
        self.playback_state.paused = true;
        self.playback_state.current_event_index = 0;
        self.playback_state.auto_play_interval_ms = None;
    }

    pub fn step_forward(&mut self) -> ReplayStepResult {
        let count = self.events_count();
        if count == 0 || self.playback_state.current_event_index >= count {
            return ReplayStepResult {
                event: None,
                index: self.playback_state.current_event_index,
                is_last: true,
            };
        }
        let idx = self.playback_state.current_event_index;
        self.playback_state.current_event_index += 1;
        let event = self.get_event(idx);
        ReplayStepResult {
            event,
            index: idx,
            is_last: self.is_at_end(),
        }
    }

    pub fn step_backward(&mut self) -> ReplayStepResult {
        if self.playback_state.current_event_index == 0 {
            return ReplayStepResult {
                event: None,
                index: 0,
                is_last: false,
            };
        }
        self.playback_state.current_event_index -= 1;
        let idx = self.playback_state.current_event_index;
        let event = self.get_event(idx);
        ReplayStepResult {
            event,
            index: idx,
            is_last: self.is_at_end(),
        }
    }

    pub fn jump_to(&mut self, event_index: usize) -> Result<(), ReplayError> {
        if event_index > self.events_count() {
            return Err(ReplayError::InvalidIndex);
        }
        self.playback_state.current_event_index = event_index;
        Ok(())
    }

    pub fn jump_to_end(&mut self) {
        self.playback_state.current_event_index = self.events_count();
    }

    pub fn jump_to_start(&mut self) {
        self.playback_state.current_event_index = 0;
    }

    pub fn create_branch(&mut self, name: &str) -> Uuid {
        self.create_branch_at(name, self.playback_state.current_event_index)
    }

    pub fn create_branch_at(&mut self, name: &str, event_index: usize) -> Uuid {
        let branch_id = Uuid::new_v4();
        let branch = ReplayBranch {
            id: branch_id,
            name: name.to_string(),
            parent_branch: Some(self.current_branch.id),
            divergence_point: event_index,
            modified_events: vec![],
            created_at: Utc::now(),
            description: None,
        };
        self.branches.push(branch);
        branch_id
    }

    pub fn switch_branch(&mut self, branch_id: Uuid) -> Result<(), ReplayError> {
        if self.current_branch.id == branch_id {
            return Ok(());
        }
        if branch_id == self.original_main_branch_id() {
            self.revert_modifications();
            return Ok(());
        }
        let branch = self.find_branch_mut(branch_id).ok_or(ReplayError::BranchNotFound)?;
        let restored = branch.clone();
        self.current_branch = restored;
        self.playback_state.current_event_index = self.current_branch.divergence_point;
        Ok(())
    }

    pub fn delete_branch(&mut self, branch_id: Uuid) -> Result<(), ReplayError> {
        if self.current_branch.id == branch_id {
            return Err(ReplayError::BranchNotFound);
        }
        let before = self.branches.len();
        self.branches.retain(|b| b.id != branch_id);
        if self.branches.len() == before {
            Err(ReplayError::BranchNotFound)
        } else {
            Ok(())
        }
    }

    pub fn list_branches(&self) -> Vec<BranchInfo> {
        let mut result = vec![BranchInfo {
            id: self.current_branch.id,
            name: self.current_branch.name.clone(),
            parent: self.current_branch.parent_branch,
            modification_count: self.current_branch.modified_events.len(),
            created_at: self.current_branch.created_at,
        }];
        for b in &self.branches {
            result.push(BranchInfo {
                id: b.id,
                name: b.name.clone(),
                parent: b.parent_branch,
                modification_count: b.modified_events.len(),
                created_at: b.created_at,
            });
        }
        result
    }

    pub fn merge_branch(&mut self, branch_id: Uuid) -> Result<MergeResult, MergeError> {
        let branch_id_clone = branch_id;
        let source_events = {
            let source = self.find_branch(branch_id_clone).ok_or(MergeError::BranchNotFound)?;
            source.modified_events.clone()
        };
        let source_event_count = source_events.len();
        if self.would_create_cycle(branch_id_clone) {
            return Err(MergeError::CircularDependency);
        }
        let mut conflicts = vec![];
        for mod_event in &source_events {
            if let Some(existing) = self
                .current_branch
                .modified_events
                .iter()
                .find(|m| m.event_index == mod_event.event_index)
            {
                if existing.modified_event != mod_event.modified_event {
                    conflicts.push(MergeConflict {
                        event_index: mod_event.event_index,
                        version_a: existing.modified_event.clone(),
                        version_b: mod_event.modified_event.clone(),
                    });
                }
            } else {
                self.current_branch.modified_events.push(mod_event.clone());
            }
        }
        if conflicts.is_empty() {
            Ok(MergeResult {
                success: true,
                conflicts: vec![],
                merged_event_count: source_event_count,
            })
        } else {
            Err(MergeError::Conflicts(conflicts))
        }
    }

    pub fn compare_branches(&self, branch_a: &Uuid, branch_b: &Uuid) -> BranchDiff {
        let mods_a = self.get_branch_modifications(branch_a);
        let mods_b = self.get_branch_modifications(branch_b);
        let common = self.events_count()
            - mods_a
                .iter()
                .map(|m| m.event_index)
                .collect::<std::collections::HashSet<_>>()
                .union(
                    &mods_b
                        .iter()
                        .map(|m| m.event_index)
                        .collect::<std::collections::HashSet<_>>(),
                )
                .count();
        let total_diff = mods_a.len().max(mods_b.len());
        let similarity = if total_diff == 0 {
            1.0
        } else {
            1.0 - (total_diff as f32 / self.events_count().max(1) as f32)
        };
        BranchDiff {
            common_events: common,
            divergent_events_a: mods_a,
            divergent_events_b: mods_b,
            similarity_score: similarity,
        }
    }

    pub fn modify_current_event(&mut self, new_event: RecordedEvent) -> Result<(), ReplayError> {
        if matches!(self.playback_state.mode, ReplayMode::ViewOnly) {
            return Err(ReplayError::CannotModifyViewOnlyMode);
        }
        let idx = self.playback_state.current_event_index;
        if idx >= self.events_count() {
            return Err(ReplayError::InvalidIndex);
        }
        let original = self.get_event(idx).unwrap().clone();
        let modification = EventModification {
            event_index: idx,
            original_event: original,
            modified_event: new_event,
            reason: ModificationReason::Experiment,
            modified_at: Utc::now(),
        };
        self.apply_or_update_modification(modification);
        Ok(())
    }

    pub fn insert_event(&mut self, index: usize, event: RecordedEvent) -> Result<(), ReplayError> {
        if matches!(self.playback_state.mode, ReplayMode::ViewOnly) {
            return Err(ReplayError::CannotModifyViewOnlyMode);
        }
        if index > self.events_count() {
            return Err(ReplayError::InvalidIndex);
        }
        let mut events = self.original_session.events.clone();
        self.apply_branch_modifications(&mut events);
        events.insert(index, event);
        self.original_session.events = events;
        self.shift_modifications_after(index, 1);
        Ok(())
    }

    pub fn remove_event(&mut self, index: usize) -> Result<(), ReplayError> {
        if matches!(self.playback_state.mode, ReplayMode::ViewOnly) {
            return Err(ReplayError::CannotModifyViewOnlyMode);
        }
        if index >= self.events_count() {
            return Err(ReplayError::InvalidIndex);
        }
        let mut events = self.original_session.events.clone();
        self.apply_branch_modifications(&mut events);
        events.remove(index);
        self.original_session.events = events;
        self.remove_modifications_at(index);
        self.shift_modifications_after(index, -1);
        if self.playback_state.current_event_index > 0 && self.playback_state.current_event_index >= index {
            self.playback_state.current_event_index =
                self.playback_state.current_event_index.saturating_sub(1);
        }
        Ok(())
    }

    pub fn revert_modifications(&mut self) {
        self.current_branch.modified_events.clear();
    }

    pub fn current_event(&self) -> Option<RecordedEvent> {
        self.get_event(self.playback_state.current_event_index)
    }

    pub fn get_event(&self, index: usize) -> Option<RecordedEvent> {
        self.effective_events().get(index).cloned()
    }

    pub fn events_count(&self) -> usize {
        self.effective_events().len()
    }

    pub fn is_at_end(&self) -> bool {
        self.events_count() == 0 || self.playback_state.current_event_index >= self.events_count()
    }

    pub fn is_at_start(&self) -> bool {
        self.playback_state.current_event_index == 0
    }

    pub fn progress_percent(&self) -> f32 {
        let count = self.events_count();
        if count == 0 {
            return 0.0;
        }
        (self.playback_state.current_event_index as f32 / count as f32) * 100.0
    }

    pub fn export_replay(&self, format: ReplayExportFormat) -> Result<Vec<u8>, ReplayError> {
        match format {
            ReplayExportFormat::JsonLog => {
                let data = serde_json::to_vec_pretty(&self.original_session)
                    .map_err(|e| ReplayError::SerializationError(e.to_string()))?;
                Ok(data)
            }
            ReplayExportFormat::MarkdownTranscript => {
                let md = self.generate_markdown_transcript();
                Ok(md.into_bytes())
            }
            ReplayExportFormat::InteractiveHtml => {
                let html = self.generate_interactive_html();
                Ok(html.into_bytes())
            }
            ReplayExportFormat::VideoGif => {
                let gif_data = self.generate_gif_placeholder();
                Ok(gif_data)
            }
        }
    }

    pub fn export_branch_diff(&self, branch_a: &Uuid, branch_b: &Uuid) -> Result<String, ReplayError> {
        let diff = self.compare_branches(branch_a, branch_b);
        let mut output = String::new();
        output.push_str(&format!("## Branch Comparison\n\n"));
        output.push_str(&format!(
            "- Common events: {}\n",
            diff.common_events
        ));
        output.push_str(&format!(
            "- Divergent events (A): {}\n",
            diff.divergent_events_a.len()
        ));
        output.push_str(&format!(
            "- Divergent events (B): {}\n",
            diff.divergent_events_b.len()
        ));
        output.push_str(&format!(
            "- Similarity: {:.1}%\n\n",
            diff.similarity_score * 100.0
        ));
        if !diff.divergent_events_a.is_empty() {
            output.push_str("### Branch A Modifications:\n");
            for m in &diff.divergent_events_a {
                output.push_str(&format!(
                    "  [{}] {:?} -> {:?}\n",
                    m.event_index,
                    self.event_type_name(&m.original_event),
                    self.event_type_name(&m.modified_event)
                ));
            }
        }
        if !diff.divergent_events_b.is_empty() {
            output.push_str("\n### Branch B Modifications:\n");
            for m in &diff.divergent_events_b {
                output.push_str(&format!(
                    "  [{}] {:?} -> {:?}\n",
                    m.event_index,
                    self.event_type_name(&m.original_event),
                    self.event_type_name(&m.modified_event)
                ));
            }
        }
        Ok(output)
    }

    fn original_main_branch_id(&self) -> Uuid {
        Uuid::nil()
    }

    fn find_branch(&self, id: Uuid) -> Option<&ReplayBranch> {
        self.branches.iter().find(|b| b.id == id)
    }

    fn find_branch_mut(&mut self, id: Uuid) -> Option<&mut ReplayBranch> {
        self.branches.iter_mut().find(|b| b.id == id)
    }

    fn would_create_cycle(&self, target_id: Uuid) -> bool {
        let mut visited = std::collections::HashSet::new();
        let mut current = target_id;
        loop {
            if !visited.insert(current) {
                return true;
            }
            if current == self.current_branch.id {
                break;
            }
            let found = self.find_branch(current);
            match found.and_then(|b| b.parent_branch) {
                Some(parent) => current = parent,
                None => break,
            }
        }
        false
    }

    fn get_branch_modifications(&self, branch_id: &Uuid) -> Vec<EventModification> {
        if branch_id == &self.current_branch.id {
            return self.current_branch.modified_events.clone();
        }
        self.find_branch(*branch_id)
            .map(|b| b.modified_events.clone())
            .unwrap_or_default()
    }

    fn apply_or_update_modification(&mut self, modification: EventModification) {
        if let Some(existing) = self
            .current_branch
            .modified_events
            .iter_mut()
            .find(|m| m.event_index == modification.event_index)
        {
            *existing = modification;
        } else {
            self.current_branch.modified_events.push(modification);
        }
    }

    fn shift_modifications_after(&mut self, from_index: usize, delta: i32) {
        for m in &mut self.current_branch.modified_events {
            if m.event_index > from_index {
                if delta > 0 {
                    m.event_index += delta as usize;
                } else {
                    m.event_index = m.event_index.saturating_sub(delta.unsigned_abs() as usize);
                }
            }
        }
        for branch in &mut self.branches {
            for m in &mut branch.modified_events {
                if m.event_index > from_index {
                    if delta > 0 {
                        m.event_index += delta as usize;
                    } else {
                        m.event_index = m.event_index.saturating_sub(delta.unsigned_abs() as usize);
                    }
                }
            }
        }
    }

    fn remove_modifications_at(&mut self, index: usize) {
        self.current_branch
            .modified_events
            .retain(|m| m.event_index != index);
    }

    fn effective_events(&self) -> Vec<RecordedEvent> {
        let mut events = self.original_session.events.clone();
        self.apply_branch_modifications(&mut events);
        events
    }

    fn apply_branch_modifications<'a>(&'a self, events: &mut Vec<RecordedEvent>) {
        for mod_event in &self.current_branch.modified_events {
            if mod_event.event_index < events.len() {
                events[mod_event.event_index] = mod_event.modified_event.clone();
            }
        }
    }

    fn generate_markdown_transcript(&self) -> String {
        let mut md = String::new();
        md.push_str("# Session Replay Transcript\n\n");
        md.push_str(&format!(
            "**Project:** {}\n",
            self.original_session.metadata.project_name
        ));
        md.push_str(&format!(
            "**Recorded:** {}\n",
            self.original_session.recorded_at.format("%Y-%m-%d %H:%M:%S UTC")
        ));
        md.push_str(&format!(
            "**Duration:** {:.1}s\n\n",
            self.original_session.metadata.total_duration.num_milliseconds() as f64 / 1000.0
        ));
        md.push_str("## Events\n\n");
        let events = self.effective_events();
        for (i, event) in events.iter().enumerate() {
            match event {
                RecordedEvent::UserInput { text, .. } => {
                    md.push_str(&format!("**[{}] 👤 User:** {}\n", i + 1, text));
                }
                RecordedEvent::ToolCall { tool, input, .. } => {
                    md.push_str(&format!(
                        "**[{}] 🔧 Tool Call:** `{}`\n```\n{:#}\n```\n",
                        i + 1,
                        tool,
                        input
                    ));
                }
                RecordedEvent::ToolResult { output, duration_ms, .. } => {
                    let icon = if output.is_error { "❌" } else { "✅" };
                    md.push_str(&format!(
                        "**[{}] {} Tool Result** ({}ms):\n{}\n",
                        i + 1, icon, duration_ms, output.content
                    ));
                }
                RecordedEvent::SystemMessage { content, .. } => {
                    md.push_str(&format!("**[{}] 📋 System:** {}\n", i + 1, content));
                }
                RecordedEvent::StateChange {
                    field,
                    old_value,
                    new_value,
                    ..
                } => {
                    md.push_str(&format!(
                        "**[{}] 🔄 State Change:** `{}`: `{:?}` -> `{:?}`\n",
                        i + 1, field, old_value, new_value
                    ));
                }
            }
            md.push('\n');
        }
        md
    }

    fn generate_interactive_html(&self) -> String {
        let events_json =
            serde_json::to_string(&self.effective_events()).unwrap_or_default();
        format!(
            r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Session Replay</title>
<style>
body{{font-family:monospace;margin:20px;background:#1e1e1e;color:#d4d4d4}}
.event{{padding:8px;border-left:3px solid #007acc;margin:4px 0}}
.user-input{{border-color:#4ec9b0}}
.tool-call{{border-color:#dcdcaa}}
.tool-result{{border-color:#ce9178}}
.system-msg{{border-color:#569cd6}}
.state-change{{border-color:#c586c0}}
.controls{{position:fixed;top:0;right:0;padding:10px;background:#333;z-index:100}}
button{{margin:2px;padding:4px 12px;cursor:pointer}}
</style></head><body>
<div class="controls">
<button onclick="stepBack()">◀ Step</button>
<button onclick="stepFwd()">Step ▶</button>
<button onclick="togglePlay()">▶/⏸</button>
<span id="pos">0/0</span>
</div>
<h1>Session Replay</h1>
<div id="events"></div>
<script>
const events={};
let idx=0;
function render(){{
const c=document.getElementById('events');
c.innerHTML=events.map((e,i)=>
`<div class="event ${{eventClass(e)}}">${{i+1}}. ${{eventText(e)}}</div>`
).join('\n');
document.getElementById('pos').textContent=(idx+1)+'/'+events.length;
}}
function eventClass(e){{return e.type?.toLowerCase()?.replace(/([A-Z])/g,'-$1')||'';}}
function eventText(e){{
if(e.UserInput)return '👤 '+e.UserInput.text;
if(e.ToolCall)return '🔧 '+e.ToolCall.tool;
if(e.ToolResult)return (e.ToolResult.output.is_error?'❌':'✅')+' '+e.ToolResult.output.content.slice(0,200);
if(e.SystemMessage)return '📋 '+e.SystemMessage.content;
if(e.StateChange)return '🔄 '+e.StateChange.field;
return JSON.stringify(e);
}}
function stepFwd(){{if(idx<events.length-1)idx++;render();}}
function stepBack(){{if(idx>0)idx--;render();}}
let playing=false;
function togglePlay(){{playing=!playing;if(playing)tick();}}
function tick(){{if(!playing)return;stepFwd();if(idx<events.length-1)setTimeout(tick,500);else playing=false;}}
render();
</script></body></html>"#,
            events_json
        )
    }

    fn generate_gif_placeholder(&self) -> Vec<u8> {
        let header: &[u8] = &[
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61,
            0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0xff, 0xff, 0xff,
            0x00, 0x00, 0x00, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00,
            0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00,
            0x02, 0x02, 0x44, 0x01, 0x00, 0x3b,
        ];
        header.to_vec()
    }

    fn event_type_name(&self, event: &RecordedEvent) -> &'static str {
        match event {
            RecordedEvent::UserInput { .. } => "UserInput",
            RecordedEvent::ToolCall { .. } => "ToolCall",
            RecordedEvent::ToolResult { .. } => "ToolResult",
            RecordedEvent::SystemMessage { .. } => "SystemMessage",
            RecordedEvent::StateChange { .. } => "StateChange",
        }
    }
}

#[cfg(test)]
fn make_test_session(event_count: usize) -> RecordedSession {
    let events: Vec<RecordedEvent> = (0..event_count)
        .map(|i| RecordedEvent::UserInput {
            text: format!("input_{}", i),
            timestamp: (i * 1000) as i64,
        })
        .collect();
    RecordedSession {
        id: Uuid::new_v4(),
        recorded_at: Utc::now(),
        metadata: SessionMetadata {
            project_name: "test-project".to_string(),
            project_path: PathBuf::from("/tmp/test"),
            git_branch: Some("main".to_string()),
            git_commit: Some("abc123".to_string()),
            user_id: Some("user1".to_string()),
            provider_model: Some("gpt-4".to_string()),
            total_duration: Duration::seconds(event_count as i64),
            token_usage: TokenUsageStats {
                input_tokens: 100,
                output_tokens: 200,
                cache_read_tokens: 50,
                estimated_cost_usd: Some(0.01),
            },
        },
        events,
        initial_state: ProjectStateSnapshot {
            files: vec![],
            environment_vars: vec![],
            working_directory: PathBuf::from("/tmp/test"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_replayer_with_valid_session() {
        let session = make_test_session(5);
        let replayer = SessionReplayer::new(session);
        assert_eq!(replayer.events_count(), 5);
        assert!(replayer.is_at_start());
        assert!(!replayer.is_at_end());
    }

    #[test]
    fn test_empty_session() {
        let session = make_test_session(0);
        let replayer = SessionReplayer::new(session);
        assert_eq!(replayer.events_count(), 0);
        assert!(replayer.is_at_end());
        assert!(replayer.is_at_start());
        assert_eq!(replayer.progress_percent(), 0.0);
    }

    #[test]
    fn test_single_event_session() {
        let session = make_test_session(1);
        let mut replayer = SessionReplayer::new(session);
        let result = replayer.step_forward();
        assert!(result.event.is_some());
        assert!(result.is_last);
        assert!(replayer.is_at_end());
    }

    #[test]
    fn test_step_forward_and_backward() {
        let session = make_test_session(3);
        let mut replayer = SessionReplayer::new(session);
        let r1 = replayer.step_forward();
        assert_eq!(r1.index, 0);
        assert!(!r1.is_last);
        let r2 = replayer.step_forward();
        assert_eq!(r2.index, 1);
        assert!(!r2.is_last);
        let r3 = replayer.step_forward();
        assert_eq!(r3.index, 2);
        assert!(r3.is_last);
        let rb = replayer.step_backward();
        assert_eq!(rb.index, 2);
        assert_eq!(replayer.playback_state.current_event_index, 2);
    }

    #[test]
    fn test_jump_to_event() {
        let session = make_test_session(10);
        let mut replayer = SessionReplayer::new(session);
        replayer.jump_to(7).unwrap();
        assert_eq!(replayer.playback_state.current_event_index, 7);
        assert!((replayer.progress_percent() - 70.0).abs() < 0.01);
    }

    #[test]
    fn test_jump_invalid_index() {
        let session = make_test_session(3);
        let mut replayer = SessionReplayer::new(session);
        let result = replayer.jump_to(10);
        assert!(result.is_err());
        matches!(result.unwrap_err(), ReplayError::InvalidIndex);
    }

    #[test]
    fn test_jump_to_end_and_start() {
        let session = make_test_session(5);
        let mut replayer = SessionReplayer::new(session);
        replayer.jump_to_end();
        assert!(replayer.is_at_end());
        replayer.jump_to_start();
        assert!(replayer.is_at_start());
    }

    #[test]
    fn test_play_pause_stop() {
        let session = make_test_session(5);
        let mut replayer = SessionReplayer::new(session);
        assert!(replayer.playback_state.paused);
        replayer.play();
        assert!(!replayer.playback_state.paused);
        replayer.pause();
        assert!(replayer.playback_state.paused);
        replayer.jump_to(3);
        replayer.stop();
        assert!(replayer.playback_state.paused);
        assert_eq!(replayer.playback_state.current_event_index, 0);
    }

    #[test]
    fn test_create_branch() {
        let session = make_test_session(5);
        let mut replayer = SessionReplayer::new(session);
        replayer.jump_to(2);
        let branch_id = replayer.create_branch("experiment");
        assert_ne!(branch_id, Uuid::nil());
        let branches = replayer.list_branches();
        assert_eq!(branches.len(), 2);
        assert!(branches.iter().any(|b| b.name == "experiment"));
    }

    #[test]
    fn test_create_branch_at_specific_point() {
        let session = make_test_session(10);
        let mut replayer = SessionReplayer::new(session);
        let branch_id = replayer.create_branch_at("fix", 5);
        let branches = replayer.list_branches();
        let fix_branch = branches.iter().find(|b| b.name == "fix").unwrap();
        assert_ne!(fix_branch.id, replayer.current_branch.id);
    }

    #[test]
    fn test_switch_branch() {
        let session = make_test_session(5);
        let mut replayer = SessionReplayer::new(session);
        let branch_id = replayer.create_branch_at("alt", 2);
        replayer.switch_branch(branch_id).unwrap();
        assert_eq!(replayer.playback_state.current_event_index, 2);
    }

    #[test]
    fn test_switch_nonexistent_branch() {
        let session = make_test_session(3);
        let mut replayer = SessionReplayer::new(session);
        let fake_id = Uuid::new_v4();
        let result = replayer.switch_branch(fake_id);
        assert!(result.is_err());
        matches!(result.unwrap_err(), ReplayError::BranchNotFound);
    }

    #[test]
    fn test_delete_branch() {
        let session = make_test_session(5);
        let mut replayer = SessionReplayer::new(session);
        let branch_id = replayer.create_branch("temp");
        assert_eq!(replayer.list_branches().len(), 2);
        replayer.delete_branch(branch_id).unwrap();
        assert_eq!(replayer.list_branches().len(), 1);
    }

    #[test]
    fn test_delete_nonexistent_branch() {
        let session = make_test_session(3);
        let mut replayer = SessionReplayer::new(session);
        let result = replayer.delete_branch(Uuid::new_v4());
        assert!(result.is_err());
    }

    #[test]
    fn test_modify_current_event_in_view_only_mode() {
        let session = make_test_session(3);
        let mut replayer = SessionReplayer::new(session);
        let new_event = RecordedEvent::SystemMessage {
            content: "modified".to_string(),
            timestamp: 999,
        };
        let result = replayer.modify_current_event(new_event);
        assert!(result.is_err());
        matches!(
            result.unwrap_err(),
            ReplayError::CannotModifyViewOnlyMode
        );
    }

    #[test]
    fn test_modify_current_event_in_interactive_mode() {
        let session = make_test_session(3);
        let mut replayer = SessionReplayer::new(session);
        replayer.playback_state.mode = ReplayMode::Interactive;
        let new_event = RecordedEvent::SystemMessage {
            content: "modified".to_string(),
            timestamp: 999,
        };
        replayer.modify_current_event(new_event).unwrap();
        assert_eq!(replayer.current_branch.modified_events.len(), 1);
        let ev = replayer.current_event().unwrap();
        matches!(ev, RecordedEvent::SystemMessage { .. });
    }

    #[test]
    fn test_insert_event() {
        let session = make_test_session(3);
        let mut replayer = SessionReplayer::new(session);
        replayer.playback_state.mode = ReplayMode::Interactive;
        let new_ev = RecordedEvent::SystemMessage {
            content: "inserted".to_string(),
            timestamp: 500,
        };
        replayer.insert_event(1, new_ev).unwrap();
        assert_eq!(replayer.events_count(), 4);
    }

    #[test]
    fn test_remove_event() {
        let session = make_test_session(3);
        let mut replayer = SessionReplayer::new(session);
        replayer.playback_state.mode = ReplayMode::Interactive;
        replayer.remove_event(1).unwrap();
        assert_eq!(replayer.events_count(), 2);
    }

    #[test]
    fn test_revert_modifications() {
        let session = make_test_session(3);
        let mut replayer = SessionReplayer::new(session);
        replayer.playback_state.mode = ReplayMode::Interactive;
        let new_ev = RecordedEvent::SystemMessage {
            content: "mod".to_string(),
            timestamp: 1,
        };
        replayer.modify_current_event(new_ev).unwrap();
        assert_eq!(replayer.current_branch.modified_events.len(), 1);
        replayer.revert_modifications();
        assert_eq!(replayer.current_branch.modified_events.len(), 0);
        matches!(
            replayer.current_event().unwrap(),
            RecordedEvent::UserInput { .. }
        );
    }

    #[test]
    fn test_compare_branches_no_divergence() {
        let session = make_test_session(5);
        let replayer = SessionReplayer::new(session);
        let main_id = replayer.current_branch.id;
        let diff = replayer.compare_branches(&main_id, &main_id);
        assert_eq!(diff.common_events, 5);
        assert!(diff.divergent_events_a.is_empty());
        assert!(diff.divergent_events_b.is_empty());
        assert!((diff.similarity_score - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_compare_branches_with_divergence() {
        let session = make_test_session(5);
        let mut replayer = SessionReplayer::new(session);
        replayer.playback_state.mode = ReplayMode::Interactive;
        let main_id = replayer.current_branch.id;
        let alt_id = replayer.create_branch_at("alt", 2);
        replayer.switch_branch(alt_id).unwrap();
        replayer.playback_state.mode = ReplayMode::Interactive;
        let mod_ev = RecordedEvent::SystemMessage {
            content: "branch change".to_string(),
            timestamp: 2000,
        };
        replayer.modify_current_event(mod_ev).unwrap();
        replayer.switch_branch(main_id).unwrap();
        let diff = replayer.compare_branches(&main_id, &alt_id);
        assert!(!diff.divergent_events_b.is_empty());
        assert!(diff.similarity_score < 1.0);
    }

    #[test]
    fn test_merge_branch_without_conflicts() {
        let session = make_test_session(5);
        let mut replayer = SessionReplayer::new(session);
        replayer.playback_state.mode = ReplayMode::Interactive;
        let branch_id = replayer.create_branch_at("feature", 1);
        replayer.switch_branch(branch_id).unwrap();
        replayer.playback_state.mode = ReplayMode::Interactive;
        let mod_ev = RecordedEvent::SystemMessage {
            content: "feature change".to_string(),
            timestamp: 1000,
        };
        replayer.modify_current_event(mod_ev).unwrap();
        replayer.switch_branch(replayer.current_branch.id).unwrap();
        let result = replayer.merge_branch(branch_id);
        assert!(result.is_ok());
        let merge_result = result.unwrap();
        assert!(merge_result.success);
        assert!(merge_result.conflicts.is_empty());
        assert_eq!(merge_result.merged_event_count, 1);
    }

    #[test]
    fn test_merge_branch_with_conflicts() {
        let session = make_test_session(5);
        let mut replayer = SessionReplayer::new(session);
        replayer.playback_state.mode = ReplayMode::Interactive;
        let branch_id = replayer.create_branch_at("conflict-branch", 0);
        replayer.switch_branch(branch_id).unwrap();
        replayer.playback_state.mode = ReplayMode::Interactive;
        let mod_ev_a = RecordedEvent::SystemMessage {
            content: "branch version".to_string(),
            timestamp: 1000,
        };
        replayer.modify_current_event(mod_ev_a).unwrap();
        replayer.switch_branch(replayer.current_branch.id).unwrap();
        let mod_ev_b = RecordedEvent::SystemMessage {
            content: "main version".to_string(),
            timestamp: 1001,
        };
        replayer.modify_current_event(mod_ev_b).unwrap();
        let result = replayer.merge_branch(branch_id);
        assert!(result.is_err());
        matches!(result.unwrap_err(), MergeError::Conflicts(_));
    }

    #[test]
    fn test_export_json_log() {
        let session = make_test_session(3);
        let replayer = SessionReplayer::new(session);
        let data = replayer.export_replay(ReplayExportFormat::JsonLog).unwrap();
        let parsed: RecordedSession = serde_json::from_slice(&data).unwrap();
        assert_eq!(parsed.events.len(), 3);
    }

    #[test]
    fn test_export_markdown_transcript() {
        let session = make_test_session(2);
        let replayer = SessionReplayer::new(session);
        let data = replayer.export_replay(ReplayExportFormat::MarkdownTranscript).unwrap();
        let text = String::from_utf8(data).unwrap();
        assert!(text.contains("# Session Replay Transcript"));
        assert!(text.contains("test-project"));
        assert!(text.contains("👤"));
    }

    #[test]
    fn test_export_html_contains_controls() {
        let session = make_test_session(2);
        let replayer = SessionReplayer::new(session);
        let data = replayer.export_replay(ReplayExportFormat::InteractiveHtml).unwrap();
        let text = String::from_utf8(data).unwrap();
        assert!(text.contains("<html"));
        assert!(text.contains("Step"));
        assert!(text.contains("session replay"));
    }

    #[test]
    fn test_export_gif_placeholder() {
        let session = make_test_session(1);
        let replayer = SessionReplayer::new(session);
        let data = replayer.export_replay(ReplayExportFormat::VideoGif).unwrap();
        assert!(data.starts_with(b"GIF89a"));
    }

    #[test]
    fn test_export_branch_diff_output() {
        let session = make_test_session(5);
        let mut replayer = SessionReplayer::new(session);
        let main_id = replayer.current_branch.id;
        let diff_text = replayer.export_branch_diff(&main_id, &main_id).unwrap();
        assert!(diff_text.contains("Branch Comparison"));
        assert!(diff_text.contains("Similarity: 100.0%"));
    }

    #[test]
    fn test_serialization_roundtrip() {
        let session = make_test_session(4);
        let json = serde_json::to_string(&session).unwrap();
        let loaded: RecordedSession = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.events.len(), 4);
        assert_eq!(loaded.metadata.project_name, "test-project");
    }

    #[test]
    fn test_save_and_load_from_file() {
        let session = make_test_session(3);
        let replayer = SessionReplayer::new(session);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_session.json");
        replayer.save_to_file(&path).unwrap();
        let loaded = SessionReplayer::from_file(&path).unwrap();
        assert_eq!(loaded.events_count(), 3);
        assert_eq!(
            loaded.original_session.metadata.project_name,
            "test-project"
        );
    }

    #[test]
    fn test_large_event_count_performance() {
        let session = make_test_session(10_000);
        let mut replayer = SessionReplayer::new(session);
        assert_eq!(replayer.events_count(), 10_000);
        replayer.jump_to(9999).unwrap();
        assert!((replayer.progress_percent() - 99.99).abs() < 0.1);
    }

    #[test]
    fn test_get_event_out_of_bounds() {
        let session = make_test_session(2);
        let replayer = SessionReplayer::new(session);
        assert!(replayer.get_event(5).is_none());
        assert!(replayer.get_event(100).is_none());
    }

    #[test]
    fn test_progress_percent_at_boundaries() {
        let session = make_test_session(4);
        let mut replayer = SessionReplayer::new(session);
        assert_eq!(replayer.progress_percent(), 0.0);
        replayer.jump_to_end();
        assert!((replayer.progress_percent() - 100.0).abs() < 0.01);
        replayer.jump_to(2);
        assert!((replayer.progress_percent() - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_all_recorded_event_variants_serializable() {
        let events = vec![
            RecordedEvent::UserInput { text: "hello".to_string(), timestamp: 1 },
            RecordedEvent::ToolCall {
                tool: "read_file".to_string(),
                input: serde_json::json!({"path": "/tmp/a.rs"}),
                timestamp: 2,
            },
            RecordedEvent::ToolResult {
                output: ToolOutput { content: "file contents".to_string(), is_error: false, truncated: false },
                duration_ms: 50,
                timestamp: 3,
            },
            RecordedEvent::SystemMessage { content: "starting".to_string(), timestamp: 4 },
            RecordedEvent::StateChange {
                field: "status".to_string(),
                old_value: Value::String("idle".to_string()),
                new_value: Value::String("running".to_string()),
                timestamp: 5,
            },
        ];
        let json = serde_json::to_string(&events).unwrap();
        let loaded: Vec<RecordedEvent> = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.len(), 5);
    }

    #[test]
    fn test_value_equality() {
        assert_eq!(Value::String("a".into()), Value::String("a".into()));
        assert_ne!(Value::String("a".into()), Value::String("b".into()));
        assert_eq!(Value::Number(1.0), Value::Number(1.0));
        assert_eq!(Value::Bool(true), Value::Bool(true));
        assert_eq!(Value::Null, Value::Null);
        assert_ne!(Value::Null, Value::Bool(false));
    }
}
