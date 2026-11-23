use super::*;

pub(crate) fn tool_name_is_allowed(allowed: &HashSet<String>, name: &str) -> bool {
    allowed.contains(name)
        || (allowed.contains("mcp") && is_mcp_tool_name(name))
        || (name == "mcp" && allowed.iter().any(|tool| is_mcp_tool_name(tool)))
        || (is_fixed_mcp_tool(name) && allowed.iter().any(|tool| tool.starts_with("mcp__")))
}

/// The allowlist that permits only what both `base` and `narrow` permit.
///
/// Entries are compared as written, never through the "`mcp` allows every MCP
/// tool" rule applied to a displayed tool list: the broad `mcp` entry survives
/// only when both lists carry it, and otherwise gives way to the other list's
/// specific MCP entries. The result never allows more than `base` does.
pub(crate) fn intersect_allowed(
    base: &HashSet<String>,
    narrow: &HashSet<String>,
) -> HashSet<String> {
    let mut out = HashSet::new();
    for name in base {
        let covered = narrow.contains(name)
            || (narrow.contains("mcp") && name != "mcp" && is_mcp_tool_name(name));
        if covered {
            out.insert(name.clone());
        }
    }
    if base.contains("mcp") {
        out.extend(narrow.iter().filter(|name| is_mcp_tool_name(name)).cloned());
    }
    out
}
