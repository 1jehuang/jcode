//! Live model discovery and persistent catalog lifecycle.

use super::*;

#[derive(Serialize)]
struct BorrowedCatalog<'a> {
    models: &'a [String],
    inference_profiles: &'a [String],
    #[serde(serialize_with = "serialize_model_set")]
    profile_required_models: &'a HashSet<String>,
    inference_profile_routes: &'a HashMap<String, String>,
    #[serde(serialize_with = "serialize_model_set")]
    legacy_models: &'a HashSet<String>,
    region: Option<String>,
    fetched_at_rfc3339: String,
}

fn serialize_model_set<S: serde::Serializer>(
    models: &HashSet<String>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    // Match the legacy iter().cloned().collect::<Vec<_>>() sequence without
    // allocating a temporary vector or cloning its strings.
    serializer.collect_seq(models.iter())
}

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
        let payload = BorrowedCatalog {
            models,
            inference_profiles,
            profile_required_models,
            inference_profile_routes,
            legacy_models,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borrowed_catalog_preserves_exact_legacy_json() {
        // Exercise independently empty/populated collections, including a future
        // model that is not part of any built-in model list. No AWS calls needed.
        for mask in 0..32 {
            let models = if mask & 1 != 0 {
                vec![
                    "future.vendor.model-2099-v42:0".to_owned(),
                    "model-b".to_owned(),
                ]
            } else {
                vec![]
            };
            let profiles = if mask & 2 != 0 {
                vec!["us.future.vendor.model-2099-v42:0".to_owned()]
            } else {
                vec![]
            };
            let required = if mask & 4 != 0 {
                HashSet::from(["required-a".to_owned(), "required-b".to_owned()])
            } else {
                HashSet::new()
            };
            let routes = if mask & 8 != 0 {
                HashMap::from([
                    ("model-a".to_owned(), "profile-a".to_owned()),
                    ("model-b".to_owned(), "profile-b".to_owned()),
                ])
            } else {
                HashMap::new()
            };
            let legacy_models = if mask & 16 != 0 {
                HashSet::from(["legacy-a".to_owned(), "legacy-b".to_owned()])
            } else {
                HashSet::new()
            };
            for region in [None, Some("us-west-2".to_owned())] {
                let timestamp = "2026-11-16T06:29:12+00:00".to_owned();
                let legacy = PersistedCatalog {
                    models: models.clone(),
                    inference_profiles: profiles.clone(),
                    profile_required_models: required.iter().cloned().collect(),
                    inference_profile_routes: routes.clone(),
                    legacy_models: legacy_models.iter().cloned().collect(),
                    region: region.clone(),
                    fetched_at_rfc3339: timestamp.clone(),
                };
                let borrowed = BorrowedCatalog {
                    models: &models,
                    inference_profiles: &profiles,
                    profile_required_models: &required,
                    inference_profile_routes: &routes,
                    legacy_models: &legacy_models,
                    region,
                    fetched_at_rfc3339: timestamp,
                };
                assert_eq!(
                    serde_json::to_string(&borrowed).unwrap(),
                    serde_json::to_string(&legacy).unwrap(),
                    "compact JSON mismatch for mask {mask}"
                );
                let pretty = serde_json::to_string_pretty(&borrowed).unwrap();
                assert_eq!(
                    pretty,
                    serde_json::to_string_pretty(&legacy).unwrap(),
                    "pretty JSON mismatch for mask {mask}"
                );
                let decoded: PersistedCatalog = serde_json::from_str(&pretty).unwrap();
                assert_eq!(decoded.models, models);
                assert_eq!(decoded.inference_profiles, profiles);
                assert_eq!(decoded.inference_profile_routes, routes);
            }
        }
    }
}
