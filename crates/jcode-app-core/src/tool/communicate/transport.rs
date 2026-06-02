use crate::protocol::{Request, ServerEvent};
use anyhow::Result;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn request_type_from_json(json: &str) -> String {
    serde_json::from_str::<Value>(json)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string())
}

pub(super) async fn send_request(request: Request) -> Result<ServerEvent> {
    send_request_with_timeout(request, None).await
}

pub(super) async fn send_request_with_timeout(
    request: Request,
    timeout: Option<std::time::Duration>,
) -> Result<ServerEvent> {
    let path = crate::server::socket_path();
    let stream = crate::server::connect_socket(&path).await?;
    let (reader, mut writer) = stream.into_split();

    let request_id = request.id();
    let deadline =
        tokio::time::Instant::now() + timeout.unwrap_or(std::time::Duration::from_secs(30));

    let json = serde_json::to_string(&request)? + "\n";
    let request_type = request_type_from_json(&json);
    writer.write_all(json.as_bytes()).await?;

    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    // Read lines until we find the terminal response for our request ID.
    // Skip: ack events, notification events, swarm_status broadcasts, etc.
    // Terminal events: done, error, comm_spawn_response, comm_await_members_response,
    //                  and any other typed response with matching id.
    loop {
        line.clear();
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            crate::logging::warn(&format!(
                "[tool:communicate] request timed out type={} id={} socket={} after waiting for response",
                request_type,
                request_id,
                path.display()
            ));
            anyhow::bail!("Timed out waiting for response")
        }
        let n = tokio::time::timeout(remaining, reader.read_line(&mut line)).await??;
        if n == 0 {
            crate::logging::warn(&format!(
                "[tool:communicate] connection closed before response type={} id={} socket={}",
                request_type,
                request_id,
                path.display()
            ));
            return Err(anyhow::anyhow!(
                "Connection closed before receiving response"
            ));
        }

        let value: Value = serde_json::from_str(line.trim()).map_err(|err| {
            crate::logging::warn(&format!(
                "[tool:communicate] failed to parse response type={} id={} socket={} error={} payload={}",
                request_type,
                request_id,
                path.display(),
                err,
                crate::util::truncate_str(line.trim(), 240)
            ));
            err
        })?;

        let event_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let event_id = value.get("id").and_then(|v| v.as_u64());

        if event_type != "ack" && event_id != Some(request_id) {
            continue;
        }

        match event_type {
            // Skip ack — not a response
            "ack" => continue,
            // Skip broadcast/async events that are not tied to our request
            "swarm_status"
            | "swarm_plan"
            | "swarm_plan_proposal"
            | "swarm_event"
            | "notification"
            | "soft_interrupt_injected"
            | "session"
            | "session_id"
            | "history"
            | "mcp_status"
            | "memory_injected"
            | "compaction"
            | "connection_type"
            | "connection_phase"
            | "status_detail"
            | "upstream_provider"
            | "reloading"
            | "reload_progress"
            | "available_models_updated"
            | "side_panel_state"
            | "transcript"
            | "interrupted" => continue,
            // Terminal responses and typed request responses with matching ids.
            _ => return Ok(serde_json::from_value(value)?),
        }
    }
}
