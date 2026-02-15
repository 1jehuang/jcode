//! Memory tool for storing and recalling information across sessions

use super::{Tool, ToolContext, ToolOutput};
use crate::memory::{MemoryCategory, MemoryEntry, MemoryManager};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

pub struct MemoryTool {
    manager: MemoryManager,
}

impl MemoryTool {
    pub fn new() -> Self {
        Self {
            manager: MemoryManager::new(),
        }
    }

    /// Create a memory tool in test mode (isolated storage)
    pub fn new_test() -> Self {
        Self {
            manager: MemoryManager::new_test(),
        }
    }

    /// Check if running in test mode
    pub fn is_test_mode(&self) -> bool {
        self.manager.is_test_mode()
    }
}

#[derive(Debug, Deserialize)]
struct MemoryInput {
    action: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    scope: Option<String>,
    /// For link action: source memory ID
    #[serde(default)]
    from_id: Option<String>,
    /// For link action: target memory ID
    #[serde(default)]
    to_id: Option<String>,
    /// For link action: relationship weight (0.0-1.0)
    #[serde(default)]
    weight: Option<f32>,
    /// For related action: traversal depth (default: 2)
    #[serde(default)]
    depth: Option<usize>,
    /// For recall action: max results (default: 10)
    #[serde(default)]
    limit: Option<usize>,
    /// For recall action: retrieval mode
    #[serde(default)]
    mode: Option<String>,
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Store and recall information across sessions. Use this to remember important facts about the codebase, user preferences, or lessons learned. IMPORTANT: When the user asks 'do you remember X?' or 'what do you know about X?', use recall with a query to search your memories."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["remember", "recall", "search", "list", "forget", "tag", "link", "related"],
                    "description": "Action: remember (store), recall (retrieve memories - use query for semantic search), search (keyword), list, forget, tag, link, related"
                },
                "content": { "type": "string", "description": "For remember: what to store" },
                "category": {
                    "type": "string",
                    "enum": ["fact", "preference", "entity", "correction"],
                    "description": "Category of memory"
                },
                "query": { "type": "string", "description": "For recall/search: what to look for. For recall, enables semantic search with graph traversal" },
                "id": { "type": "string", "description": "For forget/tag/related: memory ID" },
                "tags": { "type": "array", "items": { "type": "string" }, "description": "For remember/tag/recall: tags to apply or filter by" },
                "scope": { "type": "string", "enum": ["project", "global", "all"], "description": "Memory scope (default: project for remember, all for recall)" },
                "from_id": { "type": "string", "description": "For link: source memory ID" },
                "to_id": { "type": "string", "description": "For link: target memory ID" },
                "weight": { "type": "number", "description": "For link: relationship strength (0.0-1.0, default 0.5)" },
                "depth": { "type": "integer", "description": "For related: traversal depth (default 2)" },
                "limit": { "type": "integer", "description": "For recall: max results (default 10)" },
                "mode": { "type": "string", "enum": ["recent", "semantic", "cascade"], "description": "For recall: recent (by time), semantic (embedding similarity), cascade (semantic + graph traversal, default when query provided)" }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        use crate::memory;
        use crate::tui::info_widget::{MemoryEventKind, MemoryState};

        let input: MemoryInput = serde_json::from_value(input)?;

        match input.action.as_str() {
            "remember" => {
                let content = input
                    .content
                    .ok_or_else(|| anyhow::anyhow!("content required"))?;
                let category: MemoryCategory =
                    input.category.as_deref().unwrap_or("fact").parse().unwrap();
                let scope = input.scope.as_deref().unwrap_or("project");
                memory::set_state(MemoryState::ToolAction {
                    action: "remember".into(),
                    detail: truncate_for_widget(&content, 40),
                });
                let mut entry =
                    MemoryEntry::new(category.clone(), &content).with_source(ctx.session_id);
                if let Some(tags) = input.tags {
                    entry = entry.with_tags(tags);
                }
                let id = if scope == "global" {
                    self.manager.remember_global(entry)?
                } else {
                    self.manager.remember_project(entry)?
                };
                memory::add_event(MemoryEventKind::ToolRemembered {
                    content: truncate_for_widget(&content, 60),
                    scope: scope.to_string(),
                    category: category.to_string(),
                });
                memory::set_state(MemoryState::Idle);
                Ok(ToolOutput::new(format!(
                    "Remembered {} ({}): \"{}\" [id: {}]",
                    category, scope, content, id
                )))
            }
            "recall" => {
                let limit = input.limit.unwrap_or(10);
                let mode = input.mode.as_deref().unwrap_or_else(|| {
                    if input.query.is_some() {
                        "cascade"
                    } else {
                        "recent"
                    }
                });

                match mode {
                    "recent" => {
                        memory::set_state(MemoryState::ToolAction {
                            action: "recall".into(),
                            detail: "recent".into(),
                        });
                        let result = match self.manager.get_prompt_memories(limit) {
                            Some(memories) => {
                                let count =
                                    memories.lines().filter(|l| l.starts_with("- ")).count();
                                memory::add_event(MemoryEventKind::ToolRecalled {
                                    query: "(recent)".into(),
                                    count,
                                });
                                Ok(ToolOutput::new(format!("Recent memories:\n{}", memories)))
                            }
                            None => {
                                memory::add_event(MemoryEventKind::ToolRecalled {
                                    query: "(recent)".into(),
                                    count: 0,
                                });
                                Ok(ToolOutput::new("No memories stored yet."))
                            }
                        };
                        memory::set_state(MemoryState::Idle);
                        result
                    }
                    "semantic" | "cascade" => {
                        let query = match &input.query {
                            Some(q) => q.clone(),
                            None => {
                                return Err(anyhow::anyhow!(
                                    "query required for semantic/cascade mode"
                                ))
                            }
                        };
                        memory::set_state(MemoryState::ToolAction {
                            action: "recall".into(),
                            detail: truncate_for_widget(&query, 40),
                        });

                        let results = if mode == "cascade" {
                            self.manager.find_similar_with_cascade(&query, 0.3, limit)?
                        } else {
                            self.manager.find_similar(&query, 0.3, limit)?
                        };

                        memory::add_event(MemoryEventKind::ToolRecalled {
                            query: truncate_for_widget(&query, 40),
                            count: results.len(),
                        });
                        memory::set_state(MemoryState::Idle);

                        if results.is_empty() {
                            Ok(ToolOutput::new(format!(
                                "No memories found matching '{}'. Try recall without query to see recent memories.",
                                query
                            )))
                        } else {
                            let mut out = format!(
                                "Found {} relevant memories for '{}':\n\n",
                                results.len(),
                                query
                            );
                            for (entry, score) in results {
                                let tags_str = if entry.tags.is_empty() {
                                    String::new()
                                } else {
                                    format!(" [{}]", entry.tags.join(", "))
                                };
                                out.push_str(&format!(
                                    "- [{}] {}{}\n  id: {} (relevance: {:.0}%)\n\n",
                                    entry.category,
                                    entry.content,
                                    tags_str,
                                    entry.id,
                                    score * 100.0
                                ));
                            }
                            Ok(ToolOutput::new(out))
                        }
                    }
                    other => Err(anyhow::anyhow!(
                        "Unknown mode: {}. Use recent, semantic, or cascade",
                        other
                    )),
                }
            }
            "search" => {
                let query = input
                    .query
                    .ok_or_else(|| anyhow::anyhow!("query required"))?;
                memory::set_state(MemoryState::ToolAction {
                    action: "search".into(),
                    detail: truncate_for_widget(&query, 40),
                });
                let results = self.manager.search(&query)?;
                memory::add_event(MemoryEventKind::ToolRecalled {
                    query: truncate_for_widget(&query, 40),
                    count: results.len(),
                });
                memory::set_state(MemoryState::Idle);
                if results.is_empty() {
                    Ok(ToolOutput::new(format!("No memories matching '{}'", query)))
                } else {
                    let mut out = format!("Found {} memories:\n\n", results.len());
                    for e in results {
                        out.push_str(&format!(
                            "- [{}] {}\n  id: {}\n\n",
                            e.category, e.content, e.id
                        ));
                    }
                    Ok(ToolOutput::new(out))
                }
            }
            "list" => {
                memory::set_state(MemoryState::ToolAction {
                    action: "list".into(),
                    detail: String::new(),
                });
                let all = self.manager.list_all()?;
                memory::add_event(MemoryEventKind::ToolListed { count: all.len() });
                memory::set_state(MemoryState::Idle);
                if all.is_empty() {
                    Ok(ToolOutput::new("No memories stored."))
                } else {
                    let mut out = format!("All memories ({}):\n\n", all.len());
                    for e in all {
                        out.push_str(&format!(
                            "- [{}] {}\n  id: {}\n\n",
                            e.category, e.content, e.id
                        ));
                    }
                    Ok(ToolOutput::new(out))
                }
            }
            "forget" => {
                let id = input.id.ok_or_else(|| anyhow::anyhow!("id required"))?;
                memory::set_state(MemoryState::ToolAction {
                    action: "forget".into(),
                    detail: truncate_for_widget(&id, 30),
                });
                let found = self.manager.forget(&id)?;
                memory::add_event(MemoryEventKind::ToolForgot { id: id.clone() });
                memory::set_state(MemoryState::Idle);
                if found {
                    Ok(ToolOutput::new(format!("Forgot: {}", id)))
                } else {
                    Ok(ToolOutput::new(format!("Not found: {}", id)))
                }
            }
            "tag" => {
                let id = input.id.ok_or_else(|| anyhow::anyhow!("id required"))?;
                let tags = input.tags.ok_or_else(|| anyhow::anyhow!("tags required"))?;

                if tags.is_empty() {
                    return Err(anyhow::anyhow!("At least one tag required"));
                }

                memory::set_state(MemoryState::ToolAction {
                    action: "tag".into(),
                    detail: format!("{} +{}", truncate_for_widget(&id, 20), tags.join(",")),
                });
                for tag in &tags {
                    self.manager.tag_memory(&id, tag)?;
                }
                let tags_str = tags.join(", ");
                memory::add_event(MemoryEventKind::ToolTagged {
                    id: id.clone(),
                    tags: tags_str.clone(),
                });
                memory::set_state(MemoryState::Idle);

                Ok(ToolOutput::new(format!(
                    "Tagged memory {} with: {}",
                    id, tags_str
                )))
            }
            "link" => {
                let from_id = input
                    .from_id
                    .ok_or_else(|| anyhow::anyhow!("from_id required"))?;
                let to_id = input
                    .to_id
                    .ok_or_else(|| anyhow::anyhow!("to_id required"))?;
                let weight = input.weight.unwrap_or(0.5);

                if weight < 0.0 || weight > 1.0 {
                    return Err(anyhow::anyhow!("weight must be between 0.0 and 1.0"));
                }

                memory::set_state(MemoryState::ToolAction {
                    action: "link".into(),
                    detail: format!(
                        "{} → {}",
                        truncate_for_widget(&from_id, 15),
                        truncate_for_widget(&to_id, 15)
                    ),
                });
                self.manager.link_memories(&from_id, &to_id, weight)?;
                memory::add_event(MemoryEventKind::ToolLinked {
                    from: from_id.clone(),
                    to: to_id.clone(),
                });
                memory::set_state(MemoryState::Idle);

                Ok(ToolOutput::new(format!(
                    "Linked {} -> {} (weight: {:.2})",
                    from_id, to_id, weight
                )))
            }
            "related" => {
                let id = input.id.ok_or_else(|| anyhow::anyhow!("id required"))?;
                let depth = input.depth.unwrap_or(2);

                memory::set_state(MemoryState::ToolAction {
                    action: "related".into(),
                    detail: truncate_for_widget(&id, 30),
                });
                let related = self.manager.get_related(&id, depth)?;
                memory::add_event(MemoryEventKind::ToolRecalled {
                    query: format!("related:{}", truncate_for_widget(&id, 25)),
                    count: related.len(),
                });
                memory::set_state(MemoryState::Idle);

                if related.is_empty() {
                    Ok(ToolOutput::new(format!(
                        "No related memories found for {}",
                        id
                    )))
                } else {
                    let mut out = format!(
                        "Found {} memories related to {} (depth {}):\n\n",
                        related.len(),
                        id,
                        depth
                    );
                    for entry in related {
                        let tags_str = if entry.tags.is_empty() {
                            String::new()
                        } else {
                            format!(" [{}]", entry.tags.join(", "))
                        };
                        out.push_str(&format!(
                            "- [{}] {}{}\n  id: {}\n\n",
                            entry.category, entry.content, tags_str, entry.id
                        ));
                    }
                    Ok(ToolOutput::new(out))
                }
            }
            other => Err(anyhow::anyhow!("Unknown action: {}", other)),
        }
    }
}

fn truncate_for_widget(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}…", &s[..max])
    } else {
        s.to_string()
    }
}

impl Default for MemoryTool {
    fn default() -> Self {
        Self::new()
    }
}
