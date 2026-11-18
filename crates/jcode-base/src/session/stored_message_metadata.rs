use crate::{
    message::{ContentBlock, Role},
    process_memory::estimate_json_bytes,
};
use jcode_session_types::{StoredDisplayRole, StoredMessage, StoredTokenUsage};

/// Same ordered envelope and omissions as StoredMessage, without cloning its
/// id, token metadata, or content Arc just to serialize an empty array.
#[derive(serde::Serialize)]
struct Metadata<'a> {
    id: &'a str,
    role: &'a Role,
    content: &'a [ContentBlock],
    #[serde(skip_serializing_if = "Option::is_none")]
    display_role: &'a Option<StoredDisplayRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: &'a Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_duration_ms: &'a Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_usage: &'a Option<StoredTokenUsage>,
}

fn metadata(message: &StoredMessage) -> Metadata<'_> {
    Metadata {
        id: &message.id,
        role: &message.role,
        content: &[],
        display_role: &message.display_role,
        timestamp: &message.timestamp,
        tool_duration_ms: &message.tool_duration_ms,
        token_usage: &message.token_usage,
    }
}

pub(super) fn json_bytes(message: &StoredMessage) -> usize {
    estimate_json_bytes(&metadata(message)).saturating_sub(2)
        + message.content.serialized_json_bytes()
}

#[cfg(test)]
#[path = "stored_message_metadata_tests.rs"]
mod tests;
