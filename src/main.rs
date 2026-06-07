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
// jemalloc reads this exact exported symbol name at startup.
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static malloc_conf: Option<&'static [u8; 50]> =
    Some(b"dirty_decay_ms:1000,muzzy_decay_ms:1000,narenas:4\0");

#[cfg(feature = "jemalloc-prof")]
// jemalloc reads this exact exported symbol name at startup.
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
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // The macOS global-hotkey listener must run on the real main thread with a
    // Core Foundation run loop (Carbon `RegisterEventHotKey` delivers events
    // there). Intercept it before building the tokio runtime, which would
    // otherwise move execution onto a worker thread with no run loop and leave
    // the Cmd+; hotkey silently dead.
    if is_macos_hotkey_listener_invocation() {
        return jcode::setup_hints::run_macos_hotkey_listener_main_thread();
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async { jcode::run().await })
}

/// True when invoked as `jcode setup-hotkey --listen-macos-hotkey`.
fn is_macos_hotkey_listener_invocation() -> bool {
    args_are_macos_hotkey_listener(std::env::args().skip(1))
}

fn args_are_macos_hotkey_listener(args: impl IntoIterator<Item = String>) -> bool {
    let args: Vec<String> = args.into_iter().collect();
    args.first().map(String::as_str) == Some("setup-hotkey")
        && args.iter().any(|a| a == "--listen-macos-hotkey")
}

#[cfg(test)]
mod tests {
    use super::args_are_macos_hotkey_listener;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn detects_listener_invocation() {
        assert!(args_are_macos_hotkey_listener(argv(&[
            "setup-hotkey",
            "--listen-macos-hotkey"
        ])));
    }

    #[test]
    fn ignores_plain_setup_hotkey() {
        assert!(!args_are_macos_hotkey_listener(argv(&["setup-hotkey"])));
    }

    #[test]
    fn ignores_other_commands() {
        assert!(!args_are_macos_hotkey_listener(argv(&[
            "serve",
            "--listen-macos-hotkey"
        ])));
        assert!(!args_are_macos_hotkey_listener(argv(&[])));
    }
}
