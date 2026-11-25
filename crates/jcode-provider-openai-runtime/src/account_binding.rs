use super::*;

impl OpenAIProvider {
    pub(super) fn bind_account_pin(
        &self,
        pin: Option<jcode_provider_core::AccountPin>,
    ) -> Result<()> {
        let pin = pin
            .as_ref()
            .map(jcode_base::auth::account_pins::normalize_openai_pin)
            .transpose()?;
        let selected = pin
            .as_ref()
            .map(jcode_base::auth::account_pins::load_openai)
            .transpose()?;
        let mut credentials = self.credentials.try_write().map_err(|_| {
            anyhow::anyhow!("Cannot change OpenAI account while credentials are in use")
        })?;
        *self
            .oauth_account
            .write()
            .unwrap_or_else(|p| p.into_inner()) = pin;
        if let Some((selected, _)) = selected {
            *credentials = selected.into();
            self.browser_only.store(false, AtomicOrdering::Release);
        }
        drop(credentials);
        self.clear_persistent_ws_try("OpenAI account changed");
        self.reload_cached_reasoning_efforts();
        Ok(())
    }
}
