//! Tool name parsing for deferred tool loading.
//!
//! Parses tool names into a searchable token representation without loading
//! full tool descriptions. Handles both MCP (`mcp__server__action`) and
//! regular CamelCase tool names.

/// A parsed tool name with pre-computed search tokens.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ParsedToolName {
    /// Lowercase search tokens derived from the tool name.
    pub parts: Vec<String>,
    /// Human-readable display name (space-separated tokens).
    pub full: String,
    /// Whether this is an MCP tool (`mcp__*`).
    pub is_mcp: bool,
}

/// Parse a raw tool name into a [`ParsedToolName`].
///
/// Returns `None` if the name is empty or whitespace-only.
///
/// - MCP: `mcp__server__action` → snake_case tokens from server + action.
/// - Regular: `CamelCase` / `snake_case` → split on case boundaries and underscores.
pub fn parse_tool_name(name: &str) -> Option<ParsedToolName> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }

    if let Some(rest) = name.strip_prefix("mcp__") {
        let (server, action) = rest.split_once("__")?;
        let mut parts: Vec<String> = snake_tokens(server);
        parts.extend(snake_tokens(action));
        let full = parts.join(" ");
        Some(ParsedToolName {
            parts,
            full,
            is_mcp: true,
        })
    } else {
        let parts = split_name_tokens(name);
        let full = parts.join(" ");
        Some(ParsedToolName {
            parts,
            full,
            is_mcp: false,
        })
    }
}

/// Split a snake_case string into lowercase tokens, skipping empty segments.
fn snake_tokens(s: &str) -> Vec<String> {
    s.split('_')
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// Split a mixed-case tool name into lowercase tokens.
///
/// Handles CamelCase boundaries, consecutive uppercase acronyms, and underscores:
/// - `"WebSearch"` → `["web", "search"]`
/// - `"MCPInspector"` → `["mcp", "inspector"]`
/// - `"agent_grep"` → `["agent", "grep"]`
/// - `"getHTTPResponse"` → `["get", "http", "response"]`
fn split_name_tokens(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    // Current lowercase word being accumulated.
    let mut word = String::new();
    // Consecutive uppercase chars (potential acronym).
    let mut upper_run = String::new();

    for ch in s.chars() {
        if ch == '_' {
            // Hard boundary: flush both buffers.
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word).to_lowercase());
            }
            if !upper_run.is_empty() {
                tokens.push(std::mem::take(&mut upper_run).to_lowercase());
            }
            continue;
        }

        if ch.is_uppercase() {
            // If we were building a word and hit uppercase, the word is done.
            if !word.is_empty() && !word.chars().all(|c| c.is_uppercase()) {
                tokens.push(std::mem::take(&mut word).to_lowercase());
            }
            upper_run.push(ch);
        } else {
            // Lowercase character.
            if upper_run.len() > 1 {
                // End of an acronym run (e.g. "HTTP"). The last uppercase char
                // starts the new word (e.g. "Response" after "HTTP").
                // Length check above guarantees a char exists; treat absence
                // defensively rather than panicking.
                if let Some(last) = upper_run.pop() {
                    tokens.push(upper_run.to_lowercase());
                    upper_run.clear();
                    word.push(last);
                }
            } else if upper_run.len() == 1 {
                // Single uppercase char starts the current word.
                if let Some(ch) = upper_run.pop() {
                    word.push(ch);
                }
            }
            word.push(ch);
        }
    }

    // Flush remaining buffers.
    if !upper_run.is_empty() {
        tokens.push(upper_run.to_lowercase());
    }
    if !word.is_empty() {
        tokens.push(word.to_lowercase());
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_tool() {
        let p = parse_tool_name("mcp__zai_mcp_server__analyze_image").unwrap();
        assert!(p.is_mcp);
        assert_eq!(p.parts, vec!["zai", "mcp", "server", "analyze", "image"]);
        assert_eq!(p.full, "zai mcp server analyze image");
    }

    #[test]
    fn regular_camel_case() {
        let p = parse_tool_name("WebSearch").unwrap();
        assert!(!p.is_mcp);
        assert_eq!(p.parts, vec!["web", "search"]);
        assert_eq!(p.full, "web search");
    }

    #[test]
    fn consecutive_uppercase_acronym() {
        let p = parse_tool_name("MCPInspector").unwrap();
        assert_eq!(p.parts, vec!["mcp", "inspector"]);
    }

    #[test]
    fn underscore_regular() {
        let p = parse_tool_name("agent_grep").unwrap();
        assert!(!p.is_mcp);
        assert_eq!(p.parts, vec!["agent", "grep"]);
    }

    #[test]
    fn mixed_uppercase_acronym_and_word() {
        let p = parse_tool_name("getHTTPResponse").unwrap();
        assert_eq!(p.parts, vec!["get", "http", "response"]);
    }

    #[test]
    fn standalone_acronym() {
        let p = parse_tool_name("MCP").unwrap();
        assert_eq!(p.parts, vec!["mcp"]);
    }

    #[test]
    fn single_word() {
        let p = parse_tool_name("Read").unwrap();
        assert_eq!(p.parts, vec!["read"]);
        assert_eq!(p.full, "read");
    }

    #[test]
    fn empty_and_whitespace() {
        assert!(parse_tool_name("").is_none());
        assert!(parse_tool_name("   ").is_none());
    }

    #[test]
    fn mcp_action_with_underscores() {
        let p = parse_tool_name("mcp__srv__do_thing").unwrap();
        assert_eq!(p.parts, vec!["srv", "do", "thing"]);
    }
}
