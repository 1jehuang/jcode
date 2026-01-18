#![allow(dead_code)]
#![allow(dead_code)]

use crate::id::{extract_session_name, new_id, new_memorable_session_id};
use crate::message::{ContentBlock, Message, Role};
use crate::storage;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Session exit status - why the session ended
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SessionStatus {
    /// Session is currently active/running
    Active,
    /// User closed the session normally (Ctrl+C, /quit, etc.)
    Closed,
    /// Session crashed (panic, error)
    Crashed { message: Option<String> },
    /// Session was reloaded (hot reload)
    Reloaded,
    /// Session was compacted (context too large)
    Compacted,
    /// Session ended due to rate limiting
    RateLimited,
    /// Session ended due to an error
    Error { message: String },
}

impl Default for SessionStatus {
    fn default() -> Self {
        SessionStatus::Active
    }
}

impl SessionStatus {
    /// Get a short display string for the status
    pub fn display(&self) -> &'static str {
        match self {
            SessionStatus::Active => "active",
            SessionStatus::Closed => "closed",
            SessionStatus::Crashed { .. } => "crashed",
            SessionStatus::Reloaded => "reloaded",
            SessionStatus::Compacted => "compacted",
            SessionStatus::RateLimited => "rate limited",
            SessionStatus::Error { .. } => "error",
        }
    }

    /// Get an icon for the status
    pub fn icon(&self) -> &'static str {
        match self {
            SessionStatus::Active => "▶",
            SessionStatus::Closed => "✓",
            SessionStatus::Crashed { .. } => "💥",
            SessionStatus::Reloaded => "🔄",
            SessionStatus::Compacted => "📦",
            SessionStatus::RateLimited => "⏳",
            SessionStatus::Error { .. } => "❌",
        }
    }

    /// Get additional detail message if available
    pub fn detail(&self) -> Option<&str> {
        match self {
            SessionStatus::Crashed { message } => message.as_deref(),
            SessionStatus::Error { message } => Some(message.as_str()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: String,
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl StoredMessage {
    pub fn to_message(&self) -> Message {
        Message {
            role: self.role.clone(),
            content: self.content.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub parent_id: Option<String>,
    pub title: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub messages: Vec<StoredMessage>,
    /// Provider-specific session ID (e.g., Claude SDK session for resume)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    /// Whether this session is a canary session (testing new builds)
    #[serde(default)]
    pub is_canary: bool,
    /// Build hash this session is testing (if canary)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub testing_build: Option<String>,
    /// Working directory (for self-dev detection)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// Memorable short name (e.g., "fox", "oak")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_name: Option<String>,
    /// Session exit status - why it ended (if not active)
    #[serde(default)]
    pub status: SessionStatus,
}

impl Session {
    pub fn create_with_id(
        session_id: String,
        parent_id: Option<String>,
        title: Option<String>,
    ) -> Self {
        let now = Utc::now();
        // Try to extract short name from ID if it's a memorable ID
        let short_name = extract_session_name(&session_id).map(|s| s.to_string());
        Self {
            id: session_id,
            parent_id,
            title,
            created_at: now,
            updated_at: now,
            messages: Vec::new(),
            provider_session_id: None,
            is_canary: false,
            testing_build: None,
            working_dir: std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string()),
            short_name,
            status: SessionStatus::Active,
        }
    }

    pub fn create(parent_id: Option<String>, title: Option<String>) -> Self {
        let now = Utc::now();
        let (id, short_name) = new_memorable_session_id();
        Self {
            id,
            parent_id,
            title,
            created_at: now,
            updated_at: now,
            messages: Vec::new(),
            provider_session_id: None,
            is_canary: false,
            testing_build: None,
            working_dir: std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string()),
            short_name: Some(short_name),
            status: SessionStatus::Active,
        }
    }

    /// Get the display name for this session (short memorable name if available)
    pub fn display_name(&self) -> &str {
        self.short_name
            .as_deref()
            .or_else(|| extract_session_name(&self.id))
            .unwrap_or(&self.id)
    }

    /// Mark this session as a canary tester
    pub fn set_canary(&mut self, build_hash: &str) {
        self.is_canary = true;
        self.testing_build = Some(build_hash.to_string());
    }

    /// Clear canary status
    pub fn clear_canary(&mut self) {
        self.is_canary = false;
        self.testing_build = None;
    }

    /// Set the session status
    pub fn set_status(&mut self, status: SessionStatus) {
        self.status = status;
    }

    /// Mark session as closed normally
    pub fn mark_closed(&mut self) {
        self.status = SessionStatus::Closed;
    }

    /// Mark session as crashed
    pub fn mark_crashed(&mut self, message: Option<String>) {
        self.status = SessionStatus::Crashed { message };
    }

    /// Mark session as having an error
    pub fn mark_error(&mut self, message: String) {
        self.status = SessionStatus::Error { message };
    }

    /// Mark session as active (e.g., when resuming)
    pub fn mark_active(&mut self) {
        self.status = SessionStatus::Active;
    }

    /// Check if this session is working on the jcode repository
    pub fn is_self_dev(&self) -> bool {
        if let Some(ref dir) = self.working_dir {
            // Check if working dir contains jcode source
            let path = std::path::Path::new(dir);
            path.join("Cargo.toml").exists()
                && path.join("src/main.rs").exists()
                && std::fs::read_to_string(path.join("Cargo.toml"))
                    .map(|s| s.contains("name = \"jcode\""))
                    .unwrap_or(false)
        } else {
            false
        }
    }

    pub fn load(session_id: &str) -> Result<Self> {
        let path = session_path(session_id)?;
        storage::read_json(&path)
    }

    pub fn save(&mut self) -> Result<()> {
        self.updated_at = Utc::now();
        let path = session_path(&self.id)?;
        storage::write_json(&path, self)
    }

    pub fn add_message(&mut self, role: Role, content: Vec<ContentBlock>) -> String {
        let id = new_id("message");
        self.messages.push(StoredMessage {
            id: id.clone(),
            role,
            content,
        });
        id
    }

    pub fn messages_for_provider(&self) -> Vec<Message> {
        self.messages.iter().map(|msg| msg.to_message()).collect()
    }
}

pub fn session_path(session_id: &str) -> Result<PathBuf> {
    let base = storage::jcode_dir()?;
    Ok(base.join("sessions").join(format!("{}.json", session_id)))
}

/// Find a session by ID or memorable name
/// If the input doesn't look like a full session ID (doesn't contain underscore followed by digits),
/// try to find a session whose short name matches.
/// Returns the full session ID if found.
pub fn find_session_by_name_or_id(name_or_id: &str) -> Result<String> {
    // If it looks like a full session ID (contains session_), try loading directly first
    if name_or_id.starts_with("session_") {
        if let Ok(_) = Session::load(name_or_id) {
            return Ok(name_or_id.to_string());
        }
    }

    // Otherwise, search for a session with matching short name
    let sessions_dir = storage::jcode_dir()?.join("sessions");
    if !sessions_dir.exists() {
        anyhow::bail!("No sessions found");
    }

    let mut matches: Vec<(String, chrono::DateTime<chrono::Utc>)> = Vec::new();

    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                // Check if short name matches
                if let Some(short_name) = extract_session_name(stem) {
                    if short_name == name_or_id {
                        if let Ok(session) = Session::load(stem) {
                            matches.push((stem.to_string(), session.updated_at));
                        }
                    }
                }
            }
        }
    }

    if matches.is_empty() {
        anyhow::bail!("No session found matching '{}'", name_or_id);
    }

    // Sort by updated_at descending and return the most recent match
    matches.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(matches[0].0.clone())
}
