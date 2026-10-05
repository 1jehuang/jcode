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
    // A distinct stem keeps this file's `.bak` (written by `write_json`) from
    // replacing the transcript snapshot backup at `<id>.bak`.
    Ok(super::storage_paths::session_path(id)?.with_file_name(format!("{id}.agent-models.meta")))
}

/// Read saved overrides. Unreadable metadata degrades to "no override" with a
/// warning, so a damaged preference never blocks transcript load or save.
fn read_saved(path: &std::path::Path) -> Option<AgentModelOverrides> {
    if !path.exists() {
        return None;
    }
    match crate::storage::read_json(path) {
        Ok(saved) => Some(saved),
        Err(err) => {
            crate::logging::warn(&format!(
                "Ignoring unreadable session agent-model overrides at {}: {err}",
                path.display()
            ));
            Some(AgentModelOverrides::new())
        }
    }
}

impl Session {
    /// Reload lock-independent routing metadata, including explicit clears.
    pub fn refresh_agent_model_overrides(&mut self) -> Result<()> {
        if let Some(saved) = read_saved(&path(&self.id)?) {
            self.agent_model_overrides = saved;
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
        let current = path(&self.id)
            .ok()
            .and_then(|path| read_saved(&path))
            .unwrap_or_else(|| self.agent_model_overrides.clone());
        current.get(target).cloned().or(global)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_home(f: impl FnOnce(&std::path::Path)) {
        let _lock = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(temp.path())));
        if let Some(previous) = previous {
            crate::env::set_var("JCODE_HOME", previous);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn override_writes_keep_transcript_backup_intact() {
        with_temp_home(|_| {
            let mut session = Session::create_with_id("bak_owner".into(), None, None);
            session.save_prepared().unwrap();
            let transcript_backup = super::super::storage_paths::session_path("bak_owner")
                .unwrap()
                .with_extension("bak");
            // Stand-in for the snapshot backup written by transcript saves.
            std::fs::write(&transcript_backup, b"transcript backup").unwrap();
            let before = std::fs::read(&transcript_backup).unwrap();
            for model in ["a", "b", "c"] {
                session
                    .set_agent_model_override("swarm", Some(model.into()))
                    .unwrap();
            }
            assert_eq!(
                std::fs::read(&transcript_backup).unwrap(),
                before,
                "override writes must not replace the transcript backup"
            );
            assert!(
                path("bak_owner").unwrap().with_extension("bak").exists(),
                "override file keeps its own backup"
            );
        });
    }

    #[test]
    fn corrupt_override_file_without_backup_does_not_block_session() {
        with_temp_home(|_| {
            let mut session = Session::create_with_id("corrupt_meta".into(), None, None);
            session.save_prepared().unwrap();
            let meta = path("corrupt_meta").unwrap();
            std::fs::write(&meta, b"{ not json").unwrap();
            let _ = std::fs::remove_file(meta.with_extension("bak"));

            let mut loaded = Session::load("corrupt_meta").expect("load must not fail");
            assert!(loaded.agent_model_overrides.is_empty());
            assert!(Session::load_startup_stub("corrupt_meta").is_ok());
            assert!(Session::load_for_remote_startup("corrupt_meta").is_ok());
            loaded.save_prepared().expect("save must not fail");
            assert_eq!(
                loaded.effective_agent_model("swarm", Some("global".into())),
                Some("global".into())
            );
            // An explicit write replaces the damaged file.
            loaded
                .set_agent_model_override("swarm", Some("fresh".into()))
                .unwrap();
            assert_eq!(
                Session::load("corrupt_meta")
                    .unwrap()
                    .agent_model_overrides
                    .get("swarm")
                    .map(String::as_str),
                Some("fresh")
            );
        });
    }

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
