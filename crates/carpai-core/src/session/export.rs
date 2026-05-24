//! Session Export - Full implementation for session export/import

use crate::session::core_types::{ExportedMessage, ImportResult, SessionExport, SessionImport};
use std::collections::HashMap;

/// Session exporter
pub struct SessionExporter;

impl SessionExporter {
    pub fn new() -> Self {
        Self
    }

    /// Export a session to JSON-serializable format
    pub fn export_session(
        &self,
        session_id: &str,
        messages: Vec<ExportedMessage>,
        metadata: HashMap<String, String>,
    ) -> SessionExport {
        SessionExport {
            session_id: session_id.to_string(),
            version: "1.0".to_string(),
            exported_at: chrono::Utc::now(),
            messages,
            metadata,
        }
    }

    /// Import a session from exported format
    pub fn import_session(&self, import: SessionImport) -> ImportResult {
        let session_id = import.session_id.unwrap_or_else(|| {
            format!("imported_{}", chrono::Utc::now().timestamp())
        });

        let message_count = import.messages.len();
        let mut warnings = Vec::new();

        // Validate messages
        for (i, msg) in import.messages.iter().enumerate() {
            if msg.content.is_empty() {
                warnings.push(format!("Message {} has empty content", i));
            }
        }

        ImportResult {
            session_id,
            messages_imported: message_count,
            warnings,
        }
    }

    /// Serialize session export to JSON string
    pub fn to_json(&self, export: &SessionExport) -> Result<String, String> {
        serde_json::to_string_pretty(export).map_err(|e| e.to_string())
    }

    /// Deserialize session export from JSON string
    pub fn from_json(&self, json: &str) -> Result<SessionExport, String> {
        serde_json::from_str(json).map_err(|e| e.to_string())
    }
}

impl Default for SessionExporter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_export_and_import() {
        let exporter = SessionExporter::new();
        
        let messages = vec![
            ExportedMessage {
                id: "msg1".to_string(),
                role: "user".to_string(),
                content: "Hello".to_string(),
                timestamp: Utc::now(),
                metadata: None,
            },
        ];
        
        let export = exporter.export_session("test-session", messages.clone(), HashMap::new());
        assert_eq!(export.session_id, "test-session");
        assert_eq!(export.messages.len(), 1);
        
        let import = SessionImport {
            session_id: Some("imported-session".to_string()),
            messages: messages.clone(),
            metadata: HashMap::new(),
        };
        
        let result = exporter.import_session(import);
        assert_eq!(result.session_id, "imported-session");
        assert_eq!(result.messages_imported, 1);
    }

    #[test]
    fn test_json_serialization() {
        let exporter = SessionExporter::new();
        
        let export = SessionExport {
            session_id: "test".to_string(),
            version: "1.0".to_string(),
            exported_at: Utc::now(),
            messages: vec![],
            metadata: HashMap::new(),
        };
        
        let json = exporter.to_json(&export).unwrap();
        let deserialized = exporter.from_json(&json).unwrap();
        
        assert_eq!(deserialized.session_id, export.session_id);
    }
}
