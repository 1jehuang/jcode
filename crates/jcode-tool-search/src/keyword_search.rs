//! Keyword search with scoring for tool discovery.
//!
//! Scores tools by matching query terms against tool names and descriptions.
//! MCP tool names (`mcp__server__action`) are parsed and matched by server
//! and action tokens separately for better relevance.
//!
//! | Match type         | Points |
//! |--------------------|--------|
//! | Exact name match   | 100    |
//! | Name contains      | 50     |
//! | Description match  | 20     |
//! | Partial match      | 10     |

use crate::tool_name::parse_tool_name;
use serde::{Deserialize, Serialize};

/// Full tool name equals query (case-insensitive).
pub const SCORE_EXACT_NAME: u32 = 100;
/// Tool name display string contains the query.
pub const SCORE_NAME_CONTAINS: u32 = 50;
/// Tool description contains the query.
pub const SCORE_DESCRIPTION_CONTAINS: u32 = 20;
/// Any search token overlaps with a name part.
pub const SCORE_PARTIAL: u32 = 10;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A tool entry available for keyword search.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolEntry {
    /// Raw tool name (e.g. `"mcp__zai__analyze_image"` or `"AgentGrep"`).
    pub name: String,
    /// Optional description loaded lazily.
    pub description: Option<String>,
}

impl ToolEntry {
    /// Create a tool entry with name only (description loaded later).
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
        }
    }

    /// Create a tool entry with name and description.
    pub fn with_description(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: Some(description.into()),
        }
    }
}

/// Convenience alias used in public API signatures.
pub type Tool = ToolEntry;

/// A single scored search result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchResult {
    /// Raw tool name.
    pub tool_name: String,
    /// Relevance score (higher is better).
    pub score: u32,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Search tool entries against a query string.
///
/// Returns results sorted by descending score, then alphabetically by name.
/// MCP tool names are handled specially: their `server` and `action` segments
/// are parsed individually so queries like a server name still match well.
pub fn search_tools(query: &str, tools: &[Tool]) -> Vec<SearchResult> {
    let query_lower = query.trim().to_lowercase();
    if query_lower.is_empty() {
        return Vec::new();
    }

    // Fast path: exact full name match returns immediately.
    let query_clone = query_lower.clone();
    if let Some(entry) = tools.iter().find(|t| t.name.to_lowercase() == query_clone) {
        return vec![SearchResult {
            tool_name: entry.name.clone(),
            score: SCORE_EXACT_NAME,
        }];
    }

    let tokens: Vec<&str> = query_lower
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .collect();

    let mut results: Vec<SearchResult> = tools
        .iter()
        .filter_map(|entry| score_entry(entry, &query_lower, &tokens))
        .filter(|r| r.score > 0)
        .collect();

    rank_results(&mut results);
    results
}

/// Sort search results by descending score, then alphabetically by tool name.
pub fn rank_results(results: &mut [SearchResult]) {
    results.sort_by(|a, b| b.score.cmp(&a.score).then(a.tool_name.cmp(&b.tool_name)));
}

// ---------------------------------------------------------------------------
// Internal scoring
// ---------------------------------------------------------------------------

fn score_entry(entry: &ToolEntry, query_lower: &str, tokens: &[&str]) -> Option<SearchResult> {
    let parsed = parse_tool_name(&entry.name)?;
    let display = parsed.full.to_lowercase();

    let mut score: u32 = 0;

    // --- MCP special handling: check server name directly ---
    if parsed.is_mcp && query_lower.contains(&parsed.parts[0]) {
        // Query mentions the server name -- strong signal for MCP tools.
        score += SCORE_PARTIAL;
    }

    for token in tokens {
        // Exact part match in parsed name tokens.
        if parsed.parts.iter().any(|p| p == token) {
            score += SCORE_PARTIAL;
        } else if display.contains(token) {
            // Display string contains the token (substring).
            score += SCORE_PARTIAL;
        }

        // Description match.
        if let Some(desc) = &entry.description
            && desc.to_lowercase().contains(token)
        {
            score += SCORE_DESCRIPTION_CONTAINS;
        }
    }

    // Bonus: full display name contains entire query (stronger than per-token).
    if score > 0 && display.contains(query_lower) {
        score += SCORE_NAME_CONTAINS - SCORE_PARTIAL;
    }

    if score == 0 {
        return None;
    }

    Some(SearchResult {
        tool_name: entry.name.clone(),
        score,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_name_match_scores_highest() {
        let tools = vec![ToolEntry::new("WebSearch"), ToolEntry::new("AgentGrep")];
        let results = search_tools("websearch", &tools);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].score, SCORE_EXACT_NAME);
    }

    #[test]
    fn name_contains_scores_well() {
        let tools = vec![ToolEntry::with_description(
            "WebSearch",
            "Search the internet",
        )];
        let results = search_tools("web", &tools);
        assert_eq!(results.len(), 1);
        assert!(results[0].score >= SCORE_NAME_CONTAINS);
    }

    #[test]
    fn description_match() {
        let tools = vec![
            ToolEntry::with_description("Browser", "Automate web browser interactions"),
            ToolEntry::new("Read"),
        ];
        let results = search_tools("automate", &tools);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_name, "Browser");
        assert!(results[0].score >= SCORE_DESCRIPTION_CONTAINS);
    }

    #[test]
    fn partial_token_overlap() {
        let tools = vec![ToolEntry::new("AgentGrep")];
        let results = search_tools("grep", &tools);
        assert_eq!(results.len(), 1);
        assert!(results[0].score >= SCORE_PARTIAL);
    }

    #[test]
    fn empty_query_returns_nothing() {
        let tools = vec![ToolEntry::new("AgentGrep")];
        assert!(search_tools("", &tools).is_empty());
        assert!(search_tools("   ", &tools).is_empty());
    }

    #[test]
    fn no_match_returns_empty() {
        let tools = vec![ToolEntry::new("AgentGrep")];
        assert!(search_tools("zzzznotfound", &tools).is_empty());
    }

    #[test]
    fn combined_name_and_description_beats_description_only() {
        let tools = vec![
            ToolEntry::with_description("Browser", "Automate web browsing"),
            ToolEntry::with_description("Read", "Read files and browse content"),
        ];
        let results = search_tools("browse", &tools);
        assert!(!results.is_empty());
        assert_eq!(results[0].tool_name, "Browser");
    }

    #[test]
    fn mcp_tool_server_name_match() {
        let tools = vec![
            ToolEntry::with_description("mcp__slack__post_message", "Send a message to Slack"),
            ToolEntry::with_description("mcp__github__create_issue", "Create a GitHub issue"),
        ];
        let results = search_tools("slack", &tools);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_name, "mcp__slack__post_message");
    }

    #[test]
    fn mcp_tool_action_match() {
        let tools = vec![
            ToolEntry::with_description("mcp__slack__post_message", "Send a message to Slack"),
            ToolEntry::with_description("mcp__slack__list_channels", "List Slack channels"),
        ];
        let results = search_tools("message", &tools);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_name, "mcp__slack__post_message");
    }

    #[test]
    fn sort_by_score_then_name() {
        let tools = vec![
            ToolEntry::with_description("AlphaTool", "Search alpha tools"),
            ToolEntry::with_description("BetaTool", "Search beta tools"),
        ];
        let results = search_tools("search", &tools);
        assert_eq!(results.len(), 2);
        // Both have description match (20) + name "search" not present,
        // so same score; sorted alphabetically.
        assert_eq!(results[0].tool_name, "AlphaTool");
        assert_eq!(results[1].tool_name, "BetaTool");
    }

    #[test]
    fn rank_results_reorders() {
        let mut results = vec![
            SearchResult {
                tool_name: "B".into(),
                score: 10,
            },
            SearchResult {
                tool_name: "A".into(),
                score: 20,
            },
        ];
        rank_results(&mut results);
        assert_eq!(results[0].tool_name, "A");
        assert_eq!(results[1].tool_name, "B");
    }

    #[test]
    fn multi_token_query_scores_across_fields() {
        let tools = vec![ToolEntry::with_description(
            "AgentGrep",
            "Search code files with regex",
        )];
        let results = search_tools("agent grep", &tools);
        assert_eq!(results.len(), 1);
        // "agent" matches name part (10) + "grep" matches name part (10) = 20
        // Plus display contains "agent grep" bonus.
        assert!(results[0].score >= SCORE_PARTIAL * 2);
    }
}
