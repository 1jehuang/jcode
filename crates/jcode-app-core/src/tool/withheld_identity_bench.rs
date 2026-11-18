//! Standalone optimized benchmark. Point a scratch Cargo binary target at this
//! file, with serde, serde_json and the workspace jcode-tool-core dependencies.
//! Run `justrust run --release --manifest-path <scratch>/Cargo.toml`.
//! Allocation instrumentation is enabled only for separate allocation passes.
#[path = "withheld_identity.rs"]
mod identity;
use serde_json::{Value, json};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hash::{Hash, Hasher};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::Instant;

struct CountingAllocator;
static COUNT: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNT.load(Relaxed) {
            CALLS.fetch_add(1, Relaxed);
            BYTES.fetch_add(layout.size() as u64, Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if COUNT.load(Relaxed) {
            CALLS.fetch_add(1, Relaxed);
            BYTES.fetch_add(size as u64, Relaxed);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
fn legacy(tool: &str, input: &Value) -> u64 {
    let mut input = input.clone();
    if let Some(object) = input.as_object_mut() {
        object.remove("intent");
        object.remove(jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY);
        object.retain(|_, value| !value.is_null());
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tool.hash(&mut hasher);
    input.to_string().hash(&mut hasher);
    hasher.finish()
}
fn pass(f: fn(&str, &Value) -> u64, value: &Value, iterations: usize) -> u128 {
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(f(black_box("bash"), black_box(value)));
    }
    start.elapsed().as_nanos()
}
fn allocations(f: fn(&str, &Value) -> u64, value: &Value) -> (u64, u64) {
    CALLS.store(0, Relaxed);
    BYTES.store(0, Relaxed);
    COUNT.store(true, Relaxed);
    pass(f, value, 100);
    COUNT.store(false, Relaxed);
    (CALLS.load(Relaxed) / 100, BYTES.load(Relaxed) / 100)
}
fn main() {
    let mut large = serde_json::Map::new();
    for i in 0..128 {
        large.insert(
            format!("key-{i}"),
            json!({"text":"雪🦕".repeat(32),"null":null,"array":[1,2,3]}),
        );
    }
    let cases = [
        ("null", Value::Null),
        ("number", json!(42)),
        ("string", json!("雪🦕".repeat(64))),
        ("empty", json!({})),
        (
            "filtered",
            json!({"intent":"label", jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY:true,"x":null}),
        ),
        (
            "small",
            json!({"command":"ls -la", "intent":"inspect", "optional":null}),
        ),
        (
            "nested",
            json!({"data":[{"intent":"keep","n":null,"s":"雪"}, null, [null, 42]]}),
        ),
        ("large", Value::Object(large)),
    ];
    for (name, value) in cases {
        assert_eq!(
            legacy("bash", &value),
            identity::withheld_call_key("bash", &value)
        );
        let iterations = if name == "large" { 300 } else { 30_000 };
        pass(legacy, &value, iterations);
        pass(identity::withheld_call_key, &value, iterations);
        let mut old = Vec::new();
        let mut new = Vec::new();
        for round in 0..16 {
            if round % 2 == 0 {
                old.push(pass(legacy, &value, iterations));
                new.push(pass(identity::withheld_call_key, &value, iterations));
            } else {
                new.push(pass(identity::withheld_call_key, &value, iterations));
                old.push(pass(legacy, &value, iterations));
            }
        }
        old.sort_unstable();
        new.sort_unstable();
        print!(
            "{name}: old {:.1} ns new {:.1} ns ratio {:.3}",
            old[8] as f64 / iterations as f64,
            new[8] as f64 / iterations as f64,
            new[8] as f64 / old[8] as f64
        );
        let mut allocation_results = [(0, 0); 2];
        for round in 0..16 {
            let order = if round % 2 == 0 { [0, 1] } else { [1, 0] };
            for index in order {
                let f = [
                    legacy as fn(&str, &Value) -> u64,
                    identity::withheld_call_key,
                ][index];
                let result = allocations(f, &value);
                if round != 0 {
                    assert_eq!(result, allocation_results[index]);
                }
                allocation_results[index] = result;
            }
        }
        assert!(allocation_results[1].0 <= allocation_results[0].0);
        assert!(allocation_results[1].1 <= allocation_results[0].1);
        for (calls, bytes) in allocation_results {
            print!(" | allocs {calls} bytes {bytes}");
        }
        println!();
    }
}
