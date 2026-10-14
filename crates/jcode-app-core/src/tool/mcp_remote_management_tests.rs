use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn context() -> ToolContext {
    ToolContext {
        session_id: "mcp-remote-management-test".into(),
        message_id: "test".into(),
        tool_call_id: "test".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    }
}

fn tool(config: crate::mcp::McpConfig) -> McpManagementTool {
    McpManagementTool::new(Arc::new(RwLock::new(McpManager::with_config(config))))
}

#[test]
fn remote_management_schema_exposes_remote_fields_but_not_interactive_login() {
    let schema = tool(Default::default()).parameters_schema();
    for field in ["url", "headers", "transport"] {
        assert!(schema["properties"][field].is_object());
    }
    let actions = schema["properties"]["action"]["enum"].as_array().unwrap();
    assert!(actions.contains(&json!("status")));
    assert!(actions.contains(&json!("logout")));
    assert!(!actions.contains(&json!("login")));
}

#[test]
fn remote_management_configuration_preserves_transport_and_static_headers() {
    for transport in [None, Some("http"), Some("streamable-http"), Some("sse")] {
        let mut input = json!({"action": "connect", "server": "remote", "url": "https://example.com/mcp", "headers": {"aUtHoRiZaTiOn": "Bearer static", "X-Test": "value"}});
        if let Some(transport) = transport {
            input["transport"] = json!(transport);
        }
        let config = connection_config(&serde_json::from_value(input).unwrap(), None).unwrap();
        assert!(config.is_remote());
        assert_eq!(config.headers["aUtHoRiZaTiOn"], "Bearer static");
        assert_eq!(config.headers["X-Test"], "value");
        assert_eq!(config.transport.as_deref(), transport);
    }
    let configured: McpServerConfig = serde_json::from_value(json!({"url": "https://example.com/mcp", "type": "sse", "enabled": false, "timeout_secs": 99, "headers": {"Authorization": "Bearer configured"}})).unwrap();
    let input = serde_json::from_value(json!({"action": "connect", "server": "remote"})).unwrap();
    let selected = connection_config(&input, Some(configured)).unwrap();
    assert_eq!(selected.timeout_secs, Some(99));
    assert!(!selected.is_enabled());
    assert_eq!(selected.headers["Authorization"], "Bearer configured");
}

#[test]
fn remote_management_rejects_ambiguous_or_ignored_connection_options() {
    for input in [
        json!({"command": "test", "url": "https://example.com/mcp"}),
        json!({"command": ""}),
        json!({"command": "test", "headers": {}}),
        json!({"command": "test", "transport": "sse"}),
        json!({"url": "https://example.com/mcp", "transport": "stdio"}),
        json!({"url": "https://example.com/mcp", "env": {}}),
        json!({"url": "https://example.com/mcp", "args": []}),
        json!({"url": "file:///etc/passwd"}),
        json!({"url": "https://user:secret@example.com/mcp"}),
        json!({"url": "https://example.com/mcp#fragment"}),
        json!({"transport": "sse"}),
    ] {
        let mut input = input;
        input["action"] = json!("connect");
        let params = serde_json::from_value(input.clone()).unwrap();
        assert!(connection_config(&params, None).is_err(), "{input}");
    }
}

#[tokio::test]
async fn remote_management_login_fails_immediately_with_cli_hint() {
    let error = tool(Default::default())
        .execute(json!({"action": "login", "server": "remote"}), context())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("jcode mcp login"));
    assert!(error.to_string().contains("No browser was opened"));
}

#[tokio::test]
async fn remote_management_status_static_auth_is_secret_free() {
    let config = serde_json::from_value(json!({"servers": {"remote": {"url": "https://example.com/mcp?token=secret", "headers": {"aUtHoRiZaTiOn": "Bearer secret"}}}})).unwrap();
    let output = tool(config)
        .execute(json!({"action": "status", "server": "remote"}), context())
        .await
        .unwrap();
    assert!(output.output.contains("takes precedence"));
    assert!(!output.output.contains("secret"));
}

struct HttpServer {
    url: String,
    requests: Arc<tokio::sync::Mutex<Vec<(String, Value)>>>,
    task: tokio::task::JoinHandle<()>,
}

#[tokio::test]
async fn remote_management_status_reports_corrupt_store_without_secret_contents() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let path = crate::storage::jcode_dir().unwrap().join("mcp-oauth.json");
    std::fs::write(path, "corrupt-secret-access-token").unwrap();
    let error = tool(Default::default())
        .execute(
            json!({"action": "status", "url": "https://example.com/mcp"}),
            context(),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("OAuth credential store is invalid"));
    assert!(!error.contains("secret-access-token"));
}

#[tokio::test]
async fn remote_management_status_and_logout_use_local_store_only() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let url = "https://example.com/mcp";
    let store_path = crate::storage::jcode_dir().unwrap().join("mcp-oauth.json");
    crate::storage::write_json_secret(
        &store_path,
        &json!({"version": 1, "servers": {url: {
            "client_id": "test", "token_endpoint": "http://127.0.0.1:1/token",
            "access_token": "secret-access", "refresh_token": "secret-refresh", "expires_at": 1,
            "resource": url
        }}}),
    )
    .unwrap();
    let config = serde_json::from_value(json!({"servers": {"remote": {"url": url}}})).unwrap();
    let tool = tool(config);
    let status = tool
        .execute(json!({"action": "status", "server": "remote"}), context())
        .await
        .unwrap();
    assert!(status.output.contains("OAuth credentials stored"));
    assert!(!status.output.contains("secret"));
    let logout = tool
        .execute(json!({"action": "logout", "server": "remote"}), context())
        .await
        .unwrap();
    assert!(logout.output.contains("Removed local OAuth credentials"));
    assert!(logout.output.contains("existing connections are unchanged"));
    assert!(crate::mcp::oauth::stored(url).is_none());
    let again = tool
        .execute(json!({"action": "logout", "url": url}), context())
        .await
        .unwrap();
    assert!(again.output.contains("No local OAuth credentials stored"));
    let status = tool
        .execute(json!({"action": "status", "url": url}), context())
        .await
        .unwrap();
    assert!(status.output.contains("jcode mcp login"));
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn http_server() -> HttpServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let observed = Arc::clone(&requests);
    let task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let observed = Arc::clone(&observed);
            tokio::spawn(async move {
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    let mut bytes = Vec::new();
                    let (headers, body_start, content_length) = loop {
                        let mut chunk = [0u8; 4096];
                        let n = stream.read(&mut chunk).await.unwrap();
                        if n == 0 { return; }
                        bytes.extend_from_slice(&chunk[..n]);
                        assert!(bytes.len() < 128 * 1024);
                        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                            let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
                            let content_length = headers.lines().find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                            }).unwrap_or(0);
                            break (headers, end + 4, content_length);
                        }
                    };
                    while bytes.len() < body_start + content_length {
                        let mut chunk = [0u8; 4096];
                        let n = stream.read(&mut chunk).await.unwrap();
                        if n == 0 { return; }
                        bytes.extend_from_slice(&chunk[..n]);
                    }
                    let request: Value = serde_json::from_slice(&bytes[body_start..body_start + content_length]).unwrap_or(Value::Null);
                    observed.lock().await.push((headers.clone(), request.clone()));
                    let (status, body) = if headers.starts_with("GET ") {
                        ("405 Method Not Allowed", String::new())
                    } else if headers.starts_with("DELETE ") || request.get("id").is_none() {
                        ("204 No Content", String::new())
                    } else {
                        let result = match request["method"].as_str().unwrap() {
                            "initialize" => json!({"protocolVersion": request["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "management-http", "version": "1"}}),
                            "tools/list" => json!({"tools": [{"name": "echo", "description": "Echo", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}}]}),
                            "tools/call" => json!({"content": [{"type": "text", "text": request["params"]["arguments"]["text"]}]}),
                            method => panic!("Unexpected method {method}"),
                        };
                        ("200 OK", json!({"jsonrpc": "2.0", "id": request["id"], "result": result}).to_string())
                    };
                    let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    stream.write_all(response.as_bytes()).await.unwrap();
                }).await.unwrap();
            });
        }
    });
    HttpServer {
        url,
        requests,
        task,
    }
}

#[tokio::test]
async fn remote_management_connect_discover_call_disconnect_over_real_http() {
    for configured in [false, true] {
        let server = http_server().await;
        let mut config = crate::mcp::McpConfig::default();
        if configured {
            config.servers.insert("remote".into(), serde_json::from_value(json!({"url": server.url, "enabled": false, "headers": {"aUtHoRiZaTiOn": "Bearer static-test", "X-Test": "management"}})).unwrap());
        }
        let tool = tool(config);
        let mut input = json!({"action": "connect", "server": "remote"});
        if !configured {
            input["url"] = json!(server.url);
            input["transport"] = json!("streamable-http");
            input["headers"] =
                json!({"aUtHoRiZaTiOn": "Bearer static-test", "X-Test": "management"});
        }
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tool.execute(input, context()),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.output.contains("Connected to MCP server 'remote'"),
            "{}",
            output.output
        );
        assert!(output.output.contains("echo"));
        assert!(output.output.contains("input_schema"));
        assert!(
            output.metadata.unwrap()["tool_references"]
                .as_array()
                .unwrap()
                .contains(&json!(dispatch_name("remote", "echo")))
        );
        let result = tool
            .manager
            .read()
            .await
            .call_tool("remote", "echo", json!({"text": "round trip"}))
            .await
            .unwrap();
        assert!(matches!(&result.content[0], ContentBlock::Text { text } if text == "round trip"));
        let output = tool
            .execute(
                json!({"action": "disconnect", "server": "remote"}),
                context(),
            )
            .await
            .unwrap();
        assert!(output.output.contains("Disconnected"));
        assert!(
            tool.manager
                .read()
                .await
                .connected_servers()
                .await
                .is_empty()
        );
        let requests = server.requests.lock().await;
        assert!(
            requests
                .iter()
                .any(|(_, body)| body["method"] == "initialize")
        );
        assert!(
            requests
                .iter()
                .any(|(_, body)| body["method"] == "tools/list")
        );
        assert!(
            requests
                .iter()
                .any(|(_, body)| body["method"] == "tools/call")
        );
        for (headers, _) in requests.iter() {
            let headers = headers.to_ascii_lowercase();
            assert!(headers.contains("authorization: bearer static-test"));
            assert!(headers.contains("x-test: management"));
        }
    }
}
