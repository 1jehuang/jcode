use super::*;

impl CopilotApiProvider {
    /// Fetch the live model catalog and select a default unless the user chose a model.
    pub async fn detect_tier_and_set_default(&self) {
        let _refresh = self.catalog_refresh.lock().await;
        self.refresh_model_catalog().await;
        self.mark_init_done();
    }

    fn has_live_model_catalog(&self) -> bool {
        *self
            .catalog_source
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            == CatalogSource::Live
    }

    pub(super) async fn ensure_model_catalog(&self) {
        if self.has_live_model_catalog() {
            return;
        }
        let _refresh = self.catalog_refresh.lock().await;
        if !self.has_live_model_catalog() {
            self.refresh_model_catalog().await;
        }
    }

    async fn refresh_model_catalog(&self) {
        let detect_start = std::time::Instant::now();
        let bearer_start = std::time::Instant::now();
        let bearer = match self.get_bearer_token().await {
            Ok(t) => t,
            Err(e) => {
                jcode_base::logging::info(&format!(
                    "Copilot tier detection: failed to get bearer token after {}ms: {}",
                    bearer_start.elapsed().as_millis(),
                    e
                ));
                return;
            }
        };

        let fetch_start = std::time::Instant::now();
        match copilot_auth::fetch_available_models(&self.client, &bearer.token, &bearer.api_base)
            .await
        {
            Ok(models) => {
                let picker_models: Vec<String> = models
                    .iter()
                    .filter(|m| m.model_picker_enabled)
                    .map(|m| m.id.clone())
                    .collect();
                let all_ids: Vec<String> = models.iter().map(|m| m.id.clone()).collect();
                let default = copilot_auth::choose_default_model(&models);
                jcode_base::logging::info(&format!(
                    "Copilot tier detection: bearer={}ms, fetch_models={}ms, total={}ms, {} total, {} picker-enabled, detected default -> {:?}. Picker: [{}]. All: [{}]",
                    bearer_start.elapsed().as_millis(),
                    fetch_start.elapsed().as_millis(),
                    detect_start.elapsed().as_millis(),
                    all_ids.len(),
                    picker_models.len(),
                    default,
                    picker_models.join(", "),
                    all_ids.join(", ")
                ));
                self.set_detected_default(default);
                let routes = models
                    .into_iter()
                    .map(|model| {
                        let uses_responses =
                            copilot_model_uses_responses_api(&model.id, &model.supported_endpoints);
                        (model.id, uses_responses)
                    })
                    .collect();
                *self
                    .model_routes
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = routes;
                let display_models = if picker_models.is_empty() {
                    all_ids
                } else {
                    picker_models
                };
                if let Ok(mut fm) = self.fetched_models.try_write() {
                    *fm = display_models;
                }
                if let Ok(mut source) = self.catalog_source.try_write() {
                    *source = CatalogSource::Live;
                }
                Self::persist_catalog(
                    &self
                        .fetched_models
                        .try_read()
                        .map(|models| models.clone())
                        .unwrap_or_default(),
                );
            }
            Err(e) => {
                jcode_base::logging::info(&format!(
                    "Copilot tier detection: bearer={}ms, fetch_models={}ms, total={}ms, failed to fetch models: {}",
                    bearer_start.elapsed().as_millis(),
                    fetch_start.elapsed().as_millis(),
                    detect_start.elapsed().as_millis(),
                    e
                ));
            }
        }
    }
}
