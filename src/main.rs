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

#[cfg(all(target_os = "linux", target_env = "gnu", not(feature = "jemalloc")))]
fn configure_system_allocator() {
    unsafe extern "C" {
        fn mallopt(param: i32, value: i32) -> i32;
    }

    const M_ARENA_MAX: i32 = -8;
    const M_MMAP_THRESHOLD: i32 = -3;

    let arena_max = parse_alloc_tuning_env("JCODE_GLIBC_ARENA_MAX", 4);
    let _ = unsafe { mallopt(M_ARENA_MAX, arena_max) };

    // Pin the mmap threshold so large transient allocations (history JSON,
    // provider payloads) are served by mmap and returned to the OS
    // immediately on free, instead of landing in sbrk arenas where freed
    // blocks below the top chunk become permanent RSS retention.
    //
    // Tradeoff: setting M_MMAP_THRESHOLD via mallopt disables glibc's
    // dynamic threshold growth (normally the threshold rises toward 32 MiB
    // as large blocks are freed, keeping hot large buffers in the arena for
    // cheap reuse). Pinning trades some throughput on repeated large
    // alloc/free cycles (mmap/munmap syscalls + page faults each time) for
    // predictable, immediate memory return. For a long-running interactive
    // agent, lower steady-state RSS wins.
    let mmap_threshold = parse_alloc_tuning_env("JCODE_GLIBC_MMAP_THRESHOLD", 256 * 1024);
    let _ = unsafe { mallopt(M_MMAP_THRESHOLD, mmap_threshold) };
}

/// Parse a positive i32 allocator tuning knob from an env var, falling back
/// to `default` when unset, unparsable, or non-positive.
#[cfg(all(target_os = "linux", target_env = "gnu", not(feature = "jemalloc")))]
fn parse_alloc_tuning_env(var: &str, default: i32) -> i32 {
    parse_alloc_tuning(std::env::var(var).ok().as_deref(), default)
}

/// Pure parsing core of [`parse_alloc_tuning_env`], separated for unit tests.
#[cfg(any(
    test,
    all(target_os = "linux", target_env = "gnu", not(feature = "jemalloc"))
))]
fn parse_alloc_tuning(value: Option<&str>, default: i32) -> i32 {
    value
        .and_then(|value| value.trim().parse::<i32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

#[cfg(not(all(target_os = "linux", target_env = "gnu", not(feature = "jemalloc"))))]
fn configure_system_allocator() {}

#[cfg(windows)]
fn main() -> Result<()> {
    // Windows executables default to a much smaller main-thread stack than the
    // Unix environments where most development happens. The CLI/provider setup
    // path can exceed that reserve before Tokio takes over, producing an
    // unrecoverable STATUS_STACK_OVERFLOW. Keep the linker defaults unchanged
    // for every auxiliary binary and run the Jcode entry point on a deliberately
    // sized stack instead.
    const WINDOWS_MAIN_STACK_SIZE: usize = 8 * 1024 * 1024;
    match std::thread::Builder::new()
        .name("jcode-main".to_string())
        .stack_size(WINDOWS_MAIN_STACK_SIZE)
        .spawn(run_main)?
        .join()
    {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[cfg(not(windows))]
fn main() -> Result<()> {
    run_main()
}

fn run_main() -> Result<()> {
    configure_system_allocator();
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // SessionStart hooks should be effectively invisible to Claude Code and
    // Codex. Handle this tiny callback before the Tokio runtime and normal Jcode
    // startup path so it does not initialize providers, start cleanup threads,
    // check for updates, or emit first-run telemetry disclosure text into the
    // parent CLI's hook output.
    if let Some(source) = cli_launch_hint_source_invocation() {
        return jcode::setup_hints::run_setup_hotkey(false, false, false, Some(&source));
    }

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

fn cli_launch_hint_source_invocation() -> Option<String> {
    cli_launch_hint_source(std::env::args().skip(1))
}

fn cli_launch_hint_source(args: impl IntoIterator<Item = String>) -> Option<String> {
    let args: Vec<String> = args.into_iter().collect();
    if args.first().map(String::as_str) != Some("setup-hotkey") {
        return None;
    }
    let index = args.iter().position(|arg| arg == "--notify-cli-launch")?;
    args.get(index + 1).cloned()
}

#[cfg(test)]
mod tests {
    use super::args_are_macos_hotkey_listener;
    use super::cli_launch_hint_source;
    use super::parse_alloc_tuning;

    #[test]
    fn alloc_tuning_uses_default_when_unset() {
        assert_eq!(parse_alloc_tuning(None, 262_144), 262_144);
    }

    #[test]
    fn alloc_tuning_parses_positive_value_with_whitespace() {
        assert_eq!(parse_alloc_tuning(Some(" 131072 "), 262_144), 131_072);
    }

    #[test]
    fn alloc_tuning_rejects_garbage_zero_and_negative() {
        assert_eq!(parse_alloc_tuning(Some("not-a-number"), 4), 4);
        assert_eq!(parse_alloc_tuning(Some("0"), 4), 4);
        assert_eq!(parse_alloc_tuning(Some("-1"), 4), 4);
        // i32 overflow falls back to default rather than wrapping.
        assert_eq!(parse_alloc_tuning(Some("4294967296"), 4), 4);
    }

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

    #[test]
    fn detects_cli_launch_hint_callback() {
        assert_eq!(
            cli_launch_hint_source(argv(&["setup-hotkey", "--notify-cli-launch", "claude"])),
            Some("claude".to_string())
        );
    }

    #[test]
    fn ignores_launch_hint_flag_on_other_commands_or_without_value() {
        assert_eq!(
            cli_launch_hint_source(argv(&["serve", "--notify-cli-launch", "codex"])),
            None
        );
        assert_eq!(
            cli_launch_hint_source(argv(&["setup-hotkey", "--notify-cli-launch"])),
            None
        );
    }
}
