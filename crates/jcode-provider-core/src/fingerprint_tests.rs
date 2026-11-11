use super::*;
use serde_json::json;
use std::hint::black_box;
use std::time::Duration;

fn legacy_fingerprint<T: Serialize + ?Sized>(value: &T) -> (u64, usize) {
    let encoded = serde_json::to_string(value).unwrap_or_default();
    (stable_hash_str(&encoded), encoded.len())
}

fn provider_fixture(turns: usize) -> (Value, Vec<Value>, Value, Value) {
    let system = json!([{"type": "text", "text": "You are a coding assistant. Preserve user changes.\n日本語 🦀", "cache_control": {"type": "ephemeral"}}]);
    let tools = json!([{"name": "read_file", "description": "Read a file", "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}]);
    let source =
        "fn main() { println!(\"hello\\world\"); }\n\t// café 日本語 🦀\u{0000}".repeat(128);
    let mut items = Vec::with_capacity(turns * 3);
    for turn in 0..turns {
        items.push(json!({"role": "user", "content": [{"type": "text", "text": format!("Review src/file_{turn}.rs\n{source}")}]}));
        items.push(json!({"role": "assistant", "content": [{"type": "tool_use", "id": format!("call_{turn}"), "name": "read_file", "input": {"path": format!("src/file_{turn}.rs")}}]}));
        items.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": format!("call_{turn}"), "content": source, "is_error": false}]}));
    }
    let payload =
        json!({"model": "fixture-model", "system": system, "tools": tools, "messages": items});
    (payload, items, system, tools)
}

#[test]
fn streaming_fingerprint_matches_exact_legacy_json_bytes() {
    let (payload, items, system, tools) = provider_fixture(3);
    let values = vec![
        Value::Null,
        json!(true),
        json!(0),
        json!(-42),
        json!(1.25),
        json!(""),
        json!([]),
        json!({}),
        json!("quote\" backslash\\ newline\n tab\t nul\u{0000} é 日本語 🦀"),
        json!([null, false, 1, "nested", {"a": []}]),
        payload,
        json!(items),
        system,
        tools,
        json!({"input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Explain this"}, {"type": "input_image", "image_url": "data:image/png;base64,AAAA"}]}], "instructions": "Be precise"}),
        json!({"contents": [{"role": "model", "parts": [{"functionCall": {"name": "read_file", "args": {"path": "main.rs"}}}]}], "systemInstruction": {"parts": [{"text": "Help"}]}}),
    ];
    for value in values {
        let expected = legacy_fingerprint(&value);
        assert_eq!(stable_json_fingerprint(&value), expected, "{value:?}");
        assert_eq!(stable_hash_json(&value), expected.0, "{value:?}");
    }
    // The generic helper must also accept unsized slices and strings.
    assert_eq!(stable_json_fingerprint("é\n"), legacy_fingerprint("é\n"));
    assert_eq!(
        stable_json_fingerprint(&[1_u64, u64::MAX][..]),
        legacy_fingerprint(&[1_u64, u64::MAX][..])
    );
}

struct FailingSerialize;

impl Serialize for FailingSerialize {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        // Emit a partial document before failing, so partial hashes cannot leak through.
        let mut sequence = serializer.serialize_seq(Some(2))?;
        sequence.serialize_element("already emitted")?;
        Err(serde::ser::Error::custom(
            "intentional serialization failure",
        ))
    }
}

#[test]
fn serializer_failure_retains_empty_string_hash_and_zero_length() {
    let expected = (stable_hash_str(""), 0);
    assert_eq!(legacy_fingerprint(&FailingSerialize), expected);
    assert_eq!(stable_json_fingerprint(&FailingSerialize), expected);
    assert_eq!(stable_hash_json(&FailingSerialize), expected.0);
}

#[test]
fn item_fingerprints_preserve_order_mutations_and_empty_arrays() {
    let (_, items, _, _) = provider_fixture(2);
    let mut mutated = items.clone();
    mutated[0]["content"][0]["text"] = json!("changed");
    let mut reordered = items.clone();
    reordered.swap(0, 1);
    for case in [&[][..], &items[..], &mutated[..], &reordered[..]] {
        let hashes: Vec<_> = case.iter().map(|item| legacy_fingerprint(item).0).collect();
        assert_eq!(
            item_fingerprints(case),
            (hashes, legacy_fingerprint(case).0)
        );
    }
    assert_ne!(item_fingerprints(&items), item_fingerprints(&mutated));
    assert_ne!(item_fingerprints(&items), item_fingerprints(&reordered));
    assert_ne!(
        stable_json_fingerprint(&items),
        stable_json_fingerprint(&reordered)
    );
}

#[derive(Debug, PartialEq, Eq)]
struct ComputedFingerprints {
    request: (u64, usize),
    items: Vec<u64>,
    items_hash: u64,
    input_hash: u64,
    system: Option<(u64, usize)>,
    tools: Option<(u64, usize)>,
    first: Option<u64>,
    last: Option<u64>,
}

fn compute_legacy(
    payload: &Value,
    items: &[Value],
    system: Option<&Value>,
    tools: Option<&Value>,
) -> ComputedFingerprints {
    // Match the original logger's separate serialization for hash and length.
    let hash = |value: &Value| stable_hash_str(&serde_json::to_string(value).unwrap_or_default());
    let len = |value: &Value| serde_json::to_string(value).unwrap_or_default().len();
    let item_hashes: Vec<_> = items.iter().map(hash).collect();
    ComputedFingerprints {
        request: (hash(payload), len(payload)),
        items_hash: legacy_fingerprint(&item_hashes).0,
        input_hash: legacy_fingerprint(items).0,
        system: system.map(|value| (hash(value), len(value))),
        tools: tools.map(|value| (hash(value), len(value))),
        first: item_hashes.first().copied(),
        last: item_hashes.last().copied(),
        items: item_hashes,
    }
}

fn compute_optimized(
    payload: &Value,
    items: &[Value],
    system: Option<&Value>,
    tools: Option<&Value>,
) -> ComputedFingerprints {
    let (item_hashes, input_hash) = item_fingerprints(items);
    ComputedFingerprints {
        request: stable_json_fingerprint(payload),
        items_hash: stable_hash_json(&item_hashes),
        input_hash,
        system: system.map(stable_json_fingerprint),
        tools: tools.map(stable_json_fingerprint),
        first: item_hashes.first().copied(),
        last: item_hashes.last().copied(),
        items: item_hashes,
    }
}

#[test]
fn complete_fingerprint_computation_matches_legacy_fields() {
    let (payload, items, system, tools) = provider_fixture(4);
    for (system, tools) in [
        (None, None),
        (Some(&system), None),
        (None, Some(&tools)),
        (Some(&system), Some(&tools)),
    ] {
        assert_eq!(
            compute_optimized(&payload, &items, system, tools),
            compute_legacy(&payload, &items, system, tools)
        );
    }
}

#[test]
fn canonical_logger_preserves_legacy_baseline_snapshot() {
    let (payload, items, system, tools) = provider_fixture(2);
    let context = jcode_logging::current_context_snapshot();
    let key = format!(
        "{}\u{1f}fingerprint-test\u{1f}fixture-model\u{1f}snapshot-equivalence",
        context.session.as_deref().unwrap_or("no-session")
    );
    for (system, tools) in [(Some(&system), Some(&tools)), (None, None)] {
        let expected = compute_legacy(&payload, &items, system, tools);
        log_provider_canonical_input(
            "fingerprint-test",
            "fixture-model",
            "snapshot-equivalence",
            &payload,
            &items,
            system,
            tools,
            None,
            &[],
        );
        let snapshot = PROVIDER_INPUT_BASELINES
            .lock()
            .unwrap()
            .remove(&key)
            .expect("production logger must install its session baseline");
        assert_eq!(snapshot.request_hash, expected.request.0);
        assert_eq!(snapshot.item_hashes, expected.items);
        assert_eq!(snapshot.item_hashes_hash, expected.items_hash);
        assert_eq!(snapshot.system_hash, expected.system.map(|pair| pair.0));
        assert_eq!(snapshot.tools_hash, expected.tools.map(|pair| pair.0));
    }
}

#[test]
#[ignore = "manual pure-computation benchmark, run with --ignored --nocapture"]
fn benchmark_complete_provider_fingerprint_computation() {
    for turns in [0, 1, 16, 128] {
        let (payload, items, system, tools) = provider_fixture(turns);
        let baseline = || {
            compute_legacy(
                black_box(&payload),
                black_box(&items),
                black_box(Some(&system)),
                black_box(Some(&tools)),
            )
        };
        let optimized = || {
            compute_optimized(
                black_box(&payload),
                black_box(&items),
                black_box(Some(&system)),
                black_box(Some(&tools)),
            )
        };
        assert_eq!(
            baseline(),
            optimized(),
            "all logged fingerprint fields must match"
        );
        for _ in 0..3 {
            black_box(baseline());
            black_box(optimized());
        }
        // Alternate order to reduce systematic cache/thermal bias. Excludes fixture setup,
        // assertions, log formatting, baseline-map locking, and all logger/disk I/O.
        let rounds = 12;
        let mut old_elapsed = Duration::ZERO;
        let mut new_elapsed = Duration::ZERO;
        for round in 0..rounds {
            let mut measure_old = || {
                let start = Instant::now();
                black_box(baseline());
                old_elapsed += start.elapsed();
            };
            let mut measure_new = || {
                let start = Instant::now();
                black_box(optimized());
                new_elapsed += start.elapsed();
            };
            if round % 2 == 0 {
                measure_old();
                measure_new();
            } else {
                measure_new();
                measure_old();
            }
        }
        eprintln!(
            "fingerprint pure computation: items={} payload_bytes={} rounds={} legacy={:?} optimized={:?} speedup={:.3}x",
            items.len(),
            serde_json::to_string(&payload).unwrap().len(),
            rounds,
            old_elapsed,
            new_elapsed,
            old_elapsed.as_secs_f64() / new_elapsed.as_secs_f64()
        );
    }
}
