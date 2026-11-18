//! Run with `justrust run --release -p jcode-app-core --example browser_size_probe`.
//! Measures serialization CPU and allocation traffic, not process RSS.
use jcode_app_core::process_memory::estimate_json_bytes;
use serde_json::{Value, json};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Counting;
static COUNT: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNT.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if COUNT.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(size, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

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
fn sample(values: &[Value], iterations: usize, count: bool) -> (u128, usize, usize) {
    let allocations = ALLOCS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
    let start = cpu_ns();
    for _ in 0..iterations {
        for value in values {
            black_box(if count {
                estimate_json_bytes(black_box(value))
            } else {
                black_box(value).to_string().len()
            });
        }
    }
    (
        cpu_ns() - start,
        ALLOCS.load(Ordering::Relaxed) - allocations,
        BYTES.load(Ordering::Relaxed) - bytes,
    )
}
fn main() {
    let workloads = [
        (
            "small",
            vec![json!({"ok":true,"id":7,"text":"hello"})],
            100_000,
        ),
        (
            "many",
            (0..240)
                .map(|i| json!({"id":i,"label":"click \"é😀\"\n","ok":true}))
                .collect(),
            1_000,
        ),
        (
            "large",
            vec![
                json!({"text":"abc\"\\\n😀".repeat(16_000),"elements":(0..2000).map(|i|json!({"id":i,"text":"label"})).collect::<Vec<_>>()}),
            ],
            200,
        ),
    ];
    for (name, values, iterations) in workloads {
        for value in &values {
            assert_eq!(estimate_json_bytes(value), value.to_string().len());
        }
        sample(&values, 10, false);
        sample(&values, 10, true);
        let mut old = Vec::new();
        let mut new = Vec::new();
        for round in 0..10 {
            let (a, b) = if round % 2 == 0 {
                (
                    sample(&values, iterations, false),
                    sample(&values, iterations, true),
                )
            } else {
                let b = sample(&values, iterations, true);
                (sample(&values, iterations, false), b)
            };
            COUNT.store(true, Ordering::Relaxed);
            let old_alloc = sample(&values, 1, false);
            let new_alloc = sample(&values, 1, true);
            COUNT.store(false, Ordering::Relaxed);
            println!(
                "{name} round={round} legacy_cpu_ns={} count_cpu_ns={} legacy_allocs={} count_allocs={} legacy_alloc_bytes={} count_alloc_bytes={}",
                a.0, b.0, old_alloc.1, new_alloc.1, old_alloc.2, new_alloc.2
            );
            old.push(a.0);
            new.push(b.0);
        }
        old.sort_unstable();
        new.sort_unstable();
        println!(
            "{name} median_cpu_ratio={:.4}",
            new[5] as f64 / old[5] as f64
        );
        assert!(new[5] <= old[5], "CPU regression in {name}");
    }
}
