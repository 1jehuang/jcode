use super::*;

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
