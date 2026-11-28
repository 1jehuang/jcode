//! Identity evidence for a saved OAuth account, independent of its display label.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountPin {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_id: Option<String>,
}

impl AccountPin {
    pub fn new(label: impl Into<String>, identity: Option<String>) -> Self {
        Self {
            label: label.into(),
            identity,
            record_id: None,
        }
    }
}
