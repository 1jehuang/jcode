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

fn assert_delta(stream: &mut OpenAIResponsesStream, expected: &str) {
    assert!(matches!(next(stream), StreamEvent::ToolInputDelta(actual) if actual == expected));
}

#[test]
fn tool_name_and_argument_fragments_are_visible_before_done() {
    for name in ["read", "batch", "multi_tool_use.parallel"] {
        let (tx, mut stream) = stream();
        added(&tx, "a", name);
        assert_start(&mut stream, "a", name);
        idle(&mut stream);
        delta(&tx, "a", "{\"intent\":\"Read files\",");
        assert_delta(&mut stream, "{\"intent\":\"Read files\",");
        idle(&mut stream);
        delta(&tx, "a", "\"path\":\"文档\"}");
        assert_delta(&mut stream, "\"path\":\"文档\"}");
        idle(&mut stream);
        done(&tx, "a", "{\"intent\":\"Read files\",\"path\":\"文档\"}");
        assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
        idle(&mut stream);
    }
}

#[test]
fn done_snapshots_only_emit_unseen_suffix_and_never_duplicate_calls() {
    let (tx, mut stream) = stream();
    added(&tx, "a", "read");
    assert_start(&mut stream, "a", "read");
    delta(&tx, "a", "{\"path\":");
    assert_delta(&mut stream, "{\"path\":");
    done(&tx, "a", "{\"path\":\"README.md\"}");
    assert_delta(&mut stream, "\"README.md\"}");
    assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
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
    assert_delta(&mut stream, "{");
    send(
        &tx,
        json!({"type":"response.output_item.done", "item":{
            "id":"a", "type":"function_call", "call_id":"call_a", "name":"read", "arguments":"{}"
        }}),
    );
    assert_delta(&mut stream, "}");
    assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
    idle(&mut stream);
    assert!(stream.streaming_tool_calls.is_empty());
}

#[test]
fn interleaved_calls_keep_unkeyed_deltas_attached_to_their_own_start() {
    let (tx, mut stream) = stream();
    added(&tx, "a", "read");
    assert_start(&mut stream, "a", "read");
    delta(&tx, "a", "{\"path\":");
    assert_delta(&mut stream, "{\"path\":");
    added(&tx, "b", "bash");
    delta(&tx, "b", "{\"command\":\"pwd\"}");
    done(&tx, "b", "{\"command\":\"pwd\"}");
    added(&tx, "c", "ls");
    delta(&tx, "c", "{");
    idle(&mut stream);
    done(&tx, "a", "{\"path\":\"README.md\"}");
    assert_delta(&mut stream, "\"README.md\"}");
    assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
    assert_start(&mut stream, "b", "bash");
    assert_delta(&mut stream, "{\"command\":\"pwd\"}");
    assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
    // The next still-incomplete tool becomes visible immediately when the
    // single-current-tool protocol allows it, not when its own arguments end.
    assert_start(&mut stream, "c", "ls");
    assert_delta(&mut stream, "{");
    idle(&mut stream);
    done(&tx, "c", "{}");
    assert_delta(&mut stream, "}");
    assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
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
    assert_delta(&mut stream, "{\"path\":");
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
    assert_delta(&mut stream, "{}");
    assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
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
        assert_delta(&mut stream, "{}");
        assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
        idle(&mut stream);
    }
}

#[test]
fn mismatched_done_arguments_fail_instead_of_corrupting_tool_input() {
    let (tx, mut stream) = stream();
    added(&tx, "a", "read");
    assert_start(&mut stream, "a", "read");
    delta(&tx, "a", "{\"path\":\"文档");
    assert_delta(&mut stream, "{\"path\":\"文档");
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
    assert_delta(&mut stream, "*** Begin Patch\n");
    idle(&mut stream);
    send(
        &tx,
        json!({"type":"response.custom_tool_call_input.done", "item_id":"a", "input":"*** Begin Patch\n*** End Patch"}),
    );
    assert_delta(&mut stream, "*** End Patch");
    assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
    idle(&mut stream);
}

// Match the Copilot Responses shape: every event has a fresh item ID, while
// output_index is stable and call_id is supplied only on item events.
fn indexed_item(tx: &Sender, index: u64, id: &str, custom: bool, complete: bool, input: &str) {
    let mut item = json!({
        "type": if custom { "custom_tool_call" } else { "function_call" },
        "id": id, "call_id": format!("call_{index}"),
        "name": if custom { "apply_patch" } else { "read" },
    });
    item[if custom { "input" } else { "arguments" }] = json!(input);
    send(
        tx,
        json!({
            "type": if complete { "response.output_item.done" } else { "response.output_item.added" },
            "output_index": index, "item": item,
        }),
    );
}

fn indexed_arguments(tx: &Sender, index: u64, id: &str, custom: bool, complete: bool, input: &str) {
    let kind = if custom {
        "custom_tool_call_input"
    } else {
        "function_call_arguments"
    };
    let mut event = json!({
        "type": format!("response.{kind}.{}", if complete { "done" } else { "delta" }),
        "output_index": index, "item_id": id,
    });
    event[if !complete {
        "delta"
    } else if custom {
        "input"
    } else {
        "arguments"
    }] = json!(input);
    send(tx, event);
}

#[test]
fn unstable_item_ids_stream_incrementally_and_deduplicate_done_snapshots() {
    for custom in [false, true] {
        for item_done_only in [false, true] {
            let (tx, mut stream) = stream();
            let name = if custom { "apply_patch" } else { "read" };
            let fragments = if custom {
                vec!["*** Begin Patch\n", "*** End", " Patch"]
            } else {
                vec!["{\"path\":", "\"文档", "\"}"]
            };
            let full = fragments.concat();
            indexed_item(&tx, 0, "added", custom, false, "");
            assert_start(&mut stream, "0", name);
            idle(&mut stream);
            for (i, fragment) in fragments[..2].iter().enumerate() {
                indexed_arguments(&tx, 0, &format!("delta_{i}"), custom, false, fragment);
                assert_delta(&mut stream, fragment);
                idle(&mut stream);
            }
            if item_done_only {
                indexed_item(&tx, 0, "item_done", custom, true, &full);
            } else {
                indexed_arguments(&tx, 0, "arguments_done", custom, true, &full);
            }
            assert_delta(&mut stream, fragments[2]);
            assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
            indexed_item(&tx, 0, "another_item_done", custom, true, &full);
            indexed_arguments(&tx, 0, "another_arguments_done", custom, true, &full);
            idle(&mut stream);
            assert!(stream.streaming_tool_calls.is_empty());
        }
    }
}

#[test]
fn unstable_item_ids_keep_interleaved_calls_separate() {
    let (tx, mut stream) = stream();
    indexed_item(&tx, 0, "a_added", false, false, "");
    assert_start(&mut stream, "0", "read");
    indexed_arguments(&tx, 0, "a_delta", false, false, "{");
    assert_delta(&mut stream, "{");
    indexed_item(&tx, 1, "b_added", true, false, "");
    indexed_arguments(&tx, 1, "b_delta", true, false, "*** Begin Patch\n");
    indexed_arguments(
        &tx,
        1,
        "b_done",
        true,
        true,
        "*** Begin Patch\n*** End Patch",
    );
    indexed_item(
        &tx,
        1,
        "b_item_done",
        true,
        true,
        "*** Begin Patch\n*** End Patch",
    );
    idle(&mut stream);
    indexed_item(&tx, 0, "a_item_done", false, true, "{}");
    assert_delta(&mut stream, "}");
    assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
    assert_start(&mut stream, "1", "apply_patch");
    assert_delta(&mut stream, "*** Begin Patch\n*** End Patch");
    assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
    indexed_item(&tx, 0, "a_duplicate", false, true, "{}");
    indexed_item(
        &tx,
        1,
        "b_duplicate",
        true,
        true,
        "*** Begin Patch\n*** End Patch",
    );
    idle(&mut stream);
    assert!(stream.streaming_tool_calls.is_empty());
}

#[test]
fn call_id_alias_finishes_call_and_deduplicates_without_output_index() {
    let (tx, mut stream) = stream();
    added(&tx, "a", "read");
    assert_start(&mut stream, "a", "read");
    delta(&tx, "a", "{");
    assert_delta(&mut stream, "{");
    for id in ["new", "newer"] {
        send(
            &tx,
            json!({"type":"response.output_item.done", "item":{
                "type":"function_call", "id":id, "call_id":"call_a", "name":"read", "arguments":"{}"
            }}),
        );
        if id == "new" {
            assert_delta(&mut stream, "}");
            assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
        }
        idle(&mut stream);
    }
}

#[test]
fn item_id_alias_handles_output_index_appearing_after_start() {
    let (tx, mut stream) = stream();
    added(&tx, "a", "read");
    assert_start(&mut stream, "a", "read");
    indexed_arguments(&tx, 2, "a", false, false, "{");
    assert_delta(&mut stream, "{");
    indexed_arguments(&tx, 2, "changed", false, true, "{}");
    assert_delta(&mut stream, "}");
    assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
    done(&tx, "a", "{}");
    idle(&mut stream);
}

#[test]
fn indexed_call_preserves_item_id_fallback_without_call_id() {
    let (tx, mut stream) = stream();
    send(
        &tx,
        json!({"type":"response.output_item.added", "output_index":0, "item":{
            "type":"function_call", "id":"original", "name":"read", "arguments":""
        }}),
    );
    assert!(
        matches!(next(&mut stream), StreamEvent::ToolUseStart { id, name }
        if id == "original" && name == "read")
    );
    indexed_arguments(&tx, 0, "changed", false, false, "{");
    assert_delta(&mut stream, "{");
    indexed_arguments(&tx, 0, "changed_again", false, true, "{}");
    assert_delta(&mut stream, "}");
    assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
    idle(&mut stream);
}

#[test]
fn blank_identities_do_not_merge_distinct_indexed_calls() {
    for blank in ["", " \t "] {
        for interleaved in [false, true] {
            for item_id in [None, Some(blank), Some("present")] {
                let (tx, mut stream) = stream();
                let add = |index: u64| {
                    let mut item = json!({"type":"function_call", "call_id":blank,
                        "name":"read", "arguments":""});
                    if let Some(id) = item_id {
                        item["id"] = json!(if id == "present" {
                            format!("item_{index}")
                        } else {
                            id.to_string()
                        });
                    }
                    send(
                        &tx,
                        json!({"type":"response.output_item.added", "output_index":index, "item":item}),
                    );
                };
                let finish = |index: u64| {
                    send(
                        &tx,
                        json!({"type":"response.function_call_arguments.done",
                        "output_index":index, "item_id":blank, "call_id":blank, "arguments":"{}"}),
                    );
                };
                let start_id = |stream: &mut OpenAIResponsesStream| match next(stream) {
                    StreamEvent::ToolUseStart { id, name } => {
                        assert_eq!(name, "read");
                        assert!(!id.trim().is_empty());
                        id
                    }
                    event => panic!("expected distinct tool start, got {event:?}"),
                };
                add(0);
                let first_id = start_id(&mut stream);
                if interleaved {
                    add(1);
                    finish(1);
                    idle(&mut stream);
                }
                finish(0);
                assert_delta(&mut stream, "{}");
                assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
                if !interleaved {
                    add(1);
                }
                let second_id = start_id(&mut stream);
                assert_ne!(first_id, second_id);
                if item_id == Some("present") {
                    assert_eq!(first_id, "item_0");
                    assert_eq!(second_id, "item_1");
                }
                if !interleaved {
                    finish(1);
                }
                assert_delta(&mut stream, "{}");
                assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
                idle(&mut stream);
                assert!(stream.streaming_tool_calls.is_empty());
            }
        }
    }
}

#[test]
fn blank_snapshot_call_id_does_not_replace_buffered_stable_id() {
    let (tx, mut stream) = stream();
    indexed_item(&tx, 0, "a", false, false, "");
    assert_start(&mut stream, "0", "read");
    indexed_item(&tx, 1, "b", false, false, "");
    send(
        &tx,
        json!({"type":"response.output_item.done", "output_index":1, "item":{
            "type":"function_call", "id":"b_done", "call_id":" \t ", "name":"read", "arguments":"{}"
        }}),
    );
    idle(&mut stream);
    indexed_arguments(&tx, 0, "a_done", false, true, "{}");
    assert_delta(&mut stream, "{}");
    assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
    assert_start(&mut stream, "1", "read");
    assert_delta(&mut stream, "{}");
    assert!(matches!(next(&mut stream), StreamEvent::ToolUseEnd));
    idle(&mut stream);
}
