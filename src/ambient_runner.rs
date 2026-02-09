//! Background ambient mode runner.
//!
//! Spawned by the server when ambient mode is enabled. Manages the lifecycle of
//! ambient cycles: scheduling, spawning agent sessions, handling results, and
//! providing status for the TUI widget and debug socket.

use crate::agent::Agent;
use crate::ambient::{
    self, AmbientCycleResult, AmbientLock, AmbientManager, AmbientState, AmbientStatus,
    CycleStatus, ResourceBudget,
};
use crate::ambient_scheduler::{AdaptiveScheduler, AmbientSchedulerConfig};
use crate::config::config;
use crate::logging;
use crate::memory::MemoryManager;
use crate::provider::Provider;
use crate::safety::SafetySystem;
use crate::tool;
use crate::tool::ambient as ambient_tools;
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::{Notify, RwLock};

/// Shared ambient runner state, accessible from the server, debug socket, and TUI.
#[derive(Clone)]
pub struct AmbientRunnerHandle {
    inner: Arc<AmbientRunnerInner>,
}

struct AmbientRunnerInner {
    /// Current snapshot of ambient state (for queries)
    state: RwLock<AmbientState>,
    /// Queue item count for widget
    queue_count: RwLock<usize>,
    /// Next queue item context preview
    next_queue_preview: RwLock<Option<String>>,
    /// Wake notify (nudge the loop to re-check sooner)
    wake_notify: Notify,
    /// Whether the runner loop is active
    running: RwLock<bool>,
    /// Safety system shared with ambient tools
    safety: Arc<SafetySystem>,
    /// Number of active user sessions (for pause logic)
    active_user_sessions: RwLock<usize>,
}

impl AmbientRunnerHandle {
    pub fn new(safety: Arc<SafetySystem>) -> Self {
        let state = AmbientState::load().unwrap_or_default();
        Self {
            inner: Arc::new(AmbientRunnerInner {
                state: RwLock::new(state),
                queue_count: RwLock::new(0),
                next_queue_preview: RwLock::new(None),
                wake_notify: Notify::new(),
                running: RwLock::new(false),
                safety,
                active_user_sessions: RwLock::new(0),
            }),
        }
    }

    /// Nudge the ambient loop to check sooner (e.g., after session close/crash).
    pub fn nudge(&self) {
        self.inner.wake_notify.notify_one();
    }

    /// Update the count of active user sessions (for pause-on-active logic).
    pub async fn set_active_user_sessions(&self, count: usize) {
        *self.inner.active_user_sessions.write().await = count;
    }

    /// Get current ambient state snapshot.
    pub async fn state(&self) -> AmbientState {
        self.inner.state.read().await.clone()
    }

    /// Get queue count.
    pub async fn queue_count(&self) -> usize {
        *self.inner.queue_count.read().await
    }

    /// Get next queue preview.
    pub async fn next_queue_preview(&self) -> Option<String> {
        self.inner.next_queue_preview.read().await.clone()
    }

    /// Check if the runner loop is active.
    pub async fn is_running(&self) -> bool {
        *self.inner.running.read().await
    }

    /// Manually trigger an ambient cycle (returns immediately, cycle runs async).
    pub async fn trigger(&self) {
        // Set status to idle so should_run returns true
        let mut state = self.inner.state.write().await;
        if matches!(state.status, AmbientStatus::Scheduled { .. } | AmbientStatus::Idle) {
            state.status = AmbientStatus::Idle;
        }
        drop(state);
        self.inner.wake_notify.notify_one();
    }

    /// Stop the ambient loop.
    pub async fn stop(&self) {
        let mut state = self.inner.state.write().await;
        state.status = AmbientStatus::Disabled;
        let _ = state.save();
        drop(state);
        self.inner.wake_notify.notify_one();
    }

    /// Get status JSON for debug socket.
    pub async fn status_json(&self) -> String {
        let state = self.inner.state.read().await;
        let queue_count = *self.inner.queue_count.read().await;
        let next_preview = self.inner.next_queue_preview.read().await.clone();
        let running = *self.inner.running.read().await;
        let active_sessions = *self.inner.active_user_sessions.read().await;

        let status_str = match &state.status {
            AmbientStatus::Idle => "idle".to_string(),
            AmbientStatus::Running { detail } => format!("running: {}", detail),
            AmbientStatus::Scheduled { next_wake } => {
                let until = *next_wake - Utc::now();
                let mins = until.num_minutes().max(0);
                format!("scheduled (in {}m)", mins)
            }
            AmbientStatus::Paused { reason } => format!("paused: {}", reason),
            AmbientStatus::Disabled => "disabled".to_string(),
        };

        serde_json::json!({
            "enabled": config().ambient.enabled,
            "status": status_str,
            "loop_running": running,
            "total_cycles": state.total_cycles,
            "last_run": state.last_run.map(|t| t.to_rfc3339()),
            "last_summary": state.last_summary,
            "last_memories_modified": state.last_memories_modified,
            "last_compactions": state.last_compactions,
            "queue_count": queue_count,
            "next_queue_preview": next_preview,
            "active_user_sessions": active_sessions,
        })
        .to_string()
    }

    /// Get queue items JSON for debug socket.
    pub async fn queue_json(&self) -> String {
        match AmbientManager::new() {
            Ok(mgr) => {
                let items: Vec<serde_json::Value> = mgr
                    .queue()
                    .items()
                    .iter()
                    .map(|item| {
                        serde_json::json!({
                            "id": item.id,
                            "scheduled_for": item.scheduled_for.to_rfc3339(),
                            "context": item.context,
                            "priority": format!("{:?}", item.priority),
                            "created_at": item.created_at.to_rfc3339(),
                        })
                    })
                    .collect();
                serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string())
            }
            Err(e) => format!("{{\"error\": \"{}\"}}", e),
        }
    }

    /// Get recent transcript log summaries.
    pub async fn log_json(&self) -> String {
        let transcripts_dir = match crate::storage::jcode_dir() {
            Ok(d) => d.join("ambient").join("transcripts"),
            Err(e) => return format!("{{\"error\": \"{}\"}}", e),
        };

        if !transcripts_dir.exists() {
            return "[]".to_string();
        }

        let mut entries: Vec<serde_json::Value> = Vec::new();
        if let Ok(dir) = std::fs::read_dir(&transcripts_dir) {
            let mut files: Vec<_> = dir.flatten().collect();
            files.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
            files.truncate(20);

            for entry in files {
                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                    if let Ok(transcript) =
                        serde_json::from_str::<crate::safety::AmbientTranscript>(&content)
                    {
                        entries.push(serde_json::json!({
                            "session_id": transcript.session_id,
                            "started_at": transcript.started_at.to_rfc3339(),
                            "ended_at": transcript.ended_at.map(|t| t.to_rfc3339()),
                            "status": format!("{:?}", transcript.status),
                            "summary": transcript.summary,
                            "memories_modified": transcript.memories_modified,
                            "compactions": transcript.compactions,
                        }));
                    }
                }
            }
        }

        serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string())
    }

    /// Start the background ambient loop. Call from a tokio::spawn.
    pub async fn run_loop(self, provider: Arc<dyn Provider>) {
        {
            let mut running = self.inner.running.write().await;
            *running = true;
        }
        logging::info("Ambient runner: starting background loop");

        let amb_config = &config().ambient;
        let scheduler_config = AmbientSchedulerConfig {
            min_interval_minutes: amb_config.min_interval_minutes,
            max_interval_minutes: amb_config.max_interval_minutes,
            pause_on_active_session: amb_config.pause_on_active_session,
            ..Default::default()
        };
        let mut scheduler = AdaptiveScheduler::new(scheduler_config);

        // Initialize safety system for ambient tools
        ambient_tools::init_safety_system(Arc::clone(&self.inner.safety));

        loop {
            // Check if ambient is still enabled
            if !config().ambient.enabled {
                logging::info("Ambient runner: ambient mode disabled, exiting loop");
                break;
            }

            // Check state
            let state = {
                self.inner.state.read().await.clone()
            };

            if matches!(state.status, AmbientStatus::Disabled) {
                logging::info("Ambient runner: status is Disabled, exiting loop");
                break;
            }

            // Update scheduler's user-active state
            let active_sessions = *self.inner.active_user_sessions.read().await;
            scheduler.set_user_active(active_sessions > 0);

            // Check if we should pause
            if scheduler.should_pause() {
                let mut s = self.inner.state.write().await;
                s.status = AmbientStatus::Paused {
                    reason: "user session active".to_string(),
                };
                drop(s);

                // Sleep until nudged or 60s
                tokio::select! {
                    _ = self.inner.wake_notify.notified() => {},
                    _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {},
                }
                continue;
            }

            // Load manager to check should_run and update queue info
            let should_run = match AmbientManager::new() {
                Ok(mgr) => {
                    // Update queue info for widget
                    {
                        let mut qc = self.inner.queue_count.write().await;
                        *qc = mgr.queue().len();
                    }
                    {
                        let mut qp = self.inner.next_queue_preview.write().await;
                        *qp = mgr.queue().peek_next().map(|i| i.context.clone());
                    }
                    mgr.should_run()
                }
                Err(e) => {
                    logging::error(&format!("Ambient runner: failed to load manager: {}", e));
                    false
                }
            };

            if !should_run {
                // Calculate sleep interval
                let interval = scheduler.calculate_interval(None);
                let sleep_secs = interval.as_secs().max(30);

                logging::info(&format!(
                    "Ambient runner: not time to run, sleeping {}s",
                    sleep_secs
                ));

                tokio::select! {
                    _ = self.inner.wake_notify.notified() => {
                        logging::info("Ambient runner: nudged awake");
                    },
                    _ = tokio::time::sleep(std::time::Duration::from_secs(sleep_secs)) => {},
                }
                continue;
            }

            // Try to acquire lock
            let lock = match AmbientLock::try_acquire() {
                Ok(Some(lock)) => lock,
                Ok(None) => {
                    logging::info("Ambient runner: another instance holds the lock, waiting");
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    continue;
                }
                Err(e) => {
                    logging::error(&format!("Ambient runner: lock error: {}", e));
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    continue;
                }
            };

            // Run a cycle
            logging::info("Ambient runner: starting ambient cycle");
            {
                let mut s = self.inner.state.write().await;
                s.status = AmbientStatus::Running {
                    detail: "starting cycle".to_string(),
                };
            }

            let cycle_result = self.run_cycle(&provider).await;

            match cycle_result {
                Ok(result) => {
                    logging::info(&format!(
                        "Ambient cycle complete: {} memories modified, {} compactions",
                        result.memories_modified, result.compactions
                    ));

                    // Update state
                    if let Ok(mut mgr) = AmbientManager::new() {
                        let _ = mgr.record_cycle_result(result.clone());
                    }
                    let mut s = self.inner.state.write().await;
                    s.record_cycle(&result);
                    let _ = s.save();

                    scheduler.on_successful_cycle();

                    // Save transcript
                    let transcript = crate::safety::AmbientTranscript {
                        session_id: format!("ambient_{}", Utc::now().format("%Y%m%d_%H%M%S")),
                        started_at: result.started_at,
                        ended_at: Some(result.ended_at),
                        status: match result.status {
                            CycleStatus::Complete => crate::safety::TranscriptStatus::Complete,
                            CycleStatus::Interrupted => {
                                crate::safety::TranscriptStatus::Interrupted
                            }
                            CycleStatus::Incomplete => crate::safety::TranscriptStatus::Incomplete,
                        },
                        provider: provider.name().to_string(),
                        model: provider.model(),
                        actions: Vec::new(),
                        pending_permissions: self.inner.safety.pending_requests().len(),
                        summary: Some(result.summary.clone()),
                        compactions: result.compactions,
                        memories_modified: result.memories_modified,
                    };
                    let _ = self.inner.safety.save_transcript(&transcript);
                }
                Err(e) => {
                    logging::error(&format!("Ambient cycle failed: {}", e));
                    scheduler.on_rate_limit_hit();

                    let mut s = self.inner.state.write().await;
                    s.status = AmbientStatus::Idle;
                    let _ = s.save();
                }
            }

            // Release lock
            let _ = lock.release();

            // Calculate next sleep interval
            let interval = scheduler.calculate_interval(None);
            let sleep_secs = interval.as_secs().max(30);

            // Update state with scheduled wake
            {
                let mut s = self.inner.state.write().await;
                if matches!(s.status, AmbientStatus::Running { .. } | AmbientStatus::Idle) {
                    s.status = AmbientStatus::Scheduled {
                        next_wake: Utc::now()
                            + chrono::Duration::seconds(sleep_secs as i64),
                    };
                    let _ = s.save();
                }
            }

            logging::info(&format!(
                "Ambient runner: next cycle in {}s",
                sleep_secs
            ));

            tokio::select! {
                _ = self.inner.wake_notify.notified() => {
                    logging::info("Ambient runner: nudged awake after cycle");
                },
                _ = tokio::time::sleep(std::time::Duration::from_secs(sleep_secs)) => {},
            }
        }

        {
            let mut running = self.inner.running.write().await;
            *running = false;
        }
        logging::info("Ambient runner: loop exited");
    }

    /// Run a single ambient cycle. Returns the cycle result.
    async fn run_cycle(&self, provider: &Arc<dyn Provider>) -> anyhow::Result<AmbientCycleResult> {
        let started_at = Utc::now();

        // Fork provider for this cycle
        let cycle_provider = provider.fork();

        // Create tool registry with ambient tools
        let registry = tool::Registry::new(cycle_provider.clone()).await;
        registry.register_ambient_tools().await;

        // Create agent with ambient system prompt
        let mut agent = Agent::new(cycle_provider.clone(), registry);
        agent.set_debug(true);

        // Gather data for system prompt
        let state = self.inner.state.read().await.clone();

        let mgr = AmbientManager::new()?;
        let queue_items: Vec<_> = mgr.queue().items().to_vec();

        let memory_manager = MemoryManager::new();
        let graph_health = ambient::gather_memory_graph_health(&memory_manager);

        let recent_sessions = ambient::gather_recent_sessions(state.last_run);

        // TODO: gather actual feedback memories from memory graph
        let feedback_memories: Vec<String> = Vec::new();

        let budget = ResourceBudget {
            provider: cycle_provider.name().to_string(),
            tokens_remaining_desc: "unknown (adaptive)".to_string(),
            window_resets_desc: "unknown".to_string(),
            user_usage_rate_desc: "estimated from history".to_string(),
            cycle_budget_desc: "stay under 50k tokens".to_string(),
        };

        let active_sessions = *self.inner.active_user_sessions.read().await;

        let system_prompt = ambient::build_ambient_system_prompt(
            &state,
            &queue_items,
            &graph_health,
            &recent_sessions,
            &feedback_memories,
            &budget,
            active_sessions,
        );

        // Set system prompt on agent
        agent.set_system_prompt(&system_prompt);

        // Run the agent with the initial message
        let initial_message = "Begin your ambient cycle. Check the scheduled queue, assess memory graph health, and plan your work using the todos tool.";

        // Clear any previous cycle result
        ambient_tools::take_cycle_result();

        // Run agent turn
        let run_result = agent.run_once_capture(initial_message).await;

        // Check if end_ambient_cycle was called
        if let Some(result) = ambient_tools::take_cycle_result() {
            // Agent called end_ambient_cycle properly
            return Ok(AmbientCycleResult {
                started_at,
                ended_at: Utc::now(),
                ..result
            });
        }

        // Agent didn't call end_ambient_cycle - handle unexpected stop
        if run_result.is_err() {
            logging::warn("Ambient cycle: agent error without calling end_ambient_cycle");
        }

        // Send continuation message
        logging::info("Ambient cycle: sending continuation message (no end_ambient_cycle called)");
        let continuation = "You stopped unexpectedly without calling end_ambient_cycle. \
            If you are done with your work, call end_ambient_cycle with a summary of \
            what you accomplished and schedule your next wake. \
            If you are not done, continue what you were doing.";

        let _ = agent.run_once_capture(continuation).await;

        // Check again
        if let Some(result) = ambient_tools::take_cycle_result() {
            return Ok(AmbientCycleResult {
                started_at,
                ended_at: Utc::now(),
                ..result
            });
        }

        // Still no end_ambient_cycle after two attempts - generate partial result
        logging::warn("Ambient cycle: forced end after 2 attempts without end_ambient_cycle");
        Ok(AmbientCycleResult {
            summary: "Cycle ended without calling end_ambient_cycle (forced end after 2 attempts)"
                .to_string(),
            memories_modified: 0,
            compactions: 0,
            proactive_work: None,
            next_schedule: None,
            started_at,
            ended_at: Utc::now(),
            status: CycleStatus::Incomplete,
        })
    }
}
