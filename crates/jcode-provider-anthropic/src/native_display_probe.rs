//! Exact-output legacy comparison and opt-in real thread CPU/allocation probe.
use super::native_display_legacy::*;
use super::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;

thread_local! {
    static COUNTS: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
}
struct CountingAllocator;
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
fn record(bytes: usize) {
    let _ = COUNTS.try_with(|counts| {
        if let Some((calls, total)) = counts.get() {
            counts.set(Some((calls + 1, total + bytes as u64)));
        }
    });
}
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
/// Counts allocation/reallocation requests and requested bytes, not RSS or live memory.
pub(super) fn measure_allocations<T>(f: impl FnOnce() -> T) -> (T, (u64, u64)) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            COUNTS.with(|counts| counts.set(None));
        }
    }
    COUNTS.with(|counts| {
        assert!(counts.get().is_none());
        counts.set(Some((0, 0)));
    });
    let reset = Reset;
    let value = f();
    let counts = COUNTS.with(|counts| counts.get().unwrap());
    drop(reset);
    (value, counts)
}
#[cfg(target_os = "linux")]
pub(super) fn thread_cpu_ns() -> u64 {
    #[repr(C)]
    struct Timespec {
        seconds: std::ffi::c_long,
        nanoseconds: std::ffi::c_long,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: std::ffi::c_int, time: *mut Timespec) -> std::ffi::c_int;
    }
    let mut time = Timespec {
        seconds: 0,
        nanoseconds: 0,
    };
    // Linux CLOCK_THREAD_CPUTIME_ID, no libc package dependency required.
    assert_eq!(unsafe { clock_gettime(3, &mut time) }, 0);
    time.seconds as u64 * 1_000_000_000 + time.nanoseconds as u64
}
fn native(provider: &str, item: Value) -> ContentBlock {
    ContentBlock::ProviderNative {
        provider: provider.into(),
        item,
    }
}
fn cases() -> Vec<(&'static str, Vec<ContentBlock>)> {
    let pair = |query: String, title: String, payload: String| {
        vec![
            native(
                "anthropic",
                json!({"type":"server_tool_use","id":"same","input":{"query":query,"payload":payload}}),
            ),
            native(
                "anthropic",
                json!({"type":"web_search_tool_result","tool_use_id":"same","content":[{"title":title,"url":"https://example.test","encrypted_content":"opaque"}]}),
            ),
        ]
    };
    let mut mixed = pair("original".into(), "result".into(), String::new());
    mixed.extend([
        native("openai", json!({"type":"web_search_call","id":"same","status":"searching","action":{"query":"replacement"}})),
        native("anthropic", json!({"type":"web_search_tool_result","tool_use_id":"same","content":null})),
        native("openai", json!({"type":"web_search_call","id":"same","action":{"queries":["one","two"],"sources":[{"title":"source","url":"u"}]}})),
    ]);
    vec![
        (
            "text",
            vec![ContentBlock::Text {
                text: "ordinary text".repeat(30),
                cache_control: None,
            }],
        ),
        (
            "native",
            pair("query".into(), "title".into(), String::new()),
        ),
        (
            "large",
            pair(
                "large query".repeat(100),
                "result".repeat(1000),
                "input".repeat(10000),
            ),
        ),
        (
            "unicode",
            pair(
                "日本語 🦀 café".repeat(20),
                "标题 🔎".repeat(30),
                String::new(),
            ),
        ),
        ("mixed-collisions", mixed),
    ]
}
fn assert_same(blocks: &[ContentBlock]) {
    for oauth in [false, true] {
        for replay in [false, true] {
            let old = legacy_format_content_blocks_with_native(blocks, oauth, replay);
            let accepted =
                super::native_display_accepted::accepted_format_content_blocks_with_native(
                    blocks, oauth, replay,
                );
            let new = format_content_blocks_with_native(blocks, oauth, replay);
            assert_eq!(
                serde_json::to_value(&accepted).unwrap(),
                serde_json::to_value(&new).unwrap()
            );
            assert_eq!(
                serde_json::to_value(&old).unwrap(),
                serde_json::to_value(&new).unwrap()
            );
            let mut old = vec![ApiMessage {
                role: "assistant".into(),
                content: old,
            }];
            let mut new = vec![ApiMessage {
                role: "assistant".into(),
                content: new,
            }];
            assert_eq!(
                legacy_downgrade_native_replay(&mut old),
                downgrade_native_replay(&mut new)
            );
            assert_eq!(
                serde_json::to_value(old).unwrap(),
                serde_json::to_value(new).unwrap()
            );
        }
    }
}
#[test]
fn native_display_frozen_legacy_exact_outputs() {
    for (_, blocks) in cases() {
        assert_same(&blocks);
    }
    let fields = [
        Value::Null,
        json!(0),
        json!(""),
        json!("same"),
        json!({}),
        json!([]),
    ];
    for provider in ["anthropic", "openai", "unknown", ""] {
        for kind in [
            "server_tool_use",
            "web_search_tool_result",
            "web_search_call",
            "unknown",
        ] {
            for value in &fields {
                assert_same(&[native(
                    provider,
                    json!({"type":kind,"id":value,"tool_use_id":value,"input":value,"content":value,"action":value,"status":value}),
                )]);
            }
        }
    }
    // Missing/malformed raw inputs must overwrite the previous same-id query,
    // and fallback starts can replace borrowed entries before another raw call.
    for input in [
        None,
        Some(Value::Null),
        Some(json!(42)),
        Some(json!({"query":""})),
        Some(json!({"query":"新しい"})),
    ] {
        let mut call = json!({"type":"server_tool_use","id":"same"});
        if let Some(input) = input {
            call["input"] = input;
        }
        let mut sequence = cases().pop().unwrap().1;
        sequence.extend([
            native("anthropic", call),
            native("anthropic", json!({"type":"web_search_tool_result","tool_use_id":"same","content":[]})),
            native("openai", json!({"type":"web_search_call","id":"same","status":"in_progress","action":{"query":"owned replacement"}})),
            native("anthropic", json!({"type":"web_search_tool_result","tool_use_id":"same","content":[]})),
        ]);
        assert_same(&sequence);
    }
    // Downgrade also receives arbitrary raw items, not just formatter-produced ones.
    let mut old = vec![
        ApiMessage {
            role: "assistant".into(),
            content: vec![],
        },
        ApiMessage {
            role: "assistant".into(),
            content: vec![ApiContentBlock::Raw(Value::Null)],
        },
        ApiMessage {
            role: "assistant".into(),
            content: vec![ApiContentBlock::Raw(json!({"type":"unknown"}))],
        },
    ];
    let mut new = old.clone();
    assert_eq!(
        legacy_downgrade_native_replay(&mut old),
        downgrade_native_replay(&mut new)
    );
    assert_eq!(
        serde_json::to_value(old).unwrap(),
        serde_json::to_value(new).unwrap()
    );
    // Missing fields and non-object raw payloads are replayed just as before.
    for item in [
        Value::Null,
        json!([]),
        json!({"type":"server_tool_use"}),
        json!({"type":"web_search_tool_result"}),
    ] {
        assert_same(&[native("anthropic", item)]);
    }
}
#[test]
#[ignore = "opt-in optimized actual thread CPU and allocation measurement"]
#[cfg(target_os = "linux")]
fn native_display_actual_cpu_allocation_probe() {
    let iterations = 2000;
    assert!(
        !cfg!(debug_assertions),
        "CPU probe requires an optimized build"
    );
    for (name, blocks) in cases() {
        for mode in ["fallback", "replay", "downgrade"] {
            let replay = mode != "fallback";
            let raw = format_content_blocks_with_native(&blocks, false, true);
            let run = |legacy: bool| {
                if mode == "downgrade" {
                    let mut messages = vec![ApiMessage {
                        role: "assistant".into(),
                        content: raw.clone(),
                    }];
                    if legacy {
                        black_box(legacy_downgrade_native_replay(&mut messages));
                    } else {
                        black_box(downgrade_native_replay(&mut messages));
                    }
                    black_box(messages);
                } else {
                    black_box(if legacy {
                        legacy_format_content_blocks_with_native(&blocks, false, replay)
                    } else {
                        format_content_blocks_with_native(&blocks, false, replay)
                    });
                }
            };
            for _ in 0..20 {
                run(true);
                run(false);
            }
            let mut totals = [(0u64, 0u64, 0u64); 2];
            for round in 0..16 {
                for index in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                    let start = thread_cpu_ns();
                    let (_, (calls, bytes)) = measure_allocations(|| {
                        for _ in 0..iterations {
                            run(index == 0);
                        }
                    });
                    totals[index].0 += thread_cpu_ns() - start;
                    totals[index].1 += calls;
                    totals[index].2 += bytes;
                }
            }
            eprintln!(
                "native-probe {name}/{mode} n={} legacy(cpu_ns,alloc_calls,requested_bytes)={:?} optimized={:?}",
                iterations * 16,
                totals[0],
                totals[1]
            );
        }
    }
}

#[test]
#[ignore = "optimized three-way actual thread CPU/allocation probe"]
#[cfg(target_os = "linux")]
fn borrowed_replay_actual_cpu_allocation_probe() {
    assert!(!cfg!(debug_assertions));
    let iterations = 2000;
    for (name, blocks) in cases() {
        for replay in [false, true] {
            let formatters = [
                legacy_format_content_blocks_with_native,
                super::native_display_accepted::accepted_format_content_blocks_with_native,
                format_content_blocks_with_native,
            ];
            for formatter in formatters {
                for _ in 0..40 {
                    black_box(formatter(black_box(&blocks), false, replay));
                }
            }
            let mut totals = [(0u64, 0u64, 0u64); 3];
            let mut samples = [[0u64; 18]; 3];
            let mut sample_counts = [0usize; 3];
            // Balanced six permutations avoid favoring any implementation by order.
            for order in [
                [0, 1, 2],
                [2, 1, 0],
                [1, 2, 0],
                [0, 2, 1],
                [2, 0, 1],
                [1, 0, 2],
            ]
            .into_iter()
            .cycle()
            .take(18)
            {
                for index in order {
                    let start = thread_cpu_ns();
                    let (_, (calls, bytes)) = measure_allocations(|| {
                        for _ in 0..iterations {
                            black_box(formatters[index](black_box(&blocks), false, replay));
                        }
                    });
                    let cpu = thread_cpu_ns() - start;
                    samples[index][sample_counts[index]] = cpu;
                    sample_counts[index] += 1;
                    totals[index].0 += cpu;
                    totals[index].1 += calls;
                    totals[index].2 += bytes;
                }
            }
            eprintln!(
                "borrow-probe {name}/replay={replay} n={} original={:?} accepted3de375bd4={:?} borrowed={:?} (cpu_ns,alloc_calls,requested_bytes)",
                iterations * 18,
                totals[0],
                totals[1],
                totals[2]
            );
            let medians = samples.map(|mut row| {
                row.sort_unstable();
                (row[8] + row[9]) / 2
            });
            eprintln!(
                "borrow-probe {name}/replay={replay} median_batch_cpu_ns original/accepted/borrowed={medians:?}"
            );
            assert!(totals[2].1 <= totals[1].1);
            assert!(totals[2].2 <= totals[1].2);
        }
    }
}
