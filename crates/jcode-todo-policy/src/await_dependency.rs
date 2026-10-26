//! Durable swarm waits are todo dependencies, not process-backed background jobs.
//! Readers never require a live PID: the server resumes these records on reload.
use serde::Deserialize;
use std::path::Path;

pub const ID_PREFIX: &str = "await-";
pub const STATE_DIR: &str = "jcode-await-members";

#[derive(Debug, Deserialize)]
pub struct AwaitDependency {
    pub key: String,
    pub session_id: String,
    pub created_at_unix_ms: u64,
    pub deadline_unix_ms: u64,
    pub background: bool,
    pub final_response: Option<serde_json::Value>,
}

impl AwaitDependency {
    pub fn id(&self) -> String {
        dependency_id(&self.key, self.created_at_unix_ms)
    }

    pub fn is_running_at(&self, now_ms: u64) -> bool {
        self.background && self.final_response.is_none() && self.deadline_unix_ms > now_ms
    }

    pub fn is_running(&self) -> bool {
        self.is_running_at(now_unix_ms())
    }
}

pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// The generation prevents a completed wait from blocking again when the same
/// member selection is reused. Retries of a pending wait keep the same ID.
pub fn dependency_id(key: &str, created_at_unix_ms: u64) -> String {
    format!("{ID_PREFIX}{key}-{created_at_unix_ms}")
}

pub fn load(runtime_dir: &Path, id: &str) -> Option<AwaitDependency> {
    let rest = id.strip_prefix(ID_PREFIX)?;
    let (key, generation) = rest.rsplit_once('-')?;
    if !crate::store::is_safe_session_id(key) {
        return None;
    }
    let generation: u64 = generation.parse().ok()?;
    let bytes = std::fs::read(runtime_dir.join(STATE_DIR).join(format!("{key}.json"))).ok()?;
    let state: AwaitDependency = serde_json::from_slice(&bytes).ok()?;
    (state.key == key && state.created_at_unix_ms == generation && state.id() == id)
        .then_some(state)
}

pub fn running(runtime_dir: &Path, id: &str, session_id: &str) -> bool {
    load(runtime_dir, id).is_some_and(|state| state.session_id == session_id && state.is_running())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_dependency_validates_identity_generation_owner_and_deadline() {
        let dir = std::env::temp_dir().join(format!("await-dependency-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(STATE_DIR)).unwrap();
        let path = dir.join(STATE_DIR).join("owner-key.json");
        let mut state = serde_json::json!({
            "key": "owner-key", "session_id": "owner", "created_at_unix_ms": 42,
            "deadline_unix_ms": u64::MAX, "background": true, "final_response": null
        });
        let save = |value: &serde_json::Value| {
            std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        };
        save(&state);
        let id = dependency_id("owner-key", 42);
        assert!(running(&dir, &id, "owner"));
        assert!(!running(&dir, &id, "foreign"));
        assert!(load(&dir, &dependency_id("owner-key", 41)).is_none());
        assert!(load(&dir, "await-../../owner-key-42").is_none());
        assert!(load(&dir, "await-owner-key-042").is_none());
        // A new reader after reload observes the same pending generation.
        assert_eq!(load(&dir, &id).unwrap().id(), id);
        state["final_response"] = serde_json::json!({"completed": true});
        save(&state);
        assert!(!running(&dir, &id, "owner"));
        assert!(load(&dir, &id).is_some());
        state["final_response"] = serde_json::Value::Null;
        state["deadline_unix_ms"] = 0.into();
        save(&state);
        assert!(!running(&dir, &id, "owner"));
        state["deadline_unix_ms"] = u64::MAX.into();
        state["background"] = false.into();
        save(&state);
        assert!(!running(&dir, &id, "owner"));
        state["key"] = "other-key".into();
        save(&state);
        assert!(load(&dir, &id).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
