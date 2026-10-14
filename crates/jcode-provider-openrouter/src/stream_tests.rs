use super::*;
use futures::StreamExt;

fn drain_text(stream: &mut OpenRouterStream) -> String {
    let mut text = String::new();
    while let Some(event) = stream.parse_next_event() {
        match event {
            StreamEvent::TextDelta(delta) => text.push_str(&delta),
            StreamEvent::MessageEnd { .. } => break,
            _ => {}
        }
    }
    text
}

fn test_stream() -> OpenRouterStream {
    OpenRouterStream::new(
        futures::stream::empty(),
        "test-model".to_string(),
        Arc::new(std::sync::Mutex::new(None)),
    )
}

#[test]
fn take_sse_event_splits_crlf_delimited_events() {
    let mut buffer = "data: a\r\n\r\ndata: b\r\n\r\n".to_string();
    assert_eq!(take_sse_event(&mut buffer).as_deref(), Some("data: a"));
    assert_eq!(take_sse_event(&mut buffer).as_deref(), Some("data: b"));
    assert_eq!(take_sse_event(&mut buffer), None);
}

#[test]
fn parse_next_event_keeps_all_content_across_crlf_batched_events() {
    let mut stream = test_stream();
    stream.buffer = [
        "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}",
        "data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}",
        "data: {\"choices\":[{\"delta\":{\"content\":\"!\"}}]}",
        "data: [DONE]",
        "",
    ]
    .join("\r\n\r\n");

    assert_eq!(drain_text(&mut stream), "hello world!");
}

#[test]
fn parse_next_event_keeps_all_data_lines_within_one_event() {
    // Several data: lines inside one \n\n-delimited block must all be kept.
    let mut stream = test_stream();
    stream.buffer = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"foo\"}}]}\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"bar\"}}]}\n",
        "data: [DONE]\n\n"
    )
    .to_string();

    assert_eq!(drain_text(&mut stream), "foobar");
}

/// Issue #609: proxies that drop the event separator, split an object across
/// two events, or split a multi-byte character across TCP chunks must not
/// cause silent data loss.
#[test]
fn concatenated_json_in_one_event_keeps_both_deltas() {
    let mut stream = test_stream();
    stream.buffer = concat!(
        r#"data: {"choices":[{"delta":{"content":"hello"}}]}"#,
        r#"{"choices":[{"delta":{"content":" world"}}]}"#,
        "\n\ndata: [DONE]\n\n"
    )
    .to_string();

    assert_eq!(drain_text(&mut stream), "hello world");
}

#[test]
fn concatenated_json_with_embedded_data_prefix_keeps_both_deltas() {
    let mut stream = test_stream();
    stream.buffer = concat!(
        r#"data: {"choices":[{"delta":{"content":"hello"}}]}"#,
        r#"data: {"choices":[{"delta":{"content":" world"}}]}"#,
        "\n\ndata: [DONE]\n\n"
    )
    .to_string();

    assert_eq!(drain_text(&mut stream), "hello world");
}

#[test]
fn object_split_across_two_events_is_rejoined() {
    let mut stream = test_stream();
    stream.buffer = concat!(
        r#"data: {"choices":[{"delta":{"content":"Hello "#,
        "\n\n",
        r#"data: world"}}]}"#,
        "\n\ndata: [DONE]\n\n"
    )
    .to_string();

    assert_eq!(drain_text(&mut stream), "Hello world");
}

#[test]
fn tool_call_arguments_split_across_events_are_not_truncated() {
    // The reported symptom was `arguments must be a JSON object, got null`.
    let mut stream = test_stream();
    stream.buffer = concat!(
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"write","arguments":"{\"path\":\"a.txt\""#,
        "\n\n",
        r#"data: ,\"content\":\"hi\"}"}}]}}]}"#,
        "\n\ndata: [DONE]\n\n"
    )
    .to_string();

    let mut args = String::new();
    while let Some(event) = stream.parse_next_event() {
        if let StreamEvent::ToolInputDeltaFor { delta, .. } = event {
            args.push_str(&delta);
        }
    }
    let parsed: Value =
        serde_json::from_str(&args).expect("tool arguments should be complete JSON");
    assert_eq!(parsed["path"], "a.txt");
    assert_eq!(parsed["content"], "hi");
}

#[test]
fn tool_call_markup_inside_structured_arguments_is_inert() {
    // #1702: a `write` whose content contains literal XML/DSML tool-call
    // markup must reach the tool byte-for-byte. jcode must never scan
    // structured argument JSON for text-form tool calls.
    let content = "before\n<invoke name=\"bash\">\n<parameter name=\"command\">ls</parameter>\n\
                   <parameter name=\"intent\">x</DSML parameter>\n</invoke>\n\
                   </function_calls>\nto=functions.bash {\"command\":\"rm\"}\n+#+#\nafter";
    let full_args = serde_json::json!({"file_path": "doc.md", "content": content}).to_string();
    // Split mid-markup so the markup straddles SSE event boundaries.
    let split = full_args.find("parameter name").unwrap() + 4;
    let (first, second) = full_args.split_at(split);
    let event = |id: Option<&str>, args: &str| {
        let mut call = serde_json::json!({
            "index": 0,
            "function": {"arguments": args}
        });
        if let Some(id) = id {
            call["id"] = serde_json::json!(id);
            call["function"]["name"] = serde_json::json!("write");
        }
        serde_json::json!({"choices": [{"delta": {"tool_calls": [call]}}]})
    };
    let mut stream = test_stream();
    stream.buffer = format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        event(Some("call_1"), first),
        event(None, second)
    );

    let mut args = String::new();
    let mut starts = 0;
    let mut text = String::new();
    while let Some(event) = stream.parse_next_event() {
        match event {
            StreamEvent::ToolUseStart { .. } => starts += 1,
            StreamEvent::ToolInputDeltaFor { delta, .. } => args.push_str(&delta),
            StreamEvent::TextDelta(delta) => text.push_str(&delta),
            _ => {}
        }
    }
    assert_eq!(starts, 1, "markup must not spawn extra tool calls");
    assert!(text.is_empty(), "markup must not leak into assistant text");
    assert_eq!(args, full_args);
    let input = jcode_message_types::ToolCall::parse_streamed_input_to_object(&args);
    assert_eq!(input["content"], content);
}

#[test]
fn non_string_tool_arguments_are_preserved_for_validation() {
    for arguments in [
        serde_json::json!({"file_path": "server.py", "content": "print('hi')"}),
        serde_json::json!(null),
        serde_json::json!(["not", "an", "object"]),
        serde_json::json!(42),
        serde_json::json!(false),
    ] {
        let mut stream = test_stream();
        let event = serde_json::json!({"choices": [{"delta": {"tool_calls": [{
            "index": 0, "id": "call_1", "type": "function",
            "function": {"name": "write", "arguments": arguments}
        }]}}]});
        stream.buffer = format!("data: {event}\n\ndata: [DONE]\n\n");
        let mut received = String::new();
        while let Some(event) = stream.parse_next_event() {
            if let StreamEvent::ToolInputDeltaFor { delta, .. } = event {
                received.push_str(&delta);
            }
        }
        let parsed: Value = serde_json::from_str(&received)
            .expect("non-string arguments must not be silently dropped");
        assert_eq!(parsed, arguments);
    }
}

#[test]
fn multibyte_chars_split_across_tcp_chunks_survive_poll_next() {
    // Drive real bytes through poll_next, splitting mid-character.
    let payload =
        "data: {\"choices\":[{\"delta\":{\"content\":\"读取文件\"}}]}\n\ndata: [DONE]\n\n";
    let bytes = payload.as_bytes();
    // Split at every offset to sweep the chunk-boundary state space.
    for split in 0..bytes.len() {
        let chunks: Vec<Result<Bytes, reqwest::Error>> = vec![
            Ok(Bytes::copy_from_slice(&bytes[..split])),
            Ok(Bytes::copy_from_slice(&bytes[split..])),
        ];
        let mut stream = OpenRouterStream::new(
            futures::stream::iter(chunks),
            "test-model".to_string(),
            Arc::new(std::sync::Mutex::new(None)),
        );
        let text = futures::executor::block_on(async {
            let mut text = String::new();
            while let Some(Ok(event)) = stream.next().await {
                if let StreamEvent::TextDelta(delta) = event {
                    text.push_str(&delta);
                }
            }
            text
        });
        assert_eq!(text, "读取文件", "lost content at split offset {split}");
    }
}

#[test]
fn stream_ending_without_a_blank_line_still_flushes_the_last_event() {
    let payload = "data: {\"choices\":[{\"delta\":{\"content\":\"tail\"}}]}";
    let chunks: Vec<Result<Bytes, reqwest::Error>> =
        vec![Ok(Bytes::copy_from_slice(payload.as_bytes()))];
    let mut stream = OpenRouterStream::new(
        futures::stream::iter(chunks),
        "test-model".to_string(),
        Arc::new(std::sync::Mutex::new(None)),
    );
    let text = futures::executor::block_on(async {
        let mut text = String::new();
        while let Some(Ok(event)) = stream.next().await {
            if let StreamEvent::TextDelta(delta) = event {
                text.push_str(&delta);
            }
        }
        text
    });
    assert_eq!(text, "tail");
}

#[test]
fn parse_next_event_ignores_malformed_json_chunks() {
    let provider_pin = Arc::new(std::sync::Mutex::new(None));
    let mut stream = OpenRouterStream::new(
        futures::stream::empty(),
        "test-model".to_string(),
        provider_pin,
    );
    stream.buffer = "data: {not-json}

"
    .to_string();

    let event = stream.parse_next_event();

    assert!(event.is_none());
    assert!(stream.pending.is_empty());
    assert!(stream.tool_call_accumulators.is_empty());
}

fn drain_reasoning(stream: &mut OpenRouterStream) -> String {
    let mut reasoning = String::new();
    while let Some(event) = stream.parse_next_event() {
        match event {
            StreamEvent::ThinkingDelta(delta) => reasoning.push_str(&delta),
            StreamEvent::MessageEnd { .. } => break,
            _ => {}
        }
    }
    reasoning
}

/// Feed `chunks` as one reasoning SSE event each and return the assembled
/// stream, ready to drain.
fn reasoning_stream(model: &str, field: &str, chunks: &[&str]) -> OpenRouterStream {
    let mut stream = OpenRouterStream::new(
        futures::stream::empty(),
        model.to_string(),
        Arc::new(std::sync::Mutex::new(None)),
    );
    let mut sse = String::new();
    for chunk in chunks {
        let event = serde_json::json!({"choices": [{"delta": {field: chunk}}]});
        sse.push_str(&format!("data: {event}\n\n"));
    }
    sse.push_str("data: [DONE]\n\n");
    stream.buffer = sse;
    stream
}

/// Issue #1779: incremental reasoning backends (llama.cpp, Qwen tokenizers)
/// emit each token as its own delta. A delta equal to the previous one must
/// still be appended; the unguarded snapshot strip silently emptied it, so
/// "9","4","4","0" was stored as "940".
#[test]
fn reasoning_deltas_keep_adjacent_identical_chunks() {
    let mut stream = reasoning_stream("qwen3", "reasoning_content", &["9", "4", "4", "0"]);
    assert_eq!(drain_reasoning(&mut stream), "9440");
}

#[test]
fn reasoning_deltas_keep_repeated_words() {
    // The reported "that that" and "had had" corruptions.
    let mut stream = reasoning_stream(
        "qwen3",
        "reasoning_content",
        &["I", " knew", " that", " that", " would", " fail", "."],
    );
    assert_eq!(drain_reasoning(&mut stream), "I knew that that would fail.");

    let mut stream = reasoning_stream(
        "qwen3",
        "reasoning_content",
        &["She", " had", " had", " enough", "."],
    );
    assert_eq!(drain_reasoning(&mut stream), "She had had enough.");
}

#[test]
fn reasoning_alias_deltas_keep_adjacent_identical_chunks() {
    let mut stream = reasoning_stream("test-model", "reasoning", &["1", "0", "0", "0"]);
    assert_eq!(drain_reasoning(&mut stream), "1000");
}

#[test]
fn reasoning_delta_that_extends_the_previous_one_is_appended_verbatim() {
    // "abc" starts with "ab" but is an incremental delta, not a snapshot.
    let mut stream = reasoning_stream("qwen3", "reasoning_content", &["ab", "abc"]);
    assert_eq!(drain_reasoning(&mut stream), "ababc");
}

/// Kimi endpoints are the only known senders of cumulative reasoning
/// snapshots (every chunk repeats the full text so far). The snapshot strip
/// must stay enabled there; only the other backends stream plain deltas.
#[test]
fn kimi_reasoning_snapshots_are_still_prefix_stripped() {
    let mut stream = reasoning_stream(
        "moonshotai/kimi-k2.5",
        "reasoning_content",
        &["94", "944", "9440"],
    );
    assert_eq!(drain_reasoning(&mut stream), "9440");
}

#[test]
fn parse_next_event_accepts_reasoning_delta_alias() {
    let provider_pin = Arc::new(std::sync::Mutex::new(None));
    let mut stream = OpenRouterStream::new(
        futures::stream::empty(),
        "test-model".to_string(),
        provider_pin,
    );
    stream.buffer =
        "data: {\"choices\":[{\"delta\":{\"reasoning\":\"thinking\"}}]}\n\n".to_string();

    let event = stream.parse_next_event();

    assert!(matches!(event, Some(StreamEvent::ThinkingDelta(text)) if text == "thinking"));
}

#[test]
fn parse_next_event_propagates_finish_reason_to_message_end() {
    let provider_pin = Arc::new(std::sync::Mutex::new(None));
    let mut stream = OpenRouterStream::new(
        futures::stream::empty(),
        "test-model".to_string(),
        provider_pin,
    );
    stream.buffer =
        "data: {\"choices\":[{\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n".to_string();

    let event = stream.parse_next_event();

    assert!(matches!(
        event,
        Some(StreamEvent::MessageEnd { stop_reason: Some(reason) }) if reason == "length"
    ));
}

#[test]
fn stream_eof_emits_message_end_with_finish_reason_without_done() {
    let provider_pin = Arc::new(std::sync::Mutex::new(None));
    let bytes = Bytes::from_static(
        b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"max_tokens\"}]}\n\n",
    );
    let mut stream = OpenRouterStream::new(
        futures::stream::once(async move { Ok(bytes) }),
        "test-model".to_string(),
        provider_pin,
    );

    let event = futures::executor::block_on(stream.next());

    assert!(matches!(
        event,
        Some(Ok(StreamEvent::MessageEnd { stop_reason: Some(reason) })) if reason == "max_tokens"
    ));
    assert!(futures::executor::block_on(stream.next()).is_none());
}

fn assert_tool_calls_stream_before_end(repeated_stop: bool, done: bool, parallel: bool) {
    use futures::FutureExt;

    // Keep the transport open between chunks so premature tool completion
    // cannot be hidden by collecting an already-finished stream.
    let (sender, receiver) = futures::channel::mpsc::unbounded();
    let mut stream = OpenRouterStream::new(
        receiver,
        "test-model".to_string(),
        Arc::new(Mutex::new(None)),
    );
    let stop = serde_json::json!({
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
    });
    let send = |payload: Value| {
        sender
            .unbounded_send(Ok(Bytes::from(format!("data: {payload}\n\n"))))
            .unwrap();
    };
    let calls = if parallel { 2 } else { 1 };
    // Index-only argument fragments arrive interleaved, after the proxy's
    // first stop chunk has already followed the id/name-only delta.
    for arguments in [None, Some("{\"command\":"), Some("\"echo ok\"}")] {
        for index in 0..calls {
            let call = match arguments {
                None => serde_json::json!({
                    "index": index,
                    "id": format!("call_{index}"),
                    "function": {"name": "bash"}
                }),
                Some(arguments) => serde_json::json!({
                    "index": index,
                    "function": {"arguments": arguments}
                }),
            };
            send(serde_json::json!({"choices": [{"delta": {"tool_calls": [call]}}]}));
            if repeated_stop {
                send(stop.clone());
            }
            match arguments {
                None => assert!(matches!(
                    stream.next().now_or_never(),
                    Some(Some(Ok(StreamEvent::ToolUseStart { id, name })))
                        if id == format!("call_{index}") && name == "bash"
                )),
                Some(fragment) => assert!(matches!(
                    stream.next().now_or_never(),
                    Some(Some(Ok(StreamEvent::ToolInputDeltaFor { id, delta })))
                        if id == format!("call_{index}") && delta == fragment
                )),
            }
            assert!(
                stream.next().now_or_never().is_none(),
                "must not complete before DONE/EOF"
            );
        }
    }
    let reason = if repeated_stop { "stop" } else { "tool_calls" };
    send(serde_json::json!({
        "choices": [{"delta": {}, "finish_reason": reason}]
    }));
    assert!(stream.next().now_or_never().is_none());

    // Ordinary text remains incremental while tools are streaming.
    send(serde_json::json!({"choices": [{"delta": {"content": "ready"}}]}));
    assert!(matches!(
        stream.next().now_or_never(),
        Some(Some(Ok(StreamEvent::TextDelta(text)))) if text == "ready"
    ));

    if done {
        sender
            .unbounded_send(Ok(Bytes::from_static(b"data: [DONE]\n\n")))
            .unwrap();
    } else {
        sender.close_channel();
    }
    // For [DONE], the transport is still open. Completion must not wait for
    // EOF. For EOF, there is no [DONE] and poll_next must flush the calls.
    for index in 0..calls {
        assert!(matches!(
            stream.next().now_or_never(),
            Some(Some(Ok(StreamEvent::ToolUseEndFor { id }))) if id == format!("call_{index}")
        ));
    }
    assert!(matches!(
        stream.next().now_or_never(),
        Some(Some(Ok(StreamEvent::MessageEnd { stop_reason })))
            if stop_reason.as_deref() == Some(reason)
    ));
    assert!(stream.tool_call_accumulators.is_empty());
    assert!(stream.pending.is_empty());
    // Closing the transport after [DONE] must not emit the calls or end twice.
    sender.close_channel();
    assert!(matches!(stream.next().now_or_never(), Some(None)));
}

#[test]
fn repeated_stop_chunks_preserve_tool_arguments_until_done_or_eof() {
    for done in [true, false] {
        for parallel in [false, true] {
            assert_tool_calls_stream_before_end(true, done, parallel);
        }
    }
}

#[test]
fn normal_tool_streaming_completes_at_done_or_eof() {
    for done in [true, false] {
        for parallel in [false, true] {
            assert_tool_calls_stream_before_end(false, done, parallel);
        }
    }
}

#[test]
fn parse_next_event_coalesces_repeated_tool_call_id_chunks() {
    let provider_pin = Arc::new(std::sync::Mutex::new(None));
    let mut stream =
        OpenRouterStream::new(futures::stream::empty(), "glm-5".to_string(), provider_pin);

    let chunk1 = serde_json::json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "bash", "arguments": ""}
                }]
            }
        }]
    });
    let chunk2 = serde_json::json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "function": {"arguments": "{\"command\""}
                }]
            }
        }]
    });
    let chunk3 = serde_json::json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "function": {"arguments": ":\"echo ok\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    stream.buffer =
        format!("data: {chunk1}\n\ndata: {chunk2}\n\ndata: {chunk3}\n\ndata: [DONE]\n\n");

    let mut events = Vec::new();
    for _ in 0..8 {
        if let Some(event) = stream.parse_next_event() {
            events.push(event);
        } else {
            break;
        }
    }

    assert_eq!(events.len(), 5, "events: {events:?}");
    assert!(matches!(
        &events[0],
        StreamEvent::ToolUseStart { id, name } if id == "call_1" && name == "bash"
    ));
    assert!(matches!(
        &events[1],
        StreamEvent::ToolInputDeltaFor { id, delta } if id == "call_1" && delta == "{\"command\""
    ));
    assert!(
        matches!(&events[2], StreamEvent::ToolInputDeltaFor { id, delta } if id == "call_1" && delta == ":\"echo ok\"}")
    );
    assert!(matches!(&events[3], StreamEvent::ToolUseEndFor { id } if id == "call_1"));
    assert!(matches!(
        &events[4],
        StreamEvent::MessageEnd { stop_reason } if stop_reason.as_deref() == Some("tool_calls")
    ));
    assert!(stream.tool_call_accumulators.is_empty());
}

#[test]
fn vertex_sse_preserves_tool_call_thought_signature() {
    let mut stream = test_stream();
    let chunk = serde_json::json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_vertex",
                    "type": "function",
                    "function": {"name": "read", "arguments": "{\"path\":\"README.md\"}"},
                    "extra_content": {
                        "google": {"thought_signature": "AY89a1...verbatim"}
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    stream.buffer = format!("data: {chunk}\n\ndata: [DONE]\n\n");

    let mut events = Vec::new();
    while let Some(event) = stream.parse_next_event() {
        events.push(event);
    }

    assert!(
        matches!(
            &events[..],
            [
                StreamEvent::ToolUseStart { id, name },
                StreamEvent::ToolInputDeltaFor { id: input_id, delta: arguments },
                StreamEvent::ToolUseEndFor { id: end_id },
                StreamEvent::ToolUseSignatureFor { id: signature_id, signature },
                StreamEvent::MessageEnd { stop_reason: Some(reason) },
            ] if id == "call_vertex"
                && input_id == id && end_id == id && signature_id == id
                && name == "read"
                && arguments == "{\"path\":\"README.md\"}"
                && signature == "AY89a1...verbatim"
                && reason == "tool_calls"
        ),
        "events: {events:?}"
    );
}

#[test]
fn split_identity_starts_without_waiting_for_arguments() {
    let mut stream = test_stream();
    stream.apply_tool_call_delta(0, None, Some("bash"), None, None);
    assert!(
        stream.pending.is_empty(),
        "must await a provider continuation ID"
    );
    stream.apply_tool_call_delta(0, Some("late-id"), None, None, None);
    assert!(
        matches!(stream.pending.pop_front(), Some(StreamEvent::ToolUseStart { id, name }) if id == "late-id" && name == "bash")
    );
    assert!(stream.pending.is_empty());
    stream.apply_tool_call_delta(0, Some("late-id"), None, Some("{}"), None);
    assert!(
        matches!(stream.pending.pop_front(), Some(StreamEvent::ToolInputDeltaFor { id, delta }) if id == "late-id" && delta == "{}")
    );
    stream.flush_tool_call_accumulators();
    assert!(
        matches!(stream.pending.pop_front(), Some(StreamEvent::ToolUseEndFor { id }) if id == "late-id")
    );
    assert!(stream.pending.is_empty());
}

#[test]
fn zero_argument_calls_end_once_without_an_empty_delta() {
    let mut stream = test_stream();
    stream.apply_tool_call_delta(0, Some("empty"), Some("noop"), None, None);
    assert!(
        matches!(stream.pending.pop_front(), Some(StreamEvent::ToolUseStart { id, .. }) if id == "empty")
    );
    stream.apply_tool_call_delta(0, Some("empty"), Some("noop"), Some(""), None);
    assert!(stream.pending.is_empty());
    stream.flush_tool_call_accumulators();
    stream.flush_tool_call_accumulators();
    assert!(
        matches!(stream.pending.pop_front(), Some(StreamEvent::ToolUseEndFor { id }) if id == "empty")
    );
    assert!(stream.pending.is_empty());
}

#[test]
fn positional_fallback_tool_call_ids_are_unique_across_responses() {
    fn parse_id() -> String {
        let mut stream = test_stream();
        stream.apply_tool_call_delta(
            0,
            Some("bash:0"),
            Some("bash"),
            Some(r#"{"command":"echo ok"}"#),
            None,
        );
        stream.flush_tool_call_accumulators();

        match stream.pending.pop_front() {
            Some(StreamEvent::ToolUseStart { id, name }) => {
                assert_eq!(name, "bash");
                assert!(id.starts_with("toolu_"), "unexpected fallback id: {id}");
                id
            }
            event => panic!("expected tool-use start, got {event:?}"),
        }
    }

    let first_turn_id = parse_id();
    let second_turn_id = parse_id();

    assert_ne!(first_turn_id, second_turn_id);
}
