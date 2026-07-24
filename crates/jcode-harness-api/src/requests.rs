//! Client-to-server requests: the curated stable surface.

use serde::{Deserialize, Serialize};

/// Curated request surface. Internally-tagged on `"req"`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "req", rename_all = "snake_case")]
pub enum ApiRequest {
    /// Version negotiation. Must be the first frame on a connection.
    Hello {
        min_version: u32,
        max_version: u32,
        /// Client name and version, e.g. "jcode-desktop2/0.1.0".
        client: String,
    },

    /// List sessions visible to this client.
    ListSessions,

    /// Create a new session (optionally in a working directory) and attach.
    CreateSession {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        working_dir: Option<String>,
    },

    /// Attach to an existing session and subscribe to its event stream.
    AttachSession { session_id: String },

    /// Detach from the currently attached session.
    DetachSession { session_id: String },

    /// Send a user message to the attached session.
    SendMessage {
        session_id: String,
        content: String,
        /// (media_type, base64_data) pairs.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<(String, String)>,
    },

    /// Cancel the in-flight generation.
    Cancel { session_id: String },

    /// Inject a message at the next safe point without cancelling.
    SoftInterrupt {
        session_id: String,
        content: String,
        #[serde(default)]
        urgent: bool,
    },

    /// Fetch conversation history.
    GetHistory { session_id: String },

    /// Clear conversation history.
    Clear { session_id: String },

    /// Rewind history to the given 1-based message index.
    Rewind {
        session_id: String,
        message_index: usize,
    },

    /// Reply to a `PermissionRequest` event.
    PermissionResponse {
        session_id: String,
        request_id: String,
        decision: PermissionDecision,
    },

    /// Liveness check.
    Ping,

    /// Forward-compatibility catch-all. Servers reply with an error frame.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    AllowAlways,
    Deny,
}
