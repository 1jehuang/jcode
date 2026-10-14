//! Acceptance-oriented tests: real TCP HTTP/SSE servers and the public client.
//! No transport mocks or external network services are used.
use super::*;
use crate::mcp::client::McpClient;
use crate::mcp::protocol::ContentBlock;
use serde_json::json;
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Value,
}

#[derive(Clone)]
struct Plan {
    legacy: bool,
    fallback_status: u16,
    post_sse: bool,
    endpoint: Option<String>,
    omit_endpoint: bool,
    initialize_status: Option<u16>,
    redirect: Option<String>,
    notification_status: u16,
    protocol_version: String,
    get_content_type: String,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            legacy: false,
            fallback_status: 405,
            post_sse: false,
            endpoint: None,
            omit_endpoint: false,
            initialize_status: None,
            redirect: None,
            notification_status: 202,
            protocol_version: "2025-06-18".to_string(),
            get_content_type: "text/event-stream".to_string(),
        }
    }
}

struct FixtureState {
    plan: Plan,
    requests: std::sync::Mutex<Vec<HttpRequest>>,
    initialized: AtomicBool,
    ordering_violation: AtomicBool,
    active_streams: AtomicUsize,
    disconnected_streams: AtomicUsize,
    sse_tx: std::sync::Mutex<Option<mpsc::Sender<Value>>>,
}

struct Fixture {
    url: String,
    state: Arc<FixtureState>,
    task: JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fixture {
    async fn start(plan: Plan) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let state = Arc::new(FixtureState {
            plan,
            requests: std::sync::Mutex::new(Vec::new()),
            initialized: AtomicBool::new(false),
            ordering_violation: AtomicBool::new(false),
            active_streams: AtomicUsize::new(0),
            disconnected_streams: AtomicUsize::new(0),
            sse_tx: std::sync::Mutex::new(None),
        });
        let owned_state = Arc::clone(&state);
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        let state = Arc::clone(&owned_state);
                        connections.spawn(async move {
                            let _ = serve(socket, state).await;
                        });
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {},
                }
            }
        });
        Self { url, state, task }
    }

    fn config(&self, transport: &str) -> McpServerConfig {
        serde_json::from_value(json!({
            "type": transport,
            "url": self.url,
            "headers": {"Authorization":"Bearer fixture-secret", "X-Api-Key":"custom-secret"},
            "timeout_secs": 3
        }))
        .unwrap()
    }

    async fn connect(&self, transport: &str) -> McpClient {
        tokio::time::timeout(
            Duration::from_secs(5),
            McpClient::connect("local-fixture".into(), &self.config(transport)),
        )
        .await
        .unwrap()
        .unwrap()
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.state.requests.lock().unwrap().clone()
    }
}

async fn parse_request(socket: TcpStream) -> Result<(HttpRequest, TcpStream)> {
    let mut reader = BufReader::new(socket);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let mut first = line.split_whitespace();
    let method = first.next().context("method")?.to_string();
    let path = first.next().context("path")?.to_string();
    let mut headers = HashMap::new();
    loop {
        line.clear();
        reader.read_line(&mut line).await?;
        if line == "\r\n" {
            break;
        }
        let (key, value) = line.split_once(':').context("header")?;
        headers.insert(key.to_ascii_lowercase(), value.trim().to_string());
    }
    let size = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes).await?;
    let body = if size == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)?
    };
    Ok((
        HttpRequest {
            method,
            path,
            headers,
            body,
        },
        reader.into_inner(),
    ))
}

async fn response(
    socket: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
    extra: &str,
) -> Result<()> {
    let headers = format!(
        "HTTP/1.1 {status} Fixture\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    );
    socket.write_all(headers.as_bytes()).await?;
    socket.write_all(body.as_bytes()).await?;
    Ok(())
}

async fn open_stream(socket: &mut TcpStream, content_type: &str) -> Result<()> {
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").as_bytes()).await?;
    Ok(())
}

async fn chunk(socket: &mut TcpStream, data: &[u8]) -> Result<()> {
    socket
        .write_all(format!("{:x}\r\n", data.len()).as_bytes())
        .await?;
    socket.write_all(data).await?;
    socket.write_all(b"\r\n").await?;
    Ok(())
}

struct ActiveStream(Arc<FixtureState>);
impl ActiveStream {
    fn new(state: &Arc<FixtureState>) -> Self {
        state.active_streams.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(state))
    }
}
impl Drop for ActiveStream {
    fn drop(&mut self) {
        self.0.active_streams.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn until_disconnect(socket: &mut TcpStream, state: &Arc<FixtureState>) -> Result<()> {
    let _active = ActiveStream::new(state);
    let mut byte = [0];
    let _ = socket.read(&mut byte).await?;
    state.disconnected_streams.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

async fn serve(socket: TcpStream, state: Arc<FixtureState>) -> Result<()> {
    let (request, mut socket) = parse_request(socket).await?;
    state.requests.lock().unwrap().push(request.clone());
    let plan = &state.plan;
    if request.method == "DELETE" {
        return response(&mut socket, 405, "text/plain", "", "").await;
    }
    if request.method == "GET" {
        open_stream(&mut socket, &plan.get_content_type).await?;
        let (tx, mut rx) = mpsc::channel::<Value>(32);
        *state.sse_tx.lock().unwrap() = Some(tx);
        if !plan.omit_endpoint {
            let endpoint = plan
                .endpoint
                .as_deref()
                .unwrap_or("/messages?session=legacy");
            chunk(
                &mut socket,
                format!("\u{feff}: keepalive\r\nevent: endpoint\r\ndata: {endpoint}\r\n\r\n")
                    .as_bytes(),
            )
            .await?;
        }
        let _active = ActiveStream::new(&state);
        let mut byte = [0];
        loop {
            tokio::select! {
                read = socket.read(&mut byte) => {
                    let _ = read?;
                    state.disconnected_streams.fetch_add(1, Ordering::SeqCst);
                    break;
                }
                item = rx.recv() => {
                    let Some(item) = item else { break };
                    chunk(&mut socket, format!("event: message\ndata: {item}\n\n").as_bytes()).await?;
                }
            }
        }
        return Ok(());
    }
    let method = request
        .body
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("");
    if plan.legacy && request.path == "/mcp" {
        return response(
            &mut socket,
            plan.fallback_status,
            "text/plain",
            "legacy endpoint",
            "",
        )
        .await;
    }
    if method == "initialize"
        && let Some(status) = plan.initialize_status
    {
        let location = plan
            .redirect
            .as_ref()
            .map(|url| format!("Location: {url}\r\n"))
            .unwrap_or_default();
        return response(
            &mut socket,
            status,
            "text/plain",
            "initialization rejected",
            &location,
        )
        .await;
    }
    if method == "notifications/initialized" {
        tokio::time::sleep(Duration::from_millis(40)).await;
        state.initialized.store(true, Ordering::SeqCst);
        return response(&mut socket, plan.notification_status, "text/plain", "", "").await;
    }
    if method != "initialize" && !state.initialized.load(Ordering::SeqCst) {
        state.ordering_violation.store(true, Ordering::SeqCst);
    }
    let id = request.body.get("id").and_then(Value::as_u64).unwrap_or(0);
    let tool = request
        .body
        .pointer("/params/name")
        .and_then(Value::as_str)
        .unwrap_or("");
    if tool == "hang" {
        open_stream(&mut socket, "text/event-stream").await?;
        return until_disconnect(&mut socket, &state).await;
    }
    if tool == "slow" {
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    match tool {
        "expire" => return response(&mut socket, 404, "text/plain", "session gone", "").await,
        "invalid-json" => return response(&mut socket, 200, "application/json", "{bad", "").await,
        "empty" => return response(&mut socket, 200, "application/json", "", "").await,
        "accepted" => return response(&mut socket, 202, "text/plain", "", "").await,
        "wrong-type" => {
            return response(&mut socket, 200, "text/html", "<html>no</html>", "").await;
        }
        "oversized-json" => {
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", MAX_RESPONSE_BYTES + 1).as_bytes()).await?;
            return until_disconnect(&mut socket, &state).await;
        }
        "oversized-chunked" | "oversized-sse" => {
            open_stream(
                &mut socket,
                if tool == "oversized-sse" {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .await?;
            for _ in 0..129 {
                chunk(&mut socket, &vec![b'x'; 64 * 1024]).await?;
            }
            return until_disconnect(&mut socket, &state).await;
        }
        _ => {}
    }
    let result = match method {
        "initialize" => {
            json!({"protocolVersion":plan.protocol_version,"capabilities":{"tools":{}},"serverInfo":{"name":"localhost","version":"1"}})
        }
        "tools/list" => {
            json!({"tools":[{"name":"echo","description":"fixture","inputSchema":{"type":"object"}}]})
        }
        "tools/call" => {
            json!({"content":[{"type":"text","text":request.body.pointer("/params/arguments").unwrap().to_string()}]})
        }
        _ => json!({}),
    };
    let mut reply = json!({"jsonrpc":"2.0","id":id,"result":result});
    if tool == "wrong-id" {
        reply["id"] = json!(id + 5000);
    }
    if tool == "invalid-envelope" {
        reply["jsonrpc"] = json!("1.0");
    }
    if tool == "server-error" {
        reply = json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"bad arguments"}});
    }
    if plan.legacy {
        let tx = state
            .sse_tx
            .lock()
            .unwrap()
            .clone()
            .context("SSE channel")?;
        tx.send(reply).await?;
        return response(&mut socket, 202, "text/plain", "", "").await;
    }
    let session = if method == "initialize" {
        "Mcp-Session-Id: fixture-session\r\n"
    } else if tool == "session-rotate" {
        "Mcp-Session-Id: malicious-rotation\r\n"
    } else {
        ""
    };
    if plan.post_sse {
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n{session}\r\n").as_bytes()).await?;
        let event = format!(
            "\u{feff}: comment\revent: message\rdata: {{\rdata: \"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}}\r\rdata: {reply}\r\r"
        );
        // Deliberately fragment CRLF and UTF-8 across actual HTTP chunks.
        for fragment in event.as_bytes().chunks(7) {
            chunk(&mut socket, fragment).await?;
        }
        socket.write_all(b"0\r\n\r\n").await?;
        return Ok(());
    }
    response(
        &mut socket,
        200,
        "application/json; charset=utf-8",
        &reply.to_string(),
        session,
    )
    .await
}

async fn wait_until(predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture condition did not become true");
}

fn text(result: crate::mcp::protocol::ToolCallResult) -> String {
    match result.content.into_iter().next().unwrap() {
        ContentBlock::Text { text } => text,
        other => panic!("expected text, got {other:?}"),
    }
}

#[tokio::test]
async fn public_json_connect_list_call_refresh_and_shutdown() {
    let fixture = Fixture::start(Plan::default()).await;
    let mut client = fixture.connect("http").await;
    assert!(client.is_remote());
    assert!(client.is_running());
    assert_eq!(client.server_info().unwrap().name, "localhost");
    assert_eq!(client.tools()[0].name, "echo");
    assert_eq!(
        text(
            client
                .call_tool("echo", json!({"word":"héllo"}))
                .await
                .unwrap()
        ),
        "{\"word\":\"héllo\"}"
    );
    assert_eq!(
        text(client.call_tool("echo", Value::Null).await.unwrap()),
        "{}"
    );
    client.refresh_tools().await.unwrap();
    client.call_tool("session-rotate", json!({})).await.unwrap();
    client.call_tool("echo", json!({})).await.unwrap();
    assert!(!fixture.state.ordering_violation.load(Ordering::SeqCst));
    let handle = client.handle();
    client.shutdown().await;
    assert!(!client.is_running());
    assert!(handle.call_tool("echo", json!({})).await.is_err());
    let requests = fixture.requests();
    assert_eq!(requests[0].body["method"], "initialize");
    assert!(!requests[0].headers.contains_key(SESSION_HEADER));
    for request in requests.iter().skip(1) {
        assert_eq!(
            request.headers.get(SESSION_HEADER).map(String::as_str),
            Some("fixture-session")
        );
        assert_eq!(
            request.headers.get(PROTOCOL_HEADER).map(String::as_str),
            Some("2025-06-18")
        );
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer fixture-secret")
        );
    }
    assert_eq!(requests.last().unwrap().method, "DELETE");
}

#[tokio::test]
async fn public_post_sse_fragmentation_notifications_and_multiline_json() {
    let fixture = Fixture::start(Plan {
        post_sse: true,
        ..Plan::default()
    })
    .await;
    let mut client = fixture.connect("http").await;
    assert_eq!(
        text(
            client
                .call_tool("echo", json!({"word":"héllo 🦀"}))
                .await
                .unwrap()
        ),
        "{\"word\":\"héllo 🦀\"}"
    );
    client.shutdown().await;
}

#[tokio::test]
async fn public_legacy_and_http_fallback_workflows() {
    for (transport, status) in [("sse", 405), ("http", 400), ("http", 404), ("http", 405)] {
        let fixture = Fixture::start(Plan {
            legacy: true,
            fallback_status: status,
            protocol_version: "2024-11-05".into(),
            ..Plan::default()
        })
        .await;
        let mut client = fixture.connect(transport).await;
        assert_eq!(
            text(client.call_tool("echo", json!({"ok":true})).await.unwrap()),
            "{\"ok\":true}"
        );
        client.refresh_tools().await.unwrap();
        assert!(!fixture.state.ordering_violation.load(Ordering::SeqCst));
        let legacy_initialize = fixture
            .requests()
            .into_iter()
            .find(|r| r.path.starts_with("/messages") && r.body["method"] == "initialize")
            .unwrap();
        assert_eq!(
            legacy_initialize.body["params"]["protocolVersion"],
            "2024-11-05"
        );
        client.shutdown().await;
        wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 0).await;
        assert!(fixture.state.disconnected_streams.load(Ordering::SeqCst) >= 1);
    }
}

#[tokio::test]
async fn concurrent_calls_do_not_block_or_cross_correlate() {
    let fixture = Fixture::start(Plan::default()).await;
    let mut client = fixture.connect("http").await;
    let slow_handle = client.handle();
    let slow = tokio::spawn(async move {
        slow_handle
            .call_tool("slow", json!({"request":"slow"}))
            .await
            .unwrap()
    });
    wait_until(|| {
        fixture
            .requests()
            .iter()
            .any(|r| r.body.pointer("/params/name") == Some(&json!("slow")))
    })
    .await;
    let fast = tokio::time::timeout(
        Duration::from_millis(200),
        client.call_tool("echo", json!({"request":"fast"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(text(fast), "{\"request\":\"fast\"}");
    assert!(!slow.is_finished());
    assert_eq!(text(slow.await.unwrap()), "{\"request\":\"slow\"}");
    client.shutdown().await;
}

#[tokio::test]
async fn shutdown_and_drop_cancel_owned_http_streams_and_waiters() {
    for explicit in [true, false] {
        let fixture = Fixture::start(Plan::default()).await;
        let mut client = fixture.connect("http").await;
        let handle = client.handle();
        let request_handle = handle.clone();
        let request =
            tokio::spawn(async move { request_handle.call_tool("hang", json!({})).await });
        wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 1).await;
        if explicit {
            client.shutdown().await;
        }
        drop(client);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), request)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 0).await;
        assert_eq!(fixture.state.disconnected_streams.load(Ordering::SeqCst), 1);
        assert!(handle.call_tool("echo", json!({})).await.is_err());
    }
}

#[tokio::test]
async fn caller_cancellation_aborts_response_stream_without_shutdown() {
    let fixture = Fixture::start(Plan::default()).await;
    let mut client = fixture.connect("http").await;
    let handle = client.handle();
    let request = tokio::spawn(async move { handle.call_tool("hang", json!({})).await });
    wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 1).await;
    request.abort();
    let _ = request.await;
    wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 0).await;
    client.call_tool("echo", json!({})).await.unwrap();
    client.shutdown().await;
}

#[tokio::test]
async fn session_404_fails_all_waiters_and_requires_fresh_connect() {
    let fixture = Fixture::start(Plan::default()).await;
    let mut client = fixture.connect("http").await;
    let handle = client.handle();
    let hung = tokio::spawn(async move { handle.call_tool("hang", json!({})).await });
    wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 1).await;
    let err = client
        .call_tool("expire", json!({}))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("session expired"), "{err}");
    assert!(!client.is_running());
    assert!(
        tokio::time::timeout(Duration::from_secs(1), hung)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 0).await;
    client.shutdown().await;
    let mut fresh = fixture.connect("http").await;
    fresh.call_tool("echo", json!({})).await.unwrap();
    fresh.shutdown().await;
}

#[tokio::test]
async fn invalid_responses_and_oversized_bodies_fail_promptly() {
    let fixture = Fixture::start(Plan::default()).await;
    let mut client = fixture.connect("http").await;
    for tool in [
        "invalid-json",
        "empty",
        "accepted",
        "wrong-type",
        "wrong-id",
        "invalid-envelope",
        "oversized-json",
        "oversized-chunked",
        "oversized-sse",
        "server-error",
    ] {
        let err = tokio::time::timeout(Duration::from_secs(2), client.call_tool(tool, json!({})))
            .await
            .expect("must fail without request timeout")
            .unwrap_err();
        assert!(
            !err.to_string().contains("Request timeout"),
            "{tool}: {err:#}"
        );
    }
    client.call_tool("echo", json!({})).await.unwrap();
    client.shutdown().await;
}

#[tokio::test]
async fn initialization_and_notification_errors_do_not_return_live_client() {
    for plan in [
        Plan {
            notification_status: 500,
            ..Plan::default()
        },
        Plan {
            protocol_version: "2099-01-01".into(),
            ..Plan::default()
        },
    ] {
        let fixture = Fixture::start(plan).await;
        let err = McpClient::connect("bad-handshake".into(), &fixture.config("http"))
            .await
            .err()
            .unwrap();
        assert!(
            !fixture
                .requests()
                .iter()
                .any(|r| r.body["method"] == "tools/list"),
            "{err:#}"
        );
    }
}

#[tokio::test]
async fn static_authorization_401_never_falls_back_or_retries() {
    let fixture = Fixture::start(Plan {
        initialize_status: Some(401),
        ..Plan::default()
    })
    .await;
    let err = McpClient::connect("unauthorized".into(), &fixture.config("http"))
        .await
        .err()
        .unwrap();
    assert!(format!("{err:#}").contains("configured Authorization"));
    assert_eq!(fixture.requests().len(), 1);
}

#[tokio::test]
async fn legacy_endpoint_cross_origin_and_redirects_never_leak_headers() {
    let attacker = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let attacker_url = format!("http://{}/steal", attacker.local_addr().unwrap());
    for plan in [
        Plan {
            legacy: true,
            endpoint: Some(attacker_url.clone()),
            ..Plan::default()
        },
        Plan {
            initialize_status: Some(307),
            redirect: Some(attacker_url.clone()),
            ..Plan::default()
        },
    ] {
        let fixture = Fixture::start(plan.clone()).await;
        let result = McpClient::connect(
            "secure".into(),
            &fixture.config(if plan.legacy { "sse" } else { "http" }),
        )
        .await;
        assert!(result.is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), attacker.accept())
                .await
                .is_err(),
            "server-selected origin received a credential-bearing request"
        );
        if plan.legacy {
            wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 0).await;
        }
    }
}

#[tokio::test]
async fn endpoint_timeout_and_cancelled_connect_drop_legacy_reader() {
    for cancel in [true, false] {
        let fixture = Fixture::start(Plan {
            legacy: true,
            omit_endpoint: true,
            ..Plan::default()
        })
        .await;
        let mut config = fixture.config("sse");
        config.timeout_secs = Some(1);
        let connect =
            tokio::spawn(async move { McpClient::connect("no-endpoint".into(), &config).await });
        wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 1).await;
        if cancel {
            connect.abort();
        }
        let result = connect.await;
        if !cancel {
            assert!(result.unwrap().is_err());
        }
        wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 0).await;
        assert_eq!(fixture.state.disconnected_streams.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn remote_handshake_deadline_is_independent_of_long_tool_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let (started_tx, started_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let (request, mut socket) = parse_request(socket).await.unwrap();
        assert_eq!(request.body["method"], "initialize");
        open_stream(&mut socket, "text/event-stream").await.unwrap();
        started_tx.send(()).unwrap();
        // Never send a response. The client must close the socket on deadline.
        let mut bytes = Vec::new();
        socket.read_to_end(&mut bytes).await.unwrap();
    });
    let config: McpServerConfig = serde_json::from_value(json!({
        "type": "http", "url": url, "timeout_secs": 3600
    }))
    .unwrap();
    let connect =
        tokio::spawn(async move { McpClient::connect("bounded-handshake".into(), &config).await });
    tokio::time::timeout(Duration::from_secs(2), started_rx)
        .await
        .unwrap()
        .unwrap();

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(!connect.is_finished());
    tokio::time::advance(Duration::from_secs(2)).await;
    let err = connect.await.unwrap().err().unwrap();
    tokio::time::resume();
    assert!(format!("{err:#}").contains("connection timed out after 30s"));
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("deadline must close the response stream")
        .unwrap();
}

#[tokio::test]
async fn bounded_concurrency_applies_backpressure_and_queue_timeout() {
    let fixture = Fixture::start(Plan::default()).await;
    let mut config = fixture.config("http");
    config.timeout_secs = Some(1);
    let mut client = McpClient::connect("backpressure".into(), &config)
        .await
        .unwrap();
    let mut callers = JoinSet::new();
    for _ in 0..96 {
        let handle = client.handle();
        callers.spawn(async move { handle.call_tool("hang", json!({})).await });
    }
    wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == MAX_CONCURRENT_REQUESTS)
        .await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(
        fixture.state.active_streams.load(Ordering::SeqCst),
        MAX_CONCURRENT_REQUESTS
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(result) = callers.join_next().await {
            assert!(result.unwrap().is_err());
        }
    })
    .await
    .unwrap();
    wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 0).await;
    client.shutdown().await;
}

#[tokio::test]
async fn owned_manager_reconnects_only_on_next_call_after_session_404() {
    use crate::mcp::manager::McpManager;
    use crate::mcp::protocol::McpConfig;
    let fixture = Fixture::start(Plan::default()).await;
    let mut server = fixture.config("http");
    server.shared = false;
    let config = McpConfig {
        servers: HashMap::from([("reconnect".to_string(), server)]),
    };
    let manager = McpManager::with_config(config);
    let error = manager
        .call_tool("reconnect", "expire", json!({"operation":"first"}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("session expired"));
    assert!(manager.connected_servers().await.is_empty());
    assert!(manager.all_tools().await.is_empty());
    let (first, second) = tokio::join!(
        manager.call_tool("reconnect", "echo", json!({"operation":"second"})),
        manager.call_tool("reconnect", "echo", json!({"operation":"third"})),
    );
    assert_eq!(text(first.unwrap()), "{\"operation\":\"second\"}");
    assert_eq!(text(second.unwrap()), "{\"operation\":\"third\"}");
    assert_eq!(manager.connected_servers().await, vec!["reconnect"]);
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|r| r.body["method"] == "initialize")
            .count(),
        2
    );
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|r| r.body.pointer("/params/name") == Some(&json!("expire")))
            .count(),
        1
    );
    for request in fixture
        .requests()
        .iter()
        .filter(|r| r.body["method"] == "initialize")
    {
        assert!(!request.headers.contains_key(SESSION_HEADER));
    }
    manager.disconnect_all().await;
}

#[tokio::test]
async fn shared_manager_and_pool_replace_expired_handles_without_replaying_calls() {
    use crate::mcp::manager::McpManager;
    use crate::mcp::pool::SharedMcpPool;
    use crate::mcp::protocol::McpConfig;
    let _env_guard = crate::storage::lock_test_env();
    let fixture = Fixture::start(Plan::default()).await;
    let config = McpConfig {
        servers: HashMap::from([("reconnect".to_string(), fixture.config("http"))]),
    };
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join(".mcp.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let pool = Arc::new(SharedMcpPool::new(config));
    let manager = McpManager::with_shared_pool_for_dir(
        Arc::clone(&pool),
        "one".into(),
        Some(project.path().to_path_buf()),
    );
    let other = McpManager::with_shared_pool_for_dir(
        Arc::clone(&pool),
        "two".into(),
        Some(project.path().to_path_buf()),
    );
    manager
        .call_tool("reconnect", "echo", json!({}))
        .await
        .unwrap();
    other
        .call_tool("reconnect", "echo", json!({}))
        .await
        .unwrap();
    let expired = pool.get_handle("reconnect").await.unwrap();
    let error = manager
        .call_tool("reconnect", "expire", json!({"operation":"first"}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("session expired"));
    assert!(expired.is_closed());
    assert!(manager.connected_servers().await.is_empty());
    assert!(other.connected_servers().await.is_empty());
    assert!(pool.connected_servers().await.is_empty());
    assert!(pool.get_handle("reconnect").await.is_none());
    assert!(!pool.has_connections().await);
    let (first, second) = tokio::join!(
        manager.call_tool("reconnect", "echo", json!({"operation":"second"})),
        other.call_tool("reconnect", "echo", json!({"operation":"third"})),
    );
    assert_eq!(text(first.unwrap()), "{\"operation\":\"second\"}");
    assert_eq!(text(second.unwrap()), "{\"operation\":\"third\"}");
    assert!(pool.has_connections().await);
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|r| r.body["method"] == "initialize")
            .count(),
        2
    );
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|r| r.body.pointer("/params/name") == Some(&json!("expire")))
            .count(),
        1
    );
    manager.disconnect_all().await;
    other.disconnect_all().await;
    pool.disconnect_all().await;
}

#[test]
fn endpoint_origin_validation_accepts_relative_paths_only_on_configured_origin() {
    let base = reqwest::Url::parse("https://example.com/sse").unwrap();
    for endpoint in [
        "/messages?session=1",
        "messages",
        "https://example.com:443/messages",
    ] {
        assert!(validate_endpoint(&base, endpoint).is_ok(), "{endpoint}");
    }
    for endpoint in [
        "https://evil.com/message",
        "//evil.com/message",
        "http://example.com/message",
        "https://example.com:444/message",
        "https://user:pass@example.com/message",
        "/message#fragment",
        "file:///message",
    ] {
        assert!(validate_endpoint(&base, endpoint).is_err(), "{endpoint}");
    }
}

#[test]
fn sse_parser_arbitrary_byte_splits_bom_line_endings_and_empty_fields() {
    let data = "\u{feff}: comment\r\nevent: message\rdata: héllo 🦀\r\ndata: second\n\rdata\n\n";
    let mut parser = SseParser::default();
    let mut events = Vec::new();
    for byte in data.as_bytes() {
        events.extend(parser.push(&[*byte]).unwrap());
    }
    assert_eq!(
        events,
        vec![
            SseEvent {
                event: Some("message".into()),
                data: "héllo 🦀\nsecond".into()
            },
            SseEvent {
                event: None,
                data: String::new()
            }
        ]
    );
    assert!(
        parser
            .push(b"event: discarded\n\n: comment\n\n")
            .unwrap()
            .is_empty()
    );
    assert_eq!(parser.push(b"data: x\n\n").unwrap()[0].event, None);
}

#[test]
fn sse_parser_bounds_partial_lines_accumulated_events_and_chunk_output() {
    let mut parser = SseParser::default();
    for _ in 0..8 {
        parser.push(&vec![b'x'; 1024 * 1024]).unwrap();
    }
    assert!(parser.push(b"x").is_err());
    let mut parser = SseParser::default();
    let line = format!("data: {}\n", "x".repeat(1024 * 1024));
    for _ in 0..7 {
        parser.push(line.as_bytes()).unwrap();
    }
    assert!(parser.push(line.as_bytes()).is_err());
    let mut parser = SseParser::default();
    assert!(parser.push(&b"data\n\n".repeat(4097)).is_err());
}
