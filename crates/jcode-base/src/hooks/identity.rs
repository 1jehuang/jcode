//! Swarm identity for hooks that fire inside a worker session.
//!
//! The server registers a worker's identity when it is spawned. Every hook
//! event for that session is then enriched with swarm, coordinator, parent,
//! label and agent fields, so a hook script can tell worker activity from
//! the coordinator's without parsing the transcript.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

/// Who a worker session is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwarmIdentity {
    pub swarm_id: String,
    pub coordinator_session_id: String,
    /// Session that receives the worker's report (usually the coordinator).
    pub parent_session_id: String,
    pub label: Option<String>,
    /// Agent definition name when spawned with one, else the label.
    pub agent_type: Option<String>,
}

static REGISTRY: LazyLock<RwLock<HashMap<String, SwarmIdentity>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Remember `identity` for the worker `session_id`.
pub fn register_swarm_identity(session_id: &str, identity: SwarmIdentity) {
    REGISTRY
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session_id.to_owned(), identity);
}

/// Forget a worker, e.g. when its session is removed.
pub fn forget_swarm_identity(session_id: &str) {
    REGISTRY
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .remove(session_id);
}

/// The identity registered for `session_id`, if it is a swarm worker.
pub fn swarm_identity(session_id: &str) -> Option<SwarmIdentity> {
    REGISTRY
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_id)
        .cloned()
}

impl SwarmIdentity {
    /// Env mirror pairs (`JCODE_HOOK_<key>`) for a worker `session_id`.
    pub(super) fn env_pairs(&self, session_id: &str) -> Vec<(&'static str, String)> {
        let mut pairs = vec![
            ("SWARM_ID", self.swarm_id.clone()),
            (
                "COORDINATOR_SESSION_ID",
                self.coordinator_session_id.clone(),
            ),
            ("PARENT_SESSION_ID", self.parent_session_id.clone()),
            ("AGENT_ID", session_id.to_string()),
        ];
        if let Some(label) = &self.label {
            pairs.push(("SWARM_LABEL", label.clone()));
        }
        if let Some(agent_type) = self.agent_type.as_ref().or(self.label.as_ref()) {
            pairs.push(("AGENT_TYPE", agent_type.clone()));
        }
        pairs
    }

    /// JSON object for the `jcode.swarm` payload field.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "swarm_id": self.swarm_id,
            "coordinator_session_id": self.coordinator_session_id,
            "parent_session_id": self.parent_session_id,
            "label": self.label,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(label: Option<&str>, agent_type: Option<&str>) -> SwarmIdentity {
        SwarmIdentity {
            swarm_id: "sw".into(),
            coordinator_session_id: "coord".into(),
            parent_session_id: "parent".into(),
            label: label.map(str::to_string),
            agent_type: agent_type.map(str::to_string),
        }
    }

    #[test]
    fn registry_round_trip_and_forget() {
        register_swarm_identity("identity-test-a", identity(Some("l"), None));
        assert_eq!(swarm_identity("identity-test-a").unwrap().swarm_id, "sw");
        forget_swarm_identity("identity-test-a");
        assert_eq!(swarm_identity("identity-test-a"), None);
        assert_eq!(swarm_identity("never-registered"), None);
    }

    #[test]
    fn agent_type_prefers_definition_then_label() {
        let pairs = identity(Some("task"), Some("reviewer")).env_pairs("w1");
        assert!(pairs.contains(&("AGENT_TYPE", "reviewer".to_string())));
        assert!(pairs.contains(&("AGENT_ID", "w1".to_string())));
        let pairs = identity(Some("task"), None).env_pairs("w1");
        assert!(pairs.contains(&("AGENT_TYPE", "task".to_string())));
        let pairs = identity(None, None).env_pairs("w1");
        assert!(!pairs.iter().any(|(k, _)| *k == "AGENT_TYPE"));
    }
}
