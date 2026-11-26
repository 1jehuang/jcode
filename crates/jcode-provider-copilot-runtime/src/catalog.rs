//! Live model discovery and persistent catalog lifecycle.

use super::*;

#[derive(serde::Serialize)]
struct PersistedCatalogRef<'a> {
    models: &'a [String],
    fetched_at_rfc3339: String,
}

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
        let payload = PersistedCatalogRef {
            models,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_catalog_ref_matches_legacy_json_and_borrows_models() {
        for models in [
            vec![],
            vec!["copilot-future-2099".to_owned()],
            vec!["未来/🦀".to_owned(), "quoted\"\\\n\tmodel".to_owned()],
        ] {
            let timestamp = "2026-11-16T06:27:30.477+00:00".to_owned();
            let payload = PersistedCatalogRef {
                models: &models,
                fetched_at_rfc3339: timestamp.clone(),
            };
            let legacy = PersistedCatalog {
                models: models.clone(),
                fetched_at_rfc3339: timestamp,
            };
            assert!(std::ptr::eq(payload.models, models.as_slice()));
            for (borrowed, original) in payload.models.iter().zip(&models) {
                assert_eq!(borrowed.as_ptr(), original.as_ptr());
            }
            assert_eq!(
                serde_json::to_vec(&payload).unwrap(),
                serde_json::to_vec(&legacy).unwrap()
            );
            assert_eq!(
                serde_json::to_vec_pretty(&payload).unwrap(),
                serde_json::to_vec_pretty(&legacy).unwrap()
            );
        }
    }
}
