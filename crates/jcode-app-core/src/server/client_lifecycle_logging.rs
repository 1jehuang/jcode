use crate::protocol::Request;

pub(super) fn interrupt_request_log_fields(
    request: &Request,
    client_session_id: &str,
    client_is_processing: bool,
    message_id: Option<u64>,
    has_task: bool,
    line_bytes: usize,
) -> Option<String> {
    let base = |kind: &str, id: u64| {
        format!(
            "kind={} id={} session={} client_processing={} message_id={:?} has_task={} line_bytes={}",
            kind, id, client_session_id, client_is_processing, message_id, has_task, line_bytes
        )
    };

    match request {
        Request::Cancel { id } => Some(base("cancel", *id)),
        Request::SoftInterrupt {
            id,
            content,
            urgent,
        } => Some(format!(
            "{} urgent={} content_bytes={} content_chars={}",
            base("soft_interrupt", *id),
            urgent,
            content.len(),
            content.chars().count()
        )),
        Request::CancelSoftInterrupts { id } => Some(base("cancel_soft_interrupts", *id)),
        Request::BackgroundTool { id } => Some(base("background_tool", *id)),
        _ => None,
    }
}

pub(super) fn request_type_from_line(line: &str) -> String {
    serde_json::from_str::<serde_json::Value>(line.trim())
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(|kind| kind.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string())
}

pub(super) fn request_type_is_read_only(kind: &str) -> bool {
    matches!(
        kind,
        "ping"
            | "state"
            | "get_history"
            | "get_model_catalog"
            | "get_compacted_history"
            | "agent_capabilities"
            | "agent_context"
            | "comm_read"
            | "comm_list"
            | "comm_list_channels"
            | "comm_channel_members"
            | "comm_summary"
            | "comm_status"
            | "comm_plan_status"
            | "comm_read_context"
            | "comm_await_members"
    )
}

pub(super) fn request_payload_summary(kind: &str, line: &str) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) {
        let bytes_chars =
            |name: &str, value: &serde_json::Value, fields: &mut Vec<(String, String)>| {
                if let Some(text) = value.get(name).and_then(|v| v.as_str()) {
                    fields.push((format!("{}_bytes", name), text.len().to_string()));
                    fields.push((format!("{}_chars", name), text.chars().count().to_string()));
                }
            };
        for name in [
            "content", "message", "prompt", "task", "command", "input", "value",
        ] {
            bytes_chars(name, &value, &mut fields);
        }
        if let Some(images) = value.get("images").and_then(|v| v.as_array()) {
            fields.push(("image_count".to_string(), images.len().to_string()));
        }
        if let Some(session_id) = value.get("session_id").and_then(|v| v.as_str()) {
            fields.push(("request_session_id".to_string(), session_id.to_string()));
        }
        if let Some(target_session) = value.get("target_session").and_then(|v| v.as_str()) {
            fields.push(("target_session".to_string(), target_session.to_string()));
        }
        if let Some(client_instance_id) = value.get("client_instance_id").and_then(|v| v.as_str()) {
            fields.push((
                "request_client_instance_id".to_string(),
                client_instance_id.to_string(),
            ));
        }
        if matches!(kind, "set_model" | "set_subagent_model")
            && let Some(model) = value.get("model").and_then(|v| v.as_str())
        {
            fields.push(("model".to_string(), model.to_string()));
        }
        if let Some(title) = value.get("title").and_then(|v| v.as_str()) {
            fields.push(("title_chars".to_string(), title.chars().count().to_string()));
        }
    }
    fields
}

pub(super) struct ServerRequestLifecycleFields<'a> {
    pub(super) phase: &'a str,
    pub(super) request_id: u64,
    pub(super) request_kind: &'a str,
    pub(super) client_session_id: &'a str,
    pub(super) client_connection_id: &'a str,
    pub(super) client_instance_id: Option<&'a str>,
    pub(super) client_is_processing: bool,
    pub(super) message_id: Option<u64>,
    pub(super) processing_session_id: Option<&'a str>,
    pub(super) line_bytes: usize,
}

pub(super) fn server_request_lifecycle_fields(
    input: ServerRequestLifecycleFields<'_>,
) -> Vec<(String, String)> {
    let ServerRequestLifecycleFields {
        phase,
        request_id,
        request_kind,
        client_session_id,
        client_connection_id,
        client_instance_id,
        client_is_processing,
        message_id,
        processing_session_id,
        line_bytes,
    } = input;
    let mut fields = vec![
        ("phase".to_string(), phase.to_string()),
        ("request_id".to_string(), request_id.to_string()),
        ("request_kind".to_string(), request_kind.to_string()),
        ("session_id".to_string(), client_session_id.to_string()),
        (
            "client_connection_id".to_string(),
            client_connection_id.to_string(),
        ),
        (
            "client_instance_id".to_string(),
            client_instance_id.unwrap_or("none").to_string(),
        ),
        (
            "client_processing".to_string(),
            client_is_processing.to_string(),
        ),
        (
            "message_id".to_string(),
            message_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string()),
        ),
        (
            "processing_session_id".to_string(),
            processing_session_id.unwrap_or("none").to_string(),
        ),
        ("line_bytes".to_string(), line_bytes.to_string()),
    ];
    if let Some(ctx_session) = crate::logging::current_session() {
        fields.push(("log_context_session_id".to_string(), ctx_session));
    }
    fields
}
