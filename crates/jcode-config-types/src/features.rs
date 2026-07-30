//! Runtime feature toggles (the `[features]` config section).
//!
//! Split out of `lib.rs`, which is over the code-size budget, so this
//! section can gain fields without pushing that file further past it.

use serde::{Deserialize, Serialize};

use crate::UpdateChannel;

/// Runtime feature toggles
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FeatureConfig {
    /// Enable memory retrieval/extraction features (default: true)
    pub memory: bool,
    /// Enable swarm coordination features (default: true)
    pub swarm: bool,
    /// Enable Mermaid rendering and Mermaid-specific model guidance (default: true)
    pub mermaid: bool,
    /// Inject timestamps into user messages and tool results sent to the model (default: true)
    pub message_timestamps: bool,
    /// Persist auto-recalled memory injections into normal session history instead of sending
    /// them as request-only ephemeral suffix messages (default: false)
    pub persist_memory_injections: bool,
    /// Surface an in-chat system message whenever a request misses the KV cache
    /// for a harness-caused (avoidable) reason: the system prompt, tool set, or
    /// message prefix changed without the conversation legitimately growing.
    /// These should essentially never happen, so the notice acts as a loud alarm
    /// that something in the harness silently invalidated the prefix cache
    /// (default: true).
    pub kv_cache_miss_notices: bool,
    /// Update channel: "stable" (releases only) or "main" (latest commits)
    pub update_channel: UpdateChannel,
    /// Whether auto-poke (automatic follow-ups while todos are incomplete)
    /// starts enabled in a new session (default: true). `/poke on|off` still
    /// overrides it for the running session (issue #664).
    pub auto_poke: bool,
}

impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            memory: true,
            swarm: true,
            mermaid: true,
            message_timestamps: true,
            persist_memory_injections: false,
            kv_cache_miss_notices: true,
            update_channel: UpdateChannel::default(),
            auto_poke: true,
        }
    }
}
