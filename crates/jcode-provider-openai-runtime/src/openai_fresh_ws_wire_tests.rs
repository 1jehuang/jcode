//! Fresh websocket wire parity against the frozen d521b8240 clone/mutate path.
use super::*;
use crate::owned_request_tests::measure_allocations;
use serde_json::json;

fn original(request: &Value) -> String {
    let mut request_event = request.clone();
    let obj = request_event.as_object_mut().expect("object request");
    obj.insert(
        "type".to_string(),
        Value::String("response.create".to_string()),
    );
    obj.remove("stream");
    obj.remove("background");
    serde_json::to_string(&request_event).unwrap()
}

fn borrowed(request: &Value) -> String {
    serialize_fresh_ws_request(request.as_object().expect("object request")).unwrap()
}

fn requests() -> Vec<(&'static str, Value)> {
    vec![
        ("empty", json!({})),
        (
            "tiny",
            json!({"model":"future-model","input":[{"role":"user","content":"hi"}],"stream":true,"background":false}),
        ),
        (
            "long-history",
            json!({"model":"future-model","input":(0..256).map(|n|json!({"role":"user","content":format!("{n}: {}", "history ".repeat(512))})).collect::<Vec<_>>(),"stream":true}),
        ),
        (
            "nested-tools",
            json!({"input":(0..64).map(|n|json!({"type":"function_call_output","call_id":format!("c{n}"),"output":{"records":(0..32).map(|i|json!({"n":i,"data":[{"key":"value","text":"nested ".repeat(16)}]})).collect::<Vec<_>>()}})).collect::<Vec<_>>(),"tools":[{"type":"function","name":"example","parameters":{"type":"object","properties":{"field":{"type":"array","items":{"type":"string"}}}}}]}),
        ),
        (
            "native-image",
            json!({"input":[{"type":"reasoning","encrypted_content":"encrypted ".repeat(1024),"summary":[{"type":"summary_text","text":"summary"}]},{"type":"web_search_call","id":"search_1","action":{"type":"search","query":"query"}},{"role":"user","content":[{"type":"input_image","image_url":format!("data:image/png;base64,{}","abcd".repeat(64*1024))}]}],"reasoning":{"effort":"high"},"service_tier":"priority","prompt_cache_key":"key","stream":true,"background":true}),
        ),
    ]
}

#[test]
fn fresh_ws_wire_matches_frozen_original_bytes_and_preserves_retry_payload() {
    for (name, request) in requests() {
        let frozen_request = serde_json::to_vec(&request).unwrap();
        assert_eq!(
            borrowed(&request).as_bytes(),
            original(&request).as_bytes(),
            "{name}"
        );
        assert_eq!(
            serde_json::to_vec(&request).unwrap(),
            frozen_request,
            "retry payload {name}"
        );
        let wire: Value = serde_json::from_str(&borrowed(&request)).unwrap();
        assert_eq!(wire["type"], "response.create");
        assert!(wire.get("stream").is_none());
        assert!(wire.get("background").is_none());
        assert_eq!(
            effective_https_idle_timeout(&wire),
            effective_https_idle_timeout(&request)
        );
        assert_eq!(
            effective_ws_completion_timeout_secs(&wire),
            effective_ws_completion_timeout_secs(&request)
        );
    }
    for existing_type in [
        json!(null),
        json!(false),
        json!(123),
        json!({"nested":"ignored"}),
        json!("old.type"),
    ] {
        let request = json!({"type":existing_type,"background":{"unexpected":true},"stream":null,"z":"尾","a":"quote\" slash\\ newline\n tab\t nul\u{0000}","nested":{"type":"keep","stream":true,"background":true,"float":1.25,"neg":-42}});
        assert_eq!(borrowed(&request), original(&request));
    }
    assert_eq!(
        borrowed(&json!({"z":0,"a":1})),
        r#"{"a":1,"type":"response.create","z":0}"#
    );
}

#[test]
fn fresh_ws_borrowed_wire_allocation_regression() {
    for (name, request) in requests() {
        let (expected, old) = measure_allocations(|| original(&request));
        let (actual, new) = measure_allocations(|| borrowed(&request));
        assert_eq!(actual, expected, "{name}");
        assert!(
            new.0 <= old.0,
            "allocation count {name}: {old:?} -> {new:?}"
        );
        assert!(
            new.1 <= old.1,
            "allocation bytes {name}: {old:?} -> {new:?}"
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
#[ignore = "explicit release alternating thread CPU/allocation benchmark"]
fn fresh_ws_wire_alternating_release_probe() {
    use crate::owned_request_tests::thread_cpu_ns;
    use std::hint::black_box;
    for (name, request) in requests() {
        let iterations = if name == "empty" || name == "tiny" {
            20_000
        } else {
            100
        };
        assert_eq!(borrowed(&request), original(&request));
        let mut old = Vec::new();
        let mut new = Vec::new();
        for round in 0..12 {
            for optimized in if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let (_, allocations) = measure_allocations(|| {
                    for _ in 0..iterations {
                        black_box(if optimized {
                            borrowed(black_box(&request))
                        } else {
                            original(black_box(&request))
                        });
                    }
                });
                let start = thread_cpu_ns();
                for _ in 0..iterations {
                    black_box(if optimized {
                        borrowed(black_box(&request))
                    } else {
                        original(black_box(&request))
                    });
                }
                let elapsed = thread_cpu_ns() - start;
                if optimized {
                    new.push((elapsed, allocations));
                } else {
                    old.push((elapsed, allocations));
                }
            }
        }
        old.sort();
        new.sort();
        println!(
            "fresh-ws {name}: old={:?} new={:?} iterations={iterations} CPU ratio={:.3}",
            old[6],
            new[6],
            new[6].0 as f64 / old[6].0 as f64
        );
        assert!(
            new[6].1.0 <= old[6].1.0,
            "allocation count regression {name}"
        );
        assert!(
            new[6].1.1 <= old[6].1.1,
            "allocation bytes regression {name}"
        );
        assert!(new[6].0 <= old[6].0 * 110 / 100, "CPU regression {name}");
    }
}
