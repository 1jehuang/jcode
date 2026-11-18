//! Restoring and persisting an agent session.

use super::*;

impl Agent {
    /// Restore a session by ID (loads from disk)
    pub fn restore_session(&mut self, session_id: &str) -> Result<SessionStatus> {
        self.restore_session_with_working_dir(session_id, None)
    }

    pub(crate) fn restore_session_with_working_dir(
        &mut self,
        session_id: &str,
        working_dir: Option<&str>,
    ) -> Result<SessionStatus> {
        self.restore_session_inner(session_id, working_dir, true)
    }

    /// Restore without persisting. The caller must call
    /// [`Self::persist_restored_session`] once it is off the latency-critical
    /// path: for sessions with multi-MB compaction state that save is a full
    /// snapshot rewrite (0.4-1.6s), and it used to delay the History reply.
    pub(crate) fn restore_session_deferring_save(
        &mut self,
        session_id: &str,
        working_dir: Option<&str>,
    ) -> Result<SessionStatus> {
        self.restore_session_inner(session_id, working_dir, false)
    }

    /// Persist state recorded by [`Self::restore_session_deferring_save`].
    pub(crate) fn persist_restored_session(&mut self) {
        let start = Instant::now();
        if let Err(err) = self.session.save() {
            logging::error(&format!(
                "Failed to persist resumed session state for {}: {}",
                self.session.id, err
            ));
        }
        logging::info(&format!(
            "[TIMING] restore_session deferred save: session={}, save={}ms",
            self.session.id,
            start.elapsed().as_millis()
        ));
    }

    pub(in crate::agent) fn restore_session_inner(
        &mut self,
        session_id: &str,
        working_dir: Option<&str>,
        persist: bool,
    ) -> Result<SessionStatus> {
        let restore_start = Instant::now();
        let restore_cpu_start = crate::server::thread_cpu_ms();
        let load_start = Instant::now();
        let mut session = Session::load_for_resume(session_id)?;
        if let Some(working_dir) = working_dir {
            session.working_dir = Some(working_dir.to_string());
            session.refresh_initial_session_context_message();
        }
        let load_ms = load_start.elapsed().as_millis();
        logging::info(&format!(
            "Restoring session '{}' with {} messages, provider_session_id: {:?}, status: {}",
            session_id,
            session.messages.len(),
            session.provider_session_id,
            session.status.display()
        ));
        let previous_status = session.status.clone();

        let assign_start = Instant::now();
        // A failed load must leave the current Agent and its concurrency lease
        // alive. Close it only after the replacement is ready to install.
        self.mark_closed_for_replacement();
        let close_ms = assign_start.elapsed().as_millis();
        // Restore provider_session_id for Claude CLI session resume
        self.provider_session_id = session.provider_session_id.clone();
        let replaced_session = std::mem::replace(&mut self.session, session);
        drop(replaced_session);
        let swap_ms = assign_start.elapsed().as_millis() - close_ms;
        let lap = Instant::now();
        self.provider.set_session_id(&self.session.id);
        let provider_ms = lap.elapsed().as_millis();
        let lap = Instant::now();
        self.refresh_agents_md_snapshot();
        let agents_md_ms = lap.elapsed().as_millis();
        let lap = Instant::now();
        self._tool_policy_registration = crate::tool::register_session_tool_policy(
            &self.session.id,
            self.allowed_tools.clone(),
            self.disabled_tools.clone(),
        );
        let policy_ms = lap.elapsed().as_millis();
        let assign_ms = assign_start.elapsed().as_millis();
        if assign_ms >= 50 {
            logging::info(&format!(
                "[TIMING] restore_session assign parts: session={session_id}, close={close_ms}ms, swap={swap_ms}ms, provider={provider_ms}ms, agents_md={agents_md_ms}ms, policy={policy_ms}ms"
            ));
        }

        let reset_start = Instant::now();
        self.reset_runtime_state_for_session_change();
        let restored_soft_interrupts = self.restore_persisted_soft_interrupts();
        let reset_ms = reset_start.elapsed().as_millis();

        let model_start = Instant::now();
        self.restore_session_model_from_session();
        self.restore_reasoning_effort_from_session();
        let model_ms = model_start.elapsed().as_millis();

        let mark_active_start = Instant::now();
        // begin_concurrency_tracking writes the presence markers.
        self.session.mark_active_unregistered();
        self.begin_concurrency_tracking();
        let mark_active_ms = mark_active_start.elapsed().as_millis();
        self.sync_memory_dedup_state_from_session();

        logging::info(&format!(
            "restore_session: loaded session {} with {} messages, calling seed_compaction",
            session_id,
            self.session.messages.len()
        ));
        let compaction_start = Instant::now();
        self.seed_compaction_from_session();
        let compaction_ms = compaction_start.elapsed().as_millis();

        let env_snapshot_start = Instant::now();
        // The save just below persists this snapshot too.
        self.log_env_snapshot_unsaved("resume");
        let env_snapshot_ms = env_snapshot_start.elapsed().as_millis();
        self.fire_session_lifecycle_hook("session_start", "resume");

        let save_start = Instant::now();
        if persist && let Err(err) = self.session.save() {
            logging::error(&format!(
                "Failed to persist resumed session state for {}: {}",
                session_id, err
            ));
        }
        let save_ms = save_start.elapsed().as_millis();

        logging::info(&format!(
            "[TIMING] restore_session: session={}, messages={}, restored_soft_interrupts={}, load={}ms, assign={}ms, reset={}ms, model={}ms, mark_active={}ms, compaction={}ms, env_snapshot={}ms, save={}ms, total={}ms, cpu={:.0}ms",
            session_id,
            self.session.messages.len(),
            restored_soft_interrupts,
            load_ms,
            assign_ms,
            reset_ms,
            model_ms,
            mark_active_ms,
            compaction_ms,
            env_snapshot_ms,
            save_ms,
            restore_start.elapsed().as_millis(),
            crate::server::thread_cpu_ms() - restore_cpu_start,
        ));
        logging::info(&format!(
            "Session restored: {} messages in session",
            self.session.messages.len()
        ));
        Ok(previous_status)
    }
}
