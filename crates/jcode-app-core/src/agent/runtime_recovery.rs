//! In-place runtime recovery shared by server-owned remote commands.
use super::*;

impl Agent {
    /// Recover the live server context without clearing history, changing the
    /// session ID, or moving work into a new laptop-owned session.
    pub(crate) fn recover_runtime_context(&mut self) -> Result<serde_json::Value> {
        // Re-scan the whole transcript, including old interrupted tool calls.
        self.reset_tool_output_tracking();
        let repaired = self.repair_missing_tool_outputs();
        let had_provider_session =
            self.provider_session_id.is_some() || self.session.provider_session_id.is_some();
        self.reset_provider_session();
        self.cache_tracker.reset();
        self.kv_cache_monitor.reset();
        self.locked_tools = None;
        let (compaction_message, compaction_started) = self.request_manual_compaction();
        self.persist_session_now()?;
        Ok(serde_json::json!({
            "session_id": self.session.id,
            "repaired_tool_outputs": repaired,
            "provider_session_reset": had_provider_session,
            "compaction_started": compaction_started,
            "compaction_message": compaction_message,
            "history_preserved": true,
        }))
    }
}
