//! Standalone harness API bridge daemon.
//!
//! Usage: jcode-harness-api-bridge [api_socket] [legacy_socket]

#[tokio::main(flavor = "current_thread")]
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
