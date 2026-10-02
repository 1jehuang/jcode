use super::{Agent, JCODE_REPO_SOURCE_STATE, WORKING_GIT_STATE_CACHE};
use crate::logging;
use crate::session::{EnvSnapshot, GitState};
use chrono::Utc;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EnvSnapshotDetail {
    Minimal,
    Full,
}

pub(super) fn cached_git_state_for_dir(
    dir: &Path,
    git_state_for_dir: impl Fn(&Path) -> Option<GitState>,
) -> Option<GitState> {
    let cache_key = dir.to_path_buf();
    if let Ok(cache) = WORKING_GIT_STATE_CACHE.lock()
        && let Some(state) = cache.get(&cache_key)
    {
        return state.clone();
    }

    let state = git_state_for_dir(dir);
    if let Ok(mut cache) = WORKING_GIT_STATE_CACHE.lock() {
        cache.insert(cache_key, state.clone());
    }
    state
}

impl Agent {
    /// Set logging context for this agent's session/provider
    pub(super) fn set_log_context(&self) {
        logging::set_session(&self.session.id);
        // Log the profile this session actually talks to. `name()` is the
        // stable machine id for the provider class, which the multiplexing
        // slot reports as `OpenRouter` (a concrete runtime instance reports
        // `openrouter`); that slot also serves every direct OpenAI-compatible
        // profile, so it tagged DeepSeek sessions `prv:OpenRouter` /
        // `prv:openrouter` (issue #1286).
        logging::set_provider_info(&self.provider.display_name(), &self.provider.model());
    }

    /// Record a lightweight environment snapshot for post-mortem debugging
    pub(super) fn log_env_snapshot(&mut self, reason: &str) {
        let snapshot = self.build_env_snapshot(reason, self.env_snapshot_detail());
        self.session.record_env_snapshot(snapshot.clone());
        if !self.session.messages.is_empty() {
            self.persist_session_best_effort("environment snapshot");
        }
        if let Ok(json) = serde_json::to_string(&snapshot) {
            logging::info(&format!("ENV_SNAPSHOT {}", json));
        } else {
            logging::info("ENV_SNAPSHOT {}");
        }
    }

    pub(super) fn env_snapshot_detail(&self) -> EnvSnapshotDetail {
        if self.session.visible_conversation_message_count() == 0 {
            EnvSnapshotDetail::Minimal
        } else {
            EnvSnapshotDetail::Full
        }
    }

    pub(super) fn build_env_snapshot(
        &self,
        reason: &str,
        detail: EnvSnapshotDetail,
    ) -> EnvSnapshot {
        let (jcode_git_hash, jcode_git_dirty) = match detail {
            EnvSnapshotDetail::Full => JCODE_REPO_SOURCE_STATE.clone(),
            EnvSnapshotDetail::Minimal => (None, None),
        };

        let working_dir = self.session.working_dir.clone();
        let working_git = match detail {
            EnvSnapshotDetail::Full => working_dir.as_deref().and_then(|dir| {
                cached_git_state_for_dir(Path::new(dir), super::utils::git_state_for_dir)
            }),
            EnvSnapshotDetail::Minimal => None,
        };

        EnvSnapshot {
            captured_at: Utc::now(),
            reason: reason.to_string(),
            session_id: self.session.id.clone(),
            working_dir,
            provider: self.provider.name().to_string(),
            model: self.provider.model().to_string(),
            jcode_version: jcode_build_meta::version().to_string(),
            jcode_git_hash,
            jcode_git_dirty,
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            pid: std::process::id(),
            is_selfdev: self.session.is_self_dev(),
            is_debug: self.session.is_debug,
            is_canary: self.session.is_canary,
            testing_build: self.session.testing_build.clone(),
            working_git,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Message, ToolDefinition};
    use crate::provider::{EventStream, Provider};
    use crate::tool::Registry;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::sync::Arc;

    /// A stand-in for the multiplexing OpenRouter slot: the machine-facing
    /// `name()` is the transport, while the runtime it executes is a direct
    /// OpenAI-compatible profile. A concrete runtime instance reports the
    /// lowercase `openrouter` instead; both tag the same sessions.
    struct MultiplexedSlotProvider;

    #[async_trait]
    impl Provider for MultiplexedSlotProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<EventStream> {
            Err(anyhow::anyhow!(
                "the log context test never completes a call"
            ))
        }

        fn name(&self) -> &str {
            "OpenRouter"
        }

        fn display_name(&self) -> String {
            "DeepSeek".to_string()
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(MultiplexedSlotProvider)
        }
    }

    /// The log prefix must name the profile the session talks to. `name()` is
    /// the transport slot that also serves every direct OpenAI-compatible
    /// profile, so it tagged DeepSeek sessions as `prv:openrouter` (issue #1286).
    #[tokio::test]
    async fn log_context_names_the_profile_not_the_transport_slot() {
        // `Agent::new` only builds in-memory session state, so the test needs
        // no `JCODE_HOME`, and it must not set one either: the crate's tests
        // run in parallel.
        let provider: Arc<dyn Provider> = Arc::new(MultiplexedSlotProvider);
        let registry = Registry::new(provider.clone()).await;
        let agent = Agent::new(provider, registry);

        agent.set_log_context();

        let context = logging::current_context_snapshot();
        assert_eq!(
            context.provider.as_deref(),
            Some("DeepSeek"),
            "the log prefix must name the profile, not the slot"
        );
    }
}
