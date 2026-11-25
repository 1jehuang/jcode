use super::*;
impl AnthropicProvider {
    pub(super) fn load_oauth_credentials(&self) -> Result<auth::claude::ClaudeCredentials> {
        match self.oauth_account_pin() {
            Some(pin) => auth::account_pins::load_claude(&pin).map(|(credentials, _)| credentials),
            None => auth::claude::load_credentials(),
        }
    }

    pub(super) fn select_verified_oauth_route(
        &self,
        selection: &jcode_provider_core::RouteSelection,
        fingerprint: &str,
    ) -> Result<()> {
        anyhow::ensure!(
            self.credential_mode_snapshot() == AnthropicCredentialMode::OAuth,
            "Claude sign-in requires the subscription route"
        );
        let credentials = self.load_oauth_credentials()?;
        anyhow::ensure!(
            auth::claude_cli_completion::fingerprint(&credentials) == fingerprint,
            "The daemon cannot load the completed Claude login; its source or account changed"
        );
        self.set_model(&selection.model)
    }
}
