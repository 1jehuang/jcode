use super::*;

impl MultiProvider {
    pub(super) fn ensure_cursor_acp_provider(&self) -> Result<Arc<dyn Provider>> {
        if let Some(provider) = self.cursor_acp_provider() {
            self.bind_provider_session(provider.as_ref());
            return Ok(provider);
        }
        let candidate =
            external::instantiate_expected_external_provider(external::CURSOR_ACP_RUNTIME)
                .ok_or_else(|| anyhow!("Cursor ACP runtime is not registered"))?;
        self.bind_provider_session(candidate.as_ref());
        let provider = self
            .cursor_acp
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get_or_insert(candidate)
            .clone();
        self.bind_provider_session(provider.as_ref());
        Ok(provider)
    }

    pub(super) fn set_model_on_cursor_acp(&self, model: &str) -> Result<()> {
        let model = model.trim();
        if model.is_empty() {
            anyhow::bail!("Cursor ACP model cannot be empty");
        }
        self.ensure_cursor_acp_provider()?.set_model(model)?;
        let switching =
            self.active_external_provider().as_deref() != Some(external::CURSOR_ACP_RUNTIME);
        self.set_active_provider_external(external::CURSOR_ACP_RUNTIME);
        self.invalidate_routes_memo();
        if switching {
            self.spawn_cursor_acp_catalog_refresh_if_needed();
        }
        Ok(())
    }
}
