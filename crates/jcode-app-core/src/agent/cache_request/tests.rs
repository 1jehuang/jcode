use super::*;

fn legacy_event(
    messages: &[Message],
    tools: &[ToolDefinition],
    system: &str,
    ephemeral: &[Message],
) -> ServerEvent {
    let eager: Vec<_> = tools.iter().filter(|tool| !tool.defer_loading).collect();
    ServerEvent::KvCacheRequest {
        system_static_hash: stable_hash_str(system),
        tools_hash: stable_hash_json(&eager),
        messages_hash: stable_hash_json(&crate::message::cache_relevant_messages(messages)),
        message_hashes: message_hashes(messages),
        message_count: messages.len(),
        tool_count: eager.len(),
        system_static_chars: system.chars().count(),
        tools_json_chars: stable_json_len(&eager),
        messages_json_chars: stable_json_len(messages),
        ephemeral_hash: if ephemeral.is_empty() {
            None
        } else {
            Some(stable_hash_json(ephemeral))
        },
        ephemeral_chars: stable_json_len(ephemeral),
        ephemeral_message_count: ephemeral.len(),
    }
}

fn tools() -> Vec<ToolDefinition> {
    let eager = ToolDefinition::new(
        "lookup",
        "Unicode 🦊 and \"escapes\"",
        serde_json::json!({"type":"object","properties":{"query":{"type":"string"}}}),
    );
    let mut deferred = eager.clone();
    deferred.name = "late".into();
    deferred.defer_loading = true;
    vec![eager, deferred]
}

#[test]
fn fused_request_matches_all_legacy_fields() {
    let mut cases = vec![
        vec![],
        vec![Message::user("")],
        vec![Message::user("\"\n\r\t\\\0🦊é")],
    ];
    let mut mixed = vec![Message::user("question"), Message::assistant_text("answer")];
    mixed[1].content.push(ContentBlock::ReasoningTrace {
        text: "private reasoning".into(),
    });
    mixed[0].tool_duration_ms = Some(123);
    cases.push(mixed);
    for messages in &cases {
        for ephemeral in &cases {
            let old = legacy_event(messages, &tools(), "sys🦊", ephemeral);
            let new = kv_cache_request_event(messages, &tools(), "sys🦊", ephemeral);
            assert_eq!(
                serde_json::to_value(old).unwrap(),
                serde_json::to_value(new).unwrap()
            );
        }
    }
}

#[test]
fn projected_stream_hash_matches_string_hash_across_chunk_boundaries() {
    for size in 0..140 {
        let mut messages = vec![
            Message::user(&"x🦊\n\"".repeat(size)),
            Message::assistant_text("tail"),
        ];
        let (aggregate, per_message) = projected_message_hashes(&messages);
        assert_eq!(
            aggregate,
            stable_hash_json(&crate::message::cache_relevant_messages(&messages))
        );
        assert_eq!(per_message, message_hashes(&messages));
        messages.reverse();
        assert_eq!(
            projected_message_hashes(&messages).0,
            stable_hash_json(&crate::message::cache_relevant_messages(&messages))
        );
    }
}

#[test]
fn fused_json_failure_preserves_empty_hash_and_zero_length() {
    struct Fail;
    impl serde::Serialize for Fail {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("expected"))
        }
    }
    assert_eq!(json_hash_and_len(&Fail), (stable_hash_str(""), 0));
}

#[test]
#[ignore = "paired complete cache diagnostic computation benchmark"]
fn benchmark_request_diagnostics() {
    for count in [0, 8, 96] {
        let messages: Vec<_> = (0..count)
            .map(|i| Message::user(&format!("{i}: {}", "request 🦊 \"payload\"\n".repeat(256))))
            .collect();
        let tools = tools();
        let ephemeral = [Message::user("dynamic")];
        assert_eq!(
            serde_json::to_value(legacy_event(&messages, &tools, "system", &ephemeral)).unwrap(),
            serde_json::to_value(kv_cache_request_event(
                &messages, &tools, "system", &ephemeral
            ))
            .unwrap()
        );
        std::hint::black_box(legacy_event(&messages, &tools, "system", &ephemeral));
        std::hint::black_box(kv_cache_request_event(
            &messages, &tools, "system", &ephemeral,
        ));
        let measure = |old| {
            let start = std::time::Instant::now();
            for _ in 0..30 {
                std::hint::black_box(if old {
                    legacy_event(&messages, &tools, "system", &ephemeral)
                } else {
                    kv_cache_request_event(&messages, &tools, "system", &ephemeral)
                });
            }
            start.elapsed()
        };
        for round in 0..4 {
            let (old, new) = if round % 2 == 0 {
                (measure(true), measure(false))
            } else {
                let new = measure(false);
                (measure(true), new)
            };
            eprintln!(
                "CACHE_REQUEST count={count} round={round} old={old:?} new={new:?} speedup={:.3}x",
                old.as_secs_f64() / new.as_secs_f64()
            );
        }
    }
}
