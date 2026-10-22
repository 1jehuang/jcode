use serde::{Deserialize, Serialize};

/// Which parts of a session should return to the state before a user prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewindMode {
    ConversationAndFiles,
    ConversationOnly,
    FilesOnly,
}

impl RewindMode {
    pub fn restores_conversation(self) -> bool {
        matches!(self, Self::ConversationAndFiles | Self::ConversationOnly)
    }

    pub fn restores_files(self) -> bool {
        matches!(self, Self::ConversationAndFiles | Self::FilesOnly)
    }
}

/// A durable checkpoint immediately before a user prompt.
///
/// The ID is scoped to its session and stays stable across reloads. Clients
/// must send the ID back, rather than deriving a target from displayed rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewindCheckpoint {
    pub id: String,
    /// Zero-based stored user-message boundary, not legacy `/rewind N` numbering.
    pub message_index: usize,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    pub can_restore_files: bool,
    pub changed_file_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewindResult {
    pub mode: RewindMode,
    /// The original prompt, ready to edit and resend after conversation restore.
    pub prompt: String,
    pub removed_messages: usize,
    pub restored_files: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_files: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_select_only_the_requested_state() {
        assert!(RewindMode::ConversationAndFiles.restores_conversation());
        assert!(RewindMode::ConversationAndFiles.restores_files());
        assert!(RewindMode::ConversationOnly.restores_conversation());
        assert!(!RewindMode::ConversationOnly.restores_files());
        assert!(!RewindMode::FilesOnly.restores_conversation());
        assert!(RewindMode::FilesOnly.restores_files());
    }
}
