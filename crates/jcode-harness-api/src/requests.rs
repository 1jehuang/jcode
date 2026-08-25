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
    ListSessions {
        /// Include sessions the user archived through this API.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        include_archived: bool,
        /// Return at most this many most-recently modified persisted sessions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },

    /// Reversibly hide a session from the default list. Its transcript remains
    /// on disk and can be restored at any time.
    ArchiveSession { session_id: String },

    /// Put an archived session back in the default list.
    RestoreSession { session_id: String },

    /// Configure automatic archival of inactive sessions. `None` disables it.
    SetRetentionPolicy {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        archive_after_days: Option<u32>,
    },

    /// Create a new session (optionally in a working directory) and attach.
    CreateSession {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        working_dir: Option<String>,
    },

    /// Attach to an existing session and subscribe to its event stream.
    AttachSession { session_id: String },

    /// Clone an attached session's transcript into a new, idle session.
    ForkSession { session_id: String },

    /// Detach from the currently attached session.
    DetachSession { session_id: String },

    /// Send a user message to the attached session.
    SendMessage {
        session_id: String,
        content: String,
        /// (media_type, base64_data) pairs.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<(String, String)>,
        /// Persist the message as context without starting a model turn.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        no_reply: bool,
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

    /// Fetch the tail of *any* session's conversation, attached or not.
    ///
    /// `GetHistory` can only answer for the session this connection is
    /// attached to, because it is routed through the daemon's attachment.
    /// A client showing several sessions at once (a switcher, an overview, a
    /// dashboard) needs a glance at the others without attaching to each in
    /// turn, which would disturb the very sessions it is trying to preview.
    /// Served from the stored record and capped to the last `limit` messages.
    PeekSession {
        session_id: String,
        /// Messages to return from the end. Defaults to a small tail.
        #[serde(default)]
        limit: Option<u32>,
    },

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

    /// List the models this session can switch to.
    ///
    /// A client that cannot enumerate models cannot offer a model picker, so
    /// it is stuck on whatever the daemon defaulted to. Served from the
    /// catalog the daemon already reports on attach.
    ListModels { session_id: String },

    /// Provider routes and active runtime identity for the attached session.
    GetRuntimeInfo { session_id: String },

    /// Persist an API-key credential in jcode's owner-only provider store and
    /// notify the daemon to reload it. OAuth tokens are intentionally excluded.
    SetApiKey { provider: String, api_key: String },

    /// Remove a previously persisted API-key credential.
    ClearApiKey { provider: String },

    /// Read one UTF-8 file under the session working directory.
    ReadFile {
        session_id: String,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<u64>,
    },

    /// Find files by case-insensitive path substring under the session root.
    FindFiles {
        session_id: String,
        query: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },

    /// Search UTF-8 files for a literal text string.
    SearchText {
        session_id: String,
        query: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },

    /// Read safe filesystem metadata for a path under the session root.
    FileStatus { session_id: String, path: String },

    /// Switch the session to a different model.
    ///
    /// `model` is an id from `ListModels`, e.g. `claude-opus-5`. A route
    /// suffix like `claude-opus-4-6[1m]` selects a specific context variant.
    SetModel { session_id: String, model: String },

    /// Set how much the model deliberates before answering.
    ///
    /// The cost/quality dial: `minimal`, `low`, `medium`, `high`, `xhigh`, or
    /// `max`, depending on what the provider supports. Providers that do not
    /// support it answer with an error rather than silently ignoring it.
    SetReasoningEffort { session_id: String, effort: String },

    /// Summarize the transcript so far, freeing context.
    ///
    /// Without this a long-lived client eventually hits the context limit and
    /// has no recourse but to clear the conversation and lose everything.
    Compact { session_id: String },

    /// Set a session's title, or clear it to restore the generated one.
    RenameSession {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },

    /// Restore the history that the last `Rewind` removed.
    ///
    /// `Rewind` is destructive, so without an undo a client cannot offer it
    /// safely: a mis-click costs the user their conversation.
    RewindUndo { session_id: String },

    /// Drop soft interrupts that have been queued but not yet delivered.
    ///
    /// The counterpart to `SoftInterrupt`: a client that lets a user queue a
    /// follow-up must also let them take it back before it lands.
    CancelSoftInterrupts { session_id: String },

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
