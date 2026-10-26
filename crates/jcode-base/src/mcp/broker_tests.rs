use super::*;
use crate::mcp::client::McpClient;
use std::time::Duration;

/// A fake stdio MCP server. `echo` returns this process's pid and the caller's
/// argument, `slow` sleeps before answering, and every line is appended to
/// `$LOG` so tests can see exactly what reached the upstream.
fn fake_server(log: &Path) -> McpServerConfig {
    let script = r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"fake","version":"0"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
    *'"method":"tools/call"'*'"slow"'*)
      sleep 1
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"slow"}]}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      arg=$(printf '%s' "$line" | sed -n 's/.*"value":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pid=%s value=%s"}]}}\n' "$id" "$$" "$arg" ;;
  esac
done
"#;
    let mut env = std::collections::HashMap::new();
    env.insert("LOG".to_string(), log.display().to_string());
    McpServerConfig {
        command: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        env,
        shared: true,
        transport: None,
        url: None,
        headers: std::collections::HashMap::new(),
        enabled: None,
        disabled: None,
        timeout_secs: Some(10),
    }
}

fn text(result: &crate::mcp::ToolCallResult) -> String {
    match result.content.first() {
        Some(crate::mcp::protocol::ContentBlock::Text { text }) => text.clone(),
        _ => String::new(),
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    log: PathBuf,
    task: tokio::task::JoinHandle<Result<()>>,
}

async fn start_broker(upstream_idle: Duration, broker_idle: Duration) -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("mcp-broker.sock");
    let log = dir.path().join("upstream.log");
    let serve_socket = socket.clone();
    let task = tokio::spawn(async move { serve(&serve_socket, upstream_idle, broker_idle).await });
    for _ in 0..100 {
        if UnixStream::connect(&socket).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Harness {
        _dir: dir,
        socket,
        log,
        task,
    }
}

async fn attach(h: &Harness, config: &McpServerConfig) -> McpClient {
    match McpClient::connect_via_broker("fake".into(), config, h.socket.clone(), false).await {
        Ok(client) => client,
        Err(error) => panic!("attach failed: {error}"),
    }
}

fn upstream_lines(h: &Harness, needle: &str) -> usize {
    std::fs::read_to_string(&h.log)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(needle))
        .count()
}

#[tokio::test]
async fn two_clients_share_one_upstream_process() {
    let h = start_broker(Duration::from_secs(60), Duration::from_secs(60)).await;
    let config = fake_server(&h.log);
    let a = attach(&h, &config).await;
    let b = attach(&h, &config).await;

    let ra = a
        .call_tool("echo", json!({"value": "a"}))
        .await
        .expect("call a");
    let rb = b
        .call_tool("echo", json!({"value": "b"}))
        .await
        .expect("call b");
    let pid_a = text(&ra)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    let pid_b = text(&rb)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(
        pid_a.starts_with("pid="),
        "unexpected reply {:?}",
        text(&ra)
    );
    assert_eq!(pid_a, pid_b, "both clients must reach the same process");
    assert!(text(&ra).ends_with("value=a") && text(&rb).ends_with("value=b"));

    // The upstream saw exactly one handshake even though two clients attached.
    assert_eq!(upstream_lines(&h, "\"method\":\"initialize\""), 1);
    assert_eq!(upstream_lines(&h, "notifications/initialized"), 1);

    let status = request_status(&h.socket).await.expect("status");
    assert_eq!(status["upstreams"].as_array().map(Vec::len), Some(1));
    assert_eq!(status["upstreams"][0]["clients"], json!(2));
    h.task.abort();
}

#[tokio::test]
async fn concurrent_requests_with_colliding_ids_route_to_their_callers() {
    let h = start_broker(Duration::from_secs(60), Duration::from_secs(60)).await;
    let config = fake_server(&h.log);
    // Each client numbers its own requests from 1, so their ids collide on
    // the wire. The broker must still give every caller its own answer.
    let a = attach(&h, &config).await;
    let b = attach(&h, &config).await;
    let slow = a.call_tool("slow", json!({}));
    let fast = b.call_tool("echo", json!({"value": "fast"}));
    let (slow, fast) = tokio::join!(slow, fast);
    assert_eq!(text(&slow.expect("slow")), "slow");
    assert!(text(&fast.expect("fast")).ends_with("value=fast"));
    h.task.abort();
}

#[tokio::test]
async fn different_configs_get_different_upstreams() {
    let h = start_broker(Duration::from_secs(60), Duration::from_secs(60)).await;
    let first = fake_server(&h.log);
    let mut second = fake_server(&h.log);
    second.env.insert("EXTRA".into(), "1".into());
    let a = attach(&h, &first).await;
    let b = attach(&h, &second).await;
    let pa = text(&a.call_tool("echo", json!({"value": "x"})).await.unwrap());
    let pb = text(&b.call_tool("echo", json!({"value": "x"})).await.unwrap());
    assert_ne!(pa, pb, "distinct env must not share a process");
    h.task.abort();
}

#[tokio::test]
async fn idle_upstream_is_stopped_and_restarted_on_next_attach() {
    let h = start_broker(Duration::from_millis(200), Duration::from_secs(60)).await;
    let config = fake_server(&h.log);
    let client = attach(&h, &config).await;
    let first = text(
        &client
            .call_tool("echo", json!({"value": "1"}))
            .await
            .unwrap(),
    );
    drop(client);

    // Wait for the sweep to notice the upstream has no clients.
    let mut stopped = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let status = request_status(&h.socket).await.expect("status");
        if status["upstreams"].as_array().is_some_and(Vec::is_empty) {
            stopped = true;
            break;
        }
    }
    assert!(stopped, "idle upstream was never stopped");

    let client = attach(&h, &config).await;
    let second = text(
        &client
            .call_tool("echo", json!({"value": "2"}))
            .await
            .unwrap(),
    );
    assert_ne!(
        first.split_whitespace().next(),
        second.split_whitespace().next(),
        "a fresh upstream should have started"
    );
    h.task.abort();
}

#[tokio::test]
async fn client_reattaches_after_upstream_dies() {
    let h = start_broker(Duration::from_secs(60), Duration::from_secs(60)).await;
    let config = fake_server(&h.log);
    let client = attach(&h, &config).await;
    let reply = text(
        &client
            .call_tool("echo", json!({"value": "1"}))
            .await
            .unwrap(),
    );
    let pid: i32 = reply
        .trim_start_matches("pid=")
        .split_whitespace()
        .next()
        .and_then(|p| p.parse().ok())
        .expect("pid");
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    // The broker notices the exit and drops the link; the next call must
    // transparently reattach and reach a new upstream.
    let mut recovered = None;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(result) = client.call_tool("echo", json!({"value": "2"})).await {
            recovered = Some(text(&result));
            break;
        }
    }
    let recovered = recovered.expect("client never recovered");
    assert!(recovered.ends_with("value=2"));
    assert!(!recovered.starts_with(&format!("pid={pid} ")));
    h.task.abort();
}

#[tokio::test]
async fn failing_upstream_reports_an_upstream_error() {
    let h = start_broker(Duration::from_secs(60), Duration::from_secs(60)).await;
    let mut config = fake_server(&h.log);
    config.command = "/nonexistent/mcp-server".into();
    match McpClient::connect_via_broker("bad".into(), &config, h.socket.clone(), false).await {
        Err(BrokerConnectError::Upstream(_)) => {}
        Err(other) => panic!("expected an upstream error, got {other}"),
        Ok(_) => panic!("a missing binary must not connect"),
    }
    h.task.abort();
}

#[tokio::test]
async fn missing_broker_is_reported_as_unavailable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = fake_server(&dir.path().join("log"));
    match McpClient::connect_via_broker(
        "fake".into(),
        &config,
        dir.path().join("absent.sock"),
        false,
    )
    .await
    {
        Err(BrokerConnectError::Unavailable(_)) => {}
        Err(other) => panic!("expected unavailable, got {other}"),
        Ok(_) => panic!("no broker is listening"),
    }
}

#[tokio::test]
async fn idle_broker_exits_and_removes_its_socket() {
    let h = start_broker(Duration::from_millis(100), Duration::from_millis(200)).await;
    let result = tokio::time::timeout(Duration::from_secs(10), h.task)
        .await
        .expect("broker should exit when idle")
        .expect("join");
    assert!(result.is_ok());
    assert!(!h.socket.exists());
}

#[tokio::test]
async fn second_broker_on_same_home_exits_without_stealing_the_socket() {
    let h = start_broker(Duration::from_secs(60), Duration::from_secs(60)).await;
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        serve(&h.socket, Duration::from_secs(60), Duration::from_secs(60)),
    )
    .await
    .expect("second broker must exit promptly");
    assert!(result.is_ok());
    assert!(
        request_status(&h.socket).await.is_ok(),
        "first broker still serves"
    );
    h.task.abort();
}

#[test]
fn session_scoped_env_is_not_inherited_by_broker() {
    assert!(is_session_scoped_env("JCODE_SOCKET"));
    assert!(is_session_scoped_env("OPENRIG_NODE_ID"));
    assert!(is_session_scoped_env("TMUX_PANE"));
    assert!(!is_session_scoped_env("JCODE_HOME"));
    assert!(!is_session_scoped_env(BROKER_BIN_ENV));
    assert!(!is_session_scoped_env("PATH"));
}
