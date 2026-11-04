use super::*;

impl Agent {
    /// Speculatively prewarm the provider while a newly created session is idle.
    ///
    /// This deliberately bypasses `tool_definitions`, whose cache lock is only
    /// appropriate once an actual turn starts. Late MCP registration or user
    /// customization can therefore still change the foreground tool snapshot;
    /// the provider is responsible for discarding an incompatible warmup.
    pub(crate) async fn prewarm_provider_idle(&self) {
        self.provider.set_session_id(&self.session.id);
        let tools = self.tool_definitions_for_debug().await;
        let split_prompt = self.build_system_prompt_split(None);
        self.provider
            .prewarm(&tools, &split_prompt.static_part)
            .await;
    }
}
