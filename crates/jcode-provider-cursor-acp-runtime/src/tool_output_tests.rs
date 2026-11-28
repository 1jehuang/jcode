#![cfg(unix)]
use super::*;

async fn process() -> AcpProcess {
    let command = CursorAcpCommand::new(
        "sh",
        [
            "-c",
            r#"
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"sessionId":"output-test"}}\n' "$id"
done
"#,
        ],
    );
    AcpProcess::spawn(&command, &std::env::current_dir().unwrap(), None, None)
        .await
        .unwrap()
}

fn text(text: &str) -> Value {
    json!([{"type":"content","content":{"type":"text","text":text}}])
}

async fn apply(process: &mut AcpProcess, update: Value) -> Vec<StreamEvent> {
    let (tx, mut rx) = mpsc::channel(16);
    process
        .handle_update(
            &json!({"sessionId":"output-test","update":update}),
            Some(&tx),
        )
        .await
        .unwrap();
    drop(tx);
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event.unwrap());
    }
    events
}

fn result(events: &[StreamEvent], expected: &str, error: bool) {
    assert_eq!(events.len(), 1, "{events:?}");
    match &events[0] {
        StreamEvent::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            assert_eq!(tool_use_id, "call-1");
            assert_eq!(content, expected);
            assert_eq!(*is_error, error);
        }
        event => panic!("unexpected event: {event:?}"),
    }
}

#[tokio::test]
async fn completed_standard_text_is_persisted() {
    let mut p = process().await;
    let events = apply(&mut p, json!({"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"completed","content":text("read complete")})).await;
    result(&events, "read complete", false);
    assert!(
        apply(&mut p, json!({"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"completed","content":text("duplicate")}))
            .await.is_empty()
    );
    p.shutdown().await;
}

#[tokio::test]
async fn progress_snapshot_survives_completion_without_content() {
    let mut p = process().await;
    for value in ["old snapshot", "latest snapshot"] {
        assert!(apply(&mut p, json!({"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"in_progress","content":text(value)})).await.is_empty());
    }
    assert!(
        apply(
            &mut p,
            json!({"sessionUpdate":"tool_call_update","toolCallId":"call-1","content":null})
        )
        .await
        .is_empty()
    );
    result(
        &apply(
            &mut p,
            json!({"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"completed"}),
        )
        .await,
        "latest snapshot",
        false,
    );
    p.shutdown().await;
}

#[tokio::test]
async fn failure_preserves_standard_error_content_once() {
    let mut p = process().await;
    let update = json!({"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"failed","content":text("permission denied")});
    result(
        &apply(&mut p, update.clone()).await,
        "permission denied",
        true,
    );
    assert!(apply(&mut p, update).await.is_empty());
    assert!(apply(&mut p, json!({"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"completed","content":text("late")})).await.is_empty());
    p.shutdown().await;
}

#[tokio::test]
async fn meaningful_raw_output_wins_and_nonzero_exit_is_error() {
    let mut p = process().await;
    result(&apply(&mut p, json!({"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"completed","rawOutput":{"stdout":"out","stderr":"err","exitCode":1},"content":text("other")})).await, "out\n[stderr]\nerr", true);
    p.shutdown().await;
}

#[tokio::test]
async fn empty_raw_outputs_do_not_hide_standard_text() {
    for raw in [
        Value::Null,
        json!(""),
        json!({}),
        json!([]),
        json!({"stdout":"","stderr":"","exitCode":0}),
    ] {
        let mut p = process().await;
        result(&apply(&mut p, json!({"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"completed","rawOutput":raw,"content":text("fallback")})).await, "fallback", false);
        p.shutdown().await;
    }
}

#[tokio::test]
async fn resources_and_diff_preserve_supplied_text_without_binary_data() {
    let mut p = process().await;
    let content = json!([
        {"type":"content","content":{"type":"resource","resource":{"uri":"file:///a","text":"body"}}},
        {"type":"content","content":{"type":"resource_link","uri":"file:///b","name":"b"}},
        {"type":"diff","path":"/a","oldText":"before","newText":"after"},
        {"type":"content","content":{"type":"image","mimeType":"image/png","data":"SECRET_BASE64"}},
        {"type":"content","content":{"type":"audio","mimeType":"audio/wav","data":"SECRET_AUDIO"}},
        {"type":"content","content":{"type":"resource","resource":{"uri":"file:///binary","blob":"SECRET_BLOB"}}},
        {"type":"terminal","terminalId":"terminal-1"}
    ]);
    result(&apply(&mut p, json!({"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"completed","content":content})).await, "file:///a\nbody\nfile:///b\n/a\noldText:\nbefore\nnewText:\nafter", false);
    p.shutdown().await;
}

#[tokio::test]
async fn explicit_empty_snapshot_clears_previous_content_without_inventing_success() {
    let mut p = process().await;
    apply(
        &mut p,
        json!({"sessionUpdate":"tool_call_update","toolCallId":"call-1","content":text("old")}),
    )
    .await;
    result(&apply(&mut p, json!({"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"completed","content":[]})).await, "", false);
    p.shutdown().await;
}

#[tokio::test]
async fn initial_content_and_raw_snapshots_are_retained_and_teardown_clears_them() {
    let mut p = process().await;
    apply(&mut p, json!({"sessionUpdate":"tool_call","toolCallId":"call-1","title":"Reading","kind":"read","content":text("initial"),"rawOutput":"raw"})).await;
    result(&apply(&mut p, json!({"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"completed","rawOutput":null})).await, "raw", false);
    p.new_session(&std::env::current_dir().unwrap())
        .await
        .unwrap();
    assert!(apply(&mut p, json!({"sessionUpdate":"tool_call_update","toolCallId":"call-1","content":text("unfinished")})).await.is_empty());
    p.shutdown().await;
    result(
        &apply(
            &mut p,
            json!({"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"completed"}),
        )
        .await,
        "",
        false,
    );
}
