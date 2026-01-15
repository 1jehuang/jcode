#![allow(dead_code)]

#![allow(dead_code)]

use crate::id::new_id;
use crate::message::{ContentBlock, Message, Role};
use crate::storage;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
}

impl Session {
    pub fn create_with_id(session_id: String, parent_id: Option<String>, title: Option<String>) -> Self {
        let now = Utc::now();
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
            working_dir: std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string()),
        }
    }

    pub fn create(parent_id: Option<String>, title: Option<String>) -> Self {
        let now = Utc::now();
        Self {
            id: new_id("session"),
            parent_id,
            title,
            created_at: now,
            updated_at: now,
            messages: Vec::new(),
            provider_session_id: None,
            is_canary: false,
            testing_build: None,
            working_dir: std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string()),
        }
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

    /// Check if this session is working on the jcode repository
    pub fn is_self_dev(&self) -> bool {
        if let Some(ref dir) = self.working_dir {
            // Check if working dir contains jcode source
            let path = std::path::Path::new(dir);
            path.join("Cargo.toml").exists() &&
            path.join("src/main.rs").exists() &&
            std::fs::read_to_string(path.join("Cargo.toml"))
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
        self.messages.push(StoredMessage { id: id.clone(), role, content });
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
