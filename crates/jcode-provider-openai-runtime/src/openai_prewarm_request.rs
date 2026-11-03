use super::*;

impl OpenAIProvider {
    pub(super) async fn prewarm_request(&self, tools: &[ToolDefinition], system: &str) {
        if self.oauth_account_label().is_some_and(|label| {
            jcode_base::auth::codex::load_credentials_for_account(&label).is_err()
        }) {
            return;
        }
        if self.is_browser_only()
            || is_chatgpt_web_model(&self.model())
            || std::env::var("JCODE_OPENAI_PREWARM").is_ok_and(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "off"
                )
            })
        {
            return;
        }
        let mode = self
            .transport_mode
            .try_read()
            .map(|mode| *mode)
            .unwrap_or(OpenAITransportMode::HTTPS);
        if matches!(mode, OpenAITransportMode::HTTPS) {
            return;
        }
        // An existing response chain is more useful than a speculative prefix.
        // Never contend with active generation or keepalive for its socket.
        match self.persistent_ws.try_lock() {
            Ok(guard) if guard.is_none() => {}
            _ => return,
        }
        // Unlike foreground completion, preparation must not wait for model
        // discovery or OAuth refresh. A changed/fallback model simply makes
        // this speculative request incompatible at adoption time.
        let model = self.model();
        let model = model.strip_suffix("[1m]").unwrap_or(&model);
        let Ok(credentials) = self.credentials.try_read() else {
            return;
        };
        let request = self.response_request_for_model(
            model,
            &[],
            tools,
            system,
            Self::is_chatgpt_mode(&credentials),
        );
        drop(credentials);
        if matches!(mode, OpenAITransportMode::Auto)
            && websocket_cooldown_remaining(
                &self.websocket_cooldowns,
                &openai_request_model(&request),
            )
            .await
            .is_some()
        {
            return;
        }
        self.prewarm.start(Arc::clone(&self.credentials), &request);
    }
}
