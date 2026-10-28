//! Live model discovery and persistent catalog lifecycle.

use super::*;

impl CopilotApiProvider {
    pub(super) fn persisted_catalog_path() -> Result<std::path::PathBuf> {
        Ok(jcode_base::storage::app_config_dir()?.join("copilot_models_cache.json"))
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
                "Failed to persist Copilot model catalog {}: {}",
                path.display(),
                error
            ));
        }
    }

    pub(super) fn seed_cached_catalog(&self) {
        if let Some(catalog) = Self::load_persisted_catalog() {
            if let Ok(mut models) = self.fetched_models.try_write() {
                *models = catalog.models;
            }
            if let Ok(mut source) = self.catalog_source.try_write() {
                *source = CatalogSource::Cached;
            }
        }
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
