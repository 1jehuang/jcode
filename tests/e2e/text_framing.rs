//! Real daemon + harness bridge sockets, with only provider inference scripted.
//! Guards against the bridge dropping message boundaries emitted by the agent.
#![cfg(unix)]

use crate::test_support::*;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

async fn send(writer: &mut tokio::net::unix::OwnedWriteHalf, frame: Value) -> Result<()> {
    writer.write_all(format!("{frame}\n").as_bytes()).await?;
    Ok(())
}

async fn receive(
    reader: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
) -> Result<Value> {
    let line = timeout(Duration::from_secs(15), reader.next_line())
        .await
        .context("harness frame deadline")??
        .context("harness disconnected")?;
    let frame: Value = serde_json::from_str(&line)?;
    anyhow::ensure!(frame["ev"] != "error", "harness error: {frame}");
    Ok(frame)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn harness_socket_frames_tools_reasoning_and_consecutive_assistant_messages() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime = tempfile::Builder::new().prefix("jc-frame-").tempdir()?;
    let daemon_socket = runtime.path().join("daemon.sock");
    let debug_socket = runtime.path().join("debug.sock");
    let api_socket = runtime.path().join("api.sock");
    let fixture = runtime.path().join("logs.txt");
    std::fs::write(&fixture, "retry loop failed")?;
    let provider = MockProvider::new();
    provider.queue_response(vec![
        StreamEvent::TextDelta("Checking the logs.".into()),
        StreamEvent::ToolUseStart {
            id: "read-logs".into(),
            name: "read".into(),
        },
        StreamEvent::ToolInputDelta(
            json!({"file_path": fixture, "intent": "Inspect test logs"}).to_string(),
        ),
        StreamEvent::ToolUseEnd,
        StreamEvent::MessageEnd {
            stop_reason: Some("tool_use".into()),
        },
    ]);
    provider.queue_response(vec![
        StreamEvent::TextDelta("Discard this failed attempt.".into()),
        StreamEvent::TextDone,
        StreamEvent::RetryRollback { attempt: 1, max: 3 },
        StreamEvent::TextDelta("The root cause is ".into()),
        StreamEvent::ThinkingDelta("checking the evidence".into()),
        StreamEvent::ThinkingDone { duration_secs: 0.1 },
        StreamEvent::TextDelta("the retry loop.".into()),
        StreamEvent::TextDone,
        StreamEvent::ThinkingDelta("considering the fix".into()),
        StreamEvent::TextDelta("Use a bounded retry.".into()),
        StreamEvent::TextDone,
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".into()),
        },
    ]);
    let server = server::Server::new_with_paths(
        Arc::new(provider),
        daemon_socket.clone(),
        debug_socket.clone(),
    );
    let daemon = tokio::spawn(async move { server.run().await });
    let bridge = tokio::spawn(jcode_harness_api_server::run_bridge(
        api_socket.clone(),
        daemon_socket.clone(),
    ));
    let result = async {
        wait_for_server_ready(&daemon_socket, &debug_socket).await?;
        let socket = timeout(Duration::from_secs(10), async {
            loop {
                match UnixStream::connect(&api_socket).await {
                    Ok(socket) => break socket,
                    Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            }
        }).await?;
        let (reader, mut writer) = socket.into_split();
        let mut reader = BufReader::new(reader).lines();
        send(&mut writer, json!({"v": 1, "id": 1, "req": "hello", "min_version": 1, "max_version": 1, "client": "text-framing-regression"})).await?;
        assert_eq!(receive(&mut reader).await?["ev"], "hello_ok");
        send(&mut writer, json!({"v": 1, "id": 2, "req": "create_session", "working_dir": runtime.path()})).await?;
        let session_id = loop {
            let frame = receive(&mut reader).await?;
            if frame["ev"] == "attached" {
                break frame["session"]["session_id"].as_str().context("attached session id")?.to_owned();
            }
        };
        send(&mut writer, json!({"v": 1, "id": 3, "req": "send_message", "session_id": session_id, "content": "Inspect the logs and explain the fix."})).await?;
        let mut messages = Vec::<(String, String)>::new();
        let mut completed = Vec::<String>::new();
        let mut saw_tool = false;
        loop {
            let frame = receive(&mut reader).await?;
            match frame["ev"].as_str() {
                Some("text_delta") => {
                    assert_eq!(frame["session_id"], session_id);
                    let id = frame["message_id"].as_str().context("delta missing message id")?;
                    let text = frame["text"].as_str().context("delta missing text")?;
                    if let Some((_, content)) = messages.iter_mut().find(|(key, _)| key == id) {
                        assert!(!completed.iter().any(|key| key == id), "delta after text_done");
                        content.push_str(text);
                    } else {
                        messages.push((id.to_owned(), text.to_owned()));
                    }
                }
                Some("text_done") => {
                    let id = frame["message_id"].as_str().context("text_done missing id")?;
                    assert!(!completed.iter().any(|key| key == id), "duplicate text_done");
                    assert!(messages.iter().any(|(key, _)| key == id), "phantom message");
                    completed.push(id.to_owned());
                }
                Some("text_replace") => {
                    let id = frame["message_id"].as_str().context("replacement missing id")?;
                    let (_, text) = messages.iter_mut().find(|(key, _)| key == id)
                        .context("replacement must refer to streamed text")?;
                    *text = frame["text"].as_str().context("replacement missing text")?.to_owned();
                }
                Some("tool_exec") => {
                    assert_eq!(completed.len(), 1, "narration must close before tool execution");
                    saw_tool = true;
                }
                Some("turn_done") => break,
                _ => {}
            }
        }
        assert!(saw_tool, "test must execute a real read tool");
        assert_eq!(messages.iter().filter(|(_, text)| !text.is_empty()).map(|(_, text)| text.as_str()).collect::<Vec<_>>(), vec![
            "Checking the logs.", "The root cause is the retry loop.", "Use a bounded retry.",
        ]);
        assert_eq!(completed, messages.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>());
        Ok(())
    }.await;
    bridge.abort();
    daemon.abort();
    let _ = bridge.await;
    let _ = daemon.await;
    result
}
