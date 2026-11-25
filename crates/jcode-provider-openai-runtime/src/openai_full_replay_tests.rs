//! Replay oracle frozen from d521b8240, with an explicit alternating CPU/allocation probe.
use super::*;
use serde_json::json;

// Deliberately independent of both production builders: preserve the original
// discarded delta, its setting clones, and the subsequent full-request clone.
fn original(
    request: &Value,
    input: &[Value],
    delta: &[Value],
    full_replay: bool,
    message_count: usize,
    chatgpt: bool,
) -> Option<Value> {
    let incremental_items = if full_replay || message_count == 0 {
        input.to_vec()
    } else {
        delta.to_vec()
    };
    let previous_response_id = if full_replay { "" } else { "resp_previous" };
    let mut continuation_request = json!({
        "type": "response.create",
        "previous_response_id": previous_response_id,
        "input": incremental_items,
    });
    if let Some(model) = request.get("model") {
        continuation_request["model"] = model.clone();
    }
    if let Some(tools) = request.get("tools") {
        continuation_request["tools"] = tools.clone();
    }
    if let Some(tool_choice) = request.get("tool_choice") {
        continuation_request["tool_choice"] = tool_choice.clone();
    }
    if let Some(instructions) = request.get("instructions") {
        continuation_request["instructions"] = instructions.clone();
    }
    if let Some(max_output_tokens) = request.get("max_output_tokens") {
        continuation_request["max_output_tokens"] = max_output_tokens.clone();
    }
    if let Some(reasoning) = request.get("reasoning") {
        continuation_request["reasoning"] = reasoning.clone();
    }
    if let Some(context_management) = request.get("context_management") {
        continuation_request["context_management"] = context_management.clone();
    }
    if let Some(include) = request.get("include") {
        continuation_request["include"] = include.clone();
    }
    if let Some(service_tier) = request.get("service_tier") {
        continuation_request["service_tier"] = service_tier.clone();
    }
    if let Some(prompt_cache_key) = request.get("prompt_cache_key") {
        continuation_request["prompt_cache_key"] = prompt_cache_key.clone();
    }
    if let Some(prompt_cache_retention) = request.get("prompt_cache_retention") {
        continuation_request["prompt_cache_retention"] = prompt_cache_retention.clone();
    }
    if let Some(options) = request.get("prompt_cache_options") {
        continuation_request["prompt_cache_options"] = options.clone();
    }
    if !full_replay && message_count > 0 {
        add_cache_comparison(&mut continuation_request, chatgpt, previous_response_id);
    }
    continuation_request["store"] = json!(false);
    continuation_request["parallel_tool_calls"] = json!(false);
    if full_replay {
        continuation_request = request.clone();
        let object = continuation_request.as_object_mut()?;
        object.insert("type".into(), json!("response.create"));
        object.insert("input".into(), Value::Array(input.to_vec()));
        object.remove("previous_response_id");
        object.remove("stream");
        object.remove("background");
    }
    Some(continuation_request)
}

fn optimized(
    request: &Value,
    input: &[Value],
    delta: &[Value],
    full_replay: bool,
    message_count: usize,
    chatgpt: bool,
) -> Option<Value> {
    if full_replay {
        // Production borrows input for unchanged diagnostics and emptiness checks.
        persistent_ws_full_replay_request(request, input)
    } else {
        let incremental_items = if message_count == 0 {
            input.to_vec()
        } else {
            delta.to_vec()
        };
        let mut continuation_request =
            persistent_ws_delta_request(request, &incremental_items, "resp_previous");
        if message_count > 0 {
            add_cache_comparison(&mut continuation_request, chatgpt, "resp_previous");
        }
        continuation_request["store"] = json!(false);
        continuation_request["parallel_tool_calls"] = json!(false);
        Some(continuation_request)
    }
}

fn fixtures() -> Vec<(&'static str, Value, Vec<Value>)> {
    let tiny = vec![json!({"role":"user","content":"hello"})];
    let large: Vec<_> = (0..128)
        .map(|i| {
            json!({
                "role":"user", "content":format!("{i}:{}", "history ".repeat(1024))
            })
        })
        .collect();
    let make = |input: &[Value]| {
        json!({
            "type":"old", "model":"gpt-5.4", "input":input,
            "instructions":"system ".repeat(128),
            "tools":[{"type":"function","name":"tool","parameters":{"type":"object"}}],
            "tool_choice":"auto", "max_output_tokens":4096,
            "reasoning":{"effort":"high"}, "include":["reasoning.encrypted_content"],
            "context_management":[{"type":"compaction","compact_threshold":10000}],
            "service_tier":"priority", "prompt_cache_key":"key", "prompt_cache_retention":"24h",
            "prompt_cache_options":{"comparison_response_id":"explicit"},
            "parallel_tool_calls":true, "store":true, "generate":false,
            "previous_response_id":"must_remove", "stream":true, "background":true,
            "future_setting":{"preserve":[1,null,false]}
        })
    };
    vec![
        ("empty", json!({}), vec![]),
        ("tiny", make(&tiny), tiny),
        (
            "malformed",
            json!({"input":"not_array","tools":null,"instructions":[1],"prompt_cache_options":false,"store":null}),
            vec![Value::Null, json!({"type":42})],
        ),
        ("large", make(&large), large),
    ]
}

#[test]
fn full_replay_and_delta_match_frozen_request_oracle() {
    for (name, request, input) in fixtures() {
        for full_replay in [false, true] {
            for message_count in [0, 1, 12] {
                for chatgpt in [false, true] {
                    let delta = input.last().cloned().into_iter().collect::<Vec<_>>();
                    let old = original(
                        &request,
                        &input,
                        &delta,
                        full_replay,
                        message_count,
                        chatgpt,
                    );
                    let new = optimized(
                        &request,
                        &input,
                        &delta,
                        full_replay,
                        message_count,
                        chatgpt,
                    );
                    assert_eq!(
                        new, old,
                        "{name} full={full_replay} messages={message_count} chatgpt={chatgpt}"
                    );
                    assert_eq!(
                        serde_json::to_vec(&new).unwrap(),
                        serde_json::to_vec(&old).unwrap()
                    );
                }
            }
        }
    }
    for request in [Value::Null, json!(false), json!(1), json!("bad"), json!([])] {
        assert_eq!(
            persistent_ws_full_replay_request(&request, &[json!(1)]),
            None
        );
        assert_eq!(original(&request, &[json!(1)], &[], true, 1, false), None);
    }
}

#[test]
fn cancelled_replay_preserves_native_reasoning_and_unanswered_tool_calls() {
    let input = vec![
        json!({"type":"reasoning","id":"rs_native","encrypted_content":"opaque"}),
        json!({"type":"function_call","id":"fc_native","call_id":"unanswered","name":"tool","arguments":"{}"}),
        json!({"type":"function_call_output","call_id":"previous","output":"complete"}),
    ];
    let request = json!({"input":input,"generate":true,"model":"gpt-5.4"});
    let replay = persistent_ws_full_replay_request(&request, &input).unwrap();
    assert_eq!(replay["input"], json!(input));
    assert!(replay.get("previous_response_id").is_none());
    assert_eq!(
        Some(replay),
        original(&request, &input, &[], true, 5, false)
    );
}

#[test]
fn full_replay_preserves_arbitrary_settings_and_replaces_input() {
    let request = json!({"input":["old"],"type":"old","previous_response_id":"old","stream":false,"background":null,"generate":false,"store":true,"parallel_tool_calls":true,"unknown":{"keep":true}});
    let new = persistent_ws_full_replay_request(&request, &[json!("new")]).unwrap();
    assert_eq!(
        new,
        json!({"input":["new"],"type":"response.create","generate":false,"store":true,"parallel_tool_calls":true,"unknown":{"keep":true}})
    );
    assert_eq!(request["input"], json!(["old"]));
}

#[test]
#[cfg(target_os = "linux")]
#[ignore = "explicit alternating Linux thread CPU and allocation probe"]
fn full_replay_alternating_cpu_allocation_probe() {
    use crate::owned_request_tests::{measure_allocations, thread_cpu_ns};
    use std::hint::black_box;
    for (name, request, input) in fixtures() {
        let delta = input.last().cloned().into_iter().collect::<Vec<_>>();
        for full_replay in [false, true] {
            let iterations = if input.len() > 10 { 30 } else { 3000 };
            let mut old = Vec::new();
            let mut new = Vec::new();
            for round in 0..12 {
                for is_new in if round % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    let run = || {
                        for _ in 0..iterations {
                            black_box(if is_new {
                                optimized(
                                    black_box(&request),
                                    black_box(&input),
                                    black_box(&delta),
                                    full_replay,
                                    1,
                                    false,
                                )
                            } else {
                                original(
                                    black_box(&request),
                                    black_box(&input),
                                    black_box(&delta),
                                    full_replay,
                                    1,
                                    false,
                                )
                            });
                        }
                    };
                    let (_, allocations) = measure_allocations(run);
                    let start = thread_cpu_ns();
                    run();
                    let elapsed = thread_cpu_ns() - start;
                    if is_new {
                        new.push((elapsed, allocations));
                    } else {
                        old.push((elapsed, allocations));
                    }
                }
            }
            old.sort();
            new.sort();
            println!(
                "{name} full={full_replay}: old={:?} new={:?} iterations={iterations} CPU ratio={:.3}",
                old[6],
                new[6],
                new[6].0 as f64 / old[6].0 as f64
            );
            assert!(
                new[6].1.0 <= old[6].1.0,
                "allocation count regression {name} full={full_replay}"
            );
            assert!(
                new[6].1.1 <= old[6].1.1,
                "allocation bytes regression {name} full={full_replay}"
            );
            assert!(
                new[6].0 <= old[6].0 * 110 / 100,
                "CPU regression {name} full={full_replay}"
            );
        }
    }
}
