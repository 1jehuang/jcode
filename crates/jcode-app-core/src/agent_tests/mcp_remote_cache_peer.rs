/// A real HTTP peer whose initialize response is controlled by the test.
/// Dropping the fixture also cancels accepted sockets, including gated ones.
struct RemotePeer {
    url: String,
    initialized: Arc<Semaphore>,
    release: Arc<Semaphore>,
    tools_visible: Arc<std::sync::atomic::AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for RemotePeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl RemotePeer {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let initialized = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let tools_visible = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let started = initialized.clone();
        let gate = release.clone();
        let catalog = tools_visible.clone();
        let task = tokio::spawn(async move {
            let mut sockets = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.unwrap();
                        let started = started.clone();
                        let gate = gate.clone();
                        let catalog = catalog.clone();
                        sockets.spawn(async move {
                            serve_request(stream, started, gate, catalog).await;
                        });
                    }
                    done = sockets.join_next(), if !sockets.is_empty() => {
                        done.unwrap().unwrap();
                    }
                }
            }
        });
        Self {
            url,
            initialized,
            release,
            tools_visible,
            task,
        }
    }

    async fn wait_for_initialize(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.initialized.acquire())
            .await
            .expect("peer must receive initialize")
            .unwrap()
            .forget();
    }
}

async fn serve_request(
    mut stream: tokio::net::TcpStream,
    started: Arc<Semaphore>,
    gate: Arc<Semaphore>,
    tools_visible: Arc<std::sync::atomic::AtomicBool>,
) {
    let mut bytes = Vec::new();
    let (headers, body_start, length) = loop {
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk).await.unwrap();
        if n == 0 {
            return;
        }
        bytes.extend_from_slice(&chunk[..n]);
        assert!(bytes.len() < 128 * 1024);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
            let length = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            break (headers, end + 4, length);
        }
    };
    while bytes.len() < body_start + length {
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk).await.unwrap();
        if n == 0 {
            return;
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    let request: Value =
        serde_json::from_slice(&bytes[body_start..body_start + length]).unwrap_or(Value::Null);
    let (status, body) = if headers.starts_with("GET ") {
        ("405 Method Not Allowed", String::new())
    } else if headers.starts_with("DELETE ") || request.get("id").is_none() {
        ("204 No Content", String::new())
    } else {
        let result = match request["method"].as_str().unwrap() {
            "initialize" => {
                started.add_permits(1);
                gate.acquire().await.unwrap().forget();
                json!({"protocolVersion": request["params"]["protocolVersion"],
                    "capabilities": {"tools": {}}, "serverInfo": {"name": "cache-peer", "version": "1"}})
            }
            "tools/list" => {
                if tools_visible.load(std::sync::atomic::Ordering::SeqCst) {
                    json!({"tools": [{"name": "echo", "description": "Echo over HTTP",
                        "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}}]})
                } else {
                    json!({"tools": []})
                }
            }
            "tools/call" => {
                json!({"content": [{"type": "text", "text": request["params"]["arguments"]["text"]}]})
            }
            method => panic!("unexpected method {method}"),
        };
        (
            "200 OK",
            json!({"jsonrpc": "2.0", "id": request["id"], "result": result}).to_string(),
        )
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
}

fn context(agent: &Agent) -> crate::tool::ToolContext {
    crate::tool::ToolContext {
        session_id: agent.session_id().into(),
        message_id: "cache-test".into(),
        tool_call_id: "connect-test".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    }
}

async fn wait_for_tool(registry: &Registry, name: &str) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if registry.tool_names().await.iter().any(|tool| tool == name) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fast remote tools must publish before slow initialize completes");
}

fn cache_tools_signature(tools: &[ToolDefinition]) -> (u64, usize, usize) {
    let ServerEvent::KvCacheRequest {
        tools_hash,
        tool_count,
        tools_json_chars,
        ..
    } = kv_cache_request_event(&[Message::user("cached message")], tools, "system", &[])
    else {
        unreachable!()
    };
    (tools_hash, tool_count, tools_json_chars)
}
