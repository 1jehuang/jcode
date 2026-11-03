//! Live model discovery and persistent catalog lifecycle.

use super::*;

impl GeminiProvider {
    pub(super) fn persisted_catalog_path() -> Result<std::path::PathBuf> {
        Ok(jcode_base::storage::app_config_dir()?.join("gemini_models_cache.json"))
    }

    pub(super) fn load_persisted_catalog() -> Option<PersistedCatalog> {
        // The persisted catalog is an optional warm-start hint. Missing or
        // unreadable configuration/cache data must leave live discovery usable.
        let path = match Self::persisted_catalog_path() {
            Ok(path) => path,
            Err(error) => {
                jcode_base::logging::debug(&format!(
                    "Model catalog cache path unavailable: {error}"
                ));
                return None;
            }
        };
        match jcode_base::storage::read_json(&path) {
            Ok(catalog) => {
                Some(catalog).filter(|catalog: &PersistedCatalog| !catalog.models.is_empty())
            }
            Err(error) => {
                jcode_base::logging::debug(&format!(
                    "Model catalog cache {} unavailable: {error}",
                    path.display()
                ));
                None
            }
        }
    }

    pub(super) fn persist_catalog(models: &[String]) {
        if models.is_empty() {
            return;
        }
        let Ok(path) = Self::persisted_catalog_path() else {
            return;
        };
        let payload = PersistedCatalog {
            models: models.to_vec(),
            fetched_at_rfc3339: Utc::now().to_rfc3339(),
        };
        if let Err(error) = jcode_base::storage::write_json(&path, &payload) {
            jcode_base::logging::warn(&format!(
                "Failed to persist Gemini model catalog {}: {}",
                path.display(),
                error
            ));
        }
    }

    pub(super) fn seed_cached_catalog(&self) {
        if let Some(catalog) = Self::load_persisted_catalog()
            && let Ok(mut models) = self.fetched_models.write()
        {
            *models = catalog.models;
        }
    }

    pub(super) async fn refresh_available_models(&self) -> Result<Vec<String>> {
        if let GeminiAuthMode::ApiKey(api_key) = Self::auth_mode() {
            return self.refresh_available_models_api_key(&api_key).await;
        }
        let project_id_env = gemini_auth::cloud_project();
        let load_req = load_code_assist_request(
            project_id_env.clone(),
            client_metadata(project_id_env.clone()),
        );
        let response: Value = match self.post_json("loadCodeAssist", &load_req).await {
            Ok(response) => response,
            Err(err) if is_vpc_sc_error(&err) => Value::Null,
            Err(err) => {
                return Err(err).context("Gemini model discovery failed during loadCodeAssist");
            }
        };

        let models = extract_gemini_model_ids(&response);
        if !models.is_empty() {
            jcode_base::logging::info(&format!(
                "Discovered Gemini Code Assist models: {}",
                models.join(", ")
            ));
            if let Ok(mut guard) = self.fetched_models.write() {
                *guard = models.clone();
            }
            Self::persist_catalog(&models);
        }
        Ok(models)
    }

    /// Discover models via the official Developer API `ListModels` endpoint.
    /// Returned names look like `models/gemini-2.5-pro`, so strip the prefix
    /// before normalizing through the shared catalog merge.
    pub(super) async fn refresh_available_models_api_key(
        &self,
        api_key: &str,
    ) -> Result<Vec<String>> {
        let catalog_url = format!("{}/models", Self::developer_api_base_url());
        let mut url = reqwest::Url::parse(&catalog_url)?;
        let mut raw = Vec::new();
        let mut seen_tokens = std::collections::HashSet::new();
        loop {
            let response: Value = self
                .get_json_api_key(url.as_str(), api_key, "ListModels")
                .await
                .context("Gemini Developer API model discovery failed")?;
            let page = response
                .get("models")
                .and_then(Value::as_array)
                .context("Gemini ListModels response missing models array")?;
            raw.extend(page.iter().filter_map(|model| {
                // Exclude embedding-only models even if their names look like
                // chat models. Missing metadata remains compatible with proxies.
                if model
                    .get("supportedGenerationMethods")
                    .and_then(Value::as_array)
                    .is_some_and(|methods| {
                        !methods.iter().any(|method| method == "generateContent")
                    })
                {
                    return None;
                }
                model
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|name| name.trim_start_matches("models/").to_string())
            }));
            let Some(token) = response
                .get("nextPageToken")
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
            else {
                break;
            };
            anyhow::ensure!(
                seen_tokens.insert(token.to_string()) && seen_tokens.len() <= 100,
                "Gemini ListModels pagination repeated a token or exceeded 100 pages"
            );
            // Tokens are opaque and must be URL-encoded, not interpolated.
            url.set_query(None);
            url.query_pairs_mut().append_pair("pageToken", token);
        }

        let models = merge_gemini_model_lists(raw);
        if !models.is_empty() {
            jcode_base::logging::info(&format!(
                "Discovered Gemini Developer API models: {}",
                models.join(", ")
            ));
            if let Ok(mut guard) = self.fetched_models.write() {
                *guard = models.clone();
            }
            Self::persist_catalog(&models);
        }
        Ok(models)
    }
}
