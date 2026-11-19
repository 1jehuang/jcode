//! XJ-07: Verify failure and rollback for the Xirp/Portal MCP integration.
//!
//! Acceptance criteria (verbatim from `docs/XIRP_CUSTOM_HARNESS.md:132`):
//!   1. Missing CLI fails clearly (when `xirp` is not on PATH)
//!   2. Expired auth fails clearly (when the Portal token is revoked)
//!   3. Network loss fails clearly (when the Portal endpoint is unreachable)
//!   4. Malformed events fail clearly (when an unexpected message arrives)
//!   5. Unregister restores prior configuration
//!
//! Each test uses real stdio MCP server fixtures (shell/Python scripts) and
//! the `McpManager` / `McpClient` public API. No vendor binaries required.

use jcode_base::mcp::{McpConfig, McpManager, McpServerConfig};
use serde_json::json;
use std::collections::HashMap;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an `McpServerConfig` for a stdio server.
fn stdio_config(command: &str, args: Vec<&str>, timeout_secs: Option<u64>) -> McpServerConfig {
    McpServerConfig {
        command: command.to_string(),
        args: args.into_iter().map(String::from).collect(),
        env: HashMap::new(),
        shared: false,
        transport: None,
        url: None,
        headers: HashMap::new(),
        enabled: None,
        disabled: None,
        timeout_secs,
    }
}

/// Wrap a single server into an `McpConfig`.
fn single_config(name: &str, config: McpServerConfig) -> McpConfig {
    let mut servers = HashMap::new();
    servers.insert(name.to_string(), config);
    McpConfig { servers }
}

// ---------------------------------------------------------------------------
// AC-1: Missing CLI fails clearly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_cli_fails_clearly() {
    // Use a binary name that is guaranteed not to exist on any system.
    let config = stdio_config(
        "/nonexistent/xirp-binary-that-does-not-exist",
        vec![],
        Some(5),
    );
    let manager = McpManager::with_config(single_config("xirp", config));

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        manager.call_tool("xirp", "some_tool", json!({})),
    )
    .await;

    let inner = result.expect("must not hang");
    let err = inner.expect_err("missing CLI must produce an error");
    let msg = format!("{err:#}");

    // The error must name the server AND mention the spawn failure.
    assert!(
        msg.contains("xirp"),
        "error must name the server 'xirp': {msg}"
    );
    assert!(
        msg.contains("Failed to spawn") || msg.contains("No such file") || msg.contains("spawn"),
        "error must indicate the binary was not found or could not be spawned: {msg}"
    );
}

// ---------------------------------------------------------------------------
// AC-2: Expired auth fails clearly
// ---------------------------------------------------------------------------

/// A fake MCP server that responds to `initialize` with a JSON-RPC error
/// simulating an expired or revoked authentication token.
const AUTH_ERROR_SERVER: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32001,"message":"Authentication token expired or revoked. Please re-authenticate via Portal."}}\n' "$id"
      ;;
    *)
      # Ignore anything else (notifications, etc.)
      ;;
  esac
done
"#;

#[tokio::test]
async fn expired_auth_fails_clearly() {
    let config = stdio_config("/bin/sh", vec!["-c", AUTH_ERROR_SERVER], Some(5));
    let manager = McpManager::with_config(single_config("portal-bridge", config));

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        manager.call_tool("portal-bridge", "any_tool", json!({})),
    )
    .await;

    let inner = result.expect("must not hang on auth error");
    let err = inner.expect_err("expired auth must produce an error");
    let msg = format!("{err:#}");

    // Error must name the server and include the auth failure message.
    assert!(
        msg.contains("portal-bridge"),
        "error must name the server: {msg}"
    );
    assert!(
        msg.contains("expired") || msg.contains("Authentication") || msg.contains("-32001"),
        "error must indicate the auth failure: {msg}"
    );
}

// ---------------------------------------------------------------------------
// AC-3: Network loss fails clearly
// ---------------------------------------------------------------------------

/// A fake MCP server that initializes successfully, responds to one
/// tools/list, and then closes stdout (simulating a dropped connection
/// or network loss).
const NETWORK_LOSS_SERVER: &str = r#"
TOOL_CALLS=0
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"portal-bridge","version":"0.1"}}}\n' "$id"
      ;;
    *'"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"ping","description":"test tool","inputSchema":{"type":"object"}}]}}\n' "$id"
      # Simulate network loss: close stdout and exit after tools/list
      TOOL_CALLS=$((TOOL_CALLS + 1))
      if [ "$TOOL_CALLS" -ge 1 ]; then
        exec 1>&-  # Close stdout
        sleep 0.1
        exit 0
      fi
      ;;
    *'"shutdown"'*)
      exit 0
      ;;
  esac
done
"#;

#[tokio::test]
async fn network_loss_fails_clearly() {
    let config = stdio_config("/bin/sh", vec!["-c", NETWORK_LOSS_SERVER], Some(5));
    let manager = McpManager::with_config(single_config("portal-bridge", config));

    // First call connects + gets tools (succeeds through initialize + tools/list).
    // The server drops the connection after tools/list, so a subsequent tool
    // call must fail with a clear "exited" or "closed" error.
    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        manager.call_tool("portal-bridge", "ping", json!({})),
    )
    .await;

    // Second call must fail clearly (server is gone).
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        manager.call_tool("portal-bridge", "ping", json!({})),
    )
    .await;

    let inner = result.expect("must not hang indefinitely");
    let err = inner.expect_err("connection after network loss must fail");
    let msg = format!("{err:#}");

    assert!(
        msg.contains("portal-bridge"),
        "error must name the server: {msg}"
    );
    assert!(
        msg.contains("exited") || msg.contains("closed") || msg.contains("EOF"),
        "error must indicate the connection was lost: {msg}"
    );
}

// ---------------------------------------------------------------------------
// AC-4: Malformed events fail clearly
// ---------------------------------------------------------------------------

/// A fake MCP server that responds to `initialize` with valid JSON-RPC,
/// but then sends non-JSON garbage for `tools/list`.
const MALFORMED_SERVER: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"squab-mock","version":"0.1"}}}\n' "$id"
      ;;
    *'"tools/list"'*)
      # Send malformed data: not valid JSON-RPC
      printf 'THIS IS NOT VALID JSON AND SHOULD BE SKIPPED\n'
      # Then send a valid but malformed response (missing required fields)
      printf '{"broken": true, "missing": "result and error"}\n'
      # Then close stdout to unblock the reader
      sleep 0.2
      exec 1>&-
      exit 0
      ;;
    *'"shutdown"'*)
      exit 0
      ;;
  esac
done
"#;

#[tokio::test]
async fn malformed_events_fail_clearly() {
    let config = stdio_config("/bin/sh", vec!["-c", MALFORMED_SERVER], Some(5));
    let manager = McpManager::with_config(single_config("squab-mock", config));

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        manager.call_tool("squab-mock", "any_tool", json!({})),
    )
    .await;

    let inner = result.expect("must not hang on malformed data");
    // The call should either:
    // a) Fail with an error about tools/list failing (initialize succeeded, tools/list timed out)
    // b) Fail with an error about the server exiting
    let err = inner.expect_err("malformed events must produce an error");
    let msg = format!("{err:#}");

    assert!(
        msg.contains("squab-mock"),
        "error must name the server: {msg}"
    );
    // The error should indicate that the response was unusable, the server
    // exited, or the request timed out — all are clear failure modes.
    assert!(
        msg.contains("exited")
            || msg.contains("timeout")
            || msg.contains("closed")
            || msg.contains("tools")
            || msg.contains("failed to"),
        "error must indicate a clear failure mode: {msg}"
    );
}

// ---------------------------------------------------------------------------
// AC-5: Unregister restores prior configuration
// ---------------------------------------------------------------------------

/// A working fake MCP server (for the server that stays registered).
const WORKING_SERVER: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"keep-me","version":"0.1"}}}\n' "$id"
      ;;
    *'"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[]}}\n' "$id"
      ;;
    *'"shutdown"'*)
      exit 0
      ;;
  esac
done
"#;

#[tokio::test]
async fn unregister_restores_prior_configuration() {
    // Build initial config with two servers: "keep-me" and "portal-bridge".
    let keep_config = stdio_config("/bin/sh", vec!["-c", WORKING_SERVER], Some(5));
    let portal_config = stdio_config("/bin/sh", vec!["-c", AUTH_ERROR_SERVER], Some(5));

    let mut initial_servers = HashMap::new();
    initial_servers.insert("keep-me".to_string(), keep_config.clone());
    initial_servers.insert("portal-bridge".to_string(), portal_config.clone());
    let initial_config = McpConfig {
        servers: initial_servers,
    };

    // Verify initial config has both servers.
    assert_eq!(initial_config.servers.len(), 2);
    assert!(initial_config.servers.contains_key("keep-me"));
    assert!(initial_config.servers.contains_key("portal-bridge"));

    // Snapshot the config to disk (simulating the user's mcp.json).
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let config_path = temp_dir.path().join("mcp.json");
    initial_config
        .save_to_file(&config_path)
        .expect("save initial config");

    // Load it back and verify round-trip.
    let loaded = McpConfig::load_from_file(&config_path).expect("load config");
    assert_eq!(loaded.servers.len(), 2);

    // "Unregister" the portal-bridge: remove it from config and save.
    let mut after_unregister = loaded.clone();
    let removed = after_unregister.servers.remove("portal-bridge");
    assert!(removed.is_some(), "portal-bridge must have been present");
    after_unregister
        .save_to_file(&config_path)
        .expect("save after unregister");

    // Verify the prior server is preserved and the removed one is gone.
    let restored = McpConfig::load_from_file(&config_path).expect("load restored config");
    assert_eq!(
        restored.servers.len(),
        1,
        "must have exactly 1 server after unregister"
    );
    assert!(
        restored.servers.contains_key("keep-me"),
        "keep-me must survive unregister"
    );
    assert!(
        !restored.servers.contains_key("portal-bridge"),
        "portal-bridge must be removed after unregister"
    );

    // Verify the remaining server config is identical to the original.
    let original_keep = initial_config.servers.get("keep-me").unwrap();
    let restored_keep = restored.servers.get("keep-me").unwrap();
    assert_eq!(original_keep.command, restored_keep.command);
    assert_eq!(original_keep.args, restored_keep.args);

    // Verify runtime disconnect works: connect keep-me, add + remove
    // portal-bridge at runtime, confirm keep-me survives.
    let manager = McpManager::with_config(initial_config.clone());

    // Connect keep-me (succeeds because it's a working server).
    let keep_server_config = initial_config.servers.get("keep-me").unwrap();
    manager
        .connect("keep-me", keep_server_config)
        .await
        .expect("keep-me must connect");
    assert!(
        manager
            .connected_servers()
            .await
            .contains(&"keep-me".to_string()),
        "keep-me must be in connected set"
    );

    // Disconnect portal-bridge (simulates unregister at runtime).
    let _ = manager.disconnect("portal-bridge").await;

    // keep-me must still be connected.
    let connected = manager.connected_servers().await;
    assert!(
        connected.contains(&"keep-me".to_string()),
        "keep-me must remain connected after unregistering portal-bridge"
    );
    assert!(
        !connected.contains(&"portal-bridge".to_string()),
        "portal-bridge must not be connected after unregister"
    );

    manager.disconnect_all().await;
}
