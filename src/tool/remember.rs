//! Simple memory tool for persisting learnings across sessions
//!
//! Stores notes/facts that the model wants to remember for future sessions.
//! Uses a simple JSON file per project directory.

use super::{Tool, ToolContext, ToolOutput};
use crate::storage;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

/// A single note/memory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

/// Storage for notes
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Notes {
    pub entries: Vec<Note>,
}

impl Notes {
    fn load(path: &PathBuf) -> Result<Self> {
        if path.exists() {
            storage::read_json(path)
        } else {
            Ok(Self::default())
        }
    }

    fn save(&self, path: &PathBuf) -> Result<()> {
        storage::write_json(path, self)
    }

    fn add(&mut self, content: String, tag: Option<String>) -> String {
        let id = format!("note_{}", Utc::now().timestamp_millis());
        self.entries.push(Note {
            id: id.clone(),
            content,
            created_at: Utc::now(),
            tag,
        });
        id
    }

    fn remove(&mut self, id: &str) -> bool {
        if let Some(pos) = self.entries.iter().position(|n| n.id == id) {
            self.entries.remove(pos);
            true
        } else {
            false
        }
    }

    fn search(&self, query: &str) -> Vec<&Note> {
        let q = query.to_lowercase();
        self.entries
            .iter()
            .filter(|n| {
                n.content.to_lowercase().contains(&q)
                    || n.tag
                        .as_ref()
                        .map(|t| t.to_lowercase().contains(&q))
                        .unwrap_or(false)
            })
            .collect()
    }
}

#[derive(Debug, Deserialize)]
struct RememberInput {
    action: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    id: Option<String>,
}

pub struct RememberTool {
    test_mode: bool,
}

impl RememberTool {
    pub fn new() -> Self {
        Self { test_mode: false }
    }

    /// Create in test mode (isolated storage)
    pub fn new_test() -> Self {
        Self { test_mode: true }
    }

    fn notes_path(&self) -> Result<PathBuf> {
        if self.test_mode {
            let test_dir = storage::jcode_dir()?.join("notes").join("test");
            std::fs::create_dir_all(&test_dir)?;
            return Ok(test_dir.join("test_notes.json"));
        }

        let cwd = std::env::current_dir()?;
        let mut hasher = DefaultHasher::new();
        cwd.hash(&mut hasher);
        let hash = format!("{:016x}", hasher.finish());
        Ok(storage::jcode_dir()?
            .join("notes")
            .join(format!("{}.json", hash)))
    }
}

impl Default for RememberTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for RememberTool {
    fn name(&self) -> &str {
        "remember"
    }

    fn description(&self) -> &str {
        "Store and recall notes/learnings that should persist across sessions. \
         Use this when you learn something important about the project, user preferences, \
         or make a discovery that would be useful to remember later. \
         Also use when the user explicitly asks you to remember something."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["store", "list", "search", "forget"],
                    "description": "store: save a note, list: show all notes, search: find notes, forget: remove a note"
                },
                "content": {
                    "type": "string",
                    "description": "For store: the information to remember"
                },
                "tag": {
                    "type": "string",
                    "description": "For store: optional tag/category (e.g., 'architecture', 'preference', 'bug')"
                },
                "query": {
                    "type": "string",
                    "description": "For search: term to search for"
                },
                "id": {
                    "type": "string",
                    "description": "For forget: ID of the note to remove"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        use crate::memory;
        use crate::tui::info_widget::{MemoryEventKind, MemoryState};

        let params: RememberInput = serde_json::from_value(input)?;
        let path = self.notes_path()?;
        let mut notes = Notes::load(&path)?;

        fn truncate(s: &str, max: usize) -> String {
            if s.len() > max {
                format!("{}…", &s[..max])
            } else {
                s.to_string()
            }
        }

        match params.action.as_str() {
            "store" => {
                let content = params
                    .content
                    .ok_or_else(|| anyhow::anyhow!("'content' is required for store action"))?;

                memory::set_state(MemoryState::ToolAction {
                    action: "store".into(),
                    detail: truncate(&content, 40),
                });
                let id = notes.add(content.clone(), params.tag.clone());
                notes.save(&path)?;

                let tag_str = params.tag.as_deref().unwrap_or("");
                memory::add_event(MemoryEventKind::ToolRemembered {
                    content: truncate(&content, 60),
                    scope: "project".into(),
                    category: if tag_str.is_empty() {
                        "note".into()
                    } else {
                        tag_str.to_string()
                    },
                });
                memory::set_state(MemoryState::Idle);

                let tag_display = params.tag.map(|t| format!(" [{}]", t)).unwrap_or_default();
                Ok(ToolOutput::new(format!(
                    "Remembered{}: \"{}\"\nID: {}",
                    tag_display, content, id
                )))
            }

            "list" => {
                memory::set_state(MemoryState::ToolAction {
                    action: "list".into(),
                    detail: String::new(),
                });
                let count = notes.entries.len();
                memory::add_event(MemoryEventKind::ToolListed { count });
                memory::set_state(MemoryState::Idle);

                if notes.entries.is_empty() {
                    Ok(ToolOutput::new("No notes stored for this project."))
                } else {
                    let mut output = format!("## {} Notes\n\n", notes.entries.len());
                    for note in &notes.entries {
                        let tag = note
                            .tag
                            .as_ref()
                            .map(|t| format!(" [{}]", t))
                            .unwrap_or_default();
                        let date = note.created_at.format("%Y-%m-%d");
                        output.push_str(&format!(
                            "- **{}**{}: {}\n  _{}_\n\n",
                            note.id, tag, note.content, date
                        ));
                    }
                    Ok(ToolOutput::new(output))
                }
            }

            "search" => {
                let query = params
                    .query
                    .ok_or_else(|| anyhow::anyhow!("'query' is required for search action"))?;

                memory::set_state(MemoryState::ToolAction {
                    action: "search".into(),
                    detail: truncate(&query, 40),
                });
                let results = notes.search(&query);
                memory::add_event(MemoryEventKind::ToolRecalled {
                    query: truncate(&query, 40),
                    count: results.len(),
                });
                memory::set_state(MemoryState::Idle);

                if results.is_empty() {
                    Ok(ToolOutput::new(format!("No notes matching '{}'", query)))
                } else {
                    let mut output = format!("## {} notes matching '{}'\n\n", results.len(), query);
                    for note in results {
                        let tag = note
                            .tag
                            .as_ref()
                            .map(|t| format!(" [{}]", t))
                            .unwrap_or_default();
                        output.push_str(&format!("- **{}**{}: {}\n\n", note.id, tag, note.content));
                    }
                    Ok(ToolOutput::new(output))
                }
            }

            "forget" => {
                let id = params
                    .id
                    .ok_or_else(|| anyhow::anyhow!("'id' is required for forget action"))?;

                memory::set_state(MemoryState::ToolAction {
                    action: "forget".into(),
                    detail: truncate(&id, 30),
                });
                let found = notes.remove(&id);
                if found {
                    notes.save(&path)?;
                }
                memory::add_event(MemoryEventKind::ToolForgot { id: id.clone() });
                memory::set_state(MemoryState::Idle);

                if found {
                    Ok(ToolOutput::new(format!("Forgot note: {}", id)))
                } else {
                    Ok(ToolOutput::new(format!("Note not found: {}", id)))
                }
            }

            other => Err(anyhow::anyhow!("Unknown action: {}", other)),
        }
    }
}
