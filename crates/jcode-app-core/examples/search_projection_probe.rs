//! `justrust run --release -p jcode-app-core --example search_projection_probe`
//! Actual Linux thread CPU, cumulative allocation traffic, not RSS.
//! Hosted optimized 16-round alternating probe, 2026-11-18: small 50 -> 24 ns,
//! many text blocks 4344 -> 1829 ns; 1.5 MB single text allocated 3,000,096
//! -> 96 bytes and 1.95 MB result 3,900,096 -> 96 bytes. Every one of eleven
//! fixture shapes improved CPU, including tool JSON, intent and nested todos.
//! Fixture setup is excluded, projection result destruction is included.
//! A narrower plain-single-block followup eliminated the remaining 96 bytes:
//! small 54 -> 3 ns, many blocks 3306 -> 1183 ns, empty separators 72 -> 56 ns.
//! A generic first/second iterator-chain version was rejected because the
//! empty-separator case regressed 108 -> 144 ns despite fewer allocations.
use jcode_message_types::ContentBlock;
use jcode_session_types::StoredMessage;
#[path = "../src/tool/session_search/searchable_text_fixtures.rs"]
mod fixtures;
#[path = "../src/tool/session_search/searchable_text_legacy.rs"]
mod oracle;
#[path = "../src/tool/session_search/searchable_text.rs"]
mod projection;
use std::alloc::{GlobalAlloc, Layout, System};
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
fn execute(msg: &StoredMessage, tools: bool, new: bool) {
    if new {
        black_box(projection::searchable_message_text(black_box(msg), tools));
    } else {
        black_box(oracle::legacy(black_box(msg), tools));
    }
}
fn main() {
    for (name, msg, tools) in fixtures::cases() {
        assert_eq!(
            projection::searchable_message_text(&msg, tools),
            oracle::legacy(&msg, tools)
        );
        let mut allocations = vec![];
        for new in [false, true] {
            CALLS.store(0, Relaxed);
            BYTES.store(0, Relaxed);
            ENABLE.store(true, Relaxed);
            execute(&msg, tools, new);
            ENABLE.store(false, Relaxed);
            allocations.push((CALLS.load(Relaxed), BYTES.load(Relaxed)));
        }
        let bytes = msg
            .content
            .iter()
            .map(|b| match b {
                ContentBlock::Text { text, .. } => text.len(),
                ContentBlock::ToolResult { content, .. } => content.len(),
                _ => 0,
            })
            .sum::<usize>();
        let rounds = 16;
        let iterations = (2_000_000 / bytes.max(1)).clamp(16, 20_000);
        let mut samples = [vec![], vec![]];
        for round in 0..rounds {
            for index in [round % 2, 1 - round % 2] {
                let start = cpu_ns();
                for _ in 0..iterations {
                    execute(&msg, tools, index == 1);
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
