use super::*;

pub(crate) fn tool_name_is_allowed(allowed: &HashSet<String>, name: &str) -> bool {
    allowed.contains(name)
        || (allowed.contains("mcp") && is_mcp_tool_name(name))
        || (name == "mcp" && allowed.iter().any(|tool| is_mcp_tool_name(tool)))
        || (is_fixed_mcp_tool(name) && allowed.iter().any(|tool| tool.starts_with("mcp__")))
}
