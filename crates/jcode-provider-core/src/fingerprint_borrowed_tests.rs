//! Borrowed object parity and manual actual-thread CPU/allocation acceptance probe.
//!
//! Two hosted optimized 20-pair runs (20261118-044433509-2437540 and
//! 20261118-044522387-2440087) showed new/old actual thread CPU ratios:
//! empty .467/.443, tiny .526/.497, large .981/.982, nested .798/.795,
//! all-settings .496/.495. Large requested bytes 3,096,394 -> 2,034,372,
//! calls 140 -> 10. Counter disabled during CPU loops, separate allocation pass.
//! Baseline includes the historical temporary canonical Value construction and
//! destruction. Both paths include all fingerprint buffers and result drops.
use super::*;
use serde_json::json;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;

thread_local! { static ALLOCATIONS: Cell<Option<(usize, usize)>> = const { Cell::new(None) }; }
struct CountingAllocator;
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| {
            if let Some((calls, bytes)) = count.get() {
                count.set(Some((calls + 1, bytes + layout.size())))
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.with(|count| {
            if let Some((calls, bytes)) = count.get() {
                count.set(Some((calls + 1, bytes + size)))
            }
        });
        unsafe { System.realloc(ptr, layout, size) }
    }
}

const KEYS: [&str; 12] = [
    "context_management",
    "include",
    "input",
    "instructions",
    "model",
    "parallel_tool_calls",
    "prompt_cache_key",
    "prompt_cache_retention",
    "reasoning",
    "service_tier",
    "tool_choice",
    "tools",
];
fn fields(request: &Value) -> [(&str, &Value); 12] {
    KEYS.map(|key| (key, request.get(key).unwrap_or(&Value::Null)))
}

// Exact old OpenAI canonical object construction, before borrowing fields.
fn original_payload(request: &Value) -> Value {
    json!({
        "model":request.get("model"), "instructions":request.get("instructions"),
        "input":request.get("input"), "tools":request.get("tools"),
        "tool_choice":request.get("tool_choice"), "parallel_tool_calls":request.get("parallel_tool_calls"),
        "reasoning":request.get("reasoning"), "context_management":request.get("context_management"),
        "include":request.get("include"), "service_tier":request.get("service_tier"),
        "prompt_cache_key":request.get("prompt_cache_key"), "prompt_cache_retention":request.get("prompt_cache_retention")
    })
}
fn fingerprint(request: &Value, borrowed: bool) -> CanonicalFingerprints {
    let items = request
        .get("input")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if borrowed {
        object_fingerprints(
            fields(request),
            items,
            request.get("instructions"),
            request.get("tools"),
        )
        .unwrap()
    } else {
        let payload = original_payload(request);
        canonical_fingerprints(
            &payload,
            items,
            request.get("instructions"),
            request.get("tools"),
        )
    }
}
fn fixtures() -> Vec<(&'static str, Value)> {
    vec![
        ("empty", json!({})),
        (
            "tiny",
            json!({"model":"fixture","input":[{"role":"user","content":"hi"}],"instructions":"help","tools":[]}),
        ),
        (
            "large",
            json!({"model":"fixture","input":(0..16).map(|i|json!({"type":"function_call_output","call_id":i.to_string(),"output":"data ".repeat(13108)})).collect::<Vec<_>>(),"instructions":"help","tools":[]}),
        ),
        (
            "nested",
            json!({"model":"fixture","input":(0..128).map(|i|json!({"content":[{"type":"text","text":"é 日本語\n\t\"\\".repeat(32)}],"index":i})).collect::<Vec<_>>(),"instructions":"help","tools":[{"type":"function","parameters":{"properties":{"a":{"type":"string"}}}}]}),
        ),
        (
            "all-settings",
            json!({"model":"fixture","input":[],"instructions":null,"tools":[],"parallel_tool_calls":false,"tool_choice":"auto","reasoning":{"effort":"high"},"context_management":[{"type":"compaction","compact_threshold":10000}],"include":["reasoning.encrypted_content"],"service_tier":"priority","prompt_cache_key":"a\n\"é","prompt_cache_retention":"24h"}),
        ),
    ]
}

#[test]
fn borrowed_fields_preserve_independent_legacy_hashes_and_lengths() {
    assert!(KEYS.windows(2).all(|keys| keys[0] < keys[1]));
    for (name, request) in fixtures() {
        let expected = original_payload(&request);
        let actual = fingerprint(&request, true);
        assert_eq!(actual, fingerprint(&request, false), "{name}");
        assert_eq!(actual.request, stable_json_fingerprint(&expected), "{name}");
        let items = request
            .get("input")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        assert_eq!(
            actual.items.0,
            items.iter().map(stable_hash_json).collect::<Vec<_>>()
        );
        assert_eq!(actual.items.1, stable_hash_json(items));
        assert_eq!(
            actual.system,
            request.get("instructions").map(stable_json_fingerprint)
        );
        assert_eq!(
            actual.tools,
            request.get("tools").map(stable_json_fingerprint)
        );
    }
    let items = vec![json!({"a":"same"})];
    let payload = json!({"a":[{"a":"same"}],"b":[{"a":"same"}],"c":null,"é\n\"":true});
    let pairs = payload
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| (key.as_str(), value))
        .collect::<Vec<_>>();
    assert_eq!(
        object_fingerprints(pairs, &items, Some(&Value::Null), Some(&json!([]))).unwrap(),
        canonical_fingerprints(&payload, &items, Some(&Value::Null), Some(&json!([])))
    );
}

#[cfg(target_os = "linux")]
fn thread_cpu_ns() -> u64 {
    #[repr(C)]
    struct Timespec {
        sec: std::os::raw::c_long,
        nsec: std::os::raw::c_long,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: std::os::raw::c_int, ts: *mut Timespec) -> std::os::raw::c_int;
    }
    let mut ts = Timespec { sec: 0, nsec: 0 };
    assert_eq!(unsafe { clock_gettime(3, &mut ts) }, 0);
    ts.sec as u64 * 1_000_000_000 + ts.nsec as u64
}

#[test]
#[cfg(target_os = "linux")]
#[ignore = "manual alternating actual CPU/allocation probe: --release --lib benchmark_borrowed_fields -- --ignored --nocapture --test-threads=1"]
fn benchmark_borrowed_fields() {
    for (name, request) in fixtures() {
        let iterations = if name == "large" {
            4
        } else if name == "nested" {
            24
        } else {
            2000
        };
        let mut times = [Vec::new(), Vec::new()];
        for round in 0..20 {
            for mode in [round % 2, 1 - round % 2] {
                let start = thread_cpu_ns();
                for _ in 0..iterations {
                    black_box(fingerprint(black_box(&request), mode == 1));
                }
                times[mode].push((thread_cpu_ns() - start) / iterations);
            }
        }
        for values in &mut times {
            values.sort_unstable();
        }
        let mut allocations = [(0, 0); 2];
        for (mode, result) in allocations.iter_mut().enumerate() {
            ALLOCATIONS.with(|count| count.set(Some((0, 0))));
            black_box(fingerprint(&request, mode == 1));
            *result = ALLOCATIONS.with(|count| count.replace(None).unwrap());
        }
        println!(
            "{name}: CPU ns {} -> {}, ratio {:.4}, allocations {:?} -> {:?}",
            times[0][10],
            times[1][10],
            times[1][10] as f64 / times[0][10] as f64,
            allocations[0],
            allocations[1]
        );
        assert!(allocations[1].0 <= allocations[0].0 && allocations[1].1 <= allocations[0].1);
    }
}
