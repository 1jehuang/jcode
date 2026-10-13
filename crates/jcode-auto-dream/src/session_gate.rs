//! Session gate -- triggers consolidation after a minimum number of sessions.
//! Mirrors Claude Code's `listSessionsTouchedSince` which scans for
//! UUID-named `.jsonl` files with mtime after the last consolidation.

use std::path::Path;
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Configuration for the session gate threshold.
#[derive(Debug, Clone)]
pub struct SessionGateConfig {
    pub min_sessions: usize,
}

impl SessionGateConfig {
    pub fn new(min_sessions: usize) -> Self {
        Self { min_sessions }
    }
}

pub fn is_session_gate_open(session_ids: &[String], min_sessions: usize) -> bool {
    session_ids.len() >= min_sessions
}

/// List session IDs whose `mtime` is strictly after `since`.
/// Scans for `*.jsonl` files with UUID-shaped stems, excluding `agent-*.jsonl`.
pub fn list_sessions_touched_since(dir: &Path, since: SystemTime) -> Result<Vec<String>> {
    let mut sessions = Vec::new();
    let entries = std::fs::read_dir(dir).with_context(|| format!("read_dir: {}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        if stem.starts_with("agent-") || !looks_like_uuid(stem) {
            continue;
        }
        let mtime = match std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
        {
            Some(t) => t,
            None => continue,
        };
        if mtime > since {
            sessions.push(stem.to_owned());
        }
    }
    sessions.sort();
    sessions.dedup();
    Ok(sessions)
}

fn looks_like_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12].iter())
            .all(|(p, &len)| p.len() == len && p.chars().all(|c| c.is_ascii_hexdigit()))
}

// Stateful SessionGate (used by DreamCoordinator in lib.rs)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionGateState {
    pub sessions_since: u32,
    #[serde(default)]
    pub total_sessions: u64,
}
#[derive(Debug)]
pub struct SessionGate {
    threshold: u32,
}

impl SessionGate {
    pub fn new(threshold: u32) -> Self {
        Self { threshold }
    }
    pub fn threshold(&self) -> u32 {
        self.threshold
    }
    pub fn is_satisfied_with_state(&self, state: &SessionGateState) -> bool {
        state.sessions_since >= self.threshold
    }
    pub fn is_satisfied(&self) -> bool {
        false
    }
    pub fn record_session(&self, mut state: SessionGateState) -> SessionGateState {
        state.sessions_since = state.sessions_since.saturating_add(1);
        state.total_sessions = state.total_sessions.saturating_add(1);
        state
    }
    pub fn reset(&self) -> SessionGateState {
        SessionGateState {
            sessions_since: 0,
            total_sessions: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn gate_open_at_threshold() {
        let ids: Vec<String> = (0..5).map(|i| format!("id-{i}")).collect();
        assert!(is_session_gate_open(&ids, 5));
    }

    #[test]
    fn gate_closed_below() {
        let ids: Vec<String> = (0..4).map(|i| format!("id-{i}")).collect();
        assert!(!is_session_gate_open(&ids, 5));
    }

    #[test]
    fn lists_uuid_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        std::fs::write(base.join("a1b2c3d4-e5f6-7890-abcd-ef1234567890.jsonl"), "x").unwrap();
        std::fs::write(base.join("agent-abc123.jsonl"), "x").unwrap();
        std::fs::write(base.join("not-a-uuid.jsonl"), "x").unwrap();

        let result = list_sessions_touched_since(base, UNIX_EPOCH).unwrap();
        assert!(result.contains(&"a1b2c3d4-e5f6-7890-abcd-ef1234567890".to_string()));
        assert!(!result.iter().any(|s| s.starts_with("agent-")));
        assert!(!result.iter().any(|s| s == "not-a-uuid"));
    }

    #[test]
    fn stateful_gate_satisfied_at_threshold() {
        let gate = SessionGate::new(5);
        let state = SessionGateState {
            sessions_since: 5,
            total_sessions: 10,
        };
        assert!(gate.is_satisfied_with_state(&state));
    }

    #[test]
    fn stateful_gate_not_satisfied_below() {
        let gate = SessionGate::new(5);
        let state = SessionGateState {
            sessions_since: 3,
            total_sessions: 10,
        };
        assert!(!gate.is_satisfied_with_state(&state));
    }

    #[test]
    fn record_and_reset() {
        let gate = SessionGate::new(5);
        let state = gate.record_session(SessionGateState::default());
        assert_eq!(state.sessions_since, 1);
        assert_eq!(gate.reset().sessions_since, 0);
    }
}
