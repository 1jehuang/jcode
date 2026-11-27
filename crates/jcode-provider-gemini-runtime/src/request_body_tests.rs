//! Exact legacy wire oracle and isolated allocation/thread-CPU measurements.
use super::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! { static COUNTS: Cell<Option<(usize, usize)>> = const { Cell::new(None) }; }
struct CountingAllocator;
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        COUNTS.with(|c| {
            if let Some((n, b)) = c.get() {
                c.set(Some((n + 1, b + layout.size())))
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        COUNTS.with(|c| {
            if let Some((n, b)) = c.get() {
                c.set(Some((n + 1, b + size)))
            }
        });
        unsafe { System.realloc(ptr, layout, size) }
    }
}
fn allocations<T>(f: impl FnOnce() -> T) -> (T, (usize, usize)) {
    COUNTS.with(|c| assert!(c.replace(Some((0, 0))).is_none()));
    let result = f();
    (result, COUNTS.with(|c| c.replace(None).unwrap()))
}

fn fixture(images: bool) -> Value {
    let mut contents: Vec<Value> = (0..128).map(|i| json!({"role": if i % 2 == 0 { "user" } else { "model" }, "parts": [{"text": "history 雪\\\"\n".repeat(256)}]})).collect();
    if images {
        contents.push(json!({"role":"user", "parts":[{"inlineData":{"mimeType":"image/png", "data":"a+/9".repeat(512 * 1024)}}]}));
    }
    json!({"model":"future-model", "project":"project", "userPromptId":"id", "request":{"contents":contents, "systemInstruction":null, "tools":[{"functionDeclarations":[{"name":"test", "parameters":{"z":null,"a":{"type":"string"}}}]}], "sessionId":"session"}})
}

// Frozen old post_json/post_json_api_key pipeline, including reqwest's JSON encoding.
fn legacy(
    client: &reqwest::Client,
    input: &impl Serialize,
    attempts: usize,
) -> Vec<reqwest::Request> {
    let value = serde_json::to_value(input).unwrap();
    (0..attempts)
        .map(|_| {
            client
                .post("http://localhost/generateContent")
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .json(&value)
                .build()
                .unwrap()
        })
        .collect()
}
fn optimized(
    client: &reqwest::Client,
    input: &impl Serialize,
    attempts: usize,
) -> Vec<reqwest::Request> {
    let body = replayable_json_body(input).unwrap();
    (0..attempts)
        .map(|_| {
            client
                .post("http://localhost/generateContent")
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.clone())
                .build()
                .unwrap()
        })
        .collect()
}
#[test]
fn serialization_failure_retains_context() {
    struct Invalid;
    impl Serialize for Invalid {
        fn serialize<S: serde::Serializer>(&self, _: S) -> std::result::Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("fixture rejection"))
        }
    }
    let error = replayable_json_body(&Invalid).unwrap_err();
    assert_eq!(error.to_string(), "Failed to serialize Gemini request body");
    assert!(format!("{error:#}").contains("fixture rejection"));
}

#[test]
fn replayable_body_exact_legacy_bytes_and_allocation_nonregression() {
    let client = reqwest::Client::new();
    for images in [false, true] {
        let input = fixture(images);
        for attempts in [1, 2, 4] {
            drop(legacy(&client, &input, attempts));
            drop(optimized(&client, &input, attempts));
            let (old, old_alloc) = allocations(|| legacy(&client, &input, attempts));
            let (new, new_alloc) = allocations(|| optimized(&client, &input, attempts));
            if new.len() > 1 {
                assert_eq!(
                    new[0].body().unwrap().as_bytes().unwrap().as_ptr(),
                    new[1].body().unwrap().as_bytes().unwrap().as_ptr(),
                    "retries must share the encoded body"
                );
            }
            for (old, new) in old.iter().zip(&new) {
                assert_eq!(
                    old.body().unwrap().as_bytes(),
                    new.body().unwrap().as_bytes()
                );
                assert_eq!(old.headers(), new.headers());
            }
            eprintln!(
                "gemini images={images} attempts={attempts} allocations old={old_alloc:?} new={new_alloc:?}"
            );
            assert!(new_alloc.0 <= old_alloc.0, "allocation calls regressed");
            assert!(new_alloc.1 <= old_alloc.1, "allocation bytes regressed");
        }
    }
}
#[cfg(target_os = "linux")]
fn thread_cpu_ns() -> u64 {
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
#[test]
#[cfg(target_os = "linux")]
#[ignore = "optimized alternating actual thread CPU measurement"]
fn replayable_body_thread_cpu_probe() {
    let client = reqwest::Client::new();
    for images in [false, true] {
        let input = fixture(images);
        for attempts in [1, 2, 4] {
            drop(legacy(&client, &input, attempts));
            drop(optimized(&client, &input, attempts));
            let mut totals = [0u64; 2];
            for round in 0..16 {
                for which in [round % 2, 1 - round % 2] {
                    let start = thread_cpu_ns();
                    let output = if which == 0 {
                        legacy(&client, &input, attempts)
                    } else {
                        optimized(&client, &input, attempts)
                    };
                    std::hint::black_box(&output);
                    drop(output);
                    totals[which] += thread_cpu_ns() - start;
                }
            }
            eprintln!(
                "gemini CPU images={images} attempts={attempts} legacy_ns={} optimized_ns={}",
                totals[0], totals[1]
            );
            assert!(totals[1] <= totals[0] * 110 / 100, "CPU regressed >10%");
        }
    }
}
