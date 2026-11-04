#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn empty_config() -> McpConfig {
        McpConfig::default()
    }

    #[test]
    fn issue_790_owned_manager_keeps_its_initial_config_directory() {
        let _guard = crate::storage::lock_test_env();
        let original_cwd = std::env::current_dir().expect("current cwd");
        let previous_home = std::env::var_os("JCODE_HOME");
        let home = tempfile::tempdir().expect("home tempdir");
        let project = tempfile::tempdir().expect("project tempdir");
        let elsewhere = tempfile::tempdir().expect("elsewhere tempdir");
        crate::env::set_var("JCODE_HOME", home.path());
        std::fs::write(
            project.path().join(".mcp.json"),
            r#"{"mcpServers":{"project-only":{"command":"project-server"}}}"#,
        )
        .expect("write project config");

        std::env::set_current_dir(project.path()).expect("set project cwd");
        let manager = McpManager::new();
        std::env::set_current_dir(elsewhere.path()).expect("set different cwd");
        let fresh = manager.load_fresh_config();

        std::env::set_current_dir(original_cwd).expect("restore cwd");
        if let Some(previous_home) = previous_home {
            crate::env::set_var("JCODE_HOME", previous_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }

        assert!(fresh.servers.contains_key("project-only"));
    }

    #[tokio::test]
    async fn call_tool_unconfigured_server_bails_cleanly() {
        let manager = McpManager::with_config(empty_config());
        let err = manager
            .call_tool("ghost", "do_thing", serde_json::json!({}))
            .await
            .expect_err("calling an unknown server must error");
        assert!(
            err.to_string().contains("ghost"),
            "error should name the missing server: {err}"
        );
    }

    #[tokio::test]
    async fn ensure_server_connected_unconfigured_errors() {
        let manager = McpManager::with_config(empty_config());
        let err = manager
            .ensure_server_connected("ghost", Duration::from_millis(50))
            .await
            .expect_err("ensuring an unconfigured server must error");
        assert!(err.to_string().contains("not configured"), "{err}");
    }

    #[tokio::test]
    async fn connect_all_skips_disabled_servers() {
        // Issue #436: disabled servers stay in config but are never
        // auto-spawned. This config's command would fail to connect (and thus
        // produce a failure entry) if it were attempted at all.
        let mut config = McpConfig::default();
        config.servers.insert(
            "off".to_string(),
            McpServerConfig {
                command: "true".to_string(),
                args: vec![],
                env: HashMap::new(),
                shared: false,
                transport: None,
                url: None,
                headers: std::collections::HashMap::new(),
                enabled: Some(false),
                disabled: None,
                timeout_secs: None,
            },
        );
        let manager = McpManager::with_config(config);
        let (successes, failures) = manager.connect_all().await.expect("connect_all");
        assert_eq!(successes, 0, "disabled server must not be spawned");
        assert!(
            failures.is_empty(),
            "disabled server must not be attempted: {failures:?}"
        );
        assert!(manager.connected_servers().await.is_empty());
        // Still present in config so it can be connected on demand by name.
        assert!(manager.config().servers.contains_key("off"));
    }

    #[tokio::test]
    async fn connect_on_first_call_fails_cleanly_for_broken_server() {
        // A configured server whose command exits immediately and never speaks
        // MCP. connect-on-first-call must surface a clean, bounded tool error
        // (connection failure) rather than hanging or panicking.
        let mut config = McpConfig::default();
        config.servers.insert(
            "broken".to_string(),
            McpServerConfig {
                // `true` exits 0 immediately: the stdio handshake gets EOF, so
                // connect fails fast instead of waiting on the initialize bound.
                command: "true".to_string(),
                args: vec![],
                env: HashMap::new(),
                shared: false,
                transport: None,
                url: None,
                headers: std::collections::HashMap::new(),
                enabled: None,
                disabled: None,
                timeout_secs: None,
            },
        );
        let manager = McpManager::with_config(config);

        let started = std::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(40),
            manager.call_tool("broken", "anything", serde_json::json!({})),
        )
        .await;
        let inner = result.expect("call_tool must return, not hang");
        assert!(inner.is_err(), "broken server must yield a tool error");
        let msg = inner.unwrap_err().to_string();
        assert!(
            msg.contains("broken"),
            "tool error should name the server: {msg}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(35),
            "connect-on-first-call must be bounded"
        );
    }

    #[tokio::test]
    async fn cancelled_owned_connection_notifies_waiters_and_clears_entry() {
        let manager = McpManager::with_config(empty_config());
        let (sender, _receiver) = watch::channel(None);
        manager
            .connecting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert("cancelled".to_string(), sender.clone());
        let mut waiter = sender.subscribe();

        drop(OwnedConnectionAttemptGuard::new(
            Arc::clone(&manager.connecting),
            "cancelled",
            sender,
        ));

        tokio::time::timeout(Duration::from_secs(1), waiter.changed())
            .await
            .expect("cancellation cleanup must notify waiters")
            .expect("cancellation sender must remain alive");
        assert_eq!(
            waiter
                .borrow()
                .as_ref()
                .and_then(|result| result.as_ref().err())
                .map(String::as_str),
            Some("MCP server 'cancelled' connection attempt was cancelled")
        );
        assert!(
            !manager
                .connecting
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key("cancelled")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    // `disconnect_all` flushes process-global sponsor provenance counters, so
    // hold the test-env lock to avoid draining another test's pending reports.
    #[allow(clippy::await_holding_lock)]
    async fn concurrent_owned_connects_spawn_only_one_server() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Arc;

        let _env_lock = crate::storage::lock_test_env();

        let temp = tempfile::tempdir().expect("tempdir");
        let script = temp.path().join("counted-mcp.sh");
        let count = temp.path().join("starts");
        std::fs::write(
            &script,
            r##"#!/bin/sh
printf 'started\n' >> "$MCP_START_COUNT"
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*)
      echo '{"jsonrpc":"2.0","id":'$id',"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"counted","version":"0.0.1"}}}'
      ;;
    *'"tools/list"'*)
      echo '{"jsonrpc":"2.0","id":'$id',"result":{"tools":[]}}'
      ;;
    *'"shutdown"'*)
      exit 0
      ;;
  esac
done
"##,
        )
        .expect("write MCP script");
        let mut permissions = std::fs::metadata(&script)
            .expect("script metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("make script executable");

        let mut config = McpConfig::default();
        let mut env = HashMap::new();
        env.insert("MCP_START_COUNT".to_string(), count.display().to_string());
        config.servers.insert(
            "counted".to_string(),
            McpServerConfig {
                command: script.display().to_string(),
                args: Vec::new(),
                env,
                shared: false,
                transport: None,
                url: None,
                headers: HashMap::new(),
                enabled: None,
                disabled: None,
                timeout_secs: Some(10),
            },
        );
        let config = config
            .servers
            .get("counted")
            .cloned()
            .expect("server config");
        let manager = Arc::new(McpManager::with_config({
            let mut cfg = McpConfig::default();
            cfg.servers.insert("counted".to_string(), config.clone());
            cfg
        }));

        let background = {
            let manager = Arc::clone(&manager);
            tokio::spawn(async move { manager.connect_all().await })
        };
        let manual = {
            let manager = Arc::clone(&manager);
            tokio::spawn(async move { manager.connect("counted", &config).await })
        };

        let (_, failures) = background
            .await
            .expect("background task join")
            .expect("background connect_all");
        assert!(failures.is_empty(), "background failures: {failures:?}");
        manual
            .await
            .expect("manual task join")
            .expect("manual connect");

        let starts = std::fs::read_to_string(&count).expect("read start count");
        assert_eq!(starts.lines().count(), 1, "server starts: {starts:?}");
        manager.disconnect_all().await.unwrap();
    }
}

#[cfg(all(test, unix))]
mod provenance_integration_tests {
    use super::*;
    use std::io::Write;

    /// Write a minimal stdio MCP server as a shell script: answers
    /// initialize, tools/list, and tools/call with canned JSON-RPC replies.
    fn write_fake_mcp_server(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("fake-mcp-server.sh");
        let script = r##"#!/bin/bash
while IFS= read -r line; do
  id=$(echo "$line" | grep -o '"id":[0-9]*' | grep -o '[0-9]*' | head -1)
  case "$line" in
    *'"initialize"'*)
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"0.0.1"}}}'
      ;;
    *'"tools/list"'*)
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"tools":[{"name":"create_card","description":"fake card","inputSchema":{"type":"object"}}]}}'
      ;;
    *'"tools/call"'*)
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"content":[{"type":"text","text":"card created"}],"isError":false}}'
      ;;
    *'"shutdown"'*)
      exit 0
      ;;
  esac
done
"##;
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(script.as_bytes()).unwrap();
        drop(file);
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    /// Full loop: discovery records a setup, connecting a matching server
    /// tags provenance and counts the connect, real MCP tool calls through
    /// the manager are metered, non-matching servers are not.
    #[tokio::test]
    // The test-env mutex must intentionally stay held across awaits so the
    // JCODE_HOME/config mutation stays serialized for the whole test.
    #[allow(clippy::await_holding_lock)]
    async fn discovery_provenance_end_to_end_with_real_mcp_server() {
        let env_guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());
        std::fs::write(
            temp.path().join("config.toml"),
            "[sponsors]\nenabled = true\n",
        )
        .unwrap();
        crate::config::Config::invalidate_cache();
        crate::sponsors::provenance::reset_for_tests();

        let server_path = write_fake_mcp_server(temp.path());
        let command = server_path.to_string_lossy().to_string();

        // 1. Discovery listing recorded this setup.
        crate::sponsors::provenance::record_discovered_setups(vec![
            crate::sponsors::provenance::DiscoveredSetup {
                sponsor: "agentcard".into(),
                command: command.clone(),
                args: vec![],
            },
        ]);

        // 2. Agent connects the matching server through the real manager.
        let mut config = McpConfig::default();
        config.servers.insert(
            "agentcard".to_string(),
            McpServerConfig {
                command: command.clone(),
                args: vec![],
                env: HashMap::new(),
                shared: false,
                transport: None,
                url: None,
                headers: std::collections::HashMap::new(),
                enabled: None,
                disabled: None,
                timeout_secs: None,
            },
        );
        let manager = McpManager::with_config(config.clone());
        let server_config = config.servers.get("agentcard").unwrap().clone();
        manager
            .connect("agentcard", &server_config)
            .await
            .expect("fake MCP server must connect");
        assert!(crate::sponsors::provenance::is_tagged("agentcard"));

        // 3. Real tool calls through the manager are metered.
        let result = manager
            .call_tool("agentcard", "create_card", serde_json::json!({}))
            .await
            .expect("tool call through fake server");
        assert!(!result.is_error);

        // 4. A second, non-discovered server with a different command is
        // never tagged.
        assert!(!crate::sponsors::provenance::is_tagged("other"));

        // 5. Pending aggregates hold exactly the connect + the call.
        let reports = crate::sponsors::provenance::drain_pending_for_tests();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].sponsor, "agentcard");
        assert_eq!(reports[0].connects, 1);
        assert_eq!(reports[0].calls, 1);
        assert_eq!(reports[0].errors, 0);

        manager.disconnect_all().await.unwrap();
        drop(env_guard);
    }
}
