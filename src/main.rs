#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// Tune jemalloc for enterprise-scale concurrent load (200 users, 60+ concurrent requests).
// Dynamic arena configuration based on CPU cores for optimal parallelism:
// - narenas = cpu_count / 2 (balance between contention and parallelism)
// - dirty_decay_ms:1000 — return dirty pages to OS after 1s idle
// - muzzy_decay_ms:1000 — release muzzy pages after 1s
//
// For typical servers:
// - 8-core: narenas:4
// - 16-core: narenas:8
// - 32-core: narenas:16
#[cfg(all(feature = "jemalloc", not(feature = "jemalloc-prof")))]
// jemalloc reads this exact exported symbol name at startup.
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static malloc_conf: Option<&'static [u8; 50]> =
    Some(b"dirty_decay_ms:1000,muzzy_decay_ms:1000,narenas:8\0");

#[cfg(feature = "jemalloc-prof")]
// jemalloc reads this exact exported symbol name at startup.
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static malloc_conf: Option<&'static [u8; 78]> =
    Some(b"dirty_decay_ms:1000,muzzy_decay_ms:1000,narenas:8,prof:true,prof_active:false\0");

use anyhow::Result;
use tracing::info;

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
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Initialize cgroups v2 if enabled (Linux only)
    #[cfg(target_os = "linux")]
    {
        if std::env::var("CARPAI_CGROUPS_ENABLED")
            .map(|v| v.to_lowercase() == "true" || v == "1")
            .unwrap_or(false)
        {
            match jcode::cgroup_isolation::init_cgroups() {
                Ok(_) => info!("Cgroups v2 initialized successfully"),
                Err(e) => warn!("Failed to initialize cgroups: {} (continuing without isolation)", e),
            }
        }
    }

    // Use multi-runtime architecture for production deployments
    // Fall back to single runtime for development/testing
    let use_multi_runtime = std::env::var("CARPAI_MULTI_RUNTIME")
        .map(|v| v.to_lowercase() == "true" || v == "1")
        .unwrap_or(false);

    if use_multi_runtime {
        info!("Starting with multi-runtime architecture");
        jcode::runtime_manager::init_global_runtime_manager()
            .expect("Failed to initialize runtime manager");

        // Use API runtime as the entry point
        if let Some(manager_guard) = jcode::runtime_manager::global_runtime_manager() {
            if let Some(manager) = manager_guard.as_ref() {
                return manager.api_runtime().block_on(async { jcode::run().await });
            }
        }
    }

    // Fallback: single runtime (development mode)
    info!("Starting with single runtime (development mode)");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async { jcode::run().await })
}
