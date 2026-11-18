//! Legacy snapshot oracle and isolated release benchmark.
//!
//! Hosted release measurements (2026-11-18), alternating implementations over
//! 12 rounds, median nanoseconds per normalization. Fixture creation and result
//! destruction are excluded. Cumulative allocation uses a thread-local counting
//! System allocator in this test binary, so CPU includes counting overhead.
//! small (one 32-byte result): 560 -> 359 ns, 1947 -> 463 allocated bytes.
//! many (128 256-byte results): 79432 -> 37540 ns, 288358 -> 40866 bytes.
//! 1MiB (16 results): 5274879 -> 9870 ns, 33582138 -> 5070 bytes.
//! Reproduce: justrust test -p jcode-provider-openai --release
//! normalization_release_benchmark -- --ignored --nocapture --test-threads=1
use super::*;
use serde_json::json;

fn legacy_reorder_outputs<F: FnMut(OpenAiRequestLogLevel, &str)>(
    normalized: Vec<Value>,
    missing_output: &str,
    mut logger: F,
) -> Vec<Value> {
    let mut output_map: HashMap<String, Value> = HashMap::new();
    for item in &normalized {
        if item.get("type").and_then(|v| v.as_str()) == Some("function_call_output")
            && let Some(call_id) = item.get("call_id").and_then(|v| v.as_str())
        {
            let is_missing = item
                .get("output")
                .and_then(|v| v.as_str())
                .map(|v| v == missing_output)
                .unwrap_or(false);
            match output_map.get(call_id) {
                Some(existing) => {
                    let existing_missing = existing
                        .get("output")
                        .and_then(|v| v.as_str())
                        .map(|v| v == missing_output)
                        .unwrap_or(false);
                    if existing_missing && !is_missing {
                        output_map.insert(call_id.to_string(), item.clone());
                    }
                }
                None => {
                    output_map.insert(call_id.to_string(), item.clone());
                }
            }
        }
    }

    let mut ordered: Vec<Value> = Vec::with_capacity(normalized.len());
    let mut used_outputs: HashSet<String> = HashSet::new();
    let mut relocated_outputs: HashSet<String> = HashSet::new();
    let mut injected_ordered = 0usize;
    let mut dropped_duplicate_outputs = 0usize;
    let mut rewritten_orphans = 0usize;
    let mut skipped_empty_orphans = 0usize;

    for item in normalized {
        let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let is_call = matches!(kind, "function_call" | "custom_tool_call");
        if is_call {
            let call_id = item
                .get("call_id")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string());
            ordered.push(item);
            if let Some(call_id) = call_id {
                if let Some(output_item) = output_map.get(&call_id) {
                    ordered.push(output_item.clone());
                    if used_outputs.insert(call_id.clone()) {
                        relocated_outputs.insert(call_id);
                    }
                } else {
                    injected_ordered += 1;
                    ordered.push(serde_json::json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": missing_output
                    }));
                    used_outputs.insert(call_id);
                }
            }
            continue;
        }

        if kind == "function_call_output" {
            if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str())
                && used_outputs.contains(call_id)
            {
                // The first original was already emitted next to its call.
                // Only additional originals are actual duplicate outputs.
                if !relocated_outputs.remove(call_id) {
                    dropped_duplicate_outputs += 1;
                }
                continue;
            }
            if let Some(message_item) = orphan_tool_output_to_user_message(&item, missing_output) {
                ordered.push(message_item);
                rewritten_orphans += 1;
            } else {
                skipped_empty_orphans += 1;
            }
            continue;
        }

        ordered.push(item);
    }

    if injected_ordered > 0 {
        logger(
            OpenAiRequestLogLevel::Info,
            &format!(
                "[openai] Inserted {} tool output(s) to enforce call ordering",
                injected_ordered
            ),
        );
    }
    if dropped_duplicate_outputs > 0 {
        logger(
            OpenAiRequestLogLevel::Info,
            &format!(
                "[openai] Dropped {} duplicate tool output(s) during re-ordering",
                dropped_duplicate_outputs
            ),
        );
    }
    if rewritten_orphans > 0 {
        let total = REWRITTEN_ORPHAN_TOOL_OUTPUTS
            .fetch_add(rewritten_orphans as u64, Ordering::Relaxed)
            + rewritten_orphans as u64;
        logger(
            OpenAiRequestLogLevel::Info,
            &format!(
                "[openai] Rewrote {} orphaned tool output(s) as user messages (total={})",
                rewritten_orphans, total
            ),
        );
    }
    if skipped_empty_orphans > 0 {
        logger(
            OpenAiRequestLogLevel::Info,
            &format!(
                "[openai] Skipped {} empty orphaned tool output(s) during re-ordering",
                skipped_empty_orphans
            ),
        );
    }

    ordered
}

fn fixtures() -> Vec<Value> {
    vec![
        json!({"type":"function_call", "call_id":"a", "name":"read", "arguments":"{}"}),
        json!({"type":"custom_tool_call", "call_id":"a", "name":"native", "input":"hello"}),
        json!({"type":"function_call", "call_id":"b"}),
        json!({"type":"function_call_output", "call_id":"a", "output":TOOL_OUTPUT_MISSING_TEXT}),
        json!({"type":"function_call_output", "call_id":"a", "output":"real"}),
        json!({"type":"function_call_output", "call_id":"a", "output":"second"}),
        json!({"type":"function_call_output", "call_id":"b", "output":{"nested":["native",42]}}),
        json!({"type":"function_call_output", "output":"no id"}),
        json!({"type":"function_call_output", "call_id":"empty", "output":""}),
        json!({"type":"function_call"}),
        json!({"type":"message", "content":"prose"}),
    ]
}

fn compare(input: Vec<Value>) {
    let mut old_logs = Vec::new();
    let mut new_logs = Vec::new();
    let clean = |text: &str| text.split(" (total=").next().unwrap().to_owned();
    let expected =
        legacy_reorder_outputs(input.clone(), TOOL_OUTPUT_MISSING_TEXT, |level, text| {
            old_logs.push((level, clean(text)))
        });
    let actual = reorder_outputs(input.clone(), TOOL_OUTPUT_MISSING_TEXT, |level, text| {
        new_logs.push((level, clean(text)))
    });
    assert_eq!(actual, expected, "input: {input:?}");
    assert_eq!(new_logs, old_logs, "input: {input:?}");
}

#[test]
fn legacy_oracle_order_duplicates_orphans_and_native_calls() {
    let tokens = fixtures();
    compare(Vec::new());
    // Exhaust every ordering up to four items, including repeated calls/outputs.
    for length in 1..=4 {
        for mut pattern in 0..tokens.len().pow(length) {
            let mut input = Vec::new();
            for _ in 0..length {
                input.push(tokens[pattern % tokens.len()].clone());
                pattern /= tokens.len();
            }
            compare(input);
        }
    }
    compare(vec![
        tokens[3].clone(),
        tokens[0].clone(),
        tokens[4].clone(),
        tokens[1].clone(),
        tokens[5].clone(),
        tokens[0].clone(),
    ]);
}

#[test]
fn final_emission_moves_selected_output_storage() {
    for count in [1, 3] {
        let output = "payload".repeat(1024);
        let pointer = output.as_ptr();
        let mut input = vec![json!({"type":"function_call_output", "call_id":"a", "output":null})];
        input[0]["output"] = Value::String(output);
        for _ in 0..count {
            input.push(json!({"type":"function_call", "call_id":"a"}));
        }
        let actual = reorder_outputs(input, TOOL_OUTPUT_MISSING_TEXT, |_, _| {});
        let outputs: Vec<_> = actual
            .iter()
            .filter(|item| item["type"] == "function_call_output")
            .collect();
        assert_eq!(outputs.len(), count);
        assert_eq!(
            outputs.last().unwrap()["output"].as_str().unwrap().as_ptr(),
            pointer
        );
        for item in outputs.iter().take(count - 1) {
            assert_ne!(item["output"].as_str().unwrap().as_ptr(), pointer);
        }
    }
}

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
struct CountingAllocator;
thread_local! {
    static ALLOCATED: Cell<Option<usize>> = const { Cell::new(None) };
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.with(|bytes| {
            if let Some(n) = bytes.get() {
                bytes.set(Some(n + layout.size()));
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATED.with(|bytes| {
            if let Some(n) = bytes.get() {
                bytes.set(Some(n + size));
            }
        });
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[test]
#[ignore = "alternating release CPU and cumulative allocation benchmark"]
fn normalization_release_benchmark() {
    use std::hint::black_box;
    use std::time::Instant;
    for (name, calls, bytes, iterations) in [
        ("small", 1, 32, 2000),
        ("many", 128, 256, 100),
        ("1MiB", 16, 1 << 20, 8),
    ] {
        let input: Vec<_> = (0..calls).flat_map(|id| [
            json!({"type":"function_call", "call_id":id.to_string(), "name":"read", "arguments":"{}"}),
            json!({"type":"function_call_output", "call_id":id.to_string(), "output":"x".repeat(bytes)})
        ]).collect();
        let mut elapsed = [Vec::new(), Vec::new()];
        let mut allocations = [0, 0];
        for round in 0..12 {
            for which in [round % 2, 1 - round % 2] {
                let mut nanos = 0;
                for _ in 0..iterations {
                    let owned = input.clone(); // Exclude fixture construction from both measurements.
                    ALLOCATED.with(|n| n.set(Some(0)));
                    let start = Instant::now();
                    let result = if which == 0 {
                        legacy_reorder_outputs(owned, TOOL_OUTPUT_MISSING_TEXT, |_, _| {})
                    } else {
                        reorder_outputs(owned, TOOL_OUTPUT_MISSING_TEXT, |_, _| {})
                    };
                    nanos += start.elapsed().as_nanos();
                    allocations[which] = ALLOCATED.with(|n| n.replace(None).unwrap());
                    black_box(result);
                }
                elapsed[which].push(nanos / iterations as u128);
            }
        }
        for times in &mut elapsed {
            times.sort();
        }
        let old = elapsed[0][6];
        let new = elapsed[1][6];
        eprintln!(
            "{name}: legacy={old}ns optimized={new}ns ratio={:.3} allocated={} -> {} bytes",
            new as f64 / old as f64,
            allocations[0],
            allocations[1]
        );
        assert!(allocations[1] < allocations[0]);
        assert!(new <= old * 110 / 100, "CPU regression in {name}");
    }
}
