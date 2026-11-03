use super::{tool_name_is_allowed, tool_name_is_disabled};
use std::collections::HashSet;

#[test]
fn allowing_mcp_also_allows_dynamic_server_tools() {
    let allowed = HashSet::from(["mcp".to_string()]);

    assert!(tool_name_is_allowed(&allowed, "mcp"));
    assert!(tool_name_is_allowed(&allowed, "mcp__filesystem__read_file"));
    assert!(!tool_name_is_allowed(&allowed, "mcpish"));
    assert!(!tool_name_is_allowed(&allowed, "bash"));
}

#[test]
fn disabling_mcp_also_disables_dynamic_server_tools() {
    let disabled = HashSet::from(["mcp".to_string()]);

    assert!(tool_name_is_disabled(&disabled, "mcp"));
    assert!(tool_name_is_disabled(
        &disabled,
        "mcp__filesystem__read_file"
    ));
    assert!(!tool_name_is_disabled(&disabled, "mcpish"));
    assert!(!tool_name_is_disabled(&disabled, "bash"));
}
