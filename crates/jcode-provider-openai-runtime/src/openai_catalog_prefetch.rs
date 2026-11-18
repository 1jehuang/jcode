use super::*;

impl OpenAIProvider {
    pub(super) async fn prefetch_model_catalog(&self) -> Result<()> {
        if self.is_browser_only() {
            return Ok(());
        }
        // The loaded credential's *shape* is authoritative for which catalog
        // endpoint to hit, not the requested credential mode. In Auto mode a
        // user with only an OPENAI_API_KEY loads an API-key-shaped credential
        // while the mode stays Auto; routing by mode would send that platform
        // key to the ChatGPT/Codex endpoint and get a 401.
        let account_label = self.refresh_account_label();
        let (access_token, is_chatgpt_mode, credential_identity, scope) = {
            let creds = self.credentials.read().await;
            (
                creds.access_token.clone(),
                Self::is_chatgpt_mode(&creds),
                Self::catalog_credential_identity(&creds),
                self.selected_catalog_scope_for(&creds),
            )
        };
        let catalog = if is_chatgpt_mode {
            let access_token =
                openai_access_token(self.refresh_account_label(), &self.credentials).await?;
            match jcode_base::provider::fetch_openai_model_catalog(&access_token).await {
                Ok(catalog) => catalog,
                // The server can reject a token that still looks fresh by its
                // local expiry (revoked/rotated). The chat path recovers by
                // force-refreshing; without the same recovery here the model
                // catalog silently stays stale and newly released models never
                // show up until the user happens to re-login (observed as
                // days of bootstrap 401s in the logs).
                Err(err) if catalog_error_is_auth_rejection(&err) => {
                    let refresh_token = {
                        let creds = self.credentials.read().await;
                        creds.refresh_token.clone()
                    };
                    if refresh_token.is_empty() {
                        return Err(err);
                    }
                    jcode_base::logging::info(
                        "OpenAI model catalog fetch rejected the access token; force-refreshing and retrying",
                    );
                    let refreshed =
                        super::super::openai_stream_runtime::force_refresh_openai_token(
                            &self.credentials,
                            &refresh_token,
                            account_label.clone(),
                        )
                        .await
                        .map_err(|refresh_err| {
                            err.context(format!(
                            "token force-refresh after catalog 401/403 also failed: {refresh_err:#}"
                        ))
                        })?;
                    jcode_base::provider::fetch_openai_model_catalog(&refreshed).await?
                }
                Err(err) => return Err(err),
            }
        } else {
            jcode_base::provider::fetch_openai_api_key_model_catalog(&access_token).await?
        };
        let current_credential_identity = {
            let credentials = self.credentials.read().await;
            Self::catalog_credential_identity(&credentials)
        };
        if current_credential_identity != credential_identity
            || self.refresh_account_label() != account_label
        {
            jcode_base::logging::info(
                "Discarding OpenAI model catalog fetched for credentials that are no longer active",
            );
            return Ok(());
        }
        match self.model_reasoning_efforts.write() {
            Ok(mut efforts) => *efforts = catalog.reasoning_efforts.clone(),
            Err(poisoned) => *poisoned.into_inner() = catalog.reasoning_efforts.clone(),
        }
        *self
            .model_service_tiers
            .write()
            .unwrap_or_else(|p| p.into_inner()) = catalog.service_tiers.clone();
        self.warn_unsupported_service_tier();
        self.revalidate_reasoning_effort();
        jcode_base::provider::persist_openai_model_catalog_for_scope(&scope, &catalog);
        if !catalog.context_limits.is_empty() {
            jcode_base::provider::populate_context_limits(catalog.context_limits);
        }
        if !catalog.available_models.is_empty() {
            jcode_base::provider::populate_account_models_for_scope(
                &scope,
                catalog.available_models,
            );
        }
        Ok(())
    }
}
