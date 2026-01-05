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
