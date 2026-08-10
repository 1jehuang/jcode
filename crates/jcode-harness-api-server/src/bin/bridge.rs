//! Standalone harness API bridge daemon.
//!
//! Usage: jcode-harness-api-bridge [api_socket] [legacy_socket]

// Each API client has its own translation task. Some translation paths still
// perform bounded synchronous archive/config I/O, so a single-thread runtime
// lets one busy desktop connection prevent the accept loop from even replying
// to the next client's `hello`. That made Ctrl+Shift+N wait for the full
// handshake timeout while the socket misleadingly remained healthy. Keep an
// executor thread available for accepts and fresh-session handshakes.
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
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
