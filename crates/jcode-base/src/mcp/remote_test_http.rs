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
