//! Live model discovery and persistent catalog lifecycle.

use super::*;

impl AnthropicProvider {
    /// Fetch the live Anthropic model catalog using the resolved credential.
    ///
    /// Mirrors [`Provider::prefetch_models`] but returns the model ids to the
    /// caller (rather than only persisting them) so the doctor can assert the
    /// live `GET /v1/models` endpoint works and that the model under test is in
    /// the live catalog.
    pub async fn fetch_live_model_ids_for_doctor(&self) -> Result<Vec<String>> {
        let oauth_scope = self.catalog_scope_for_route(true);
        let api_scope = self.catalog_scope_for_route(false);
        let (token, is_oauth) = self.get_access_token().await?;
        let scope = if is_oauth { oauth_scope } else { api_scope };
        if token.trim().is_empty() {
            anyhow::bail!("resolved an empty Anthropic access token");
        }
        let catalog = if is_oauth {
            jcode_base::provider::fetch_anthropic_model_catalog_oauth(&token).await?
        } else {
            // Discovery must go out with the same `[provider.headers]`
            // overrides the chat transport resolves, or a key overridden
            // there 401s here and every model shows as unavailable.
            let headers = super::configured_direct_headers();
            jcode_base::provider::fetch_anthropic_model_catalog_with_headers(
                &token,
                headers.ok().as_ref(),
            )
            .await?
        };
        // Persist so the rest of the process benefits from the warm catalog,
        // exactly like the runtime's own prefetch.
        jcode_base::provider::persist_anthropic_model_catalog_for_scope(&scope, &catalog);
        if !catalog.context_limits.is_empty() {
            jcode_base::provider::populate_context_limits(catalog.context_limits.clone());
        }
        if !catalog.available_models.is_empty() {
            jcode_base::provider::populate_anthropic_models_for_scope(
                &scope,
                catalog.available_models.clone(),
            );
        }
        Ok(catalog.available_models)
    }

    pub(super) async fn refresh_catalogs(&self, force: bool) -> Result<()> {
        // Custom messages gateways must never leak their API keys to the
        // official catalog. Genuine subscription OAuth may still discover
        // directly even when messages are routed through a local proxy.
        let custom_gateway = self.direct_transport.api_url != API_URL;
        let mut errors = Vec::new();
        // Catalog discovery must not mutate the selected chat credential mode.
        // A failed API route must not suppress discovery via OAuth.
        for oauth in [false, true] {
            if custom_gateway && !oauth {
                continue;
            }
            let configured = if oauth {
                auth::claude::load_credentials().is_ok()
            } else {
                self.direct_api_key().is_ok()
            };
            if !configured {
                continue;
            }
            let scope = self.catalog_scope_for_route(oauth);
            let started = if force {
                jcode_base::provider::begin_forced_anthropic_model_catalog_refresh_for_scope(&scope)
            } else {
                jcode_base::provider::begin_anthropic_model_catalog_refresh_for_scope(&scope)
            };
            if !started {
                continue;
            }
            let result = self.refresh_model_catalog_for_route(oauth, &scope).await;
            jcode_base::provider::finish_anthropic_model_catalog_refresh_for_scope(&scope);
            if let Err(err) = result {
                let message = format!(
                    "Anthropic {} model catalog refresh failed; keeping cached list: {err}",
                    if oauth { "OAuth" } else { "API key" }
                );
                jcode_base::logging::warn(&message);
                errors.push(message);
            }
        }
        if force && !errors.is_empty() {
            anyhow::bail!("{}", errors.join("; "));
        }
        Ok(())
    }

    pub(super) async fn refresh_model_catalog_for_route(
        &self,
        oauth: bool,
        scope: &str,
    ) -> Result<()> {
        let catalog = if oauth {
            let (token, _) = self.get_oauth_access_token().await?;
            match jcode_base::provider::fetch_anthropic_model_catalog_oauth(&token).await {
                Ok(catalog) => catalog,
                Err(err) if is_oauth_catalog_auth_error(&err.to_string()) => {
                    let token = force_refresh_oauth_token(
                        Arc::clone(&self.credentials),
                        &self.oauth_refresh_label(),
                    )
                    .await?;
                    jcode_base::provider::fetch_anthropic_model_catalog_oauth(&token).await?
                }
                Err(err) => return Err(err),
            }
        } else {
            // Same header-override parity as the doctor path: chat honors
            // `[provider.headers]`, so the direct catalog refresh must too.
            let headers = super::configured_direct_headers();
            jcode_base::provider::fetch_anthropic_model_catalog_with_headers(
                &self.direct_api_key()?,
                headers.ok().as_ref(),
            )
            .await?
        };
        jcode_base::provider::persist_anthropic_model_catalog_for_scope(scope, &catalog);
        if !catalog.context_limits.is_empty() {
            jcode_base::provider::populate_context_limits(catalog.context_limits);
        }
        jcode_base::provider::populate_anthropic_models_for_scope(scope, catalog.available_models);
        Ok(())
    }
}
