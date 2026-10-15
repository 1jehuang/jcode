use super::*;
use crate::mcp::{McpConfig, McpManager, McpSchemaCache, McpToolDef};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;

#[test]
fn headless_mcp_cold_wait_excludes_disabled_and_cached_servers() {
    let config: McpConfig = serde_json::from_value(json!({"servers": {
        "cold": {"url": "https://example.test/cold"},
        "disabled": {"url": "https://example.test/disabled", "enabled": false},
        "warm": {"url": "https://example.test/warm"}
    }}))
    .unwrap();
    let mut cache = McpSchemaCache::default();
    cache.update("warm", &config.servers["warm"], vec![echo_definition()]);
    assert_eq!(uncached_enabled_mcp_servers(&config, &cache), vec!["cold"]);
    let mut changed = config.clone();
    changed.servers.get_mut("warm").unwrap().url = Some("https://example.test/new".into());
    let mut uncached = uncached_enabled_mcp_servers(&changed, &cache);
    uncached.sort();
    assert_eq!(uncached, vec!["cold", "warm"]);
}

fn echo_definition() -> McpToolDef {
    McpToolDef {
        name: "echo".into(),
        description: Some("Test-only cached schema".into()),
        input_schema: json!({"type": "object"}),
    }
}

#[tokio::test]
async fn headless_mcp_cold_wait_uses_exact_identity_for_colliding_aliases() {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let config: McpConfig = serde_json::from_value(json!({"servers": {
        "remote-prod": {"url": "https://example.test/one"},
        "remote.prod": {"url": "https://example.test/two"}
    }}))
    .unwrap();
    std::fs::write(
        sandbox.root().join("mcp.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let manager = Arc::new(RwLock::new(McpManager::with_config(config)));
    let registry = crate::tool::Registry::empty();
    let defs = vec![
        ("remote-prod".into(), echo_definition()),
        ("remote.prod".into(), echo_definition()),
    ];
    registry
        .reconcile_mcp_tools(crate::mcp::create_mcp_tools_from_cached_many(
            &defs, manager,
        ))
        .await;
    assert!(registry.has_mcp_tools_for_server("remote-prod"));
    assert!(registry.has_mcp_tools_for_server("remote.prod"));
    assert!(!registry.has_mcp_tools_for_server("remote_prod"));
    tokio::time::timeout(
        std::time::Duration::from_millis(100),
        wait_for_cold_cache_mcp_tools(&registry),
    )
    .await
    .expect("normalized/collision-suffixed aliases must not cause a five-second wait");
    registry.unregister_mcp_server("remote-prod").await;
    assert!(!registry.has_mcp_tools_for_server("remote-prod"));
    assert!(registry.has_mcp_tools_for_server("remote.prod"));
}

#[tokio::test]
async fn headless_mcp_disabled_only_configuration_returns_without_waiting() {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    std::fs::write(
        sandbox.root().join("mcp.json"),
        json!({"servers": {
            "disabled": {"url": "https://example.test/mcp", "enabled": false}
        }})
        .to_string(),
    )
    .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_millis(100),
        wait_for_cold_cache_mcp_tools(&crate::tool::Registry::empty()),
    )
    .await
    .expect("disabled servers must not consume the cold-cache wait budget");
}
