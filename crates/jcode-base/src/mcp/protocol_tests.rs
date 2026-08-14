use super::*;

#[test]
fn issue_790_load_uses_process_cwd_while_unbound_load_for_dir_does_not() {
    let _guard = crate::storage::lock_test_env();
    let original_cwd = std::env::current_dir().expect("current cwd");
    let previous_home = std::env::var_os("JCODE_HOME");
    let home = tempfile::tempdir().expect("home tempdir");
    let project = tempfile::tempdir().expect("project tempdir");
    let other_project = tempfile::tempdir().expect("other project tempdir");
    crate::env::set_var("JCODE_HOME", home.path());
    std::env::set_current_dir(project.path()).expect("set project cwd");
    std::fs::write(
        project.path().join(".mcp.json"),
        r#"{"mcpServers":{"cwd-only":{"command":"cwd-server"}}}"#,
    )
    .expect("write project MCP config");
    std::fs::write(
        other_project.path().join(".mcp.json"),
        r#"{"mcpServers":{"other-cwd-only":{"command":"other-cwd-server"}}}"#,
    )
    .expect("write other project MCP config");

    let result = std::panic::catch_unwind(|| {
        let unbound = McpConfig::load_for_dir(None);
        assert!(!unbound.servers.contains_key("cwd-only"));

        let default = McpConfig::load();
        assert!(default.servers.contains_key("cwd-only"));
        assert!(!default.servers.contains_key("other-cwd-only"));

        std::env::set_current_dir(other_project.path()).expect("set other project cwd");
        let other_default = McpConfig::load();
        assert!(!other_default.servers.contains_key("cwd-only"));
        assert!(other_default.servers.contains_key("other-cwd-only"));

        let bound = McpConfig::load_for_dir(Some(project.path()));
        assert!(bound.servers.contains_key("cwd-only"));
        assert!(!bound.servers.contains_key("other-cwd-only"));
    });

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    if let Some(previous_home) = previous_home {
        crate::env::set_var("JCODE_HOME", previous_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    result.expect("MCP cwd isolation assertions");
}

#[test]
fn test_json_rpc_request_serialization() {
    let request = JsonRpcRequest::new(1, "tools/list", None);
    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("\"jsonrpc\":\"2.0\""));
    assert!(json.contains("\"id\":1"));
    assert!(json.contains("\"method\":\"tools/list\""));
}

#[test]
fn test_json_rpc_notification_serialization_omits_id() {
    let notification = JsonRpcNotification::new("notifications/initialized", None);
    let value = serde_json::to_value(notification).unwrap();
    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["method"], "notifications/initialized");
    assert!(value.get("id").is_none());
    assert!(value.get("params").is_none());
}

#[test]
fn test_json_rpc_response_deserialization() {
    let json = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
    let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.id, Some(1));
    assert!(response.result.is_some());
    assert!(response.error.is_none());
}

#[test]
fn test_json_rpc_error_response() {
    let json = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid Request"}}"#;
    let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
    assert!(response.error.is_some());
    let err = response.error.unwrap();
    assert_eq!(err.code, -32600);
    assert_eq!(err.message, "Invalid Request");
}

#[test]
fn test_mcp_config_deserialization() {
    let json = r#"{
            "servers": {
                "test-server": {
                    "command": "/usr/bin/test-mcp",
                    "args": ["--port", "8080"],
                    "env": {"API_KEY": "secret"}
                }
            }
        }"#;
    let config: McpConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.servers.len(), 1);
    let server = config.servers.get("test-server").unwrap();
    assert_eq!(server.command, "/usr/bin/test-mcp");
    assert_eq!(server.args, vec!["--port", "8080"]);
    assert_eq!(server.env.get("API_KEY"), Some(&"secret".to_string()));
}

#[test]
fn test_mcp_config_empty() {
    let json = r#"{}"#;
    let config: McpConfig = serde_json::from_str(json).unwrap();
    assert!(config.servers.is_empty());
}

#[test]
fn test_mcp_config_accepts_claude_mcp_servers_key() {
    // Claude Code uses `mcpServers`, not `servers`.
    let json = r#"{
            "mcpServers": {
                "claude-server": {
                    "command": "npx",
                    "args": ["-y", "some-mcp"]
                }
            }
        }"#;
    let config: McpConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.servers.len(), 1);
    let server = config.servers.get("claude-server").unwrap();
    assert_eq!(server.command, "npx");
    assert!(server.is_stdio());
}

#[test]
fn test_mcp_http_server_is_not_stdio() {
    let json = r#"{
            "mcpServers": {
                "remote": {
                    "type": "http",
                    "url": "https://example.com/mcp"
                }
            }
        }"#;
    let config: McpConfig = serde_json::from_str(json).unwrap();
    let server = config.servers.get("remote").unwrap();
    assert!(!server.is_stdio());
    assert_eq!(server.url.as_deref(), Some("https://example.com/mcp"));
}

#[test]
fn environment_expansion_matches_claude_syntax_across_config_fields() {
    let json = r#"{
        "mcpServers": {
            "probe": {
                "command": "${BIN_DIR}/probe",
                "args": ["--token=${TOKEN:-fallback-token}", "${MISSING_DEFAULT:-fallback}", "${MISSING}"],
                "env": {
                    "PROBE_PATH": "${BIN_DIR}/marker",
                    "STILL_MISSING": "prefix-${MISSING}"
                },
                "type": "http",
                "url": "${BASE_URL:-https://example.test}/mcp",
                "headers": {"Authorization": "Bearer ${TOKEN}"}
            }
        }
    }"#;
    let mut config: McpConfig = serde_json::from_str(json).unwrap();

    let warnings = config.expand_environment_variables_with(|variable| match variable {
        "BIN_DIR" => Some("/opt/tools".to_string()),
        "TOKEN" => Some("secret-token".to_string()),
        _ => None,
    });

    let server = config.servers.get("probe").unwrap();
    assert_eq!(server.command, "/opt/tools/probe");
    assert_eq!(
        server.args,
        vec!["--token=secret-token", "fallback", "${MISSING}"]
    );
    assert_eq!(server.env["PROBE_PATH"], "/opt/tools/marker");
    assert_eq!(server.env["STILL_MISSING"], "prefix-${MISSING}");
    assert_eq!(server.url.as_deref(), Some("https://example.test/mcp"));
    assert_eq!(server.headers["Authorization"], "Bearer secret-token");
    assert_eq!(
        warnings,
        vec![UnresolvedEnvironmentVariable {
            server: "probe".to_string(),
            variable: "MISSING".to_string(),
        }],
        "an unresolved variable is preserved and warned once per server"
    );
}

#[test]
fn environment_expansion_preserves_malformed_and_unclosed_expressions() {
    let mut unresolved = std::collections::BTreeSet::new();
    let expanded = expand_environment_string(
        "${1INVALID} ${:-no} ${UNCLOSED",
        &|_| Some("unexpected".to_string()),
        &mut unresolved,
    );

    assert_eq!(expanded, "${1INVALID} ${:-no} ${UNCLOSED");
    assert!(unresolved.is_empty());
}

#[test]
fn expansion_after_merge_ignores_shadowed_references() {
    let mut merged = McpConfig::default();
    merged.servers.insert(
        "same-name".to_string(),
        serde_json::from_value(serde_json::json!({"command": "${SHADOWED}"})).unwrap(),
    );
    let incoming = serde_json::from_value(serde_json::json!({
        "same-name": {"command": "${WINNER}"}
    }))
    .unwrap();
    McpConfig::merge_servers_preferring_runnable(&mut merged.servers, incoming);

    let warnings = merged.expand_environment_variables_with(|variable| {
        (variable == "WINNER").then(|| "winning-command".to_string())
    });

    assert_eq!(merged.servers["same-name"].command, "winning-command");
    assert!(warnings.is_empty(), "shadowed definitions must not warn");
}

#[test]
fn load_for_dir_expands_the_winning_merged_definition() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let previous_value = std::env::var_os("JCODE_MCP_EXPANSION_TEST_VALUE");
    let home = tempfile::tempdir().expect("home tempdir");
    let project = tempfile::tempdir().expect("project tempdir");
    crate::env::set_var("JCODE_HOME", home.path());
    crate::env::set_var("JCODE_MCP_EXPANSION_TEST_VALUE", "expanded-value");

    std::fs::write(
        home.path().join("mcp.json"),
        r#"{"mcpServers":{"same-name":{"command":"${SHADOWED_MISSING}"}}}"#,
    )
    .unwrap();
    std::fs::write(
        project.path().join(".mcp.json"),
        r#"{"mcpServers":{"same-name":{"command":"project-bin","args":["${JCODE_MCP_EXPANSION_TEST_VALUE}"]}}}"#,
    )
    .unwrap();

    let result = std::panic::catch_unwind(|| {
        let config = McpConfig::load_for_dir(Some(project.path()));
        let server = &config.servers["same-name"];
        assert_eq!(server.command, "project-bin");
        assert_eq!(server.args, ["expanded-value"]);
    });

    match previous_home {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    match previous_value {
        Some(value) => crate::env::set_var("JCODE_MCP_EXPANSION_TEST_VALUE", value),
        None => crate::env::remove_var("JCODE_MCP_EXPANSION_TEST_VALUE"),
    }
    result.expect("merged config expansion assertions");
}

#[test]
fn expanded_values_invalidate_schema_cache_fingerprint() {
    let raw = r#"{"mcpServers":{"srv":{"command":"node","args":["${SCRIPT}"],"env":{"TOKEN":"${TOKEN}"}}}}"#;
    let mut first: McpConfig = serde_json::from_str(raw).unwrap();
    first.expand_environment_variables_with(|variable| match variable {
        "SCRIPT" => Some("first.js".to_string()),
        "TOKEN" => Some("token-one".to_string()),
        _ => None,
    });
    let mut second: McpConfig = serde_json::from_str(raw).unwrap();
    second.expand_environment_variables_with(|variable| match variable {
        "SCRIPT" => Some("second.js".to_string()),
        "TOKEN" => Some("token-two".to_string()),
        _ => None,
    });

    assert_ne!(
        crate::mcp::schema_cache::fingerprint_config(&first.servers["srv"]),
        crate::mcp::schema_cache::fingerprint_config(&second.servers["srv"]),
        "changing expanded environment values must invalidate cached schemas"
    );
}

#[test]
fn test_load_claude_json_global_and_project_servers() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path().join("myproject");
    std::fs::create_dir_all(&cwd).unwrap();
    let claude_json = temp.path().join(".claude.json");

    let body = serde_json::json!({
        "mcpServers": {
            "global-srv": { "command": "global-bin" }
        },
        "projects": {
            cwd.to_string_lossy(): {
                "mcpServers": {
                    "project-srv": { "command": "project-bin", "args": ["--flag"] }
                }
            }
        }
    });
    std::fs::write(&claude_json, serde_json::to_string_pretty(&body).unwrap()).unwrap();

    let config = McpConfig::load_claude_json(&claude_json, Some(&cwd));
    assert_eq!(config.servers.len(), 2);
    assert_eq!(
        config.servers.get("global-srv").unwrap().command,
        "global-bin"
    );
    assert_eq!(
        config.servers.get("project-srv").unwrap().command,
        "project-bin"
    );
}

#[test]
fn test_load_project_locals_resolves_against_given_dir_not_cwd() {
    // Issue #420: remote/client sessions must load project-local MCP config
    // from the session working directory, not the server process cwd.
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("client-project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join(".mcp.json"),
        r#"{"mcpServers":{"example-server":{"command":"npx","args":["-y","some-mcp-server"]}}}"#,
    )
    .unwrap();

    // The process cwd (this repo) is unrelated to `project`; resolution must
    // come from the explicit path.
    let config = McpConfig::load_project_locals(&project);
    assert_eq!(config.servers.len(), 1);
    let server = config.servers.get("example-server").unwrap();
    assert_eq!(server.command, "npx");
    assert!(server.is_stdio());

    // A directory with no project-local config yields nothing.
    let empty = temp.path().join("empty-project");
    std::fs::create_dir_all(&empty).unwrap();
    assert!(McpConfig::load_project_locals(&empty).servers.is_empty());
}

#[test]
fn test_load_project_locals_merge_order() {
    // `.jcode/mcp.json` loads first, then `.mcp.json` overrides same-named
    // servers, then `.claude/mcp.json`.
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path();
    std::fs::create_dir_all(project.join(".jcode")).unwrap();
    std::fs::create_dir_all(project.join(".claude")).unwrap();
    std::fs::write(
        project.join(".jcode/mcp.json"),
        r#"{"servers":{"shared-name":{"command":"jcode-bin"},"jcode-only":{"command":"a"}}}"#,
    )
    .unwrap();
    std::fs::write(
        project.join(".mcp.json"),
        r#"{"mcpServers":{"shared-name":{"command":"claude-bin"}}}"#,
    )
    .unwrap();
    std::fs::write(
        project.join(".claude/mcp.json"),
        r#"{"mcpServers":{"legacy-only":{"command":"c"}}}"#,
    )
    .unwrap();

    let config = McpConfig::load_project_locals(project);
    assert_eq!(config.servers.len(), 3);
    assert_eq!(
        config.servers.get("shared-name").unwrap().command,
        "claude-bin",
        ".mcp.json must override .jcode/mcp.json for same-named servers"
    );
    assert!(config.servers.contains_key("jcode-only"));
    assert!(config.servers.contains_key("legacy-only"));
}

#[test]
fn test_server_enabled_defaults_true() {
    // Existing configs without the flag keep current behavior (issue #436).
    let json = r#"{"servers":{"srv":{"command":"bin"}}}"#;
    let config: McpConfig = serde_json::from_str(json).unwrap();
    assert!(config.servers.get("srv").unwrap().is_enabled());
}

#[test]
fn test_server_enabled_false_opencode_style() {
    let json = r#"{"servers":{"srv":{"command":"bin","enabled":false}}}"#;
    let config: McpConfig = serde_json::from_str(json).unwrap();
    assert!(!config.servers.get("srv").unwrap().is_enabled());
}

#[test]
fn test_server_disabled_true_claude_style() {
    let json = r#"{"mcpServers":{"srv":{"command":"bin","disabled":true}}}"#;
    let config: McpConfig = serde_json::from_str(json).unwrap();
    assert!(!config.servers.get("srv").unwrap().is_enabled());
}

#[test]
fn test_server_disabled_wins_over_enabled() {
    // `disabled` (Claude Code style) wins when both spellings are present.
    let json = r#"{"servers":{"srv":{"command":"bin","enabled":true,"disabled":true}}}"#;
    let config: McpConfig = serde_json::from_str(json).unwrap();
    assert!(!config.servers.get("srv").unwrap().is_enabled());

    let json = r#"{"servers":{"srv":{"command":"bin","enabled":false,"disabled":false}}}"#;
    let config: McpConfig = serde_json::from_str(json).unwrap();
    assert!(config.servers.get("srv").unwrap().is_enabled());
}

#[test]
fn test_disabled_server_survives_save_roundtrip() {
    // Disabled servers must stay in config (kept, not spawned), including
    // through a save/load cycle.
    let json = r#"{"servers":{"off":{"command":"bin","enabled":false}}}"#;
    let config: McpConfig = serde_json::from_str(json).unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("mcp.json");
    config.save_to_file(&path).unwrap();
    let reloaded = McpConfig::load_from_file(&path).unwrap();
    assert!(!reloaded.servers.get("off").unwrap().is_enabled());
}

#[test]
fn test_tool_def_deserialization() {
    let json = r#"{
            "name": "read_file",
            "description": "Read a file from disk",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }
        }"#;
    let tool: McpToolDef = serde_json::from_str(json).unwrap();
    assert_eq!(tool.name, "read_file");
    assert_eq!(tool.description, Some("Read a file from disk".to_string()));
}

#[test]
fn test_tool_call_result_text() {
    let json = r#"{
            "content": [{"type": "text", "text": "File contents here"}],
            "isError": false
        }"#;
    let result: ToolCallResult = serde_json::from_str(json).unwrap();
    assert!(!result.is_error);
    assert_eq!(result.content.len(), 1);
    match &result.content[0] {
        ContentBlock::Text { text, .. } => assert_eq!(text, "File contents here"),
        _ => panic!("Expected text block"),
    }
}

#[test]
fn test_tool_call_result_error() {
    let json = r#"{
            "content": [{"type": "text", "text": "File not found"}],
            "isError": true
        }"#;
    let result: ToolCallResult = serde_json::from_str(json).unwrap();
    assert!(result.is_error);
}

#[test]
fn test_initialize_result() {
    let json = r#"{
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {"listChanged": true}
            },
            "serverInfo": {
                "name": "test-server",
                "version": "1.0.0"
            }
        }"#;
    let result: InitializeResult = serde_json::from_str(json).unwrap();
    assert_eq!(result.protocol_version, "2024-11-05");
    assert!(result.server_info.is_some());
}

#[test]
fn http_entry_does_not_displace_a_working_stdio_server_of_the_same_name() {
    // A `type: http` entry from a lower-precedence config used to overwrite the
    // stdio definition and then get dropped by the non-stdio filter, silently
    // losing a working server (issue #653).
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path();
    std::fs::create_dir_all(project.join(".jcode")).unwrap();
    std::fs::write(
        project.join(".jcode/mcp.json"),
        r#"{"servers":{"github":{"type":"stdio","command":"npx","args":["-y","mcp-remote"]}}}"#,
    )
    .unwrap();
    std::fs::write(
        project.join(".mcp.json"),
        r#"{"mcpServers":{"github":{"type":"http","url":"https://api.githubcopilot.com/mcp/"}}}"#,
    )
    .unwrap();

    let config = McpConfig::load_project_locals(project);
    let github = config
        .servers
        .get("github")
        .expect("stdio github server must survive the http entry");
    assert_eq!(github.command, "npx");
    assert!(github.is_stdio());
}

#[test]
fn stdio_entry_still_overrides_an_existing_http_entry() {
    // The precedence guard is one-directional: a runnable stdio definition must
    // still win over a non-runnable http one from an earlier config.
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path();
    std::fs::create_dir_all(project.join(".jcode")).unwrap();
    std::fs::write(
        project.join(".jcode/mcp.json"),
        r#"{"servers":{"github":{"type":"http","url":"https://example.invalid/mcp/"}}}"#,
    )
    .unwrap();
    std::fs::write(
        project.join(".mcp.json"),
        r#"{"mcpServers":{"github":{"type":"stdio","command":"npx"}}}"#,
    )
    .unwrap();

    let config = McpConfig::load_project_locals(project);
    assert_eq!(config.servers.get("github").unwrap().command, "npx");
}

#[test]
fn stdio_entry_of_same_transport_still_overrides_by_precedence() {
    // Same-transport collisions keep the existing last-writer-wins behavior.
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path();
    std::fs::create_dir_all(project.join(".jcode")).unwrap();
    std::fs::write(
        project.join(".jcode/mcp.json"),
        r#"{"servers":{"github":{"command":"old-bin"}}}"#,
    )
    .unwrap();
    std::fs::write(
        project.join(".mcp.json"),
        r#"{"mcpServers":{"github":{"command":"new-bin"}}}"#,
    )
    .unwrap();

    let config = McpConfig::load_project_locals(project);
    assert_eq!(config.servers.get("github").unwrap().command, "new-bin");
}

#[test]
fn claude_json_http_entry_does_not_displace_jcode_stdio_server() {
    // The exact configuration from issue #653: `github` is stdio in
    // ~/.jcode/mcp.json and http in ~/.claude.json. The http entry used to win
    // the merge and then be dropped by the non-stdio filter, so a working
    // server vanished with no indication it had been overwritten.
    let _guard = crate::storage::lock_test_env();
    let original_cwd = std::env::current_dir().expect("current cwd");
    let previous_home = std::env::var_os("JCODE_HOME");
    let home = tempfile::tempdir().expect("home tempdir");
    let project = tempfile::tempdir().expect("project tempdir");
    crate::env::set_var("JCODE_HOME", home.path());
    std::env::set_current_dir(project.path()).expect("set project cwd");

    std::fs::write(
        home.path().join("mcp.json"),
        r#"{"mcpServers":{"github":{"type":"stdio","command":"npx","args":["-y","mcp-remote"]}}}"#,
    )
    .expect("write jcode mcp config");
    // `user_home_path()` maps external configs under JCODE_HOME to an
    // `external/` subdirectory, so this is where ~/.claude.json is read from.
    let external = home.path().join("external");
    std::fs::create_dir_all(&external).expect("create external dir");
    std::fs::write(
        external.join(".claude.json"),
        r#"{"mcpServers":{"github":{"type":"http","url":"https://api.githubcopilot.com/mcp/"}}}"#,
    )
    .expect("write claude config");

    let result = std::panic::catch_unwind(|| {
        let merged = McpConfig::load_for_dir(Some(project.path()));
        let github = merged
            .servers
            .get("github")
            .expect("the stdio github server must survive the http entry");
        assert_eq!(github.command, "npx");
        assert!(github.is_stdio());
    });

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    if let Some(previous_home) = previous_home {
        crate::env::set_var("JCODE_HOME", previous_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    result.expect("issue #653 merge assertions");
}

#[test]
fn claude_is_live_while_codex_is_a_one_time_snapshot() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let home = tempfile::tempdir().expect("home tempdir");
    crate::env::set_var("JCODE_HOME", home.path());

    let external = home.path().join("external");
    let codex_dir = external.join(".codex");
    std::fs::create_dir_all(&codex_dir).expect("create external config dirs");
    let claude_path = external.join(".claude.json");
    let codex_path = codex_dir.join("config.toml");

    std::fs::write(
        &claude_path,
        r#"{"mcpServers":{"alpha":{"command":"claude-alpha","args":["--first"],"env":{"TOKEN":"claude-inline-secret"}},"beta":{"command":"claude-beta"}}}"#,
    )
    .expect("write Claude config");
    std::fs::write(
        &codex_path,
        r#"[mcp_servers.codex_only]
command = "codex-bin"
args = ["--snapshot"]
env = { TOKEN = "codex-inline-secret" }
"#,
    )
    .expect("write Codex config");

    let result = std::panic::catch_unwind(|| {
        let first = McpConfig::load_for_dir(None);
        assert!(first.servers.contains_key("alpha"));
        assert!(first.servers.contains_key("beta"));
        assert!(first.servers.contains_key("codex_only"));

        let snapshot_path = home.path().join("mcp.json");
        let snapshot = std::fs::read_to_string(&snapshot_path).expect("Codex snapshot");
        assert!(snapshot.contains("codex_only"));
        assert!(snapshot.contains("codex-inline-secret"));
        assert!(!snapshot.contains("alpha"));
        assert!(!snapshot.contains("beta"));
        assert!(!snapshot.contains("claude-inline-secret"));

        std::fs::write(
            &claude_path,
            r#"{"mcpServers":{"alpha":{"command":"claude-alpha","args":["--edited"]}}}"#,
        )
        .expect("edit Claude config");
        std::fs::remove_file(&codex_path).expect("remove Codex source");

        let second = McpConfig::load_for_dir(None);
        assert_eq!(
            second.servers.get("alpha").expect("live alpha").args,
            vec!["--edited"]
        );
        assert!(!second.servers.contains_key("beta"));
        assert!(
            second.servers.contains_key("codex_only"),
            "the one-time Codex snapshot remains authoritative after import"
        );
    });

    if let Some(previous_home) = previous_home {
        crate::env::set_var("JCODE_HOME", previous_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    result.expect("live Claude and snapshot Codex assertions");
}

#[test]
fn claude_only_config_never_creates_a_jcode_snapshot() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let home = tempfile::tempdir().expect("home tempdir");
    crate::env::set_var("JCODE_HOME", home.path());

    let external = home.path().join("external");
    std::fs::create_dir_all(&external).expect("create external config dir");
    std::fs::write(
        external.join(".claude.json"),
        r#"{"mcpServers":{"private":{"command":"claude-bin","env":{"TOKEN":"must-not-be-copied"}}}}"#,
    )
    .expect("write Claude config");

    let result = std::panic::catch_unwind(|| {
        let config = McpConfig::load_for_dir(None);
        assert!(config.servers.contains_key("private"));
        assert!(
            !home.path().join("mcp.json").exists(),
            "a live Claude source must not be persisted into jcode config"
        );
    });

    if let Some(previous_home) = previous_home {
        crate::env::set_var("JCODE_HOME", previous_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    result.expect("Claude no-snapshot assertions");
}

#[test]
fn legacy_claude_config_is_live_and_deletions_do_not_leave_a_snapshot() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let home = tempfile::tempdir().expect("home tempdir");
    crate::env::set_var("JCODE_HOME", home.path());

    let legacy_dir = home.path().join("external/.claude");
    std::fs::create_dir_all(&legacy_dir).expect("create legacy Claude config dir");
    let legacy_path = legacy_dir.join("mcp.json");
    std::fs::write(
        &legacy_path,
        r#"{"mcpServers":{"alpha":{"command":"legacy-alpha"},"beta":{"command":"legacy-beta"}}}"#,
    )
    .expect("write legacy Claude config");

    let result = std::panic::catch_unwind(|| {
        let first = McpConfig::load_for_dir(None);
        assert!(first.servers.contains_key("alpha"));
        assert!(first.servers.contains_key("beta"));
        assert!(!home.path().join("mcp.json").exists());

        std::fs::write(
            &legacy_path,
            r#"{"mcpServers":{"alpha":{"command":"legacy-alpha-edited"}}}"#,
        )
        .expect("edit legacy Claude config");

        let second = McpConfig::load_for_dir(None);
        assert_eq!(second.servers["alpha"].command, "legacy-alpha-edited");
        assert!(
            !second.servers.contains_key("beta"),
            "deleting a server from the live legacy source must remove it"
        );
        assert!(
            !home.path().join("mcp.json").exists(),
            "legacy Claude values must never be snapshotted"
        );
    });

    if let Some(previous_home) = previous_home {
        crate::env::set_var("JCODE_HOME", previous_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    result.expect("legacy Claude live-source assertions");
}

#[test]
fn disabling_claude_mcp_skips_both_live_sources_but_preserves_jcode_sources() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let previous_disable = std::env::var_os("JCODE_DISABLE_CLAUDE_MCP");
    let home = tempfile::tempdir().expect("home tempdir");
    let project = tempfile::tempdir().expect("project tempdir");
    crate::env::set_var("JCODE_HOME", home.path());
    crate::env::set_var("JCODE_DISABLE_CLAUDE_MCP", "1");

    std::fs::write(
        home.path().join("mcp.json"),
        r#"{"mcpServers":{"jcode-global":{"command":"jcode-global"}}}"#,
    )
    .expect("write jcode global config");
    std::fs::create_dir_all(project.path().join(".jcode")).expect("create jcode project dir");
    std::fs::write(
        project.path().join(".jcode/mcp.json"),
        r#"{"mcpServers":{"jcode-project":{"command":"jcode-project"}}}"#,
    )
    .expect("write jcode project config");

    let external = home.path().join("external");
    std::fs::create_dir_all(external.join(".claude")).expect("create Claude config dirs");
    std::fs::write(
        external.join(".claude.json"),
        r#"{"mcpServers":{"claude-current":{"command":"claude-current"}}}"#,
    )
    .expect("write current Claude config");
    std::fs::write(
        external.join(".claude/mcp.json"),
        r#"{"mcpServers":{"claude-legacy":{"command":"claude-legacy"}}}"#,
    )
    .expect("write legacy Claude config");

    let result = std::panic::catch_unwind(|| {
        let config = McpConfig::load_for_dir(Some(project.path()));
        assert!(config.servers.contains_key("jcode-global"));
        assert!(config.servers.contains_key("jcode-project"));
        assert!(!config.servers.contains_key("claude-current"));
        assert!(!config.servers.contains_key("claude-legacy"));
    });

    if let Some(previous_home) = previous_home {
        crate::env::set_var("JCODE_HOME", previous_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    if let Some(previous_disable) = previous_disable {
        crate::env::set_var("JCODE_DISABLE_CLAUDE_MCP", previous_disable);
    } else {
        crate::env::remove_var("JCODE_DISABLE_CLAUDE_MCP");
    }
    result.expect("Claude MCP opt-out assertions");
}

#[test]
fn mcp_source_logs_explain_provenance_without_config_values() {
    let live = McpConfig::live_claude_log_message(2, "~/.claude.json");
    assert!(live.contains("Loaded 2 server(s) live from Claude Code (~/.claude.json)"));
    assert!(live.contains("source values were not copied"));

    let imported =
        McpConfig::codex_import_log_message(1, 2, std::path::Path::new("/sandbox/.jcode/mcp.json"));
    assert!(imported.contains("One-time imported 1 server(s) from Codex CLI"));
    assert!(imported.contains("/sandbox/.jcode/mcp.json"));
    assert!(imported.contains("2 configured environment value(s)"));
    assert!(imported.contains("may contain secrets"));
    assert!(imported.contains("Claude Code MCP configuration remains live and was not copied"));
    assert!(!imported.contains("TOKEN"));
    assert!(!imported.contains("inline-secret"));
}
