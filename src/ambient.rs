use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::config::config;
use crate::storage;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Ambient mode status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AmbientStatus {
    Idle,
    Running { detail: String },
    Scheduled { next_wake: DateTime<Utc> },
    Paused { reason: String },
    Disabled,
}

impl Default for AmbientStatus {
    fn default() -> Self {
        Self::Idle
    }
}

/// Priority for scheduled items
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low,
    Normal,
    High,
}

/// A scheduled ambient task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledItem {
    pub id: String,
    pub scheduled_for: DateTime<Utc>,
    pub context: String,
    pub priority: Priority,
    pub created_by_session: String,
    pub created_at: DateTime<Utc>,
}

/// Persistent ambient state
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AmbientState {
    pub status: AmbientStatus,
    pub last_run: Option<DateTime<Utc>>,
    pub last_summary: Option<String>,
    pub last_compactions: Option<u32>,
    pub last_memories_modified: Option<u32>,
    pub total_cycles: u64,
}

/// Result from an ambient cycle
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmbientCycleResult {
    pub summary: String,
    pub memories_modified: u32,
    pub compactions: u32,
    pub proactive_work: Option<String>,
    pub next_schedule: Option<ScheduleRequest>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub status: CycleStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CycleStatus {
    Complete,
    Interrupted,
    Incomplete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleRequest {
    pub wake_in_minutes: Option<u32>,
    pub wake_at: Option<DateTime<Utc>>,
    pub context: String,
    pub priority: Priority,
}

// ---------------------------------------------------------------------------
// Storage paths
// ---------------------------------------------------------------------------

fn ambient_dir() -> Result<PathBuf> {
    let dir = storage::jcode_dir()?.join("ambient");
    storage::ensure_dir(&dir)?;
    Ok(dir)
}

fn state_path() -> Result<PathBuf> {
    Ok(ambient_dir()?.join("state.json"))
}

fn queue_path() -> Result<PathBuf> {
    Ok(ambient_dir()?.join("queue.json"))
}

fn lock_path() -> Result<PathBuf> {
    Ok(ambient_dir()?.join("ambient.lock"))
}

fn transcripts_dir() -> Result<PathBuf> {
    let dir = ambient_dir()?.join("transcripts");
    storage::ensure_dir(&dir)?;
    Ok(dir)
}

// ---------------------------------------------------------------------------
// AmbientState persistence
// ---------------------------------------------------------------------------

impl AmbientState {
    pub fn load() -> Result<Self> {
        let path = state_path()?;
        if path.exists() {
            storage::read_json(&path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        storage::write_json(&state_path()?, self)
    }

    pub fn record_cycle(&mut self, result: &AmbientCycleResult) {
        self.last_run = Some(result.ended_at);
        self.last_summary = Some(result.summary.clone());
        self.last_compactions = Some(result.compactions);
        self.last_memories_modified = Some(result.memories_modified);
        self.total_cycles += 1;

        match result.status {
            CycleStatus::Complete => {
                if let Some(ref req) = result.next_schedule {
                    let next = req
                        .wake_at
                        .unwrap_or_else(|| {
                            Utc::now()
                                + chrono::Duration::minutes(
                                    req.wake_in_minutes.unwrap_or(30) as i64,
                                )
                        });
                    self.status = AmbientStatus::Scheduled { next_wake: next };
                } else {
                    self.status = AmbientStatus::Idle;
                }
            }
            CycleStatus::Interrupted | CycleStatus::Incomplete => {
                self.status = AmbientStatus::Idle;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ScheduledQueue
// ---------------------------------------------------------------------------

pub struct ScheduledQueue {
    items: Vec<ScheduledItem>,
    path: PathBuf,
}

impl ScheduledQueue {
    pub fn load(path: PathBuf) -> Self {
        let items: Vec<ScheduledItem> = if path.exists() {
            storage::read_json(&path).unwrap_or_default()
        } else {
            Vec::new()
        };
        Self { items, path }
    }

    pub fn save(&self) -> Result<()> {
        storage::write_json(&self.path, &self.items)
    }

    pub fn push(&mut self, item: ScheduledItem) {
        self.items.push(item);
        let _ = self.save();
    }

    /// Pop items whose `scheduled_for` is in the past, sorted by priority
    /// (highest first) then by time (earliest first).
    pub fn pop_ready(&mut self) -> Vec<ScheduledItem> {
        let now = Utc::now();
        let (ready, remaining): (Vec<_>, Vec<_>) =
            self.items.drain(..).partition(|i| i.scheduled_for <= now);

        self.items = remaining;

        let mut ready = ready;
        // Sort: highest priority first, then earliest scheduled_for
        ready.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.scheduled_for.cmp(&b.scheduled_for))
        });

        if !ready.is_empty() {
            let _ = self.save();
        }

        ready
    }

    pub fn peek_next(&self) -> Option<&ScheduledItem> {
        self.items
            .iter()
            .min_by_key(|i| i.scheduled_for)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[ScheduledItem] {
        &self.items
    }
}

// ---------------------------------------------------------------------------
// AmbientLock  (single-instance guard)
// ---------------------------------------------------------------------------

pub struct AmbientLock {
    lock_path: PathBuf,
}

impl AmbientLock {
    /// Try to acquire the ambient lock.
    /// Returns `Ok(Some(lock))` if acquired, `Ok(None)` if another instance
    /// already holds it, or `Err` on I/O failure.
    pub fn try_acquire() -> Result<Option<Self>> {
        let path = lock_path()?;

        // Check existing lock
        if path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&path) {
                if let Ok(pid) = contents.trim().parse::<u32>() {
                    if is_pid_alive(pid) {
                        return Ok(None); // Another instance is running
                    }
                    // Stale lock from a dead process — remove it
                }
            }
            let _ = std::fs::remove_file(&path);
        }

        // Write our PID
        let pid = std::process::id();
        if let Some(parent) = path.parent() {
            storage::ensure_dir(parent)?;
        }
        std::fs::write(&path, pid.to_string())?;

        Ok(Some(Self { lock_path: path }))
    }

    pub fn release(self) -> Result<()> {
        let _ = std::fs::remove_file(&self.lock_path);
        // Drop runs, but we already cleaned up
        std::mem::forget(self);
        Ok(())
    }
}

impl Drop for AmbientLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

#[cfg(unix)]
fn is_pid_alive(pid: u32) -> bool {
    // kill(pid, 0) checks if the process exists without sending a signal
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
fn is_pid_alive(_pid: u32) -> bool {
    // Conservative: assume alive on non-Unix
    true
}

// ---------------------------------------------------------------------------
// AmbientManager
// ---------------------------------------------------------------------------

pub struct AmbientManager {
    state: AmbientState,
    queue: ScheduledQueue,
}

impl AmbientManager {
    pub fn new() -> Result<Self> {
        // Ensure storage layout exists
        let _ = ambient_dir()?;
        let _ = transcripts_dir()?;

        let state = AmbientState::load()?;
        let queue = ScheduledQueue::load(queue_path()?);

        Ok(Self { state, queue })
    }

    pub fn is_enabled() -> bool {
        config().ambient.enabled
    }

    /// Check whether it's time to run a cycle based on current state and queue.
    pub fn should_run(&self) -> bool {
        if !Self::is_enabled() {
            return false;
        }

        match &self.state.status {
            AmbientStatus::Disabled | AmbientStatus::Paused { .. } => false,
            AmbientStatus::Running { .. } => false, // already running
            AmbientStatus::Idle => true,
            AmbientStatus::Scheduled { next_wake } => Utc::now() >= *next_wake,
        }
    }

    pub fn record_cycle_result(&mut self, result: AmbientCycleResult) -> Result<()> {
        self.state.record_cycle(&result);
        self.state.save()?;

        // If the cycle produced a schedule request, enqueue it
        if let Some(ref req) = result.next_schedule {
            self.schedule(req.clone())?;
        }

        Ok(())
    }

    /// Add a schedule request to the queue. Returns the item ID.
    pub fn schedule(&mut self, request: ScheduleRequest) -> Result<String> {
        let id = format!("sched_{:08x}", rand::random::<u32>());
        let scheduled_for = request.wake_at.unwrap_or_else(|| {
            Utc::now()
                + chrono::Duration::minutes(request.wake_in_minutes.unwrap_or(30) as i64)
        });

        let item = ScheduledItem {
            id: id.clone(),
            scheduled_for,
            context: request.context,
            priority: request.priority,
            created_by_session: String::new(), // filled in by caller if needed
            created_at: Utc::now(),
        };

        self.queue.push(item);
        Ok(id)
    }

    pub fn state(&self) -> &AmbientState {
        &self.state
    }

    pub fn queue(&self) -> &ScheduledQueue {
        &self.queue
    }
}

// ---------------------------------------------------------------------------
// Ambient System Prompt Builder
// ---------------------------------------------------------------------------

/// Health stats for the memory graph, used in the ambient system prompt.
#[derive(Debug, Clone, Default)]
pub struct MemoryGraphHealth {
    pub total: usize,
    pub active: usize,
    pub inactive: usize,
    pub low_confidence: usize,
    pub contradictions: usize,
    pub missing_embeddings: usize,
    pub duplicate_candidates: usize,
    pub last_consolidation: Option<DateTime<Utc>>,
}

/// Summary of a recent session for the ambient prompt.
#[derive(Debug, Clone)]
pub struct RecentSessionInfo {
    pub id: String,
    pub status: String,
    pub topic: Option<String>,
    pub duration_secs: i64,
    pub extraction_status: String,
}

/// Resource budget info for the ambient prompt.
#[derive(Debug, Clone, Default)]
pub struct ResourceBudget {
    pub provider: String,
    pub tokens_remaining_desc: String,
    pub window_resets_desc: String,
    pub user_usage_rate_desc: String,
    pub cycle_budget_desc: String,
}

/// Gather memory graph health stats from the MemoryManager.
pub fn gather_memory_graph_health(
    memory_manager: &crate::memory::MemoryManager,
) -> MemoryGraphHealth {
    let mut health = MemoryGraphHealth::default();

    // Accumulate stats from project + global graphs
    for graph in [
        memory_manager.load_project_graph(),
        memory_manager.load_global_graph(),
    ]
    .into_iter()
    .flatten()
    {
        let active_count = graph.memories.values().filter(|m| m.active).count();
        let inactive_count = graph.memories.values().filter(|m| !m.active).count();
        health.total += graph.memories.len();
        health.active += active_count;
        health.inactive += inactive_count;

        // Low confidence: effective confidence < 0.1
        health.low_confidence += graph
            .memories
            .values()
            .filter(|m| m.active && m.effective_confidence() < 0.1)
            .count();

        // Missing embeddings
        health.missing_embeddings += graph
            .memories
            .values()
            .filter(|m| m.active && m.embedding.is_none())
            .count();

        // Count contradiction edges
        for edges in graph.edges.values() {
            for edge in edges {
                if matches!(edge.kind, crate::memory_graph::EdgeKind::Contradicts) {
                    health.contradictions += 1;
                }
            }
        }

        // Use last_cluster_update as a proxy for last consolidation
        if let Some(ts) = graph.metadata.last_cluster_update {
            match health.last_consolidation {
                Some(existing) if ts > existing => health.last_consolidation = Some(ts),
                None => health.last_consolidation = Some(ts),
                _ => {}
            }
        }
    }

    // Contradicts edges are bidirectional, so divide by 2
    health.contradictions /= 2;

    // Duplicate candidates would require embedding similarity scan;
    // placeholder for now — ambient agent will discover them during its cycle.
    health.duplicate_candidates = 0;

    health
}

/// Gather recent sessions since a given timestamp.
pub fn gather_recent_sessions(since: Option<DateTime<Utc>>) -> Vec<RecentSessionInfo> {
    let sessions_dir = match crate::storage::jcode_dir() {
        Ok(d) => d.join("sessions"),
        Err(_) => return Vec::new(),
    };
    if !sessions_dir.exists() {
        return Vec::new();
    }

    let cutoff = since.unwrap_or_else(|| Utc::now() - chrono::Duration::hours(24));

    let mut recent = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(session) = crate::session::Session::load(stem) {
                        // Skip debug sessions
                        if session.is_debug {
                            continue;
                        }
                        // Only include sessions updated after cutoff
                        if session.updated_at < cutoff {
                            continue;
                        }
                        let duration =
                            (session.updated_at - session.created_at).num_seconds().max(0);
                        let extraction = if session.messages.is_empty() {
                            "no messages"
                        } else {
                            // Heuristic: if session closed normally, assume extracted
                            match &session.status {
                                crate::session::SessionStatus::Closed => "extracted",
                                crate::session::SessionStatus::Crashed { .. } => "missed",
                                crate::session::SessionStatus::Active => "in progress",
                                _ => "unknown",
                            }
                        };
                        recent.push(RecentSessionInfo {
                            id: session.id.clone(),
                            status: session.status.display().to_string(),
                            topic: session.title.clone(),
                            duration_secs: duration,
                            extraction_status: extraction.to_string(),
                        });
                    }
                }
            }
        }
    }

    // Sort by most recent first (we don't have created_at easily, sort by id which embeds timestamp)
    recent.sort_by(|a, b| b.id.cmp(&a.id));
    recent.truncate(20); // Cap at 20 to keep prompt reasonable
    recent
}

/// Build the dynamic system prompt for an ambient cycle.
///
/// Populates the template from AMBIENT_MODE.md with real data from the
/// current state, queue, memory graph, sessions, and resource budget.
pub fn build_ambient_system_prompt(
    state: &AmbientState,
    queue: &[ScheduledItem],
    graph_health: &MemoryGraphHealth,
    recent_sessions: &[RecentSessionInfo],
    feedback_memories: &[String],
    budget: &ResourceBudget,
    active_user_sessions: usize,
) -> String {
    let mut prompt = String::with_capacity(4096);

    prompt.push_str(
        "You are the ambient agent for jcode. You operate autonomously without \
         user prompting. Your job is to maintain and improve the user's \
         development environment.\n\n",
    );

    // --- Current State ---
    prompt.push_str("## Current State\n");
    if let Some(last_run) = state.last_run {
        let ago = Utc::now() - last_run;
        let ago_str = format_duration_rough(ago);
        prompt.push_str(&format!(
            "- Last ambient cycle: {} ({} ago)\n",
            last_run.format("%Y-%m-%d %H:%M UTC"),
            ago_str,
        ));
    } else {
        prompt.push_str("- Last ambient cycle: never (first run)\n");
    }
    if active_user_sessions > 0 {
        prompt.push_str(&format!(
            "- Active user sessions: {}\n",
            active_user_sessions
        ));
    } else {
        prompt.push_str("- Active user sessions: none\n");
    }
    prompt.push_str(&format!("- Total cycles completed: {}\n", state.total_cycles));
    prompt.push('\n');

    // --- Scheduled Queue ---
    prompt.push_str("## Scheduled Queue\n");
    if queue.is_empty() {
        prompt.push_str("Empty -- do general ambient work.\n");
    } else {
        for item in queue {
            let age = Utc::now() - item.created_at;
            let priority = match item.priority {
                Priority::Low => "low",
                Priority::Normal => "normal",
                Priority::High => "HIGH",
            };
            prompt.push_str(&format!(
                "- [{}] {} (scheduled {} ago, priority: {})\n",
                item.id,
                item.context,
                format_duration_rough(age),
                priority,
            ));
        }
    }
    prompt.push('\n');

    // --- Recent Sessions ---
    prompt.push_str("## Recent Sessions (since last cycle)\n");
    if recent_sessions.is_empty() {
        prompt.push_str("No sessions since last cycle.\n");
    } else {
        for s in recent_sessions {
            let topic = s.topic.as_deref().unwrap_or("(no title)");
            let dur = format_duration_rough(chrono::Duration::seconds(s.duration_secs));
            prompt.push_str(&format!(
                "- {} | {} | {} | {} | extraction: {}\n",
                s.id, s.status, dur, topic, s.extraction_status,
            ));
        }
    }
    prompt.push('\n');

    // --- Memory Graph Health ---
    prompt.push_str("## Memory Graph Health\n");
    prompt.push_str(&format!(
        "- Total memories: {} ({} active, {} inactive)\n",
        graph_health.total, graph_health.active, graph_health.inactive,
    ));
    prompt.push_str(&format!(
        "- Memories with confidence < 0.1: {}\n",
        graph_health.low_confidence,
    ));
    prompt.push_str(&format!(
        "- Unresolved contradictions: {}\n",
        graph_health.contradictions,
    ));
    prompt.push_str(&format!(
        "- Memories without embeddings: {}\n",
        graph_health.missing_embeddings,
    ));
    if graph_health.duplicate_candidates > 0 {
        prompt.push_str(&format!(
            "- Duplicate candidates (similarity > 0.95): {}\n",
            graph_health.duplicate_candidates,
        ));
    } else {
        prompt.push_str("- Duplicate candidates: run embedding scan to detect\n");
    }
    if let Some(ts) = graph_health.last_consolidation {
        let ago = format_duration_rough(Utc::now() - ts);
        prompt.push_str(&format!("- Last consolidation: {} ago\n", ago));
    } else {
        prompt.push_str("- Last consolidation: never\n");
    }
    prompt.push('\n');

    // --- User Feedback History ---
    prompt.push_str("## User Feedback History\n");
    if feedback_memories.is_empty() {
        prompt.push_str("No feedback memories found about ambient mode yet.\n");
    } else {
        for mem in feedback_memories {
            prompt.push_str(&format!("- {}\n", mem));
        }
    }
    prompt.push('\n');

    // --- Resource Budget ---
    prompt.push_str("## Resource Budget\n");
    prompt.push_str(&format!("- Provider: {}\n", budget.provider));
    prompt.push_str(&format!(
        "- Tokens remaining in window: {}\n",
        budget.tokens_remaining_desc,
    ));
    prompt.push_str(&format!("- Window resets: {}\n", budget.window_resets_desc));
    prompt.push_str(&format!(
        "- User usage rate: {}\n",
        budget.user_usage_rate_desc,
    ));
    prompt.push_str(&format!(
        "- Budget for this cycle: {}\n",
        budget.cycle_budget_desc,
    ));
    prompt.push('\n');

    // --- Instructions ---
    prompt.push_str(
        "## Instructions\n\n\
         Start by using the todos tool to plan what you'll do this cycle.\n\n\
         Priority order:\n\
         1. Execute any scheduled queue items first.\n\
         2. Garden the memory graph -- consolidate duplicates, resolve \
            contradictions, prune dead memories, verify stale facts, \
            extract from missed sessions.\n\
         3. Scout for proactive work (only if enabled and past cold start) -- \
            look at recent sessions and git history to identify useful work \
            the user would appreciate.\n\n\
         For gardening: focus on highest-value maintenance first. Duplicates \
         and contradictions before pruning. Verify stale facts only if you \
         have budget left.\n\n\
         For proactive work: be conservative. A bad surprise is worse than \
         no surprise. Check the user feedback memories -- if they've rejected \
         similar work before, don't do it. Code changes must go on a worktree \
         branch with a PR via request_permission.\n\n\
         When done, you MUST call end_ambient_cycle with a summary of \
         everything you did, including compaction count. Always schedule \
         your next wake time with context for what you plan to do next.\n",
    );

    prompt
}

/// Format a chrono::Duration into a rough human-readable string.
fn format_duration_rough(d: chrono::Duration) -> String {
    let secs = d.num_seconds().max(0);
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m > 0 {
            format!("{}h {}m", h, m)
        } else {
            format!("{}h", h)
        }
    } else {
        let days = secs / 86400;
        format!("{}d", days)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_ambient_status_default() {
        let status = AmbientStatus::default();
        assert_eq!(status, AmbientStatus::Idle);
    }

    #[test]
    fn test_priority_ordering() {
        assert!(Priority::High > Priority::Normal);
        assert!(Priority::Normal > Priority::Low);
    }

    #[test]
    fn test_scheduled_queue_push_and_pop() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let mut queue = ScheduledQueue::load(path);
        assert!(queue.is_empty());

        let past = Utc::now() - Duration::minutes(5);
        let future = Utc::now() + Duration::hours(1);

        queue.push(ScheduledItem {
            id: "s1".into(),
            scheduled_for: past,
            context: "past item".into(),
            priority: Priority::Low,
            created_by_session: "test".into(),
            created_at: Utc::now(),
        });

        queue.push(ScheduledItem {
            id: "s2".into(),
            scheduled_for: future,
            context: "future item".into(),
            priority: Priority::High,
            created_by_session: "test".into(),
            created_at: Utc::now(),
        });

        assert_eq!(queue.len(), 2);

        let ready = queue.pop_ready();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "s1");

        // Future item still in queue
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.peek_next().unwrap().id, "s2");
    }

    #[test]
    fn test_pop_ready_sorts_by_priority_then_time() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let mut queue = ScheduledQueue::load(path);
        let past1 = Utc::now() - Duration::minutes(10);
        let past2 = Utc::now() - Duration::minutes(5);

        queue.push(ScheduledItem {
            id: "low_early".into(),
            scheduled_for: past1,
            context: "low early".into(),
            priority: Priority::Low,
            created_by_session: "test".into(),
            created_at: Utc::now(),
        });

        queue.push(ScheduledItem {
            id: "high_late".into(),
            scheduled_for: past2,
            context: "high late".into(),
            priority: Priority::High,
            created_by_session: "test".into(),
            created_at: Utc::now(),
        });

        let ready = queue.pop_ready();
        assert_eq!(ready.len(), 2);
        // High priority should come first
        assert_eq!(ready[0].id, "high_late");
        assert_eq!(ready[1].id, "low_early");
    }

    #[test]
    fn test_ambient_state_record_cycle() {
        let mut state = AmbientState::default();
        assert_eq!(state.total_cycles, 0);

        let result = AmbientCycleResult {
            summary: "Merged 2 duplicates".into(),
            memories_modified: 3,
            compactions: 1,
            proactive_work: None,
            next_schedule: None,
            started_at: Utc::now() - Duration::seconds(30),
            ended_at: Utc::now(),
            status: CycleStatus::Complete,
        };

        state.record_cycle(&result);
        assert_eq!(state.total_cycles, 1);
        assert_eq!(state.last_summary.as_deref(), Some("Merged 2 duplicates"));
        assert_eq!(state.last_compactions, Some(1));
        assert_eq!(state.last_memories_modified, Some(3));
        assert_eq!(state.status, AmbientStatus::Idle);
    }

    #[test]
    fn test_ambient_state_record_cycle_with_schedule() {
        let mut state = AmbientState::default();

        let result = AmbientCycleResult {
            summary: "Done".into(),
            memories_modified: 0,
            compactions: 0,
            proactive_work: None,
            next_schedule: Some(ScheduleRequest {
                wake_in_minutes: Some(15),
                wake_at: None,
                context: "check CI".into(),
                priority: Priority::Normal,
            }),
            started_at: Utc::now() - Duration::seconds(10),
            ended_at: Utc::now(),
            status: CycleStatus::Complete,
        };

        state.record_cycle(&result);
        assert!(matches!(state.status, AmbientStatus::Scheduled { .. }));
    }

    #[test]
    fn test_ambient_lock_release() {
        // Use a temp dir so we don't conflict with real state
        let tmp_dir = tempfile::tempdir().unwrap();
        let lock_file = tmp_dir.path().join("test.lock");

        // Manually create a lock to test release/drop
        std::fs::write(&lock_file, std::process::id().to_string()).unwrap();
        let lock = AmbientLock {
            lock_path: lock_file.clone(),
        };
        lock.release().unwrap();
        assert!(!lock_file.exists());
    }

    #[test]
    fn test_schedule_id_format() {
        let id = format!("sched_{:08x}", rand::random::<u32>());
        assert!(id.starts_with("sched_"));
        assert_eq!(id.len(), 6 + 8); // "sched_" + 8 hex chars
    }

    #[test]
    fn test_format_duration_rough() {
        assert_eq!(format_duration_rough(Duration::seconds(30)), "30s");
        assert_eq!(format_duration_rough(Duration::minutes(5)), "5m");
        assert_eq!(format_duration_rough(Duration::hours(2)), "2h");
        assert_eq!(
            format_duration_rough(Duration::hours(2) + Duration::minutes(30)),
            "2h 30m"
        );
        assert_eq!(format_duration_rough(Duration::days(3)), "3d");
        assert_eq!(format_duration_rough(Duration::seconds(-5)), "0s");
    }

    #[test]
    fn test_build_ambient_system_prompt_minimal() {
        let state = AmbientState::default();
        let queue = vec![];
        let health = MemoryGraphHealth::default();
        let sessions = vec![];
        let feedback: Vec<String> = vec![];
        let budget = ResourceBudget {
            provider: "anthropic-oauth".into(),
            tokens_remaining_desc: "unknown".into(),
            window_resets_desc: "unknown".into(),
            user_usage_rate_desc: "0 tokens/min".into(),
            cycle_budget_desc: "stay under 50k tokens".into(),
        };

        let prompt = build_ambient_system_prompt(
            &state, &queue, &health, &sessions, &feedback, &budget, 0,
        );

        assert!(prompt.contains("ambient agent for jcode"));
        assert!(prompt.contains("## Current State"));
        assert!(prompt.contains("never (first run)"));
        assert!(prompt.contains("Active user sessions: none"));
        assert!(prompt.contains("## Scheduled Queue"));
        assert!(prompt.contains("Empty"));
        assert!(prompt.contains("## Memory Graph Health"));
        assert!(prompt.contains("Total memories: 0"));
        assert!(prompt.contains("## User Feedback History"));
        assert!(prompt.contains("No feedback memories"));
        assert!(prompt.contains("## Resource Budget"));
        assert!(prompt.contains("anthropic-oauth"));
        assert!(prompt.contains("## Instructions"));
        assert!(prompt.contains("end_ambient_cycle"));
    }

    #[test]
    fn test_build_ambient_system_prompt_with_data() {
        let mut state = AmbientState::default();
        state.last_run = Some(Utc::now() - Duration::minutes(15));
        state.total_cycles = 7;

        let queue = vec![ScheduledItem {
            id: "sched_001".into(),
            scheduled_for: Utc::now(),
            context: "Check CI status".into(),
            priority: Priority::High,
            created_by_session: "session_abc".into(),
            created_at: Utc::now() - Duration::minutes(10),
        }];

        let health = MemoryGraphHealth {
            total: 42,
            active: 38,
            inactive: 4,
            low_confidence: 3,
            contradictions: 1,
            missing_embeddings: 5,
            duplicate_candidates: 0,
            last_consolidation: Some(Utc::now() - Duration::hours(2)),
        };

        let sessions = vec![RecentSessionInfo {
            id: "session_fox_123".into(),
            status: "closed".into(),
            topic: Some("Fix auth bug".into()),
            duration_secs: 900,
            extraction_status: "extracted".into(),
        }];

        let feedback = vec![
            "User approved ambient fixing typos in docs".into(),
            "User rejected ambient refactoring tests".into(),
        ];

        let budget = ResourceBudget {
            provider: "openai-oauth".into(),
            tokens_remaining_desc: "~85k".into(),
            window_resets_desc: "in 3h 20m".into(),
            user_usage_rate_desc: "120 tokens/min".into(),
            cycle_budget_desc: "stay under 15k tokens".into(),
        };

        let prompt =
            build_ambient_system_prompt(&state, &queue, &health, &sessions, &feedback, &budget, 2);

        assert!(prompt.contains("15m ago"));
        assert!(prompt.contains("Active user sessions: 2"));
        assert!(prompt.contains("Total cycles completed: 7"));
        assert!(prompt.contains("Check CI status"));
        assert!(prompt.contains("HIGH"));
        assert!(prompt.contains("42"));
        assert!(prompt.contains("38 active"));
        assert!(prompt.contains("confidence < 0.1: 3"));
        assert!(prompt.contains("contradictions: 1"));
        assert!(prompt.contains("without embeddings: 5"));
        assert!(prompt.contains("Fix auth bug"));
        assert!(prompt.contains("approved ambient fixing typos"));
        assert!(prompt.contains("rejected ambient refactoring"));
        assert!(prompt.contains("openai-oauth"));
        assert!(prompt.contains("~85k"));
    }

    #[test]
    fn test_scheduled_queue_items_accessor() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut queue = ScheduledQueue::load(path);

        queue.push(ScheduledItem {
            id: "s1".into(),
            scheduled_for: Utc::now(),
            context: "test item".into(),
            priority: Priority::Normal,
            created_by_session: "test".into(),
            created_at: Utc::now(),
        });

        let items = queue.items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "s1");
    }
}
