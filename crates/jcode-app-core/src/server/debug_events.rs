use super::state::MAX_EVENT_HISTORY;
use super::{SwarmEvent, SwarmEventType};
use anyhow::Result;
use std::sync::Arc;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{RwLock, broadcast};

pub(super) async fn maybe_handle_event_query_command(
    cmd: &str,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
) -> Option<String> {
    if cmd == "events:recent" || cmd.starts_with("events:recent:") {
        let count: usize = cmd
            .strip_prefix("events:recent:")
            .and_then(|s| s.parse().ok())
            .unwrap_or(50);

        let history = event_history.read().await;
        let events: Vec<serde_json::Value> = history
            .iter()
            .rev()
            .take(count)
            .map(event_payload)
            .collect();
        return Some(serde_json::to_string_pretty(&events).unwrap_or_else(|_| "[]".to_string()));
    }

    if cmd.starts_with("events:since:") {
        let since_id: u64 = cmd
            .strip_prefix("events:since:")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let history = event_history.read().await;
        let events: Vec<serde_json::Value> = history
            .iter()
            .filter(|event| event.id > since_id)
            .map(event_payload)
            .collect();
        return Some(serde_json::to_string_pretty(&events).unwrap_or_else(|_| "[]".to_string()));
    }

    if cmd == "events:types" {
        return Some(
            serde_json::json!({
                "types": [
                    "file_touch",
                    "notification",
                    "plan_update",
                    "plan_proposal",
                    "context_update",
                    "status_change",
                    "member_change"
                ],
                "description": "Use events:recent, events:since:<id>, or events:subscribe to get events"
            })
            .to_string(),
        );
    }

    if cmd == "events:count" {
        let history = event_history.read().await;
        let latest_id = history.back().map(|event| event.id).unwrap_or(0);
        return Some(
            serde_json::json!({
                "count": history.len(),
                "latest_id": latest_id,
                "max_history": MAX_EVENT_HISTORY,
            })
            .to_string(),
        );
    }

    None
}

pub(super) async fn maybe_handle_event_subscription_command<W: AsyncWrite + Unpin>(
    id: u64,
    cmd: &str,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    writer: &mut W,
) -> Result<bool> {
    if cmd != "events:subscribe" && !cmd.starts_with("events:subscribe:") {
        return Ok(false);
    }

    let type_filter: Option<Vec<String>> = cmd
        .strip_prefix("events:subscribe:")
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());

    let ack = crate::protocol::ServerEvent::DebugResponse {
        id,
        ok: true,
        output: serde_json::json!({
            "subscribed": true,
            "filter": type_filter.as_ref().map(|f| f.join(",")),
        })
        .to_string(),
    };
    let json = crate::protocol::encode_event(&ack);
    writer.write_all(json.as_bytes()).await?;

    let mut rx = swarm_event_tx.subscribe();
    loop {
        match rx.recv().await {
            Ok(event) => {
                let event_type = match &event.event {
                    SwarmEventType::FileTouch { .. } => "file_touch",
                    SwarmEventType::Notification { .. } => "notification",
                    SwarmEventType::PlanUpdate { .. } => "plan_update",
                    SwarmEventType::PlanProposal { .. } => "plan_proposal",
                    SwarmEventType::ContextUpdate { .. } => "context_update",
                    SwarmEventType::StatusChange { .. } => "status_change",
                    SwarmEventType::MemberChange { .. } => "member_change",
                };
                if let Some(ref filter) = type_filter
                    && !filter.iter().any(|f| f == event_type)
                {
                    continue;
                }
                let timestamp_unix = event
                    .absolute_time
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let event_json = serde_json::json!({
                    "type": "event",
                    "id": event.id,
                    "session_id": event.session_id,
                    "session_name": event.session_name,
                    "swarm_id": event.swarm_id,
                    "event": event.event,
                    "timestamp_unix": timestamp_unix,
                });
                let mut line = serde_json::to_string(&event_json).unwrap_or_default();
                line.push('\n');
                if writer.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                let lag_json = serde_json::json!({
                    "type": "lag",
                    "missed": missed,
                });
                let mut line = serde_json::to_string(&lag_json).unwrap_or_default();
                line.push('\n');
                if writer.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }

    Ok(true)
}

fn event_payload(event: &SwarmEvent) -> serde_json::Value {
    let timestamp_unix = event
        .absolute_time
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    serde_json::json!({
        "id": event.id,
        "session_id": event.session_id,
        "session_name": event.session_name,
        "swarm_id": event.swarm_id,
        "event": event.event,
        "age_secs": event.timestamp.elapsed().as_secs(),
        "timestamp_unix": timestamp_unix,
    })
}
