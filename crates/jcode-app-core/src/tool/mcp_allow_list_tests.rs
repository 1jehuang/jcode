//! Tests for `#[path]`-attributed module `mcp_allow_list_tests` of `mod.rs`.
//!
//! Extracted from the parent file: an inline `#[cfg(test)] mod` counts toward
//! the code-size budget, which only exempts `*_tests.rs` files.

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
