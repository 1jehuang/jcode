//! Manual check for the machine-wide MCP broker.
//!
//! Does what a jcode daemon does at startup (load the MCP config and connect
//! every shared server through `SharedMcpPool`), lists the tools, calls one if
//! asked, then holds the connections open for `HOLD_SECS` so several copies
//! can be counted side by side.
//!
//!   JCODE_HOME=/tmp/x JCODE_MCP_BROKER_BIN=target/release/jcode \
//!     cargo run --release -p jcode-base --example mcp_broker_probe

use std::time::Duration;

#[tokio::main]
async fn main() {
    let pool = jcode_base::mcp::SharedMcpPool::from_default_config();
    let started = std::time::Instant::now();
    let (ok, failed) = pool.connect_all().await;
    println!(
        "pid={} connected={ok} failed={} in {:?}",
        std::process::id(),
        failed.len(),
        started.elapsed()
    );
    for (name, error) in &failed {
        println!("  FAIL {name}: {error}");
    }
    let tools = pool.all_tools().await;
    println!("tools={}", tools.len());
    if let Ok(spec) = std::env::var("PROBE_CALL") {
        // PROBE_CALL=server:tool:{json args}
        let mut parts = spec.splitn(3, ':');
        let (server, tool, args) = (
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or("{}"),
        );
        let args = serde_json::from_str(args).unwrap_or_default();
        match pool.call_tool(server, tool, args).await {
            Ok(result) => println!("call ok error={:?}", result.is_error),
            Err(error) => println!("call FAIL {error:#}"),
        }
    }
    let hold = std::env::var("HOLD_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    tokio::time::sleep(Duration::from_secs(hold)).await;
}
