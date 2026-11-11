use super::*;
use crate::message::{Message, ToolDefinition};
use crate::protocol::{Request, decode_request};
use crate::provider::EventStream;
use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const CHILD_TEST: &str = "server::runtime::error_tests::isolated_lightweight_pipe_log_child";
const PRIVATE_PAYLOAD: &str = "private-payload-sentinel";

struct UnusedProvider;

#[async_trait]
impl Provider for UnusedProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        panic!("lightweight control must not invoke a provider")
    }

    fn name(&self) -> &str {
        "pipe-log-test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        panic!("lightweight control must not fork a provider")
    }
}

#[tokio::test]
async fn initial_lightweight_failure_reaches_runtime_log() {
    let home = tempfile::tempdir().expect("isolated home");
    let mut command = tokio::process::Command::new(std::env::current_exe().expect("test binary"));
    command
        .args(["--exact", CHILD_TEST, "--ignored", "--nocapture"])
        .env_clear()
        .env("JCODE_HOME", home.path())
        .env("JCODE_PIPE_LOG_TEST_HOME", home.path())
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("TEMP", home.path())
        .env("TMP", home.path())
        .current_dir(home.path())
        .kill_on_drop(true);
    for key in ["SystemRoot", "WINDIR"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = match tokio::time::timeout(Duration::from_secs(30), command.output()).await {
        Ok(Ok(output)) => output,
        other => {
            let preserved = home.keep();
            panic!(
                "isolated child failed: {other:?}; home={}",
                preserved.display()
            );
        }
    };
    if !output.status.success() {
        let preserved = home.keep();
        panic!(
            "child failed: home={} stdout={} stderr={}",
            preserved.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    let receipt = std::fs::read_to_string(home.path().join("handler-log-verified"))
        .expect("child must verify the actual runtime log");
    assert!(receipt.contains("operation=write_direct_event"));
    assert!(!receipt.contains(PRIVATE_PAYLOAD));
    println!("verified_runtime_log={receipt}");
}

#[tokio::test]
#[ignore]
async fn isolated_lightweight_pipe_log_child() {
    let home = PathBuf::from(
        std::env::var_os("JCODE_PIPE_LOG_TEST_HOME").expect("run through isolated parent test"),
    );
    assert_eq!(crate::storage::jcode_dir().expect("test home"), home);
    crate::logging::init();
    let log_path = crate::logging::log_path().expect("isolated log path");
    assert!(log_path.starts_with(&home));

    let server = crate::server::Server::new_with_name(
        Arc::new(UnusedProvider),
        Some("pipe-log-test".into()),
    );
    let mut runtime = ServerRuntime::from_server(&server);
    runtime.ambient_runner = None;
    assert!(
        runtime
            .mcp_pool
            .set(Arc::new(crate::mcp::SharedMcpPool::new(
                crate::mcp::McpConfig::default(),
            )))
            .is_ok()
    );
    runtime.increment_client_count().await;
    let (server_stream, client_stream) = Stream::pair().expect("private transport pair");
    let handler_runtime = runtime.clone();
    let handler = tokio::spawn(async move {
        handler_runtime
            .run_client_stream(
                server_stream,
                "Client error",
                false,
                CancellationToken::new(),
            )
            .await;
    });
    let (reader, mut writer) = client_stream.into_split();
    let mut reader = BufReader::new(reader);
    writer
        .write_all(b"{\"type\":\"ping\",\"id\":1}\n")
        .await
        .expect("live ping");
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .expect("live pong deadline")
        .expect("live pong bytes");
    assert!(matches!(
        serde_json::from_str::<ServerEvent>(&line).expect("live pong"),
        ServerEvent::Pong { id: 1, .. }
    ));

    let reply_gate = runtime.client_connections.write().await;
    let mut requests = String::new();
    for id in 100..164 {
        let request = serde_json::json!({
            "type": "ping", "id": id, "private_payload": PRIVATE_PAYLOAD,
        })
        .to_string();
        assert!(matches!(decode_request(&request), Ok(Request::Ping { .. })));
        requests.push_str(&request);
        requests.push('\n');
    }
    writer
        .write_all(requests.as_bytes())
        .await
        .expect("queued control requests");
    writer
        .write_all(b"\n")
        .await
        .expect("queued transfer completion");
    drop(reader);
    drop(writer);
    drop(reply_gate);
    tokio::time::timeout(Duration::from_secs(10), handler)
        .await
        .expect("handler completion deadline")
        .expect("handler task");

    assert_eq!(*runtime.client_count.read().await, 0);
    assert!(runtime.client_connections.read().await.is_empty());
    assert!(runtime.sessions.read().await.is_empty());
    let log = std::fs::read_to_string(log_path).expect("actual runtime log");
    assert!(!log.contains(PRIVATE_PAYLOAD));
    let error = log
        .lines()
        .find(|line| line.contains("Client error: phase=initial_lightweight_control"))
        .expect("runtime must log the handler failure");
    assert!(error.contains("request_kind=ping"));
    assert!(error.contains("operation=write_direct_event"));
    assert!(error.contains("io_error_kind=Some("));
    let request_id = error
        .split("request_id=")
        .nth(1)
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse::<u64>().ok())
        .expect("numeric request correlation ID");
    assert!((100..164).contains(&request_id));
    std::fs::write(home.join("handler-log-verified"), error).expect("verified log receipt");
}
