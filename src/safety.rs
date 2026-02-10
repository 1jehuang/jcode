use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use crate::notifications::NotificationDispatcher;
use crate::storage;

// ---------------------------------------------------------------------------
// Action classification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionTier {
    AutoAllowed,
    RequiresPermission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    Low,
    Normal,
    High,
}

// ---------------------------------------------------------------------------
// Permission request / result / decision
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub id: String,
    pub action: String,
    pub description: String,
    pub rationale: String,
    pub urgency: Urgency,
    pub wait: bool,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionResult {
    Approved { message: Option<String> },
    Denied { reason: Option<String> },
    Queued { request_id: String },
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub request_id: String,
    pub approved: bool,
    pub decided_at: DateTime<Utc>,
    pub decided_via: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// Action log / transcript
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionLog {
    pub action_type: String,
    pub description: String,
    pub tier: ActionTier,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptStatus {
    Complete,
    Interrupted,
    Incomplete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmbientTranscript {
    pub session_id: String,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    pub status: TranscriptStatus,
    pub provider: String,
    pub model: String,
    pub actions: Vec<ActionLog>,
    pub pending_permissions: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub compactions: u32,
    pub memories_modified: u32,
    /// Full conversation transcript (markdown) for email notifications
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
}

// ---------------------------------------------------------------------------
// Tier-1 (auto-allowed) action names
// ---------------------------------------------------------------------------

const AUTO_ALLOWED: &[&str] = &[
    "read",
    "glob",
    "grep",
    "ls",
    "memory",
    "remember",
    "todowrite",
    "todoread",
    "conversation_search",
    "session_search",
    "codesearch",
];

// ---------------------------------------------------------------------------
// SafetySystem
// ---------------------------------------------------------------------------

pub struct SafetySystem {
    queue: Mutex<Vec<PermissionRequest>>,
    history: Mutex<Vec<Decision>>,
    actions: Mutex<Vec<ActionLog>>,
    notifier: NotificationDispatcher,
}

impl SafetySystem {
    /// Create a new SafetySystem, loading persisted queue/history from disk.
    pub fn new() -> Self {
        let queue: Vec<PermissionRequest> = queue_path()
            .ok()
            .and_then(|p| storage::read_json(&p).ok())
            .unwrap_or_default();

        let history: Vec<Decision> = history_path()
            .ok()
            .and_then(|p| storage::read_json(&p).ok())
            .unwrap_or_default();

        SafetySystem {
            queue: Mutex::new(queue),
            history: Mutex::new(history),
            actions: Mutex::new(Vec::new()),
            notifier: NotificationDispatcher::new(),
        }
    }

    /// Classify an action name into a tier.
    pub fn classify(&self, action: &str) -> ActionTier {
        let lower = action.to_lowercase();
        if AUTO_ALLOWED.iter().any(|&a| a == lower) {
            ActionTier::AutoAllowed
        } else {
            ActionTier::RequiresPermission
        }
    }

    /// Submit a permission request. Returns `Queued` with the request id.
    pub fn request_permission(&self, request: PermissionRequest) -> PermissionResult {
        let request_id = request.id.clone();
        let action = request.action.clone();
        let description = request.description.clone();
        if let Ok(mut q) = self.queue.lock() {
            q.push(request);
            let _ = persist_queue(&q);
        }
        // Send high-priority notification for permission request
        self.notifier
            .dispatch_permission_request(&action, &description, &request_id);
        PermissionResult::Queued { request_id }
    }

    /// Record a user decision (approve / deny) for a pending request.
    pub fn record_decision(
        &self,
        request_id: &str,
        approved: bool,
        via: &str,
        message: Option<String>,
    ) -> Result<()> {
        // Remove from queue
        if let Ok(mut q) = self.queue.lock() {
            q.retain(|r| r.id != request_id);
            let _ = persist_queue(&q);
        }

        let decision = Decision {
            request_id: request_id.to_string(),
            approved,
            decided_at: Utc::now(),
            decided_via: via.to_string(),
            message,
        };

        if let Ok(mut h) = self.history.lock() {
            h.push(decision);
            let _ = persist_history(&h);
        }

        Ok(())
    }

    /// Return all pending permission requests.
    pub fn pending_requests(&self) -> Vec<PermissionRequest> {
        self.queue.lock().map(|q| q.clone()).unwrap_or_default()
    }

    /// Append an action to the in-memory log.
    pub fn log_action(&self, log: ActionLog) {
        if let Ok(mut actions) = self.actions.lock() {
            actions.push(log);
        }
    }

    /// Generate a human-readable summary of logged actions.
    pub fn generate_summary(&self) -> String {
        let actions = self.actions.lock().map(|a| a.clone()).unwrap_or_default();
        let pending = self.pending_requests();

        let mut lines: Vec<String> = Vec::new();

        if actions.is_empty() && pending.is_empty() {
            return "No actions recorded.".to_string();
        }

        // Separate auto vs permission-required
        let auto: Vec<&ActionLog> = actions
            .iter()
            .filter(|a| a.tier == ActionTier::AutoAllowed)
            .collect();
        let perm: Vec<&ActionLog> = actions
            .iter()
            .filter(|a| a.tier == ActionTier::RequiresPermission)
            .collect();

        if !auto.is_empty() {
            lines.push("Done (auto-allowed):".to_string());
            for a in &auto {
                lines.push(format!("- {} — {}", a.action_type, a.description));
            }
        }

        if !perm.is_empty() {
            lines.push(String::new());
            lines.push("Done (with permission):".to_string());
            for a in &perm {
                lines.push(format!("- {} — {}", a.action_type, a.description));
            }
        }

        if !pending.is_empty() {
            lines.push(String::new());
            lines.push("Needs your review:".to_string());
            for r in &pending {
                lines.push(format!(
                    "- [{:?}] {} — {}",
                    r.urgency, r.action, r.description
                ));
            }
        }

        lines.join("\n")
    }

    /// Persist a transcript to ~/.jcode/ambient/transcripts/{timestamp}.json
    pub fn save_transcript(&self, transcript: &AmbientTranscript) -> Result<()> {
        let dir = storage::jcode_dir()?.join("ambient").join("transcripts");
        storage::ensure_dir(&dir)?;

        let filename = transcript.started_at.format("%Y-%m-%d-%H%M%S").to_string();
        let path = dir.join(format!("{}.json", filename));
        storage::write_json(&path, transcript)
    }
}

// ---------------------------------------------------------------------------
// Persistence helpers
// ---------------------------------------------------------------------------

fn queue_path() -> Result<std::path::PathBuf> {
    Ok(storage::jcode_dir()?.join("safety").join("queue.json"))
}

fn history_path() -> Result<std::path::PathBuf> {
    Ok(storage::jcode_dir()?.join("safety").join("history.json"))
}

fn persist_queue(queue: &[PermissionRequest]) -> Result<()> {
    let path = queue_path()?;
    storage::write_json(&path, queue)
}

fn persist_history(history: &[Decision]) -> Result<()> {
    let path = history_path()?;
    storage::write_json(&path, history)
}

// ---------------------------------------------------------------------------
// File-based permission decision (for IMAP poller / external callers)
// ---------------------------------------------------------------------------

/// Record a permission decision by directly manipulating the queue/history JSON files.
/// Used by the IMAP reply poller which doesn't have access to the live SafetySystem instance.
pub fn record_permission_via_file(
    request_id: &str,
    approved: bool,
    via: &str,
    message: Option<String>,
) -> Result<()> {
    let qp = queue_path()?;
    if let Some(parent) = qp.parent() {
        storage::ensure_dir(parent)?;
    }
    let mut queue: Vec<PermissionRequest> = if qp.exists() {
        storage::read_json(&qp).unwrap_or_default()
    } else {
        Vec::new()
    };
    queue.retain(|r| r.id != request_id);
    persist_queue(&queue)?;

    let hp = history_path()?;
    if let Some(parent) = hp.parent() {
        storage::ensure_dir(parent)?;
    }
    let mut history: Vec<Decision> = if hp.exists() {
        storage::read_json(&hp).unwrap_or_default()
    } else {
        Vec::new()
    };
    history.push(Decision {
        request_id: request_id.to_string(),
        approved,
        decided_at: Utc::now(),
        decided_via: via.to_string(),
        message,
    });
    persist_history(&history)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// ID generation helper
// ---------------------------------------------------------------------------

/// Generate a unique permission request id: `req_{timestamp}_{random}`
pub fn new_request_id() -> String {
    crate::id::new_id("req")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_auto_allowed() {
        let sys = SafetySystem::new();
        assert_eq!(sys.classify("read"), ActionTier::AutoAllowed);
        assert_eq!(sys.classify("glob"), ActionTier::AutoAllowed);
        assert_eq!(sys.classify("grep"), ActionTier::AutoAllowed);
        assert_eq!(sys.classify("ls"), ActionTier::AutoAllowed);
        assert_eq!(sys.classify("memory"), ActionTier::AutoAllowed);
        assert_eq!(sys.classify("remember"), ActionTier::AutoAllowed);
        assert_eq!(sys.classify("todowrite"), ActionTier::AutoAllowed);
        assert_eq!(sys.classify("todoread"), ActionTier::AutoAllowed);
        assert_eq!(sys.classify("conversation_search"), ActionTier::AutoAllowed);
        assert_eq!(sys.classify("session_search"), ActionTier::AutoAllowed);
        assert_eq!(sys.classify("codesearch"), ActionTier::AutoAllowed);
    }

    #[test]
    fn test_classify_requires_permission() {
        let sys = SafetySystem::new();
        assert_eq!(sys.classify("bash"), ActionTier::RequiresPermission);
        assert_eq!(sys.classify("write"), ActionTier::RequiresPermission);
        assert_eq!(sys.classify("edit"), ActionTier::RequiresPermission);
        assert_eq!(sys.classify("multiedit"), ActionTier::RequiresPermission);
        assert_eq!(sys.classify("patch"), ActionTier::RequiresPermission);
        assert_eq!(sys.classify("apply_patch"), ActionTier::RequiresPermission);
        assert_eq!(sys.classify("communicate"), ActionTier::RequiresPermission);
        assert_eq!(sys.classify("webfetch"), ActionTier::RequiresPermission);
        assert_eq!(sys.classify("websearch"), ActionTier::RequiresPermission);
        assert_eq!(sys.classify("unknown_tool"), ActionTier::RequiresPermission);
    }

    #[test]
    fn test_classify_case_insensitive() {
        let sys = SafetySystem::new();
        assert_eq!(sys.classify("Read"), ActionTier::AutoAllowed);
        assert_eq!(sys.classify("GLOB"), ActionTier::AutoAllowed);
        assert_eq!(sys.classify("Bash"), ActionTier::RequiresPermission);
    }

    #[test]
    fn test_request_permission_returns_queued() {
        let sys = SafetySystem::new();
        let baseline = sys.pending_requests().len();
        let req = PermissionRequest {
            id: "req_test_1".to_string(),
            action: "create_pull_request".to_string(),
            description: "Create PR for test fixes".to_string(),
            rationale: "Found failing tests".to_string(),
            urgency: Urgency::Normal,
            wait: false,
            created_at: Utc::now(),
            context: None,
        };

        let result = sys.request_permission(req);
        match result {
            PermissionResult::Queued { request_id } => {
                assert_eq!(request_id, "req_test_1");
            }
            _ => panic!("Expected Queued result"),
        }

        assert_eq!(sys.pending_requests().len(), baseline + 1);
    }

    #[test]
    fn test_record_decision_removes_from_queue() {
        let sys = SafetySystem::new();
        let baseline = sys.pending_requests().len();
        let req = PermissionRequest {
            id: "req_test_2".to_string(),
            action: "push".to_string(),
            description: "Push to origin".to_string(),
            rationale: "Ready for review".to_string(),
            urgency: Urgency::Low,
            wait: false,
            created_at: Utc::now(),
            context: None,
        };

        sys.request_permission(req);
        assert_eq!(sys.pending_requests().len(), baseline + 1);

        sys.record_decision("req_test_2", true, "tui", Some("looks good".to_string()))
            .unwrap();
        assert_eq!(sys.pending_requests().len(), baseline);
    }

    #[test]
    fn test_log_action_and_summary() {
        let sys = SafetySystem::new();
        sys.log_action(ActionLog {
            action_type: "memory_consolidation".to_string(),
            description: "Merged 2 duplicate memories".to_string(),
            tier: ActionTier::AutoAllowed,
            details: None,
            timestamp: Utc::now(),
        });
        sys.log_action(ActionLog {
            action_type: "edit".to_string(),
            description: "Fixed typo in README".to_string(),
            tier: ActionTier::RequiresPermission,
            details: None,
            timestamp: Utc::now(),
        });

        let summary = sys.generate_summary();
        assert!(summary.contains("memory_consolidation"));
        assert!(summary.contains("edit"));
        assert!(summary.contains("Done (auto-allowed)"));
        assert!(summary.contains("Done (with permission)"));
    }

    #[test]
    fn test_empty_summary() {
        // generate_summary checks in-memory actions (always empty on new instance)
        // and pending queue (may have persisted items from other test runs).
        // Only assert when queue is truly empty.
        let sys = SafetySystem::new();
        if sys.pending_requests().is_empty() {
            let summary = sys.generate_summary();
            assert_eq!(summary, "No actions recorded.");
        }
    }

    #[test]
    fn test_new_request_id_format() {
        let id = new_request_id();
        assert!(id.starts_with("req_"));
    }

    #[test]
    fn test_record_permission_via_file() {
        // Seed a request into the queue via SafetySystem
        let sys = SafetySystem::new();
        let baseline = sys.pending_requests().len();
        let req = PermissionRequest {
            id: "req_file_test".to_string(),
            action: "push".to_string(),
            description: "Push to origin".to_string(),
            rationale: "Ready for review".to_string(),
            urgency: Urgency::Low,
            wait: false,
            created_at: Utc::now(),
            context: None,
        };
        sys.request_permission(req);
        assert_eq!(sys.pending_requests().len(), baseline + 1);

        // Use the file-based function to approve it
        record_permission_via_file("req_file_test", true, "email_reply", None).unwrap();

        // Reload from disk — the in-memory SafetySystem won't see the change,
        // but a fresh load should
        let sys2 = SafetySystem::new();
        let still_pending = sys2
            .pending_requests()
            .iter()
            .any(|r| r.id == "req_file_test");
        assert!(!still_pending, "request should have been removed from queue");
    }
}
