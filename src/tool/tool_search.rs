//! ToolSearch: deferred-tool discovery.
//!
//! Under Anthropic OAuth (Claude Code), only a fixed set of tools is advertised
//! up front. ToolSearch lets the model discover and unlock additional tools
//! from the local registry at runtime by querying with a natural-language
//! string. Unlocked tools are then included in the next API request's tool
//! list so the model can actually call them.

use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// Session-scoped registry of tools that have been unlocked via ToolSearch.
///
/// Keyed by `session_id`. The agent reads from this when assembling the tool
/// list for the next API request.
fn unlocked_store() -> &'static Mutex<HashMap<String, HashSet<String>>> {
    static STORE: OnceLock<Mutex<HashMap<String, HashSet<String>>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Tools that ToolSearch is allowed to surface. Excludes the always-on OAuth
/// hardcoded tools (those are already callable) and excludes internal tools
/// that shouldn't be model-callable directly.
///
/// Names here are the registry keys (the names the model should use when
/// calling the tool). Descriptions and keywords are used for matching.
fn searchable_registry() -> Vec<(&'static str, &'static str, &'static str)> {
    // (registry_name, short_description, search_keywords)
    vec![
        (
            "askUserQuestion",
            "Ask the user a structured multiple-choice question with a recommended option.",
            "ask user question prompt choose confirm preference quiz options recommend",
        ),
        (
            "webfetch",
            "Fetch a URL and return its contents.",
            "fetch http url web download page content webfetch",
        ),
        (
            "websearch",
            "Search the web and return results.",
            "search web google find query results websearch",
        ),
        (
            "open",
            "Open a file, URL, or application using the system handler.",
            "open launch file url application reveal",
        ),
        (
            "todo",
            "Manage a structured todo list for the current task.",
            "todo task list plan steps checklist progress todowrite",
        ),
        (
            "batch",
            "Run multiple tool calls in a single batched invocation.",
            "batch parallel multiple tools group",
        ),
        (
            "patch",
            "Apply a patch to a file using a structured diff.",
            "patch diff apply file edit",
        ),
        (
            "multiedit",
            "Perform multiple edits to one file in a single call.",
            "multi edit multiple changes file replace multiedit",
        ),
        (
            "apply_patch",
            "Apply a v4a-format patch across one or more files.",
            "apply patch diff files multi-file change",
        ),
        (
            "lsp",
            "Query the language server for symbols, references, diagnostics.",
            "lsp language server symbol reference diagnostic definition hover",
        ),
        (
            "codesearch",
            "Semantic code search over the workspace.",
            "code search semantic find symbol function codesearch",
        ),
        (
            "conversation_search",
            "Search past conversations and journal entries.",
            "conversation search history past journal",
        ),
        (
            "side_panel",
            "Create or update a side-panel page with rich markdown content.",
            "side panel page markdown ui display sidepanel",
        ),
        (
            "memory",
            "Read, write, and manage long-term memory entries.",
            "memory remember recall note long term storage",
        ),
        (
            "goal",
            "Manage long-running goals with milestones and checkpoints.",
            "goal milestone checkpoint long task plan",
        ),
    ]
}

/// Map a ToolSearch-surfaced name to the registry key. Currently a no-op
/// because ToolSearch surfaces registry keys directly, but kept as a hook for
/// future display-name vs registry-key divergence.
pub fn registry_key_for_search_name(name: &str) -> &str {
    name
}

/// Mark `tool_name` as unlocked for `session_id`.
pub fn unlock_tool(session_id: &str, tool_name: &str) {
    let mut map = unlocked_store().lock().expect("unlocked tools mutex");
    map.entry(session_id.to_string())
        .or_default()
        .insert(tool_name.to_string());
}

/// Get a snapshot of unlocked tools for `session_id` (registry-key form).
pub fn unlocked_for_session(session_id: &str) -> HashSet<String> {
    let map = unlocked_store().lock().expect("unlocked tools mutex");
    map.get(session_id).cloned().unwrap_or_default()
}

/// Clear unlocked tools for `session_id` (e.g. on session reset).
#[allow(dead_code)]
pub fn clear_session(session_id: &str) {
    let mut map = unlocked_store().lock().expect("unlocked tools mutex");
    map.remove(session_id);
}

pub struct ToolSearchTool;

impl ToolSearchTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct ToolSearchInput {
    #[serde(default)]
    intent: Option<String>,
    /// Free-text query, e.g. "ask the user a question" or "fetch a URL".
    query: String,
    /// Maximum number of results to return. Defaults to 5.
    #[serde(default)]
    max_results: Option<usize>,
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "ToolSearch"
    }

    fn description(&self) -> &str {
        concat!(
            "Fetches full schema definitions for deferred tools so they can be called. ",
            "Use this when you need a capability beyond the always-available core tools ",
            "(Bash, Read, Write, Edit, Glob, Grep, Agent, Skill, ScheduleWakeup). ",
            "Returns matching tool names with their input schemas. ",
            "After ToolSearch returns, the matched tools become callable on the next turn."
        )
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["query", "max_results"],
            "properties": {
                "intent": super::intent_schema_property(),
                "query": {
                    "type": "string",
                    "description": "Natural-language description of the capability you need."
                },
                "max_results": {
                    "type": "number",
                    "description": "Maximum number of results to return.",
                    "default": 5
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: ToolSearchInput = serde_json::from_value(input)?;
        let max_results = params.max_results.unwrap_or(5).max(1).min(20);
        let query = params.query.trim().to_lowercase();

        let scored = score_matches(&query, &searchable_registry(), max_results);

        if scored.is_empty() {
            return Ok(ToolOutput::new(format!(
                "No deferred tools matched query: {:?}. Core tools (Bash, Read, Write, Edit, Glob, Grep, Agent, Skill, ScheduleWakeup) are always available.",
                params.query
            ))
            .with_title("ToolSearch"));
        }

        // Unlock matched tools for this session so the next API request
        // includes them in the tools array.
        let mut matched_summaries: Vec<Value> = Vec::with_capacity(scored.len());
        for entry in &scored {
            let registry_key = registry_key_for_search_name(entry.name);
            unlock_tool(&ctx.session_id, registry_key);
            matched_summaries.push(json!({
                "name": entry.name,
                "description": entry.description,
                "score": entry.score,
            }));
        }

        let mut text = String::new();
        text.push_str(&format!(
            "Found {} matching tool(s) for {:?}. These tools are now callable on subsequent turns:\n\n",
            scored.len(),
            params.query
        ));
        for entry in &scored {
            text.push_str(&format!("- `{}` — {}\n", entry.name, entry.description));
        }
        text.push_str("\nCall any of these tools by name in your next tool_use block.");

        Ok(ToolOutput::new(text)
            .with_title("ToolSearch")
            .with_metadata(json!({
                "query": params.query,
                "matches": matched_summaries,
                "unlocked_for_session": ctx.session_id,
            })))
    }
}

struct ScoredEntry {
    name: &'static str,
    description: &'static str,
    score: i64,
}

fn score_matches(
    query: &str,
    entries: &[(&'static str, &'static str, &'static str)],
    max_results: usize,
) -> Vec<ScoredEntry> {
    let q_terms: Vec<&str> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    if q_terms.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<ScoredEntry> = entries
        .iter()
        .filter_map(|(name, desc, keywords)| {
            let haystack = format!(
                "{} {} {}",
                name.to_lowercase(),
                desc.to_lowercase(),
                keywords.to_lowercase()
            );
            let mut score: i64 = 0;
            for term in &q_terms {
                if haystack.contains(term) {
                    score += 10;
                    if name.to_lowercase().contains(term) {
                        score += 20;
                    }
                }
            }
            if score > 0 {
                Some(ScoredEntry {
                    name,
                    description: desc,
                    score,
                })
            } else {
                None
            }
        })
        .collect();

    scored.sort_by(|a, b| b.score.cmp(&a.score).then(a.name.cmp(b.name)));
    scored.truncate(max_results);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_ask_user_question_for_natural_language_queries() {
        let entries = searchable_registry();
        let cases = [
            "ask the user a question",
            "ask user question",
            "prompt the user for confirmation",
            "askuserquestion",
        ];
        for q in cases {
            let results = score_matches(&q.to_lowercase(), &entries, 5);
            assert!(
                results.iter().any(|r| r.name == "askUserQuestion"),
                "query {:?} did not surface askUserQuestion (got {:?})",
                q,
                results.iter().map(|r| r.name).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn finds_webfetch_for_fetch_query() {
        let entries = searchable_registry();
        let results = score_matches("fetch a url", &entries, 5);
        assert!(results.iter().any(|r| r.name == "webfetch"));
    }

    #[test]
    fn empty_query_returns_nothing() {
        let entries = searchable_registry();
        let results = score_matches("", &entries, 5);
        assert!(results.is_empty());
    }

    #[test]
    fn unlock_and_read_roundtrip() {
        let sid = "test-session-tool-search-unlock";
        clear_session(sid);
        assert!(unlocked_for_session(sid).is_empty());
        unlock_tool(sid, "askUserQuestion");
        unlock_tool(sid, "webfetch");
        let set = unlocked_for_session(sid);
        assert!(set.contains("askUserQuestion"));
        assert!(set.contains("webfetch"));
        clear_session(sid);
        assert!(unlocked_for_session(sid).is_empty());
    }

    #[test]
    fn registry_key_mapping_covers_all_entries() {
        for (name, _, _) in searchable_registry() {
            let key = registry_key_for_search_name(name);
            assert!(!key.is_empty(), "no registry key for search name {}", name);
        }
    }
}
