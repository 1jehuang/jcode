use super::*;

const DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const FAILURE_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Default)]
pub(super) struct ModelCatalog {
    pub models: Vec<String>,
    pub routes: HashMap<String, bool>,
    pub source: CatalogSource,
    pub api_base: Option<String>,
    retry_after: Option<std::time::Instant>,
}

impl ModelCatalog {
    fn needs_refresh(&self, api_base: &str) -> bool {
        self.api_base.as_deref() != Some(api_base)
            || (self.source != CatalogSource::Live
                && self
                    .retry_after
                    .is_none_or(|deadline| std::time::Instant::now() >= deadline))
    }
}

impl CopilotApiProvider {
    /// Fetch the live model catalog and select a default unless the user chose a model.
    pub async fn detect_tier_and_set_default(&self) {
        let _refresh = self.catalog_refresh.lock().await;
        match self.get_bearer_token().await {
            Ok(bearer) => self.refresh_model_catalog(&bearer).await,
            Err(error) => jcode_base::logging::info(&format!(
                "Copilot tier detection: failed to get bearer token: {error}"
            )),
        }
        self.mark_init_done();
    }

    fn catalog_needs_refresh(&self, api_base: &str) -> bool {
        self.catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .needs_refresh(api_base)
    }

    pub(super) async fn ensure_model_catalog(&self, bearer: &copilot_auth::CopilotApiToken) {
        if !self.catalog_needs_refresh(&bearer.api_base) {
            return;
        }
        // A concurrent refresh must not queue completions behind discovery.
        let Ok(_refresh) = self.catalog_refresh.try_lock() else {
            return;
        };
        if self.catalog_needs_refresh(&bearer.api_base) {
            self.refresh_model_catalog(bearer).await;
        }
    }

    async fn refresh_model_catalog(&self, bearer: &copilot_auth::CopilotApiToken) {
        let started = std::time::Instant::now();
        // Cover connection, response headers, and the complete JSON body.
        let result = tokio::time::timeout(
            DISCOVERY_TIMEOUT,
            copilot_auth::fetch_available_models(&self.client, &bearer.token, &bearer.api_base),
        )
        .await
        .map_err(anyhow::Error::from)
        .and_then(|result| result);
        match result {
            Ok(models) => {
                let default = copilot_auth::choose_default_model(&models);
                let mut display_models: Vec<String> = models
                    .iter()
                    .filter(|model| model.model_picker_enabled)
                    .map(|model| model.id.clone())
                    .collect();
                if display_models.is_empty() {
                    display_models = models.iter().map(|model| model.id.clone()).collect();
                }
                let routes = models
                    .into_iter()
                    .map(|model| {
                        let uses_responses =
                            copilot_model_uses_responses_api(&model.id, &model.supported_endpoints);
                        (model.id, uses_responses)
                    })
                    .collect();
                self.set_detected_default(default);
                // Readers see one host-scoped snapshot, never Live with an old picker list.
                {
                    let mut catalog = self
                        .catalog
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    *catalog = ModelCatalog {
                        models: display_models,
                        routes,
                        source: CatalogSource::Live,
                        api_base: Some(bearer.api_base.clone()),
                        retry_after: None,
                    };
                }
                let catalog = self
                    .catalog
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                Self::persist_catalog(&catalog.models);
                jcode_base::logging::info(&format!(
                    "Copilot catalog: host={}, fetched in {}ms, {} models",
                    bearer.api_base,
                    started.elapsed().as_millis(),
                    catalog.models.len()
                ));
            }
            Err(error) => {
                let mut catalog = self
                    .catalog
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if catalog.api_base.as_deref() != Some(&bearer.api_base) {
                    // Never reuse routes advertised by another authenticated API host.
                    catalog.routes.clear();
                    if catalog.api_base.is_some() {
                        catalog.models.clear();
                        catalog.source = CatalogSource::None;
                    }
                    catalog.api_base = Some(bearer.api_base.clone());
                }
                catalog.retry_after = Some(std::time::Instant::now() + FAILURE_COOLDOWN);
                jcode_base::logging::info(&format!(
                    "Copilot catalog: host={}, failed after {}ms: {error}; using fallback until next refresh",
                    bearer.api_base,
                    started.elapsed().as_millis()
                ));
            }
        }
    }
}
