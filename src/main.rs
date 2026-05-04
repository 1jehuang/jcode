#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// Tune jemalloc for a long-running server with bursty allocations (e.g. loading
// and unloading an ~87 MB ONNX embedding model). The defaults (muzzy_decay_ms:0,
// retain:true, narenas:8*ncpu) caused 1.4 GB RSS in previous testing.
//
// dirty_decay_ms:1000  — return dirty pages to OS after 1 s idle
// muzzy_decay_ms:1000  — release muzzy pages after 1 s
// narenas:4            — limit arena count (17 threads don't need 64 arenas)
// prof:true            — enable profiling support in jemalloc-prof builds
// prof_active:false    — keep sampling disabled until explicitly enabled at runtime
#[cfg(all(feature = "jemalloc", not(feature = "jemalloc-prof")))]
// jemalloc locates its configuration by looking up the exported C symbol `malloc_conf`
// at process startup. The symbol name is defined by the jemalloc ABI and must be exactly
// `malloc_conf` (lower_snake_case). `#[allow(non_upper_case_globals)]` is therefore
// load-bearing and cannot be removed.
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static malloc_conf: Option<&'static [u8; 50]> =
    Some(b"dirty_decay_ms:1000,muzzy_decay_ms:1000,narenas:4\0");

#[cfg(feature = "jemalloc-prof")]
// Same as above: `malloc_conf` is an ABI-defined symbol name required by jemalloc.
// `#[allow(non_upper_case_globals)]` is load-bearing and cannot be removed.
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static malloc_conf: Option<&'static [u8; 78]> =
    Some(b"dirty_decay_ms:1000,muzzy_decay_ms:1000,narenas:4,prof:true,prof_active:false\0");

use anyhow::Result;

#[cfg(all(target_os = "linux", not(feature = "jemalloc")))]
fn configure_system_allocator() {
    unsafe extern "C" {
        fn mallopt(param: i32, value: i32) -> i32;
    }

    const M_ARENA_MAX: i32 = -8;
    let arena_max = std::env::var("JCODE_GLIBC_ARENA_MAX")
        .ok()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(4);

    let _ = unsafe { mallopt(M_ARENA_MAX, arena_max) };
}

#[cfg(not(all(target_os = "linux", not(feature = "jemalloc"))))]
fn configure_system_allocator() {}

fn main() -> Result<()> {
    configure_system_allocator();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async { jcode::run().await })
}
