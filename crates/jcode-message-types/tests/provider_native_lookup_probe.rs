//! Standalone allocator isolates this probe from the library test allocator.
use jcode_message_types::provider_native::{ProviderNativeItems, provider_native_display};
use serde_json::{Value, json};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
thread_local! { static ALLOCS: Cell<Option<(usize, usize)>> = const { Cell::new(None) }; }
struct Counting;
#[global_allocator]
static ALLOCATOR: Counting = Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.with(|n| {
            if let Some((calls, bytes)) = n.get() {
                n.set(Some((calls + 1, bytes + layout.size())))
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCS.with(|n| {
            if let Some((calls, bytes)) = n.get() {
                n.set(Some((calls + 1, bytes + size)))
            }
        });
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[derive(Default)]
struct Recorded {
    native: ProviderNativeItems,
    items: Vec<(usize, String, Value)>,
}
impl Recorded {
    fn push(&mut self, offset: usize, provider: String, item: Value) {
        self.items.push((offset, provider.clone(), item.clone()));
        self.native.push(offset, provider, item);
    }
    fn call_input(&self, id: &str) -> Value {
        self.native.call_input(id)
    }
}
// Frozen original lookup expression from 3de375bd4:323-329.
#[inline(never)]
fn original(items: &Recorded, id: &str) -> Value {
    items
        .items
        .iter()
        .filter_map(|(_, provider, item)| provider_native_display(provider, item))
        .find(|display| display.id == id && display.output.is_none())
        .and_then(|display| display.input)
        .unwrap_or(Value::Null)
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
#[ignore = "explicit alternating thread CPU and allocation probe"]
#[cfg(target_os = "linux")]
fn lookup_alternating_thread_cpu_probe() {
    use std::hint::black_box;
    for (name, count, results, hit) in [
        ("empty", 0, 0, false),
        ("tiny-call-only", 0, 0, true),
        ("tiny-hit", 1, 0, true),
        ("tiny-nohit", 1, 0, false),
        ("large-results-hit", 100, 100, true),
        ("large-results-nohit", 100, 100, false),
    ] {
        let mut items = Recorded::default();
        for i in 0..count {
            items.push(0,"anthropic".into(),json!({"type":"web_search_tool_result","tool_use_id":format!("r{i}"),"content":(0..results).map(|n|json!({"title":format!("Result {n}"),"url":"https://example.org/long-result"})).collect::<Vec<_>>()}));
            items.push(0,"openai".into(),json!({"type":"web_search_call","id":format!("o{i}"),"action":{"query":"query","sources":[{"title":"source","url":"https://example.org"}]}}));
        }
        if hit {
            items.push(
                0,
                "anthropic".into(),
                json!({"type":"server_tool_use","id":"target","input":{"query":"query"}}),
            );
        }
        let iterations = if count > 10 { 100 } else { 100_000 };
        let mut old = Vec::new();
        let mut new = Vec::new();
        for round in 0..12 {
            for optimized in if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                // Count separately so allocation-counter updates are not timed.
                ALLOCS.with(|v| v.set(Some((0, 0))));
                for _ in 0..iterations {
                    black_box(if optimized {
                        black_box(&items).call_input(black_box("target"))
                    } else {
                        original(black_box(&items), black_box("target"))
                    });
                }
                let allocs = ALLOCS.with(|v| v.replace(None).unwrap());
                let start = thread_cpu_ns();
                for _ in 0..iterations {
                    black_box(if optimized {
                        black_box(&items).call_input(black_box("target"))
                    } else {
                        original(black_box(&items), black_box("target"))
                    });
                }
                let elapsed = thread_cpu_ns() - start;
                if optimized {
                    new.push((elapsed, allocs));
                } else {
                    old.push((elapsed, allocs));
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
        assert!(new[6].1.0 <= old[6].1.0);
        assert!(new[6].1.1 <= old[6].1.1);
        assert!(new[6].0 <= old[6].0 * 110 / 100, "CPU regression {name}");
    }
}
