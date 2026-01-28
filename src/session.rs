#![allow(dead_code)]
#![allow(dead_code)]

use crate::id::{extract_session_name, new_id, new_memorable_session_id};
use crate::message::{ContentBlock, Message, Role, ToolCall};
use crate::storage;
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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
    /// Provider-specific session ID (e.g., Claude Code CLI session for resume)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    /// Model identifier for this session (e.g., "gpt-5.2-codex")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
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
    /// PID of the process that last owned this session (for crash detection)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pid: Option<u32>,
    /// Last time the session was marked active
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_active_at: Option<DateTime<Utc>>,
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
            model: None,
            is_canary: false,
            testing_build: None,
            working_dir: std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string()),
            short_name,
            status: SessionStatus::Active,
            last_pid: Some(std::process::id()),
            last_active_at: Some(now),
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
            model: None,
            is_canary: false,
            testing_build: None,
            working_dir: std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string()),
            short_name: Some(short_name),
            status: SessionStatus::Active,
            last_pid: Some(std::process::id()),
            last_active_at: Some(now),
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
        self.last_pid = Some(std::process::id());
        self.last_active_at = Some(Utc::now());
    }

    /// Mark session as active for a specific PID
    pub fn mark_active_with_pid(&mut self, pid: u32) {
        self.status = SessionStatus::Active;
        self.last_pid = Some(pid);
        self.last_active_at = Some(Utc::now());
    }

    /// Detect if an active session likely crashed (process no longer running)
    /// Returns true if status was updated.
    pub fn detect_crash(&mut self) -> bool {
        if self.status != SessionStatus::Active {
            return false;
        }

        if let Some(pid) = self.last_pid {
            if !is_pid_running(pid) {
                self.mark_crashed(Some(format!("Process {} not running", pid)));
                return true;
            }
        } else {
            // No PID info (older sessions): fall back to age heuristic
            let age = Utc::now().signed_duration_since(self.updated_at);
            if age.num_seconds() > 120 {
                self.mark_crashed(Some("Stale active session".to_string()));
                return true;
            }
        }

        false
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

#[derive(Debug, Clone)]
pub struct RenderedMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<String>,
    pub tool_data: Option<ToolCall>,
}

/// Convert stored session messages into renderable messages (including tool output).
pub fn render_messages(session: &Session) -> Vec<RenderedMessage> {
    let mut rendered: Vec<RenderedMessage> = Vec::new();
    let mut tool_map: HashMap<String, ToolCall> = HashMap::new();

    for msg in &session.messages {
        let role = match msg.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        let mut text = String::new();
        let mut tool_calls: Vec<String> = Vec::new();

        for block in &msg.content {
            match block {
                ContentBlock::Text { text: t, .. } => {
                    text.push_str(t);
                }
                ContentBlock::ToolUse { id, name, input } => {
                    tool_map.insert(
                        id.clone(),
                        ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        },
                    );
                    tool_calls.push(name.clone());
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    if !text.is_empty() {
                        rendered.push(RenderedMessage {
                            role: role.to_string(),
                            content: std::mem::take(&mut text),
                            tool_calls: tool_calls.clone(),
                            tool_data: None,
                        });
                    }

                    let tool_data = tool_map.get(tool_use_id).cloned().or_else(|| {
                        Some(ToolCall {
                            id: tool_use_id.clone(),
                            name: "tool".to_string(),
                            input: serde_json::Value::Null,
                        })
                    });

                    rendered.push(RenderedMessage {
                        role: "tool".to_string(),
                        content: content.clone(),
                        tool_calls: Vec::new(),
                        tool_data,
                    });
                }
            }
        }

        if !text.is_empty() {
            rendered.push(RenderedMessage {
                role: role.to_string(),
                content: text,
                tool_calls,
                tool_data: None,
            });
        }
    }

    rendered
}

fn session_path_in_dir(base: &std::path::Path, session_id: &str) -> PathBuf {
    base.join("sessions").join(format!("{}.json", session_id))
}

pub fn session_path(session_id: &str) -> Result<PathBuf> {
    let base = storage::jcode_dir()?;
    Ok(session_path_in_dir(&base, session_id))
}

pub fn session_exists(session_id: &str) -> bool {
    session_path(session_id)
        .map(|path| path.exists())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_exists_roundtrip() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "jcode-session-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(tmp_dir.join("sessions")).unwrap();

        assert!(!session_path_in_dir(&tmp_dir, "missing-session").exists());

        let session_path = session_path_in_dir(&tmp_dir, "exists-session");
        std::fs::write(&session_path, "{}").unwrap();
        assert!(session_path.exists());

        let random_id = format!(
            "missing-session-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        assert!(!session_exists(&random_id));
    }
}

/// Recover crashed sessions from the most recent crash window (text-only).
/// Returns new recovery session IDs (most recent first).
pub fn recover_crashed_sessions() -> Result<Vec<String>> {
    let sessions_dir = storage::jcode_dir()?.join("sessions");
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions: Vec<Session> = Vec::new();
    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if let Ok(mut session) = Session::load(stem) {
                    if session.detect_crash() {
                        let _ = session.save();
                    }
                    sessions.push(session);
                }
            }
        }
    }

    // Track existing recovery sessions to avoid duplicates
    let mut recovered_parents: HashSet<String> = HashSet::new();
    for s in &sessions {
        if s.id.starts_with("session_recovery_") {
            if let Some(parent) = s.parent_id.as_ref() {
                recovered_parents.insert(parent.clone());
            }
        }
    }

    let mut crashed: Vec<Session> = sessions
        .into_iter()
        .filter(|s| matches!(s.status, SessionStatus::Crashed { .. }))
        .collect();
    if crashed.is_empty() {
        return Ok(Vec::new());
    }

    let crash_window = Duration::seconds(60);
    let most_recent = crashed
        .iter()
        .map(|s| s.last_active_at.unwrap_or(s.updated_at))
        .max()
        .unwrap_or_else(Utc::now);
    crashed.retain(|s| {
        let ts = s.last_active_at.unwrap_or(s.updated_at);
        let delta = most_recent.signed_duration_since(ts);
        delta >= Duration::zero() && delta <= crash_window
    });
    crashed.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    let mut new_ids = Vec::new();
    for mut old in crashed {
        if recovered_parents.contains(&old.id) {
            continue;
        }

        let new_id = format!("session_recovery_{}", crate::id::new_id("rec"));
        let mut new_session =
            Session::create_with_id(new_id.clone(), Some(old.id.clone()), old.title.clone());
        new_session.working_dir = old.working_dir.clone();
        new_session.model = old.model.clone();
        new_session.is_canary = old.is_canary;
        new_session.testing_build = old.testing_build.clone();
        new_session.provider_session_id = None;
        new_session.status = SessionStatus::Closed;

        // Add a recovery header
        new_session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: format!(
                    "Recovered from crashed session `{}` ({})",
                    old.id,
                    old.display_name()
                ),
                cache_control: None,
            }],
        );

        for msg in old.messages.drain(..) {
            let kept_blocks: Vec<ContentBlock> = msg
                .content
                .into_iter()
                .filter(|block| matches!(block, ContentBlock::Text { .. }))
                .collect();
            if kept_blocks.is_empty() {
                continue;
            }
            new_session.add_message(msg.role, kept_blocks);
        }

        new_session.save()?;
        new_ids.push(new_id);
    }

    Ok(new_ids)
}

/// Info about crashed sessions pending batch restore
#[derive(Debug, Clone)]
pub struct CrashedSessionsInfo {
    /// Session IDs that crashed
    pub session_ids: Vec<String>,
    /// Display names of crashed sessions
    pub display_names: Vec<String>,
    /// When the most recent crash occurred
    pub most_recent_crash: DateTime<Utc>,
}

/// Detect crashed sessions that can be batch restored.
/// Returns info about crashed sessions within the crash window (60 seconds),
/// excluding any that have already been recovered.
pub fn detect_crashed_sessions() -> Result<Option<CrashedSessionsInfo>> {
    let sessions_dir = storage::jcode_dir()?.join("sessions");
    if !sessions_dir.exists() {
        return Ok(None);
    }

    let mut sessions: Vec<Session> = Vec::new();
    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if let Ok(mut session) = Session::load(stem) {
                    // Detect if this session crashed (updates status if so)
                    if session.detect_crash() {
                        let _ = session.save();
                    }
                    sessions.push(session);
                }
            }
        }
    }

    // Track existing recovery sessions to avoid showing already-recovered crashes
    let mut recovered_parents: HashSet<String> = HashSet::new();
    for s in &sessions {
        if s.id.starts_with("session_recovery_") {
            if let Some(parent) = s.parent_id.as_ref() {
                recovered_parents.insert(parent.clone());
            }
        }
    }

    // Filter to crashed sessions that haven't been recovered
    let mut crashed: Vec<Session> = sessions
        .into_iter()
        .filter(|s| matches!(s.status, SessionStatus::Crashed { .. }))
        .filter(|s| !recovered_parents.contains(&s.id))
        .collect();

    if crashed.is_empty() {
        return Ok(None);
    }

    // Apply 60-second crash window filter
    let crash_window = Duration::seconds(60);
    let most_recent = crashed
        .iter()
        .map(|s| s.last_active_at.unwrap_or(s.updated_at))
        .max()
        .unwrap_or_else(Utc::now);

    crashed.retain(|s| {
        let ts = s.last_active_at.unwrap_or(s.updated_at);
        let delta = most_recent.signed_duration_since(ts);
        delta >= Duration::zero() && delta <= crash_window
    });

    if crashed.is_empty() {
        return Ok(None);
    }

    // Sort by most recent first
    crashed.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    let session_ids: Vec<String> = crashed.iter().map(|s| s.id.clone()).collect();
    let display_names: Vec<String> = crashed.iter().map(|s| s.display_name().to_string()).collect();

    Ok(Some(CrashedSessionsInfo {
        session_ids,
        display_names,
        most_recent_crash: most_recent,
    }))
}

#[cfg(unix)]
fn is_pid_running(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as i32, 0) };
    if result == 0 {
        return true;
    }
    let err = std::io::Error::last_os_error();
    !matches!(err.raw_os_error(), Some(code) if code == libc::ESRCH)
}

#[cfg(not(unix))]
fn is_pid_running(_pid: u32) -> bool {
    true
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

#[cfg(test)]
mod batch_crash_tests {
    use super::*;

    #[test]
    fn test_crashed_sessions_info_struct() {
        let info = CrashedSessionsInfo {
            session_ids: vec!["session_test_1".to_string(), "session_test_2".to_string()],
            display_names: vec!["fox".to_string(), "oak".to_string()],
            most_recent_crash: Utc::now(),
        };
        assert_eq!(info.session_ids.len(), 2);
        assert_eq!(info.display_names.len(), 2);
        assert_eq!(info.display_names[0], "fox");
    }
}
