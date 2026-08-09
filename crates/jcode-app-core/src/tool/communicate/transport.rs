use crate::protocol::{Request, ServerEvent};
use anyhow::Result;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const SERVER_NOT_RUNNING: &str = "jcode server is not running; start it with jcode server start";

fn map_socket_connection_error(err: anyhow::Error) -> anyhow::Error {
    let server_is_unavailable = err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_err| {
                matches!(
                    io_err.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                )
            })
    });

    if server_is_unavailable {
        anyhow::anyhow!(SERVER_NOT_RUNNING)
    } else {
        err
    }
}

async fn connect_swarm_socket(path: &std::path::Path) -> Result<crate::transport::Stream> {
    crate::server::connect_socket(path)
        .await
        .map_err(map_socket_connection_error)
}

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
    let stream = connect_swarm_socket(&path).await?;
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

#[cfg(test)]
mod tests {
    use super::{SERVER_NOT_RUNNING, connect_swarm_socket};

    #[tokio::test]
    async fn missing_daemon_socket_has_actionable_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("missing.sock");

        let err = connect_swarm_socket(&socket_path)
            .await
            .expect_err("missing socket should fail");

        assert_eq!(err.to_string(), SERVER_NOT_RUNNING);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refused_daemon_socket_has_actionable_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("refused.sock");
        {
            let _listener = crate::transport::Listener::bind(&socket_path).expect("bind listener");
        }

        let err = connect_swarm_socket(&socket_path)
            .await
            .expect_err("stale socket should refuse the connection");

        assert_eq!(err.to_string(), SERVER_NOT_RUNNING);
    }
}
