use serde::{Deserialize, Serialize};

/// Type of notification from another agent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum NotificationType {
    /// Another agent touched a file you've worked with
    #[serde(rename = "file_conflict")]
    FileConflict {
        path: String,
        /// What the other agent did: "read", "wrote", "edited"
        operation: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// Another agent shared context
    #[serde(rename = "shared_context")]
    SharedContext { key: String, value: String },
    /// Direct message from another agent
    #[serde(rename = "message")]
    Message {
        /// Message scope: "dm", "channel", or "broadcast"
        #[serde(skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        /// Channel name for channel messages (e.g. "parser")
        #[serde(skip_serializing_if = "Option::is_none")]
        channel: Option<String>,
    },
}

/// Runtime feature names that can be toggled per session
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FeatureToggle {
    Memory,
    Swarm,
    Autoreview,
    Autojudge,
}
