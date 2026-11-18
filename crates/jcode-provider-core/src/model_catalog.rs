use crate::{ModelRoute, Provider};
use serde::{Deserialize, Serialize};

/// Canonical snapshot of a provider's model catalog at a point in time.
///
/// This is the local contract shared by server-side providers, remote clients,
/// and persisted remote catalog caches. The websocket wire format may still
/// flatten these fields for backwards compatibility, but internal code should
/// pass catalog state as this single value instead of loose parallel vectors.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelCatalogSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available_models: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_routes: Vec<ModelRoute>,
}

impl ModelCatalogSnapshot {
    pub fn new(
        provider_name: Option<String>,
        provider_model: Option<String>,
        available_models: Vec<String>,
        model_routes: Vec<ModelRoute>,
    ) -> Self {
        Self {
            provider_name,
            provider_model,
            available_models,
            model_routes,
        }
    }

    pub fn from_provider(provider: &dyn Provider) -> Self {
        provider.model_catalog_snapshot()
    }

    pub fn has_routes(&self) -> bool {
        !self.model_routes.is_empty()
    }
}
