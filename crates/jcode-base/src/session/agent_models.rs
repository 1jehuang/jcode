//! Session-specific worker routing. A separate atomic metadata file permits
//! updates while a turn owns the agent mutex, without rewriting its transcript.
use super::Session;
use anyhow::{Result, bail};
use std::collections::BTreeMap;

pub type AgentModelOverrides = BTreeMap<String, String>;

// Serialize read-modify-write updates from multiple attached clients. This is
// metadata only and never waits for the agent's long-running turn lock.
static UPDATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn path(id: &str) -> Result<std::path::PathBuf> {
    if id.is_empty() || id.contains('/') || id.contains('\\') || id == ".." {
        bail!("Invalid session id");
    }
    Ok(super::storage_paths::session_path(id)?.with_extension("agent-models"))
}

impl Session {
    /// Reload lock-independent routing metadata, including explicit clears.
    pub fn refresh_agent_model_overrides(&mut self) -> Result<()> {
        let path = path(&self.id)?;
        if path.exists() {
            self.agent_model_overrides = crate::storage::read_json(&path)?;
        }
        Ok(())
    }

    /// Persist a single preference without taking the live Agent lock.
    /// None uses the global default, while "inherit" bypasses that default.
    pub fn set_agent_model_override(&mut self, target: &str, model: Option<String>) -> Result<()> {
        let _guard = UPDATE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !matches!(target, "swarm" | "review" | "judge" | "memory" | "ambient") {
            bail!("Unknown agent model target: {target}");
        }
        self.refresh_agent_model_overrides()?;
        let mut overrides = self.agent_model_overrides.clone();
        match model
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            Some(model) => {
                overrides.insert(target.to_string(), model);
            }
            None => {
                overrides.remove(target);
            }
        }
        crate::storage::write_json(&path(&self.id)?, &overrides)?;
        self.agent_model_overrides = overrides;
        Ok(())
    }

    /// Resolve session > global. "inherit" remains explicit for downstream
    /// routing, rather than becoming indistinguishable from use-global.
    pub fn effective_agent_model(&self, target: &str, global: Option<String>) -> Option<String> {
        let mut current = self.agent_model_overrides.clone();
        if let Ok(path) = path(&self.id) {
            if path.exists() {
                if let Ok(saved) = crate::storage::read_json::<AgentModelOverrides>(&path) {
                    current = saved;
                }
            }
        }
        current.get(target).cloned().or(global)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_agent_models_isolate_clear_inherit_and_resume() {
        let _lock = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        let mut first = Session::create_with_id("first".into(), None, None);
        let mut second = Session::create_with_id("second".into(), None, None);
        first.save_prepared().unwrap();
        second.save_prepared().unwrap();
        let stale = Session::load_startup_stub("first").unwrap();
        let config_path = temp.path().join("config.toml");
        std::fs::write(&config_path, "[agents]\nswarm_model = \"global-model\"\n").unwrap();
        let before = std::fs::read(&config_path).unwrap();
        first
            .set_agent_model_override("swarm", Some("session-model".into()))
            .unwrap();
        assert_eq!(
            first
                .effective_agent_model("swarm", Some("global-model".into()))
                .as_deref(),
            Some("session-model")
        );
        assert_eq!(
            second
                .effective_agent_model("swarm", Some("global-model".into()))
                .as_deref(),
            Some("global-model")
        );
        assert_eq!(
            stale.effective_agent_model("swarm", None).as_deref(),
            Some("session-model")
        );
        for restored in [
            Session::load("first").unwrap(),
            Session::load_startup_stub("first").unwrap(),
            Session::load_for_remote_startup("first").unwrap(),
        ] {
            assert_eq!(
                restored
                    .agent_model_overrides
                    .get("swarm")
                    .map(String::as_str),
                Some("session-model")
            );
        }
        first
            .set_agent_model_override("swarm", Some("inherit".into()))
            .unwrap();
        assert_eq!(
            first
                .effective_agent_model("swarm", Some("global-model".into()))
                .as_deref(),
            Some("inherit")
        );
        first.set_agent_model_override("swarm", None).unwrap();
        assert_eq!(
            first
                .effective_agent_model("swarm", Some("new-global".into()))
                .as_deref(),
            Some("new-global")
        );
        // A stale turn saving after an out-of-band clear cannot revive its pin.
        let mut stale = stale;
        stale
            .agent_model_overrides
            .insert("swarm".into(), "old-pin".into());
        stale.save_prepared().unwrap();
        assert!(
            Session::load_startup_stub("first")
                .unwrap()
                .agent_model_overrides
                .is_empty()
        );
        assert_eq!(std::fs::read(&config_path).unwrap(), before);
        assert!(
            first
                .set_agent_model_override("invalid", Some("model".into()))
                .is_err()
        );
        if let Some(previous) = previous {
            crate::env::set_var("JCODE_HOME", previous);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}
