use std::path::PathBuf;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionExport {
    pub session_id: String,
    pub export_time: chrono::DateTime<chrono::Utc>,
    pub messages: Vec<SessionMessage>,
    pub metadata: SessionMetadata,
    pub stats: SessionStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub tokens_used: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub model: Option<String>,
    pub total_tokens: u32,
    pub duration_secs: u64,
    pub file_edits: usize,
    pub commands_run: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    pub message_count: usize,
    pub user_messages: usize,
    pub assistant_messages: usize,
    pub tool_calls: usize,
    pub estimated_cost_usd: f64,
}

pub struct SessionExporter;

impl SessionExporter {
    pub fn export_to_json(
        session_id: &str,
        messages: Vec<(MessageRole, String)>,
        output_path: &PathBuf,
    ) -> Result<String, String> {
        let now = chrono::Utc::now();
        let session_msgs: Vec<SessionMessage> = messages
            .into_iter()
            .map(|(role, content)| SessionMessage {
                role,
                content,
                timestamp: now,
                tokens_used: None,
            })
            .collect();

        let user_count = session_msgs.iter().filter(|m| matches!(m.role, MessageRole::User)).count();
        let assistant_count = session_msgs.iter().filter(|m| matches!(m.role, MessageRole::Assistant)).count();
        let tool_count = session_msgs.iter().filter(|m| matches!(m.role, MessageRole::Tool)).count();

        let export = SessionExport {
            session_id: session_id.to_string(),
            export_time: now,
            messages: session_msgs,
            metadata: SessionMetadata {
                model: None,
                total_tokens: 0,
                duration_secs: 0,
                file_edits: 0,
                commands_run: 0,
            },
            stats: SessionStats {
                message_count: user_count + assistant_count + tool_count,
                user_messages: user_count,
                assistant_messages: assistant_count,
                tool_calls: tool_count,
                estimated_cost_usd: 0.0,
            },
        };

        let json = serde_json::to_string_pretty(&export)
            .map_err(|e| format!("Serialization error: {}", e))?;

        std::fs::write(output_path, &json)
            .map_err(|e| format!("Write error: {}", e))?;

        Ok(format!(
            "✓ Session exported to {} ({} bytes)",
            output_path.display(),
            json.len()
        ))
    }

    pub fn export_to_markdown(
        session_id: &str,
        messages: Vec<(MessageRole, String)>,
        output_path: &PathBuf,
    ) -> Result<String, String> {
        let mut md = format!("# Session Export: {}\n\n", session_id);
        md.push_str(&format!("Exported: {}\n\n", chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")));

        for (role, content) in &messages {
            match role {
                MessageRole::User => md.push_str(&format!("## 👤 User\n\n{}\n\n", content)),
                MessageRole::Assistant => md.push_str(&format!("## 🤖 Assistant\n\n{}\n\n", content)),
                MessageRole::System => md.push_str(&format!("## ⚙️ System\n\n{}\n\n", content)),
                MessageRole::Tool => md.push_str(&format!("## 🔧 Tool\n\n```\n{}\n```\n\n", content)),
            }
        }

        std::fs::write(output_path, &md)
            .map_err(|e| format!("Write error: {}", e))?;

        Ok(format!(
            "✓ Session exported to {} (Markdown format)",
            output_path.display()
        ))
    }

    pub fn list_sessions(session_dir: &PathBuf) -> Result<Vec<SessionInfo>, String> {
        if !session_dir.exists() {
            return Ok(vec![]);
        }

        let mut sessions = vec![];
        if let Ok(entries) = std::fs::read_dir(session_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "json") {
                    let meta = std::fs::metadata(&path).map_err(|e| format!("{}", e))?;
                    let modified = meta.modified()
                        .ok()
                        .and_then(|t| chrono::DateTime::from_timestamp(t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64, 0))
                        .unwrap_or_else(|| chrono::Utc::now());

                    sessions.push(SessionInfo {
                        id: path.file_stem().unwrap_or_default().to_string_lossy().to_string(),
                        path,
                        modified_at: modified,
                        size_bytes: meta.len(),
                    });
                }
            }
        }

        sessions.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
        Ok(sessions)
    }
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub path: PathBuf,
    pub modified_at: chrono::DateTime<chrono::Utc>,
    pub size_bytes: u64,
}
