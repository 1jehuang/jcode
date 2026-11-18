//! Ignored optimized CPU/allocation probe. Run with:
//! justrust test -p jcode-session-types --release tokenization_probe -- --ignored --nocapture
use super::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::time::Instant;

#[cfg(target_os = "linux")]
fn thread_cpu_ns() -> u128 {
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

struct CountingAllocator;
thread_local! {
    static COUNTS: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = COUNTS.try_with(|counts| {
            if let Some((calls, bytes)) = counts.get() {
                counts.set(Some((calls + 1, bytes + layout.size())));
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, old: Layout, size: usize) -> *mut u8 {
        let _ = COUNTS.try_with(|counts| {
            if let Some((calls, bytes)) = counts.get() {
                counts.set(Some((calls + 1, bytes + size)));
            }
        });
        unsafe { System.realloc(ptr, old, size) }
    }
}

fn optimized(text: &str) -> Vec<String> {
    tokenize_prelowercased_session_search_text(&text.to_lowercase())
}

#[test]
#[ignore = "optimized CPU/allocation probe"]
fn tokenization_probe() {
    assert!(!cfg!(debug_assertions), "run --release");
    let cases = [
        ("small-ascii", "Find alpha ALPHA and beta 42".to_owned()),
        (
            "small-unicode",
            "ΟΣ ΟΣΑ İSTANBUL É e\u{301} Straße".to_owned(),
        ),
        (
            "many-tokens",
            (0..2000).map(|i| format!("Token{i} ")).collect(),
        ),
        (
            "large-ascii",
            "THE alpha ALPHA beta 123 gamma delta ".repeat(4000),
        ),
        (
            "large-unicode",
            "ΟΣ ΟΣΑ İSTANBUL É e\u{301} Straße 中文 ".repeat(4000),
        ),
    ];
    for (name, text) in cases {
        assert_eq!(legacy(&text), optimized(&text));
        let mut allocations = Vec::new();
        for f in [legacy as fn(&str) -> Vec<String>, optimized] {
            COUNTS.with(|c| c.set(Some((0, 0))));
            let result = f(&text);
            let counts = COUNTS.with(|c| c.replace(None).unwrap());
            black_box(result);
            allocations.push(counts);
        }
        assert!(allocations[1].0 <= allocations[0].0);
        assert!(allocations[1].1 <= allocations[0].1);
        let iterations = (500_000 / text.len().max(1)).clamp(8, 5000);
        let mut samples = [Vec::new(), Vec::new()];
        for round in 0..15 {
            for slot in 0..2 {
                let index = (round + slot) % 2;
                let f = [legacy as fn(&str) -> Vec<String>, optimized][index];
                let start = Instant::now();
                #[cfg(target_os = "linux")]
                let cpu_start = thread_cpu_ns();
                for _ in 0..iterations {
                    black_box(f(black_box(&text)));
                }
                #[cfg(target_os = "linux")]
                let elapsed = thread_cpu_ns() - cpu_start;
                #[cfg(not(target_os = "linux"))]
                let elapsed = start.elapsed().as_nanos();
                black_box(start.elapsed());
                samples[index].push(elapsed / iterations as u128);
            }
        }
        for times in &mut samples {
            times.sort_unstable();
        }
        println!(
            "{name}: median ns (Linux thread CPU, otherwise elapsed) legacy={} optimized={} ratio={:.3}, allocations/bytes {:?} -> {:?}",
            samples[0][7],
            samples[1][7],
            samples[1][7] as f64 / samples[0][7] as f64,
            allocations[0],
            allocations[1]
        );
    }
}
