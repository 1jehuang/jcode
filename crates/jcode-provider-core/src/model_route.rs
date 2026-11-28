use crate::RouteCheapnessEstimate;
use serde::{Deserialize, Serialize};

/// A single route to access a model: model + provider + API method
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRoute {
    pub model: String,
    pub provider: String,
    pub api_method: String,
    pub available: bool,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cheapness: Option<RouteCheapnessEstimate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<jcode_usage_types::ModelUsage>,
}
