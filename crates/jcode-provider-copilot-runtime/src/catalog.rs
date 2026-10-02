use super::*;

const DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const FAILURE_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Default)]
pub(super) struct ModelCatalog {
    pub models: Vec<String>,
    pub routes: HashMap<String, bool>,
    pub model_efforts: HashMap<String, Vec<String>>,
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

#[derive(serde::Deserialize)]
struct CachedCatalog {
    #[serde(flatten)]
    catalog: PersistedCatalog,
    #[serde(default)]
    model_efforts: HashMap<String, Vec<String>>,
}

#[derive(serde::Serialize)]
struct CatalogSnapshot<'a> {
    models: &'a [String],
    fetched_at_rfc3339: String,
    model_efforts: &'a HashMap<String, Vec<String>>,
}

impl CopilotApiProvider {
    fn load_persisted_catalog() -> Option<CachedCatalog> {
        let path = Self::persisted_catalog_path().ok()?;
        jcode_base::storage::read_json(&path)
            .ok()
            .filter(|cached: &CachedCatalog| !cached.catalog.models.is_empty())
    }

    fn persist_catalog(catalog: &ModelCatalog) {
        if catalog.models.is_empty() {
            return;
        }
        let Ok(path) = Self::persisted_catalog_path() else {
            return;
        };
        let payload = CatalogSnapshot {
            models: &catalog.models,
            fetched_at_rfc3339: Utc::now().to_rfc3339(),
            model_efforts: &catalog.model_efforts,
        };
        if let Err(error) = jcode_base::storage::write_json(&path, &payload) {
            jcode_base::logging::warn(&format!(
                "Failed to persist Copilot model catalog {}: {}",
                path.display(),
                error
            ));
        }
    }

    pub(super) fn seed_cached_catalog(&self) {
        if let Some(cached) = Self::load_persisted_catalog() {
            self.apply_cached_catalog(cached);
        }
    }

    fn apply_cached_catalog(&self, cached: CachedCatalog) {
        let mut catalog = self.catalog.write();
        catalog.models = cached.catalog.models;
        catalog.model_efforts = cached.model_efforts;
        catalog.source = CatalogSource::Cached;
    }

    /// Fetch the live model catalog and select a default unless the user chose a model.
    pub async fn detect_tier_and_set_default(&self) {
        let _refresh = self.catalog_refresh.lock().await;
        match self.get_bearer_token().await {
            Ok(bearer) => {
                self.refresh_model_catalog(&bearer, tokio::time::Instant::now() + DISCOVERY_TIMEOUT)
                    .await
            }
            Err(error) => jcode_base::logging::info(&format!(
                "Copilot tier detection: failed to get bearer token: {error}"
            )),
        }
        self.mark_init_done();
    }

    fn catalog_needs_refresh(&self, api_base: &str) -> bool {
        self.catalog.read().needs_refresh(api_base)
    }

    pub(super) async fn ensure_model_catalog(
        &self,
        bearer: &copilot_auth::CopilotApiToken,
        model: &str,
    ) {
        if !self.catalog_needs_refresh(&bearer.api_base) {
            return;
        }
        let deadline = tokio::time::Instant::now() + DISCOVERY_TIMEOUT;
        // Known routes can proceed immediately; unknown models need the shared result.
        let _refresh = match self.catalog_refresh.try_lock() {
            Ok(refresh) => refresh,
            Err(_) if self.known_model_route(model, &bearer.api_base).is_some() => return,
            Err(_) => match tokio::time::timeout_at(deadline, self.catalog_refresh.lock()).await {
                Ok(refresh) => refresh,
                Err(_) => return,
            },
        };
        if self.catalog_needs_refresh(&bearer.api_base) {
            self.refresh_model_catalog(bearer, deadline).await;
        }
    }

    async fn refresh_model_catalog(
        &self,
        bearer: &copilot_auth::CopilotApiToken,
        deadline: tokio::time::Instant,
    ) {
        let started = std::time::Instant::now();
        // Cover connection, response headers, and the complete JSON body.
        let result = tokio::time::timeout_at(
            deadline,
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
                let mut model_efforts = HashMap::new();
                let routes = models
                    .into_iter()
                    .map(|mut model| {
                        if let Some(efforts) = model
                            .capabilities
                            .as_mut()
                            .and_then(|capabilities| capabilities.supports.as_mut())
                            .and_then(|supports| supports.reasoning_effort.take())
                        {
                            model_efforts.insert(model.id.clone(), efforts);
                        }
                        let uses_responses =
                            copilot_model_uses_responses_api(&model.id, &model.supported_endpoints);
                        (model.id, uses_responses)
                    })
                    .collect();
                self.set_detected_default(default);
                // Readers see one host-scoped snapshot, never Live with old metadata.
                {
                    let mut catalog = self.catalog.write();
                    *catalog = ModelCatalog {
                        models: display_models,
                        routes,
                        model_efforts,
                        source: CatalogSource::Live,
                        api_base: Some(bearer.api_base.clone()),
                        retry_after: None,
                    };
                }
                let catalog = self.catalog.read();
                Self::persist_catalog(&catalog);
                jcode_base::logging::info(&format!(
                    "Copilot catalog: host={}, fetched in {}ms, {} models",
                    bearer.api_base,
                    started.elapsed().as_millis(),
                    catalog.models.len()
                ));
            }
            Err(error) => {
                let mut catalog = self.catalog.write();
                if catalog.api_base.as_deref() != Some(&bearer.api_base) {
                    // Never reuse routes advertised by another authenticated API host.
                    catalog.routes.clear();
                    if catalog.api_base.is_some() {
                        catalog.models.clear();
                        catalog.model_efforts.clear();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_cache_restores_models_without_disabling_sonnet_fallback() {
        let cached: CachedCatalog = serde_json::from_value(json!({
            "models": ["claude-sonnet-5", "gpt-4o"],
            "fetched_at_rfc3339": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        let provider = crate::tests::make_test_provider(vec!["stale-model".to_string()]);
        provider
            .catalog
            .write()
            .model_efforts
            .insert("claude-sonnet-5".to_string(), Vec::new());

        provider.apply_cached_catalog(cached);

        assert_eq!(
            provider.available_models_display(),
            vec!["claude-sonnet-5".to_string(), "gpt-4o".to_string()]
        );
        assert_eq!(provider.catalog.read().source, CatalogSource::Cached);
        assert_eq!(
            provider.efforts_for_model("claude-sonnet-5"),
            vec!["low", "medium", "high", "xhigh", "max"]
        );
        assert!(provider.efforts_for_model("gpt-4o").is_empty());
    }

    #[test]
    fn snapshot_restores_advertised_levels_and_explicit_empty_capabilities() {
        let levels = vec![
            "low".to_string(),
            "high".to_string(),
            "future-level".to_string(),
        ];
        let catalog = ModelCatalog {
            models: vec!["gpt-5.5".to_string(), "claude-sonnet-5".to_string()],
            model_efforts: HashMap::from([
                ("gpt-5.5".to_string(), levels.clone()),
                ("claude-sonnet-5".to_string(), Vec::new()),
            ]),
            ..ModelCatalog::default()
        };
        let snapshot = CatalogSnapshot {
            models: &catalog.models,
            fetched_at_rfc3339: "2026-01-01T00:00:00Z".to_string(),
            model_efforts: &catalog.model_efforts,
        };
        let bytes = serde_json::to_vec(&snapshot).unwrap();
        let cached = serde_json::from_slice(&bytes).unwrap();
        let provider = crate::tests::make_test_provider(Vec::new());

        provider.apply_cached_catalog(cached);

        assert_eq!(provider.available_models_display(), catalog.models);
        assert_eq!(
            provider.catalog.read().model_efforts.get("gpt-5.5"),
            Some(&levels)
        );
        assert_eq!(provider.efforts_for_model("gpt-5.5"), vec!["low", "high"]);
        assert!(provider.efforts_for_model("claude-sonnet-5").is_empty());
        provider.set_model("claude-sonnet-5").unwrap();
        assert!(provider.set_reasoning_effort("high").is_err());
    }
}
