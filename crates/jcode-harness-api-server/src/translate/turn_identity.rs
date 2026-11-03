//! Identity-aware normalization at the wire-to-API boundary.
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Default)]
pub(super) struct TurnFence {
    session_id: Option<String>,
    generation: Option<String>,
    retired: HashSet<String>,
    latest: u64,
    completed: u64,
    active: Option<u64>,
}
impl TurnFence {
    pub(super) fn scoped(&self) -> bool {
        self.generation.is_some()
    }
    pub(super) fn active(&self) -> bool {
        self.active.is_some()
    }
    pub(super) fn state(&mut self, event: &Value) -> bool {
        let Some(session_id) = event["session_id"].as_str() else {
            return false;
        };
        if self
            .session_id
            .as_deref()
            .is_some_and(|current| current != session_id)
        {
            *self = Self::default();
        }
        self.session_id = Some(session_id.to_string());
        let Some(generation) = event["runtime_generation"].as_str() else {
            return false;
        };
        let Some(latest) = event["latest_turn_id"].as_u64() else {
            return false;
        };
        if self.retired.contains(generation) {
            return false;
        }
        if self.generation.as_deref() != Some(generation) {
            if let Some(old) = self.generation.replace(generation.to_string()) {
                self.retired.insert(old);
            }
            self.latest = 0;
            self.completed = 0;
            self.active = None;
        }
        if latest < self.latest {
            return false;
        }
        self.latest = latest;
        self.active = event["turn_id"].as_u64().filter(|id| *id > self.completed);
        if self.active.is_none() {
            self.completed = self.completed.max(latest);
        }
        true
    }
    pub(super) fn start(&mut self, identity: &Value) -> bool {
        let Some(generation) = identity["runtime_generation"].as_str() else {
            return false;
        };
        let Some(id) = identity["turn_id"].as_u64() else {
            return false;
        };
        if self.retired.contains(generation) {
            return false;
        }
        if let Some(current) = &self.generation {
            if current != generation {
                return false;
            }
        } else {
            self.generation = Some(generation.to_string());
        }
        if id < self.latest || id <= self.completed {
            return false;
        }
        self.latest = id;
        self.active = Some(id);
        true
    }
    pub(super) fn accepts(&self, identity: &Value) -> bool {
        self.active.is_some()
            && identity["turn_id"].as_u64() == self.active
            && identity["runtime_generation"].as_str() == self.generation.as_deref()
    }
    pub(super) fn is_latest(&self, identity: &Value) -> bool {
        identity["turn_id"].as_u64() == Some(self.latest)
            && identity["runtime_generation"].as_str() == self.generation.as_deref()
    }
    pub(super) fn complete(&mut self) {
        if let Some(id) = self.active.take() {
            self.completed = self.completed.max(id);
        }
    }
}
