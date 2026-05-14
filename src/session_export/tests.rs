#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_to_json() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let output_path = temp_dir.path().join("session.json");

        let messages = vec![
            (MessageRole::User, "Hello".to_string()),
            (MessageRole::Assistant, "Hi there!".to_string()),
        ];

        let result = SessionExporter::export_to_json(
            "test-session-123",
            messages,
            &output_path
        );

        assert!(result.is_ok());
        assert!(result.unwrap().contains("Exported"));
        assert!(output_path.exists());

        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.contains("test-session-123"));
        assert!(content.contains("User"));
        assert!(content.contains("Assistant"));
    }

    #[test]
    fn test_export_to_markdown() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let output_path = temp_dir.path().join("session.md");

        let messages = vec![
            (MessageRole::User, "Help me debug".to_string()),
            (MessageRole::Assistant, "Sure, I can help with that.".to_string()),
            (MessageRole::Tool, "Running tests...".to_string()),
        ];

        let result = SessionExporter::export_to_markdown(
            "debug-session",
            messages,
            &output_path
        );

        assert!(result.is_ok());
        assert!(output_path.exists());

        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.contains("# Session Export: debug-session"));
        assert!(content.contains("## 👤 User"));
        assert!(content.contains("## 🤖 Assistant"));
        assert!(content.contains("## 🔧 Tool"));
    }

    #[test]
    fn test_export_empty_session() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let output_path = temp_dir.path().join("empty.json");

        let messages: Vec<(MessageRole, String)> = vec![];

        let result = SessionExporter::export_to_json(
            "empty-session",
            messages,
            &output_path
        );

        assert!(result.is_ok());
    }

    #[test]
    fn test_list_sessions_empty_directory() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let sessions = SessionExporter::list_sessions(temp_dir.path()).unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_list_sessions_with_files() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

        std::fs::write(
            temp_dir.path().join("session1.json"),
            r#"{"id": "1"}"#
        ).unwrap();

        std::fs::write(
            temp_dir.path().join("session2.json"),
            r#"{"id": "2"}"#
        ).unwrap();

        let sessions = SessionExporter::list_sessions(temp_dir.path()).unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn test_message_role_serialization() {
        let roles = vec![
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::System,
            MessageRole::Tool,
        ];

        for role in &roles {
            let serialized = serde_json::to_string(role).unwrap();
            let deserialized: MessageRole = serde_json::from_str(&serialized).unwrap();
            assert_eq!(*role, deserialized);
        }
    }

    #[test]
    fn test_session_stats_calculation() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let output_path = temp_dir.path().join("stats.json");

        let messages = vec![
            (MessageRole::User, "msg1".to_string()),
            (MessageRole::User, "msg2".to_string()),
            (MessageRole::Assistant, "response1".to_string()),
            (MessageRole::Tool, "tool_result".to_string()),
        ];

        SessionExporter::export_to_json("stats-test", messages, &output_path).ok();

        let content = std::fs::read_to_string(&output_path).unwrap();
        let export: SessionExport = serde_json::from_str(&content).unwrap();

        assert_eq!(export.stats.message_count, 4);
        assert_eq!(export.stats.user_messages, 2);
        assert_eq!(export.stats.assistant_messages, 1);
        assert_eq!(export.stats.tool_calls, 1);
    }

    #[test]
    fn test_metadata_inclusion() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let output_path = temp_dir.path().join("meta.json");

        let messages = vec![(MessageRole::User, "test".to_string())];
        SessionExporter::export_to_json("meta-test", messages, &output_path).ok();

        let content = std::fs::read_to_string(&output_path).unwrap();
        let export: SessionExport = serde_json::from_str(&content).unwrap();

        assert_eq!(export.session_id, "meta-test");
        assert!(export.metadata.total_tokens >= 0);
    }
}
