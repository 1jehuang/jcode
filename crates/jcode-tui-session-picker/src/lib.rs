use chrono::{DateTime, Utc};
use jcode_message_types::ToolCall;
use jcode_session_types::SessionStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SessionSource {
    Jcode,
    ClaudeCode,
    Codex,
    Pi,
    OpenCode,
}

impl SessionSource {
    pub fn badge(self) -> Option<&'static str> {
        match self {
            Self::Jcode => None,
            Self::ClaudeCode => Some("🧵 Claude Code"),
            Self::Codex => Some("🧠 Codex"),
            Self::Pi => Some("π Pi"),
            Self::OpenCode => Some("◌ OpenCode"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ResumeTarget {
    JcodeSession {
        session_id: String,
    },
    ClaudeCodeSession {
        session_id: String,
        session_path: String,
    },
    CodexSession {
        session_id: String,
        session_path: String,
    },
    PiSession {
        session_path: String,
    },
    OpenCodeSession {
        session_id: String,
        session_path: String,
    },
}

impl ResumeTarget {
    pub fn stable_id(&self) -> &str {
        match self {
            Self::JcodeSession { session_id } => session_id,
            Self::ClaudeCodeSession { session_id, .. } => session_id,
            Self::CodexSession { session_id, .. } => session_id,
            Self::PiSession { session_path } => session_path,
            Self::OpenCodeSession { session_id, .. } => session_id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SessionFilterMode {
    All,
    CatchUp,
    Saved,
    ClaudeCode,
    Codex,
    Pi,
    OpenCode,
}

impl SessionFilterMode {
    pub fn next(self) -> Self {
        match self {
            Self::All => Self::CatchUp,
            Self::CatchUp => Self::Saved,
            Self::Saved => Self::ClaudeCode,
            Self::ClaudeCode => Self::Codex,
            Self::Codex => Self::Pi,
            Self::Pi => Self::OpenCode,
            Self::OpenCode => Self::All,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::All => Self::OpenCode,
            Self::CatchUp => Self::All,
            Self::Saved => Self::CatchUp,
            Self::ClaudeCode => Self::Saved,
            Self::Codex => Self::ClaudeCode,
            Self::Pi => Self::Codex,
            Self::OpenCode => Self::Pi,
        }
    }

    pub fn label(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::CatchUp => Some("⏭ catch up"),
            Self::Saved => Some("📌 saved"),
            Self::ClaudeCode => Some("🧵 Claude Code"),
            Self::Codex => Some("🧠 Codex"),
            Self::Pi => Some("π Pi"),
            Self::OpenCode => Some("◌ OpenCode"),
        }
    }
}

/// Session info for display in the interactive session picker.
#[derive(Clone)]
pub struct SessionInfo {
    pub id: String,
    pub parent_id: Option<String>,
    pub short_name: String,
    pub icon: String,
    pub title: String,
    pub message_count: usize,
    pub user_message_count: usize,
    pub assistant_message_count: usize,
    pub created_at: DateTime<Utc>,
    pub last_message_time: DateTime<Utc>,
    pub last_active_at: Option<DateTime<Utc>>,
    pub working_dir: Option<String>,
    pub model: Option<String>,
    pub provider_key: Option<String>,
    pub is_canary: bool,
    pub is_debug: bool,
    pub saved: bool,
    pub save_label: Option<String>,
    pub status: SessionStatus,
    pub needs_catchup: bool,
    pub estimated_tokens: usize,
    pub messages_preview: Vec<PreviewMessage>,
    /// Lowercased searchable text used by picker filtering.
    pub search_index: String,
    /// Server name this session belongs to (if running).
    pub server_name: Option<String>,
    /// Server icon.
    pub server_icon: Option<String>,
    /// Human/session source classification shown in the UI.
    pub source: SessionSource,
    /// How this entry should be resumed when selected.
    pub resume_target: ResumeTarget,
    /// Backing external transcript/storage path when available.
    pub external_path: Option<String>,
}

/// A group of sessions under a server.
#[derive(Clone)]
pub struct ServerGroup {
    pub name: String,
    pub icon: String,
    pub version: String,
    pub git_hash: String,
    pub is_running: bool,
    pub sessions: Vec<SessionInfo>,
}

#[derive(Clone)]
pub struct PreviewMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<String>,
    pub tool_data: Option<ToolCall>,
    pub timestamp: Option<DateTime<Utc>>,
}

/// An item in the picker list, either a server/header row or a session row.
#[derive(Clone)]
pub enum PickerItem {
    ServerHeader {
        name: String,
        icon: String,
        version: String,
        session_count: usize,
    },
    Session,
    OrphanHeader {
        session_count: usize,
    },
    SavedHeader {
        session_count: usize,
    },
}

pub fn session_is_claude_code(source: SessionSource, id: &str) -> bool {
    source == SessionSource::ClaudeCode || id.starts_with("imported_cc_")
}

pub fn session_is_codex(source: SessionSource, model: Option<&str>) -> bool {
    if source == SessionSource::Codex {
        return true;
    }
    model
        .map(|model| model.to_ascii_lowercase().contains("codex"))
        .unwrap_or(false)
}

pub fn session_is_pi(
    source: SessionSource,
    provider_key: Option<&str>,
    model: Option<&str>,
) -> bool {
    if source == SessionSource::Pi {
        return true;
    }
    let provider_matches = provider_key
        .map(|key| {
            let key = key.to_ascii_lowercase();
            key == "pi" || key.starts_with("pi-")
        })
        .unwrap_or(false);
    let model_matches = model
        .map(|model| {
            let model = model.to_ascii_lowercase();
            model == "pi"
                || model.starts_with("pi-")
                || model.starts_with("pi/")
                || model.contains("/pi-")
        })
        .unwrap_or(false);
    provider_matches || model_matches
}

pub fn session_is_open_code(source: SessionSource, provider_key: Option<&str>) -> bool {
    if source == SessionSource::OpenCode {
        return true;
    }
    provider_key
        .map(|key| {
            let key = key.to_ascii_lowercase();
            key == "opencode" || key == "opencode-go" || key.contains("opencode")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_target_stable_id_uses_durable_identifier() {
        let target = ResumeTarget::CodexSession {
            session_id: "abc".into(),
            session_path: "/tmp/session.json".into(),
        };
        assert_eq!(target.stable_id(), "abc");

        let target = ResumeTarget::PiSession {
            session_path: "/tmp/pi.jsonl".into(),
        };
        assert_eq!(target.stable_id(), "/tmp/pi.jsonl");
    }

    #[test]
    fn source_predicates_cover_provider_and_model_fallbacks() {
        assert!(session_is_claude_code(
            SessionSource::Jcode,
            "imported_cc_123"
        ));
        assert!(session_is_codex(
            SessionSource::Jcode,
            Some("openai/codex-mini")
        ));
        assert!(session_is_pi(SessionSource::Jcode, Some("pi-main"), None));
        assert!(session_is_pi(
            SessionSource::Jcode,
            None,
            Some("vendor/pi-fast")
        ));
        assert!(session_is_open_code(
            SessionSource::Jcode,
            Some("opencode-go")
        ));
    }
}
