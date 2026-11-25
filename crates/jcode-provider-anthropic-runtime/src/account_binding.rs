use super::*;

impl AnthropicProvider {
    pub(super) fn bind_account_pin(
        &self,
        pin: Option<jcode_provider_core::AccountPin>,
    ) -> Result<()> {
        let pin = pin
            .as_ref()
            .map(auth::account_pins::normalize_claude_pin)
            .transpose()?;
        if let Some(pin) = &pin {
            auth::account_pins::load_claude(pin)?;
        }
        let mut cached = self.credentials.try_write().map_err(|_| {
            anyhow::anyhow!("Cannot change Claude account while credentials are in use")
        })?;
        *self
            .oauth_account
            .write()
            .unwrap_or_else(|p| p.into_inner()) = pin;
        *cached = None;
        self.request_credential.store(0, Ordering::Release);
        self.oauth_preflight_done.store(false, Ordering::Release);
        Ok(())
    }
}
