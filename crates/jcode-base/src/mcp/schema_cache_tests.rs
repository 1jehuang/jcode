//! Tests for the MCP tool-schema disk cache.

use super::*;
use crate::mcp::protocol::McpServerConfig;
use serde_json::json;
use std::collections::HashMap;

fn cfg(command: &str, args: &[&str]) -> McpServerConfig {
    McpServerConfig {
        command: command.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        env: HashMap::new(),
        shared: true,
        transport: None,
        url: None,
    }
}

fn tool(name: &str) -> McpToolDef {
    McpToolDef {
        name: name.to_string(),
        description: Some(format!("{name} desc")),
        input_schema: json!({"type": "object"}),
    }
}

#[test]
fn fingerprint_is_stable_and_sensitive() {
    let a = cfg("node", &["server.js"]);
    let b = cfg("node", &["server.js"]);
    assert_eq!(
        fingerprint_config(&a),
        fingerprint_config(&b),
        "identical configs must fingerprint equally"
    );

    // Different args -> different fingerprint.
    let c = cfg("node", &["other.js"]);
    assert_ne!(fingerprint_config(&a), fingerprint_config(&c));

    // Different command -> different fingerprint.
    let d = cfg("python", &["server.js"]);
    assert_ne!(fingerprint_config(&a), fingerprint_config(&d));

    // Different shared flag -> different fingerprint.
    let mut e = cfg("node", &["server.js"]);
    e.shared = false;
    assert_ne!(fingerprint_config(&a), fingerprint_config(&e));
}

#[test]
fn fingerprint_env_order_independent() {
    let mut a = cfg("node", &["s.js"]);
    a.env.insert("A".into(), "1".into());
    a.env.insert("B".into(), "2".into());
    let mut b = cfg("node", &["s.js"]);
    b.env.insert("B".into(), "2".into());
    b.env.insert("A".into(), "1".into());
    assert_eq!(
        fingerprint_config(&a),
        fingerprint_config(&b),
        "env map ordering must not affect fingerprint"
    );

    let mut c = cfg("node", &["s.js"]);
    c.env.insert("A".into(), "different".into());
    assert_ne!(fingerprint_config(&a), fingerprint_config(&c));
}

#[test]
fn tools_for_respects_fingerprint() {
    let mut cache = McpSchemaCache::default();
    let config = cfg("node", &["s.js"]);
    cache.update("srv", &config, vec![tool("alpha"), tool("beta")]);

    // Same config -> cached tools returned.
    let got = cache.tools_for("srv", &config).expect("cached tools");
    assert_eq!(got.len(), 2);

    // Reconfigured server -> stale schemas must NOT be returned.
    let reconfigured = cfg("node", &["s2.js"]);
    assert!(
        cache.tools_for("srv", &reconfigured).is_none(),
        "config change must invalidate cached schemas"
    );

    // Unknown server -> None.
    assert!(cache.tools_for("nope", &config).is_none());
}

#[test]
fn update_reports_change_only_on_diff() {
    let mut cache = McpSchemaCache::default();
    let config = cfg("node", &["s.js"]);

    assert!(
        cache.update("srv", &config, vec![tool("a")]),
        "new entry changes"
    );
    assert!(
        !cache.update("srv", &config, vec![tool("a")]),
        "identical re-update must not be marked changed"
    );
    assert!(
        cache.update("srv", &config, vec![tool("a"), tool("b")]),
        "added tool must be marked changed"
    );
    // Tool reordering should NOT count as a change (set comparison).
    assert!(
        !cache.update("srv", &config, vec![tool("b"), tool("a")]),
        "reordered identical tools must not churn the cache"
    );
}

#[test]
fn retain_prunes_removed_servers() {
    let mut cache = McpSchemaCache::default();
    let config = cfg("node", &["s.js"]);
    cache.update("keep", &config, vec![tool("a")]);
    cache.update("drop", &config, vec![tool("b")]);
    assert_eq!(cache.server_count(), 2);

    let pruned = cache.retain_servers(&["keep".to_string()]);
    assert!(pruned, "retain must report pruning");
    assert_eq!(cache.server_count(), 1);
    assert!(cache.tools_for("keep", &config).is_some());
    assert!(cache.tools_for("drop", &config).is_none());

    // No-op retain reports false.
    assert!(!cache.retain_servers(&["keep".to_string()]));
}

#[test]
fn load_save_roundtrip_via_temp_home() {
    let _guard = crate::storage::lock_test_env();
    let tmp = tempfile::tempdir().unwrap();
    // Point the jcode dir at a temp location.
    unsafe {
        std::env::set_var("JCODE_HOME", tmp.path());
    }

    let mut cache = McpSchemaCache::default();
    let config = cfg("node", &["s.js"]);
    cache.update("srv", &config, vec![tool("alpha")]);
    cache.save();

    let reloaded = McpSchemaCache::load();
    let tools = reloaded.tools_for("srv", &config).expect("reloaded tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "alpha");

    unsafe {
        std::env::remove_var("JCODE_HOME");
    }
}
