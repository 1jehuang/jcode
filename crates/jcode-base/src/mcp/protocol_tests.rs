use super::*;

#[test]
fn load_for_dir_without_project_does_not_load_process_cwd_config() {
    let _guard = crate::storage::lock_test_env();
    let original_cwd = std::env::current_dir().expect("current cwd");
    let previous_home = std::env::var_os("JCODE_HOME");
    let home = tempfile::tempdir().expect("home tempdir");
    let project = tempfile::tempdir().expect("project tempdir");
    crate::env::set_var("JCODE_HOME", home.path());
    std::env::set_current_dir(project.path()).expect("set project cwd");
    std::fs::write(
        project.path().join(".mcp.json"),
        r#"{"mcpServers":{"cwd-only":{"command":"cwd-server"}}}"#,
    )
    .expect("write project MCP config");

    let result = std::panic::catch_unwind(|| {
        let unbound = McpConfig::load_for_dir(None);
        assert!(!unbound.servers.contains_key("cwd-only"));

        let bound = McpConfig::load_for_dir(Some(project.path()));
        assert!(bound.servers.contains_key("cwd-only"));
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
