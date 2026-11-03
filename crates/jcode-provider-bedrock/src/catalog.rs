//! Live model discovery and persistent catalog lifecycle.

use super::*;

impl BedrockProvider {
    pub(super) fn persisted_catalog_path() -> Result<std::path::PathBuf> {
        Ok(jcode_storage::app_config_dir()?.join("bedrock_models_cache.json"))
    }

    pub(super) fn load_persisted_catalog() -> Option<PersistedCatalog> {
        // The persisted catalog is an optional warm-start hint. Missing or
        // unreadable configuration/cache data must leave live discovery usable.
        let path = match Self::persisted_catalog_path() {
            Ok(path) => path,
            Err(error) => {
                jcode_logging::debug(&format!("Model catalog cache path unavailable: {error}"));
                return None;
            }
        };
        match jcode_storage::read_json(&path) {
            Ok(catalog) => Some(catalog),
            Err(error) => {
                jcode_logging::debug(&format!(
                    "Model catalog cache {} unavailable: {error}",
                    path.display()
                ));
                None
            }
        }
    }

    // Only written from aws-sdk catalog refreshes, but kept ungated so cached
    // catalogs behave identically in both build modes (and for tests).
    #[cfg_attr(not(feature = "aws-sdk"), allow(dead_code))]
    pub(super) fn persist_catalog(
        models: &[String],
        inference_profiles: &[String],
        profile_required_models: &HashSet<String>,
        inference_profile_routes: &HashMap<String, String>,
        legacy_models: &HashSet<String>,
    ) {
        let Ok(path) = Self::persisted_catalog_path() else {
            return;
        };
        let payload = PersistedCatalog {
            models: models.to_vec(),
            inference_profiles: inference_profiles.to_vec(),
            profile_required_models: profile_required_models.iter().cloned().collect(),
            inference_profile_routes: inference_profile_routes.clone(),
            legacy_models: legacy_models.iter().cloned().collect(),
            region: Self::configured_region(),
            fetched_at_rfc3339: chrono::Utc::now().to_rfc3339(),
        };
        if let Err(err) = jcode_storage::write_json(&path, &payload) {
            jcode_logging::warn(&format!(
                "Failed to persist Bedrock model catalog {}: {}",
                path.display(),
                err
            ));
        }
    }

    pub(super) fn seed_cached_catalog(&self) {
        if let Some(catalog) = Self::load_persisted_catalog() {
            let configured_region = Self::configured_region();
            if catalog.region.as_deref() != configured_region.as_deref() {
                jcode_logging::info(&format!(
                    "Ignoring Bedrock model cache for region {:?}; configured region is {:?}",
                    catalog.region, configured_region
                ));
                return;
            }
            let PersistedCatalog {
                models: cached_models,
                inference_profiles,
                profile_required_models,
                inference_profile_routes,
                legacy_models,
                ..
            } = catalog;
            let mut inference_profile_routes = inference_profile_routes;
            Self::merge_profile_routes_from_profile_ids(
                &mut inference_profile_routes,
                inference_profiles.iter(),
            );
            if let Ok(mut guard) = self.fetched_models.write() {
                *guard = cached_models;
            }
            if let Ok(mut profiles) = self.fetched_inference_profiles.write() {
                *profiles = inference_profiles;
            }
            if let Ok(mut required) = self.profile_required_models.write() {
                *required = profile_required_models.into_iter().collect();
            }
            if let Ok(mut routes) = self.inference_profile_routes.write() {
                *routes = inference_profile_routes;
            }
            if let Ok(mut legacy) = self.legacy_models.write() {
                *legacy = legacy_models.into_iter().collect();
            }
        }
    }
}
