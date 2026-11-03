//! Live model discovery and persistent catalog lifecycle.

use super::*;

impl CopilotApiProvider {
    pub(super) fn persisted_catalog_filename(&self) -> String {
        if self.endpoints.is_enterprise() {
            format!("copilot_models_cache.{}.json", self.endpoints.host)
        } else {
            "copilot_models_cache.json".to_string()
        }
    }

    pub(super) fn persisted_catalog_path(&self) -> Result<std::path::PathBuf> {
        Ok(jcode_base::storage::app_config_dir()?.join(self.persisted_catalog_filename()))
    }

    pub(super) fn load_persisted_catalog(&self) -> Option<PersistedCatalog> {
        let path = match self.persisted_catalog_path() {
            Ok(path) => path,
            Err(error) => {
                jcode_base::logging::warn(&format!(
                    "Could not resolve Copilot catalog cache: {error:#}"
                ));
                return None;
            }
        };
        if !path.exists() {
            return None;
        }
        match jcode_base::storage::read_json(&path) {
            Ok(catalog) => {
                Some(catalog).filter(|catalog: &PersistedCatalog| !catalog.models.is_empty())
            }
            Err(error) => {
                jcode_base::logging::warn(&format!(
                    "Could not load Copilot catalog cache {}: {error:#}",
                    path.display()
                ));
                None
            }
        }
    }

    pub(super) fn persist_catalog(&self, models: &[String]) {
        if models.is_empty() {
            return;
        }
        let path = match self.persisted_catalog_path() {
            Ok(path) => path,
            Err(error) => {
                jcode_base::logging::warn(&format!(
                    "Could not resolve Copilot catalog cache: {error:#}"
                ));
                return;
            }
        };
        let payload = PersistedCatalog {
            models: models.to_vec(),
            fetched_at_rfc3339: Utc::now().to_rfc3339(),
            model_metadata: self
                .model_metadata
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
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
        if let Some(catalog) = self.load_persisted_catalog() {
            self.restore_cached_catalog(catalog);
        } else {
            self.install_model_metadata(HashMap::new());
        }
    }

    pub(super) fn restore_cached_catalog(&self, catalog: PersistedCatalog) {
        self.install_model_metadata(catalog.model_metadata);
        if let Ok(mut models) = self.fetched_models.try_write() {
            *models = catalog.models;
        }
        if let Ok(mut source) = self.catalog_source.try_write() {
            *source = CatalogSource::Cached;
        }
        self.reconcile_reasoning_effort(&self.model());
    }

    pub(super) fn model_catalog_detail_impl(&self) -> String {
        match self
            .catalog_source
            .try_read()
            .map(|g| *g)
            .unwrap_or(CatalogSource::None)
        {
            CatalogSource::Live => String::new(),
            CatalogSource::Cached => "cached live catalog".to_string(),
            CatalogSource::None => "catalog still loading".to_string(),
        }
    }
}
