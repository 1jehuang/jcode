//! Mapping from Claude Code tool names to jcode tool names.
//!
//! Agent definition files written for Claude Code list tools such as `Read`,
//! `Bash(git push *)` or `Task`. This maps them onto jcode's tools. A
//! specifier in parentheses removes or grants the whole tool, as Claude's own
//! documentation says for `disallowedTools`. Unknown names map to nothing.

/// jcode tool names a Claude tool entry stands for. Empty for unknown names.
pub fn map_claude_tool(entry: &str) -> Vec<String> {
    let name = entry.split('(').next().unwrap_or(entry).trim();
    let mapped: &[&str] = match name {
        "Read" | "read" => &["read"],
        "Write" | "write" => &["write"],
        "Edit" | "MultiEdit" | "edit" => &["edit"],
        "Bash" | "bash" => &["bash"],
        "Grep" | "Glob" | "LS" | "agentgrep" => &["agentgrep"],
        "WebFetch" | "webfetch" => &["webfetch"],
        "WebSearch" | "web_search" => &["web_search"],
        "Task" | "Agent" => &["swarm", "subagent"],
        "TodoWrite" | "TodoRead" | "todo" => &["todo"],
        "NotebookEdit" => &[],
        other if is_plain_tool_name(other) => {
            return vec![jcode_tool_types::resolve_tool_name(other).to_string()];
        }
        _ => &[],
    };
    mapped.iter().map(|tool| (*tool).to_string()).collect()
}

/// jcode tool names are lowercase identifiers. Anything else is not a tool.
fn is_plain_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

/// Map a list of entries, de-duplicated, in first-seen order.
pub fn map_claude_tools<'a>(entries: impl IntoIterator<Item = &'a String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in entries {
        for tool in map_claude_tool(entry) {
            if !out.contains(&tool) {
                out.push(tool);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_names_map_to_jcode_tools() {
        assert_eq!(map_claude_tool("Read"), ["read"]);
        assert_eq!(map_claude_tool("MultiEdit"), ["edit"]);
        assert_eq!(map_claude_tool("Glob"), ["agentgrep"]);
        assert_eq!(map_claude_tool("WebSearch"), ["web_search"]);
        assert_eq!(map_claude_tool("Task"), ["swarm", "subagent"]);
        assert_eq!(map_claude_tool("TodoWrite"), ["todo"]);
    }

    #[test]
    fn specifier_applies_to_the_whole_tool() {
        assert_eq!(map_claude_tool("Bash(git push *)"), ["bash"]);
    }

    #[test]
    fn jcode_names_pass_through_and_unknown_is_dropped() {
        assert_eq!(map_claude_tool("agentgrep"), ["agentgrep"]);
        assert_eq!(map_claude_tool("shell"), ["bash"]);
        assert!(map_claude_tool("NotebookEdit").is_empty());
        assert!(map_claude_tool("Totally Unknown!").is_empty());
        assert!(map_claude_tool("").is_empty());
    }

    #[test]
    fn list_mapping_dedups_in_order() {
        let entries = [
            "Read".to_string(),
            "Grep".to_string(),
            "read".to_string(),
            "Bash(x)".to_string(),
        ];
        assert_eq!(map_claude_tools(&entries), ["read", "agentgrep", "bash"]);
    }
}
