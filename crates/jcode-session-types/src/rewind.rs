use serde::{Deserialize, Serialize};

/// Which parts of a session should return to the selected rewind boundary.
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

/// How a selected message determines the rewind boundary and composer behavior.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewindTargetKind {
    /// Remove this input and its future, then restore the input to the editor.
    #[default]
    UserPrompt,
    /// Keep this assistant message, remove its future, and preserve the editor.
    AssistantMessage,
}

impl RewindTargetKind {
    pub fn is_user_prompt(&self) -> bool {
        matches!(self, Self::UserPrompt)
    }
}

/// A conversation target with an optional durable file checkpoint.
///
/// The ID is scoped to its session and stays stable across reloads. Clients
/// must send the ID back, rather than deriving a target from displayed rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewindCheckpoint {
    pub id: String,
    #[serde(default, skip_serializing_if = "RewindTargetKind::is_user_prompt")]
    pub target_kind: RewindTargetKind,
    /// Exclusive stored boundary, not legacy `/rewind N` numbering. Before the
    /// original user input, or immediately after the selected assistant message.
    pub message_index: usize,
    /// Original user prompt or assistant excerpt for the selector.
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    pub can_restore_files: bool,
    pub changed_file_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewindResult {
    pub mode: RewindMode,
    #[serde(default, skip_serializing_if = "RewindTargetKind::is_user_prompt")]
    pub target_kind: RewindTargetKind,
    /// Original user input, ready to edit after conversation restore. Empty for
    /// assistant targets, which must leave the current composer unchanged.
    pub prompt: String,
    /// Original user attachments as (media type, base64 data), in input order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<(String, String)>,
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

    #[test]
    fn legacy_results_default_images_and_attachment_tuples_roundtrip() {
        let legacy = serde_json::json!({
            "mode": "conversation_only",
            "prompt": "original prompt",
            "removed_messages": 2,
            "restored_files": 0,
        });
        let mut result: RewindResult = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(result.target_kind, RewindTargetKind::UserPrompt);
        assert!(result.images.is_empty());
        assert_eq!(serde_json::to_value(&result).unwrap(), legacy);
        result.images = vec![
            ("image/png".into(), "b25l".into()),
            ("image/jpeg".into(), "dHdv".into()),
        ];
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(
            value["images"],
            serde_json::json!([["image/png", "b25l"], ["image/jpeg", "dHdv"],])
        );
        assert_eq!(
            serde_json::from_value::<RewindResult>(value).unwrap(),
            result
        );
    }

    #[test]
    fn rewind_target_kinds_preserve_legacy_payloads_and_roundtrip_assistant_targets() {
        let legacy = serde_json::json!({
            "id": "message:original-input",
            "message_index": 0,
            "prompt": "original input",
            "can_restore_files": false,
            "changed_file_count": 0,
        });
        let mut checkpoint: RewindCheckpoint = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(checkpoint.target_kind, RewindTargetKind::UserPrompt);
        assert_eq!(serde_json::to_value(&checkpoint).unwrap(), legacy);
        checkpoint.id = "assistant:original-reply".into();
        checkpoint.target_kind = RewindTargetKind::AssistantMessage;
        checkpoint.message_index = 2;
        let value = serde_json::to_value(&checkpoint).unwrap();
        assert_eq!(value["target_kind"], "assistant_message");
        assert_eq!(
            serde_json::from_value::<RewindCheckpoint>(value).unwrap(),
            checkpoint
        );

        let value = serde_json::json!({
            "mode": "conversation_only",
            "target_kind": "assistant_message",
            "prompt": "",
            "removed_messages": 2,
            "restored_files": 0,
        });
        let result: RewindResult = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(result.target_kind, RewindTargetKind::AssistantMessage);
        assert!(result.prompt.is_empty());
        assert_eq!(serde_json::to_value(result).unwrap(), value);
    }
}
