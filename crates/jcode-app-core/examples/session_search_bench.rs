//! Invoke the public search tool on a private corpus; print no result contents.
use jcode_app_core::tool::{SessionSearchTool, Tool, ToolContext, ToolExecutionMode};
use sha2::{Digest, Sha256};
use std::time::Instant;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let query = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("missing query"))?;
    let exhaustive = std::env::args().nth(2).as_deref() == Some("exhaustive");
    let start = Instant::now();
    let output = SessionSearchTool::new()
        .execute(
            serde_json::json!({
                "query":query, "source":"jcode", "include_external":false,
                "include_current":true, "include_tools":true, "include_system":true,
                "limit":50, "max_scan_sessions":10000, "exhaustive":exhaustive
            }),
            ToolContext {
                session_id: "benchmark-current".into(),
                message_id: "benchmark-message".into(),
                tool_call_id: "benchmark-search".into(),
                working_dir: None,
                stdin_request_tx: None,
                graceful_shutdown_signal: None,
                execution_mode: ToolExecutionMode::Direct,
            },
        )
        .await?;
    anyhow::ensure!(
        output.output.contains("### Result ") || output.output.starts_with("No results found for "),
        "tool did not execute a search; response withheld"
    );
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    let results = output
        .output
        .lines()
        .filter(|line| line.starts_with("### Result "))
        .count();
    // Candidate counts are diagnostic, not part of result equivalence.
    let result_text = output
        .output
        .lines()
        .filter(|line| !line.starts_with("_Scanned:"))
        .collect::<Vec<_>>()
        .join("\n");
    println!(
        "{}",
        serde_json::json!({"elapsed_ms":elapsed_ms,"results":results,
        "result_sha256":format!("{:x}",Sha256::digest(result_text.as_bytes()))})
    );
    Ok(())
}
