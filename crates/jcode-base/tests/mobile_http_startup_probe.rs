//! Standalone startup benchmark: JUSTRUST ... --test mobile_http_startup_probe.
//! Run this executable directly in fresh processes with MOBILE_HTTP_MODE=two|shared.
//! CPU and allocation passes are separate: MOBILE_HTTP_COUNT=1 enables counting.
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

struct Counting;
static ENABLED: AtomicBool = AtomicBool::new(false);
static BYTES: AtomicU64 = AtomicU64::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() && ENABLED.load(Ordering::Relaxed) {
            BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            CALLS.fetch_add(1, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) }
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(p, layout, size) };
        if !next.is_null() && ENABLED.load(Ordering::Relaxed) {
            BYTES.fetch_add(size as u64, Ordering::Relaxed);
            CALLS.fetch_add(1, Ordering::Relaxed);
        }
        next
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[cfg(target_os = "linux")]
fn cpu_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) },
        0
    );
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

#[test]
#[ignore = "explicit optimized startup CPU/allocation probe"]
#[cfg(target_os = "linux")]
fn mobile_http_startup_probe() {
    let mode = std::env::var("MOBILE_HTTP_MODE").unwrap_or_else(|_| "shared".into());
    let count = std::env::var_os("MOBILE_HTTP_COUNT").is_some();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _guard = runtime.enter();
    ENABLED.store(count, Ordering::Relaxed);
    let cpu = cpu_ns();
    let wall = Instant::now();
    // Exactly the old two constructors versus the new OnceLock initializer and
    // two owned handles. The retained third handle models the static owner.
    let first = reqwest::Client::new();
    let (sync, runner) = if mode == "two" {
        (first.clone(), reqwest::Client::new())
    } else {
        (first.clone(), first.clone())
    };
    black_box((&sync, &runner));
    let startup_wall_ns = wall.elapsed().as_nanos();
    let startup_cpu_ns = cpu_ns() - cpu;
    ENABLED.store(false, Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
    let calls = CALLS.load(Ordering::Relaxed);
    // Warm identical work: the loops already retain their handles. Build their
    // normal per-request auth and timeout, without performing network I/O.
    BYTES.store(0, Ordering::Relaxed);
    CALLS.store(0, Ordering::Relaxed);
    ENABLED.store(count, Ordering::Relaxed);
    let cpu = cpu_ns();
    for _ in 0..10_000 {
        black_box(
            sync.get("http://127.0.0.1/mobile/sessions")
                .bearer_auth("sync-key")
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap(),
        );
        black_box(
            runner
                .post("http://127.0.0.1/mobile/commands/claim-local")
                .bearer_auth("runner-key")
                .timeout(std::time::Duration::from_secs(30))
                .json(&serde_json::json!({"machine":"test", "sessions":[]}))
                .build()
                .unwrap(),
        );
    }
    let warm_cpu_ns = cpu_ns() - cpu;
    ENABLED.store(false, Ordering::Relaxed);
    println!(
        "mobile_http mode={mode} counted={count} startup_cpu_ns={startup_cpu_ns} startup_wall_ns={startup_wall_ns} startup_bytes={bytes} startup_calls={calls} warm_cpu_ns={warm_cpu_ns} warm_bytes={} warm_calls={}",
        BYTES.load(Ordering::Relaxed),
        CALLS.load(Ordering::Relaxed)
    );
}
