//! Frozen pre-ownership request oracle and isolated per-thread allocation/CPU probe.
use super::*;
use serde_json::json;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! { static COUNTS: Cell<Option<(usize, usize)>> = const { Cell::new(None) }; }
struct CountingAllocator;
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        COUNTS.with(|c| {
            if let Some((calls, bytes)) = c.get() {
                c.set(Some((calls + 1, bytes + layout.size())))
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        COUNTS.with(|c| {
            if let Some((calls, bytes)) = c.get() {
                c.set(Some((calls + 1, bytes + size)))
            }
        });
        unsafe { System.realloc(ptr, layout, size) }
    }
}

pub(crate) fn measure_allocations<T>(f: impl FnOnce() -> T) -> (T, (usize, usize)) {
    COUNTS.with(|counts| {
        assert!(
            counts.replace(Some((0, 0))).is_none(),
            "nested allocation measurement"
        );
    });
    let result = f();
    let counts = COUNTS.with(|counts| counts.replace(None).expect("active allocation measurement"));
    (result, counts)
}

// Frozen original request builder from d521b8240, before moving owned input.
#[expect(clippy::too_many_arguments)]
fn original(
    model_id: &str,
    instructions: String,
    input: &[Value],
    api_tools: &[Value],
    is_chatgpt_mode: bool,
    max_output_tokens: Option<u32>,
    reasoning_effort: Option<&str>,
    service_tier: Option<&str>,
    prompt_cache_key: Option<&str>,
    prompt_cache_retention: Option<&str>,
    native_compaction_threshold: Option<usize>,
) -> Value {
    let mut tools = api_tools.to_vec();
    if is_chatgpt_mode && model_supports_image_generation(model_id) {
        tools.push(json!({"type":"image_generation"}));
    }
    let mut request = json!({
        "model": model_id, "instructions": instructions, "input": input,
        "tools": tools, "tool_choice":"auto", "parallel_tool_calls":false,
        "stream":true, "store":false, "include":["reasoning.encrypted_content"]
    });
    if !is_chatgpt_mode && let Some(max_output_tokens) = max_output_tokens {
        request["max_output_tokens"] = json!(max_output_tokens);
    }
    if let Some(effort) = reasoning_effort {
        request["reasoning"] = openai_stream_runtime::reasoning_payload(effort);
    }
    if let Some(service_tier) = service_tier {
        request["service_tier"] = json!(service_tier);
    }
    if let Some(compact_threshold) = native_compaction_threshold {
        request["context_management"] =
            json!([{"type":"compaction","compact_threshold":compact_threshold}]);
    }
    if let Some(key) = prompt_cache_key {
        request["prompt_cache_key"] = json!(key);
    }
    if !is_chatgpt_mode
        && let Some(retention) =
            OpenAIProvider::effective_prompt_cache_retention(model_id, prompt_cache_retention)
    {
        request["prompt_cache_retention"] = json!(retention);
    }
    request
}

fn histories() -> Vec<(&'static str, Vec<Value>)> {
    vec![
        ("empty", vec![]),
        ("tiny", vec![json!({"role":"user","content":"hi"})]),
        ("large", (0..256).map(|n| json!({"role":"user","content":format!("{n}: {}", "history ".repeat(512))})).collect()),
        ("nested", (0..64).map(|n| json!({"type":"function_call_output","call_id":format!("c{n}"),"output":{"records":(0..32).map(|i|json!({"n":i,"data":[{"key":"value","text":"nested ".repeat(16)}]})).collect::<Vec<_>>()}})).collect()),
        ("native", (0..128).flat_map(|n| [json!({"type":"reasoning","id":format!("r{n}"),"encrypted_content":"encrypted ".repeat(128),"summary":[{"type":"summary_text","text":"reasoning summary"}]}),json!({"type":"web_search_call","id":format!("w{n}"),"action":{"type":"search","query":"history query","sources":[{"url":"https://example.org","title":"source"}]}})]).collect()),
    ]
}

#[test]
fn owned_request_matches_frozen_oracle_and_retains_input_storage() {
    for (name, input) in histories() {
        for model in ["gpt-5.4", "gpt-5.5", "gpt-5.6", "gpt-5.3-codex"] {
            for chatgpt in [false, true] {
                for configured in [false, true] {
                    let tools = if configured {
                        vec![
                            json!({"type":"function","name":"test","parameters":{"type":"object"}}),
                        ]
                    } else {
                        vec![]
                    };
                    let effort = configured.then_some("high");
                    let tier = configured.then_some("priority");
                    let key = configured.then_some("cache-key");
                    let retention = configured.then_some("24h");
                    let threshold = configured.then_some(120_000);
                    let expected = original(
                        model,
                        "system\nverbatim".into(),
                        &input,
                        &tools,
                        chatgpt,
                        Some(4096),
                        effort,
                        tier,
                        key,
                        retention,
                        threshold,
                    );
                    let owned = input.clone();
                    let ptr = owned.as_ptr();
                    let actual = OpenAIProvider::build_response_request(
                        model,
                        "system\nverbatim".into(),
                        owned,
                        &tools,
                        chatgpt,
                        Some(4096),
                        effort,
                        tier,
                        key,
                        retention,
                        threshold,
                    );
                    assert_eq!(actual, expected, "{name}/{model}/{chatgpt}/{configured}");
                    assert_eq!(
                        serde_json::to_vec(&actual).unwrap(),
                        serde_json::to_vec(&expected).unwrap()
                    );
                    assert_eq!(
                        actual["input"].as_array().unwrap().as_ptr(),
                        ptr,
                        "history storage must move"
                    );
                    let borrowed = OpenAIProvider::build_response_request(
                        model,
                        "system\nverbatim".into(),
                        input.as_slice(),
                        &tools,
                        chatgpt,
                        Some(4096),
                        effort,
                        tier,
                        key,
                        retention,
                        threshold,
                    );
                    assert_eq!(borrowed, expected, "borrowed users remain supported");
                    // Canonical logging and continuation see exactly the same full items.
                    assert_eq!(actual["input"], json!(input));
                    assert_eq!(
                        persistent_ws_input_item_hashes(actual["input"].as_array().unwrap()),
                        persistent_ws_input_item_hashes(&input)
                    );
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn thread_cpu_ns() -> u64 {
    #[repr(C)]
    struct Timespec {
        sec: std::os::raw::c_long,
        nsec: std::os::raw::c_long,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
    }
    let mut time = Timespec { sec: 0, nsec: 0 };
    assert_eq!(unsafe { clock_gettime(3, &mut time) }, 0);
    time.sec as u64 * 1_000_000_000 + time.nsec as u64
}

#[cfg(target_os = "linux")]
fn old_pipeline(input: Vec<Value>) -> Value {
    original(
        "gpt-5.4",
        "system".into(),
        &input,
        &[],
        false,
        None,
        None,
        None,
        None,
        None,
        None,
    )
}
#[cfg(target_os = "linux")]
fn new_pipeline(input: Vec<Value>) -> Value {
    OpenAIProvider::build_response_request(
        "gpt-5.4",
        "system".into(),
        input,
        &[],
        false,
        None,
        None,
        None,
        None,
        None,
        None,
    )
}

#[test]
#[cfg(target_os = "linux")]
#[ignore = "explicit alternating Linux thread CPU and allocation probe"]
fn owned_request_alternating_thread_cpu_probe() {
    use std::hint::black_box;
    for (name, input) in histories() {
        let iterations = if input.len() > 10 { 20 } else { 20_000 };
        let mut old = Vec::new();
        let mut new = Vec::new();
        for round in 0..12 {
            for optimized in if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                // Both complete paths already own their input. Prepare it outside measured work.
                let batches: Vec<_> = (0..iterations).map(|_| input.clone()).collect();
                let (_, allocations) = measure_allocations(|| {
                    for history in batches {
                        black_box(if optimized {
                            new_pipeline(black_box(history))
                        } else {
                            old_pipeline(black_box(history))
                        });
                    }
                });
                let batches: Vec<_> = (0..iterations).map(|_| input.clone()).collect();
                let start = thread_cpu_ns();
                for history in batches {
                    black_box(if optimized {
                        new_pipeline(black_box(history))
                    } else {
                        old_pipeline(black_box(history))
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
            "{name}: old={:?} new={:?}, iterations={iterations}, CPU ratio={:.3}",
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
