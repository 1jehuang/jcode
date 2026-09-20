use super::*;
use futures::{FutureExt, StreamExt, channel::mpsc};
use serde_json::json;

type Sender = mpsc::UnboundedSender<Result<Bytes, reqwest::Error>>;

fn stream() -> (Sender, OpenAIResponsesStream) {
    let (tx, rx) = mpsc::unbounded();
    (tx, OpenAIResponsesStream::new(rx))
}

fn send(tx: &Sender, event: Value) {
    tx.unbounded_send(Ok(Bytes::from(format!("data: {event}\n\n"))))
        .unwrap();
}

fn next(stream: &mut OpenAIResponsesStream) -> StreamEvent {
    // The upstream connection remains open, with no later event supplied. A
    // buffering parser returns Pending here instead of passing this assertion.
    stream
        .next()
        .now_or_never()
        .expect("event must be visible now")
        .expect("stream remains open")
        .expect("valid stream event")
}

fn idle(stream: &mut OpenAIResponsesStream) {
    assert!(stream.next().now_or_never().is_none());
}

fn added(tx: &Sender, id: &str, name: &str) {
    send(
        tx,
        json!({"type":"response.output_item.added", "item": {
            "type":"function_call", "id":id, "call_id":format!("call_{id}"),
            "name":name, "arguments":""
        }}),
    );
}

fn delta(tx: &Sender, id: &str, fragment: &str) {
    send(
        tx,
        json!({"type":"response.function_call_arguments.delta", "item_id":id, "delta":fragment}),
    );
}

fn done(tx: &Sender, id: &str, arguments: &str) {
    send(
        tx,
        json!({"type":"response.function_call_arguments.done", "item_id":id, "arguments":arguments}),
    );
}

fn assert_start(stream: &mut OpenAIResponsesStream, id: &str, name: &str) {
    assert!(
        matches!(next(stream), StreamEvent::ToolUseStart { id: actual_id, name: actual_name }
        if actual_id == format!("call_{id}") && actual_name == name)
    );
}

fn assert_delta(stream: &mut OpenAIResponsesStream, id: &str, expected: &str) {
    assert!(
        matches!(next(stream), StreamEvent::ToolInputDeltaFor { id: actual_id, delta }
        if actual_id == format!("call_{id}") && delta == expected)
    );
}

fn assert_end(stream: &mut OpenAIResponsesStream, id: &str) {
    assert!(
        matches!(next(stream), StreamEvent::ToolUseEndFor { id: actual_id }
        if actual_id == format!("call_{id}"))
    );
}

#[test]
fn tool_name_and_argument_fragments_are_visible_before_done() {
    for name in ["read", "batch", "multi_tool_use.parallel"] {
        let (tx, mut stream) = stream();
        added(&tx, "a", name);
        assert_start(&mut stream, "a", name);
        idle(&mut stream);
        delta(&tx, "a", "{\"intent\":\"Read files\",");
        assert_delta(&mut stream, "a", "{\"intent\":\"Read files\",");
        idle(&mut stream);
        delta(&tx, "a", "\"path\":\"文档\"}");
        assert_delta(&mut stream, "a", "\"path\":\"文档\"}");
        idle(&mut stream);
        done(&tx, "a", "{\"intent\":\"Read files\",\"path\":\"文档\"}");
        assert_end(&mut stream, "a");
        idle(&mut stream);
    }
}

#[test]
fn done_snapshots_only_emit_unseen_suffix_and_never_duplicate_calls() {
    let (tx, mut stream) = stream();
    added(&tx, "a", "read");
    assert_start(&mut stream, "a", "read");
    delta(&tx, "a", "{\"path\":");
    assert_delta(&mut stream, "a", "{\"path\":");
    done(&tx, "a", "{\"path\":\"README.md\"}");
    assert_delta(&mut stream, "a", "\"README.md\"}");
    assert_end(&mut stream, "a");
    for _ in 0..2 {
        done(&tx, "a", "{\"path\":\"README.md\"}");
        send(
            &tx,
            json!({"type":"response.output_item.done", "item":{
                "id":"a", "type":"function_call", "call_id":"call_a", "name":"read",
                "arguments":"{\"path\":\"README.md\"}"
            }}),
        );
        idle(&mut stream);
    }
}

#[test]
fn output_item_done_finishes_started_call_without_arguments_done() {
    let (tx, mut stream) = stream();
    added(&tx, "a", "read");
    assert_start(&mut stream, "a", "read");
    delta(&tx, "a", "{");
    assert_delta(&mut stream, "a", "{");
    send(
        &tx,
        json!({"type":"response.output_item.done", "item":{
            "id":"a", "type":"function_call", "call_id":"call_a", "name":"read", "arguments":"{}"
        }}),
    );
    assert_delta(&mut stream, "a", "}");
    assert_end(&mut stream, "a");
    idle(&mut stream);
    assert!(stream.streaming_tool_calls.is_empty());
}

#[test]
fn stalled_first_call_does_not_block_other_names_arguments_or_completion() {
    let (tx, mut stream) = stream();
    added(&tx, "a", "read");
    assert_start(&mut stream, "a", "read");
    idle(&mut stream);
    added(&tx, "b", "bash");
    assert_start(&mut stream, "b", "bash");
    idle(&mut stream);
    delta(&tx, "b", "{\"command\":");
    assert_delta(&mut stream, "b", "{\"command\":");
    added(&tx, "c", "ls");
    assert_start(&mut stream, "c", "ls");
    idle(&mut stream);
    delta(&tx, "a", "{\"path\":");
    assert_delta(&mut stream, "a", "{\"path\":");
    done(&tx, "c", "");
    assert_delta(&mut stream, "c", "{}");
    assert_end(&mut stream, "c");
    done(&tx, "b", "{\"command\":\"pwd\"}");
    assert_delta(&mut stream, "b", "\"pwd\"}");
    assert_end(&mut stream, "b");
    idle(&mut stream);
    done(&tx, "a", "{\"path\":\"README.md\"}");
    assert_delta(&mut stream, "a", "\"README.md\"}");
    assert_end(&mut stream, "a");
    idle(&mut stream);
}

#[test]
fn emitted_id_stays_stable_when_call_id_arrives_late() {
    let (tx, mut stream) = stream();
    send(
        &tx,
        json!({"type":"response.output_item.added", "item":{
            "type":"function_call", "id":"item.a", "name":"read", "arguments":""
        }}),
    );
    let StreamEvent::ToolUseStart { id, .. } = next(&mut stream) else {
        panic!("expected start")
    };
    assert_eq!(id, sanitize_tool_id("item.a"));
    send(
        &tx,
        json!({"type":"response.function_call_arguments.delta", "item_id":"item.a",
        "call_id":"late_call", "delta":"{"}),
    );
    assert!(
        matches!(next(&mut stream), StreamEvent::ToolInputDeltaFor { id: actual, delta }
        if actual == id && delta == "{")
    );
    send(
        &tx,
        json!({"type":"response.output_item.done", "item":{
            "type":"function_call", "id":"item.a", "call_id":"different_call", "name":"read", "arguments":"{}"
        }}),
    );
    assert!(
        matches!(next(&mut stream), StreamEvent::ToolInputDeltaFor { id: actual, delta }
        if actual == id && delta == "}")
    );
    assert!(matches!(next(&mut stream), StreamEvent::ToolUseEndFor { id: actual } if actual == id));
    idle(&mut stream);
}

#[test]
fn late_name_releases_accumulated_arguments_without_waiting_for_done() {
    let (tx, mut stream) = stream();
    delta(&tx, "a", "{");
    idle(&mut stream);
    send(
        &tx,
        json!({"type":"response.function_call_arguments.delta", "item_id":"a",
        "call_id":"call_a", "name":"read", "delta":"\"path\":"}),
    );
    assert_start(&mut stream, "a", "read");
    assert_delta(&mut stream, "a", "{\"path\":");
    idle(&mut stream);
}

#[test]
fn done_only_call_keeps_compatibility() {
    let (tx, mut stream) = stream();
    send(
        &tx,
        json!({"type":"response.function_call_arguments.done", "item_id":"a",
        "call_id":"call_a", "name":"read", "arguments":"{}"}),
    );
    assert_start(&mut stream, "a", "read");
    assert_delta(&mut stream, "a", "{}");
    assert_end(&mut stream, "a");
    idle(&mut stream);
}

#[test]
fn null_and_empty_arguments_are_normalized_without_delaying_start() {
    for arguments in ["", " ", "null", " null "] {
        let (tx, mut stream) = stream();
        added(&tx, "a", "ls");
        assert_start(&mut stream, "a", "ls");
        for ch in arguments.chars() {
            delta(&tx, "a", &ch.to_string());
            idle(&mut stream);
        }
        done(&tx, "a", arguments);
        assert_delta(&mut stream, "a", "{}");
        assert_end(&mut stream, "a");
        idle(&mut stream);
    }
}

#[test]
fn mismatched_done_arguments_fail_instead_of_corrupting_tool_input() {
    let (tx, mut stream) = stream();
    added(&tx, "a", "read");
    assert_start(&mut stream, "a", "read");
    delta(&tx, "a", "{\"path\":\"文档");
    assert_delta(&mut stream, "a", "{\"path\":\"文档");
    done(&tx, "a", "{}");
    assert!(matches!(next(&mut stream), StreamEvent::Error { .. }));
    idle(&mut stream);
}

#[test]
fn custom_tool_input_events_stream_before_completion() {
    let (tx, mut stream) = stream();
    send(
        &tx,
        json!({"type":"response.output_item.added", "item":{
            "id":"a", "type":"custom_tool_call", "call_id":"call_a", "name":"apply_patch", "input":""
        }}),
    );
    assert_start(&mut stream, "a", "apply_patch");
    send(
        &tx,
        json!({"type":"response.custom_tool_call_input.delta", "item_id":"a", "delta":"*** Begin Patch\n"}),
    );
    assert_delta(&mut stream, "a", "*** Begin Patch\n");
    idle(&mut stream);
    send(
        &tx,
        json!({"type":"response.custom_tool_call_input.done", "item_id":"a", "input":"*** Begin Patch\n*** End Patch"}),
    );
    assert_delta(&mut stream, "a", "*** End Patch");
    assert_end(&mut stream, "a");
    idle(&mut stream);
}

#[test]
fn unnamed_first_call_does_not_block_a_named_call() {
    let (tx, mut stream) = stream();
    delta(&tx, "a", "{");
    idle(&mut stream);
    added(&tx, "b", "ls");
    assert_start(&mut stream, "b", "ls");
    done(&tx, "b", "{}");
    assert_delta(&mut stream, "b", "{}");
    assert_end(&mut stream, "b");
    idle(&mut stream);
    send(
        &tx,
        json!({"type":"response.function_call_arguments.done", "item_id":"a",
        "call_id":"call_a", "name":"read", "arguments":"{}"}),
    );
    assert_start(&mut stream, "a", "read");
    assert_delta(&mut stream, "a", "{}");
    assert_end(&mut stream, "a");
    idle(&mut stream);
}

#[test]
fn recovered_tool_call_uses_its_own_id_while_another_call_is_open() {
    let (tx, mut stream) = stream();
    added(&tx, "a", "read");
    assert_start(&mut stream, "a", "read");
    send(
        &tx,
        json!({"type":"response.output_text.delta",
        "delta":"to=functions.ls {}"}),
    );
    let StreamEvent::ToolUseStart { id, name } = next(&mut stream) else {
        panic!("expected recovered tool start");
    };
    assert_eq!(name, "ls");
    assert!(id.starts_with("fallback_text_call_"));
    assert!(
        matches!(next(&mut stream), StreamEvent::ToolInputDeltaFor { id: actual, delta }
        if actual == id && delta == "{}")
    );
    assert!(matches!(next(&mut stream), StreamEvent::ToolUseEndFor { id: actual } if actual == id));
    done(&tx, "a", "{}");
    assert_delta(&mut stream, "a", "{}");
    assert_end(&mut stream, "a");
    idle(&mut stream);
}
