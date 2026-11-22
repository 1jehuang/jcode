//! Tool JSON-only frozen baseline versus production, alternating thread CPU samples.
//! Run: justrust run --release -p jcode-app-core --example tool_projection_probe
use jcode_message_types::ContentBlock;
use jcode_session_types::StoredMessage;
#[path = "../src/tool/session_search/searchable_text_fixtures.rs"]
mod fixtures;
#[path = "../src/tool/session_search/searchable_text.rs"]
mod projection;
use std::alloc::{GlobalAlloc, Layout, System};
use std::borrow::Cow;
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
struct Counting;
static ENABLE: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
#[global_allocator]
static ALLOCATOR: Counting = Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ENABLE.load(Relaxed) {
            CALLS.fetch_add(1, Relaxed);
            BYTES.fetch_add(layout.size(), Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if ENABLE.load(Relaxed) {
            CALLS.fetch_add(1, Relaxed);
            BYTES.fetch_add(size, Relaxed);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
// Frozen pre-change include_tools projection. All fixtures contain only ToolUse.
fn baseline(msg: &StoredMessage) -> Cow<'_, str> {
    let mut parts: Vec<Cow<'_, str>> = msg
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { name, input, .. } => {
                let input = input.to_string();
                Some(Cow::Owned(if input == "null" {
                    format!("[tool call: {name}]")
                } else {
                    format!("[tool call: {name}] {input}")
                }))
            }
            _ => None,
        })
        .collect();
    match parts.len() {
        0 => Cow::Borrowed(""),
        1 => parts.pop().unwrap(),
        _ => Cow::Owned(parts.join("\n")),
    }
}
fn cpu_ns() -> u128 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) },
        0
    );
    time.tv_sec as u128 * 1_000_000_000 + time.tv_nsec as u128
}
fn execute(msg: &StoredMessage, new: bool) {
    if new {
        black_box(projection::searchable_message_text(black_box(msg), true));
    } else {
        black_box(baseline(black_box(msg)));
    }
}
fn cases() -> Vec<(&'static str, StoredMessage)> {
    use serde_json::{Value, json};
    let call = |name: &str, input: Value| ContentBlock::ToolUse {
        id: "call".into(),
        name: name.into(),
        input,
        thought_signature: None,
    };
    vec![
        ("null", fixtures::message(vec![call("bash", Value::Null)])),
        ("tiny-bool", fixtures::message(vec![call("b", json!(true))])),
        (
            "tiny-number",
            fixtures::message(vec![call("bash", json!(12))]),
        ),
        (
            "tiny-object",
            fixtures::message(vec![call("bash", json!({"x":1}))]),
        ),
        (
            "null-string",
            fixtures::message(vec![call("bash", json!("null"))]),
        ),
        (
            "escaped",
            fixtures::message(vec![call("東京\n\"", json!({"x":"\"\\\n\t\u{0000}東京"}))]),
        ),
        (
            "nested",
            fixtures::message(vec![call(
                "todo",
                json!({"a":[null,true,1,{"x":["東京",false]}],"b":{"c":{"d":"payload"}}}),
            )]),
        ),
        (
            "large",
            fixtures::message(vec![call(
                "bash",
                json!({"command":"abc東京\n".repeat(100_000)}),
            )]),
        ),
        (
            "large-array",
            fixtures::message(vec![call(
                "todo",
                json!(
                    (0..10_000)
                        .map(|i| json!({"name":format!("task {i}"),"done":false}))
                        .collect::<Vec<_>>()
                ),
            )]),
        ),
        (
            "multiple",
            fixtures::message(vec![
                call("a", Value::Null),
                call("b", json!({"x":"\""})),
                call("c", json!(42)),
            ]),
        ),
    ]
}
fn main() {
    for (name, msg) in cases() {
        assert_eq!(
            projection::searchable_message_text(&msg, true),
            baseline(&msg),
            "{name}"
        );
        let mut allocations = vec![];
        for new in [false, true] {
            CALLS.store(0, Relaxed);
            BYTES.store(0, Relaxed);
            ENABLE.store(true, Relaxed);
            execute(&msg, new);
            ENABLE.store(false, Relaxed);
            allocations.push((CALLS.load(Relaxed), BYTES.load(Relaxed)));
        }
        let size = baseline(&msg).len();
        let rounds = 24;
        let iterations = (8_000_000 / size.max(1)).clamp(16, 100_000);
        let mut samples = [vec![], vec![]];
        for _ in 0..100 {
            execute(&msg, false);
            execute(&msg, true);
        }
        for round in 0..rounds {
            for index in [round % 2, 1 - round % 2] {
                let start = cpu_ns();
                for _ in 0..iterations {
                    execute(&msg, index == 1);
                }
                samples[index].push((cpu_ns() - start) / iterations as u128);
            }
        }
        for samples in &mut samples {
            samples.sort_unstable();
        }
        println!(
            "{name}: CPU ns {} -> {}, allocation calls/bytes {:?} -> {:?}",
            samples[0][rounds / 2],
            samples[1][rounds / 2],
            allocations[0],
            allocations[1]
        );
    }
}
#[test]
fn tool_serialization_matches_frozen_oracle() {
    for (name, msg) in cases() {
        assert_eq!(
            projection::searchable_message_text(&msg, true),
            baseline(&msg),
            "{name}"
        );
    }
}
