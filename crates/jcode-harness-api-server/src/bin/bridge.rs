//! Standalone harness API bridge daemon.
//!
//! Usage: jcode-harness-api-bridge [api_socket] [legacy_socket]

// Each API client has its own translation task. Some translation paths still
// perform bounded synchronous archive/config I/O, so a single-thread runtime
// lets one busy desktop connection prevent the accept loop from even replying
// to the next client's `hello`. That made Ctrl+Shift+N wait for the full
// handshake timeout while the socket misleadingly remained healthy. Keep an
// executor thread available for accepts and fresh-session handshakes.
//
// `block_in_place` hands the worker role to a blocking-pool thread, and
// Tokio's default pool allows 512 threads kept alive for 10 s. Each one got
// its own glibc arena, which is how an idle bridge reached 34 threads and
// ~94 MB. Bound the pool and let idle threads exit quickly.
fn main() -> anyhow::Result<()> {
    // Tooling (reload-bridge preflight) probes the binary with `--help`.
    // Without this, the flag was taken as the API socket path and the probe
    // started a real bridge listening on `./--help` that never exited.
    if let Some(flag) = std::env::args().nth(1).filter(|arg| arg.starts_with('-')) {
        match flag.as_str() {
            "-h" | "--help" => {
                println!(
                    "usage: jcode-harness-api-bridge [API_SOCKET] [LEGACY_SOCKET]\n\
                     Translates the harness API onto the jcode daemon socket."
                );
                return Ok(());
            }
            "-V" | "--version" => {
                println!("jcode-harness-api-bridge {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            other => anyhow::bail!("unknown option {other}; see --help"),
        }
    }
    jcode_harness_api_server::allocator::configure();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(8)
        .thread_keep_alive(std::time::Duration::from_secs(2))
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let api_socket = args
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(jcode_harness_api_server::api_socket_path);
    let legacy_socket = args
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(jcode_harness_api_server::legacy_socket_path);
    jcode_harness_api_server::run_bridge(api_socket, legacy_socket).await
}
