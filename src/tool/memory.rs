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
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Store and recall information across sessions. Use this to remember important facts about the codebase, user preferences, or lessons learned."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["remember", "recall", "search", "list", "forget", "tag", "link", "related"],
                    "description": "Action: remember (store), recall (get context), search, list, forget, tag (add tags), link (connect memories), related (find connected)"
                },
                "content": { "type": "string", "description": "For remember: what to store" },
                "category": {
                    "type": "string",
                    "enum": ["fact", "preference", "entity", "correction"],
                    "description": "Category of memory"
                },
                "query": { "type": "string", "description": "For search: search term" },
                "id": { "type": "string", "description": "For forget/tag/related: memory ID" },
                "tags": { "type": "array", "items": { "type": "string" }, "description": "For remember/tag: tags to apply" },
                "scope": { "type": "string", "enum": ["project", "global"] },
                "from_id": { "type": "string", "description": "For link: source memory ID" },
                "to_id": { "type": "string", "description": "For link: target memory ID" },
                "weight": { "type": "number", "description": "For link: relationship strength (0.0-1.0, default 0.5)" },
                "depth": { "type": "integer", "description": "For related: traversal depth (default 2)" }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let input: MemoryInput = serde_json::from_value(input)?;

        match input.action.as_str() {
            "remember" => {
                let content = input
                    .content
                    .ok_or_else(|| anyhow::anyhow!("content required"))?;
                let category: MemoryCategory =
                    input.category.as_deref().unwrap_or("fact").parse().unwrap();
                let mut entry =
                    MemoryEntry::new(category.clone(), &content).with_source(ctx.session_id);
                if let Some(tags) = input.tags {
                    entry = entry.with_tags(tags);
                }
                let scope = input.scope.as_deref().unwrap_or("project");
                let id = if scope == "global" {
                    self.manager.remember_global(entry)?
                } else {
                    self.manager.remember_project(entry)?
                };
                Ok(ToolOutput::new(format!(
                    "Remembered {} ({}): \"{}\" [id: {}]",
                    category, scope, content, id
                )))
            }
            "recall" => match self.manager.get_prompt_memories(10) {
                Some(memories) => Ok(ToolOutput::new(format!("Memories:\n{}", memories))),
                None => Ok(ToolOutput::new("No memories stored yet.")),
            },
            "search" => {
                let query = input
                    .query
                    .ok_or_else(|| anyhow::anyhow!("query required"))?;
                let results = self.manager.search(&query)?;
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
                let all = self.manager.list_all()?;
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
                if self.manager.forget(&id)? {
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

                for tag in &tags {
                    self.manager.tag_memory(&id, tag)?;
                }

                Ok(ToolOutput::new(format!(
                    "Tagged memory {} with: {}",
                    id,
                    tags.join(", ")
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

                self.manager.link_memories(&from_id, &to_id, weight)?;

                Ok(ToolOutput::new(format!(
                    "Linked {} -> {} (weight: {:.2})",
                    from_id, to_id, weight
                )))
            }
            "related" => {
                let id = input.id.ok_or_else(|| anyhow::anyhow!("id required"))?;
                let depth = input.depth.unwrap_or(2);

                let related = self.manager.get_related(&id, depth)?;

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

impl Default for MemoryTool {
    fn default() -> Self {
        Self::new()
    }
}
