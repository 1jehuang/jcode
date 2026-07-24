//! Pure JSON-to-JSON translation between the harness API and the legacy
//! internal protocol. Kept side-effect free so it is trivially unit-testable.

use jcode_harness_api::{ApiEvent, ErrorCode, HistoryMessage, ServerFrame, SessionInfo};
use serde_json::{Value, json};

/// Where a translated client request should go.
#[derive(Debug)]
pub enum Outbound {
    /// Forward to the legacy daemon connection.
    Legacy(Value),
    /// Answer the API client directly (no daemon round trip needed).
    Reply(ServerFrame),
}

/// Per-connection translation state.
#[derive(Debug, Default)]
pub struct BridgeState {
    /// Session id assigned by the daemon for this connection.
    pub session_id: Option<String>,
    /// Next id to use on the legacy connection.
    next_legacy_id: u64,
    /// Legacy id of the in-flight `message` request, so `done` maps to
    /// `turn_done`.
    pending_message_id: Option<u64>,
    /// Legacy id of an in-flight `create/attach` subscribe.
    pending_attach_id: Option<(u64, u64)>,
    /// Legacy id -> API id for simple acked requests (ping, clear, ...).
    pending_simple: Vec<(u64, u64, SimpleKind)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SimpleKind {
    Ping,
    History,
    Ok,
}

impl BridgeState {
    fn legacy_id(&mut self) -> u64 {
        self.next_legacy_id += 1;
        self.next_legacy_id
    }

    /// Translate one API request (raw JSON) into outbound actions.
    pub fn api_request_to_legacy(&mut self, request: &Value) -> Vec<Outbound> {
        let api_id = request["id"].as_u64().unwrap_or(0);
        let req = request["req"].as_str().unwrap_or("");
        match req {
            "create_session" | "attach_session" => {
                let id = self.legacy_id();
                let state_id = self.legacy_id();
                self.pending_attach_id = Some((state_id, api_id));
                let working_dir =
                    request["working_dir"]
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| {
                            std::env::current_dir()
                                .ok()
                                .map(|d| d.display().to_string())
                        });
                let mut subscribe = json!({
                    "type": "subscribe",
                    "id": id,
                    "working_dir": working_dir,
                });
                if req == "attach_session"
                    && let Some(target) = request["session_id"].as_str()
                {
                    subscribe["target_session_id"] = json!(target);
                }
                // The daemon assigns the session during subscribe but reports
                // the id via `state`, so chase the subscribe with get_state.
                vec![
                    Outbound::Legacy(subscribe),
                    Outbound::Legacy(json!({"type": "state", "id": state_id})),
                ]
            }
            "send_message" => {
                let id = self.legacy_id();
                self.pending_message_id = Some(id);
                let mut message = json!({
                    "type": "message",
                    "id": id,
                    "content": request["content"].as_str().unwrap_or(""),
                });
                if let Some(images) = request["images"].as_array()
                    && !images.is_empty()
                {
                    message["images"] = json!(images);
                }
                vec![Outbound::Legacy(message)]
            }
            "cancel" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::Ok));
                vec![Outbound::Legacy(json!({"type": "cancel", "id": id}))]
            }
            "soft_interrupt" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::Ok));
                vec![Outbound::Legacy(json!({
                    "type": "soft_interrupt",
                    "id": id,
                    "content": request["content"].as_str().unwrap_or(""),
                    "urgent": request["urgent"].as_bool().unwrap_or(false),
                }))]
            }
            "clear" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::Ok));
                vec![Outbound::Legacy(json!({"type": "clear", "id": id}))]
            }
            "rewind" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::Ok));
                vec![Outbound::Legacy(json!({
                    "type": "rewind",
                    "id": id,
                    "message_index": request["message_index"].as_u64().unwrap_or(1),
                }))]
            }
            "get_history" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::History));
                vec![Outbound::Legacy(json!({"type": "get_history", "id": id}))]
            }
            "ping" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::Ping));
                vec![Outbound::Legacy(json!({"type": "ping", "id": id}))]
            }
            "list_sessions" => {
                // Not yet mapped onto the legacy protocol; answer with what we
                // know (the attached session, if any).
                let sessions = self
                    .session_id
                    .iter()
                    .map(|session_id| SessionInfo {
                        session_id: session_id.clone(),
                        working_dir: None,
                        title: None,
                        status: "attached".into(),
                    })
                    .collect();
                vec![Outbound::Reply(ServerFrame::reply(
                    api_id,
                    ApiEvent::Sessions { sessions },
                ))]
            }
            "detach_session" => vec![Outbound::Reply(ServerFrame::reply(api_id, ApiEvent::Ok))],
            "permission_response" => {
                // Permission flow is not yet exposed by the legacy protocol on
                // this path. Surface a clear error instead of silence.
                vec![Outbound::Reply(ServerFrame::reply(
                    api_id,
                    ApiEvent::Error {
                        code: ErrorCode::InvalidRequest,
                        message: "permission_response not yet supported by bridge".into(),
                    },
                ))]
            }
            other => vec![Outbound::Reply(ServerFrame::reply(
                api_id,
                ApiEvent::Error {
                    code: ErrorCode::UnknownRequest,
                    message: format!("unknown request: {other}"),
                },
            ))],
        }
    }

    /// Translate one legacy server event (raw JSON) into API frames.
    pub fn legacy_event_to_api(&mut self, event: &Value) -> Vec<ServerFrame> {
        let kind = event["type"].as_str().unwrap_or("");
        let session = |state: &Self| state.session_id.clone().unwrap_or_default();
        match kind {
            "session" => {
                let session_id = event["session_id"].as_str().unwrap_or("").to_string();
                self.session_id = Some(session_id.clone());
                vec![ServerFrame::event(ApiEvent::SessionStatus {
                    session_id,
                    status: "attached".into(),
                })]
            }
            "state" => {
                let session_id = event["session_id"].as_str().unwrap_or("").to_string();
                if !session_id.is_empty() {
                    self.session_id = Some(session_id.clone());
                }
                let id = event["id"].as_u64().unwrap_or(0);
                if let Some((state_id, api_id)) = self.pending_attach_id
                    && state_id == id
                {
                    self.pending_attach_id = None;
                    return vec![ServerFrame::reply(
                        api_id,
                        ApiEvent::Attached {
                            session: SessionInfo {
                                session_id,
                                working_dir: None,
                                title: None,
                                status: if event["is_processing"].as_bool().unwrap_or(false) {
                                    "processing".into()
                                } else {
                                    "idle".into()
                                },
                            },
                        },
                    )];
                }
                vec![]
            }
            "text_delta" => vec![ServerFrame::event(ApiEvent::TextDelta {
                session_id: session(self),
                text: event["text"].as_str().unwrap_or("").to_string(),
            })],
            "reasoning_delta" => vec![ServerFrame::event(ApiEvent::ReasoningDelta {
                session_id: session(self),
                text: event["text"].as_str().unwrap_or("").to_string(),
            })],
            "reasoning_done" => vec![ServerFrame::event(ApiEvent::ReasoningDone {
                session_id: session(self),
                duration_secs: event["duration_secs"].as_f64(),
            })],
            "tool_start" => vec![ServerFrame::event(ApiEvent::ToolStart {
                session_id: session(self),
                call_id: event["id"].as_str().unwrap_or("").to_string(),
                name: event["name"].as_str().unwrap_or("").to_string(),
            })],
            "tool_input" => vec![ServerFrame::event(ApiEvent::ToolInputDelta {
                session_id: session(self),
                call_id: String::new(),
                delta: event["delta"].as_str().unwrap_or("").to_string(),
            })],
            "tool_exec" => vec![ServerFrame::event(ApiEvent::ToolExec {
                session_id: session(self),
                call_id: event["id"].as_str().unwrap_or("").to_string(),
                name: event["name"].as_str().unwrap_or("").to_string(),
            })],
            "tool_done" => vec![ServerFrame::event(ApiEvent::ToolDone {
                session_id: session(self),
                call_id: event["id"].as_str().unwrap_or("").to_string(),
                name: event["name"].as_str().unwrap_or("").to_string(),
                output: event["output"].as_str().unwrap_or("").to_string(),
                error: event["error"].as_str().map(str::to_string),
            })],
            "tokens" => vec![ServerFrame::event(ApiEvent::TokenUsage {
                session_id: session(self),
                input: event["input"].as_u64().unwrap_or(0),
                output: event["output"].as_u64().unwrap_or(0),
                cache_read_input: event["cache_read_input"].as_u64(),
            })],
            "done" => {
                let id = event["id"].as_u64().unwrap_or(0);
                // Subscribe and other requests also emit `done`; only a
                // completed `message` is a turn boundary.
                if self.pending_message_id == Some(id) {
                    self.pending_message_id = None;
                    vec![ServerFrame::event(ApiEvent::TurnDone {
                        session_id: session(self),
                    })]
                } else {
                    vec![]
                }
            }
            "pong" => self
                .take_simple(event["id"].as_u64().unwrap_or(0), SimpleKind::Ping)
                .map(|api_id| vec![ServerFrame::reply(api_id, ApiEvent::Pong)])
                .unwrap_or_default(),
            "history" => {
                let id = event["id"].as_u64().unwrap_or(0);
                let Some(api_id) = self.take_simple(id, SimpleKind::History) else {
                    return vec![];
                };
                let messages = event["messages"]
                    .as_array()
                    .map(|messages| {
                        messages
                            .iter()
                            .map(|m| HistoryMessage {
                                role: m["role"].as_str().unwrap_or("").to_string(),
                                content: m["content"].as_str().unwrap_or("").to_string(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                vec![ServerFrame::reply(
                    api_id,
                    ApiEvent::History {
                        session_id: session(self),
                        messages,
                    },
                )]
            }
            "ack" => {
                let id = event["id"].as_u64().unwrap_or(0);
                self.take_simple(id, SimpleKind::Ok)
                    .map(|api_id| vec![ServerFrame::reply(api_id, ApiEvent::Ok)])
                    .unwrap_or_default()
            }
            "error" => {
                let id = event["id"].as_u64().unwrap_or(0);
                let message = event["message"].as_str().unwrap_or("").to_string();
                // Route to a pending request when possible, else stream it.
                let reply_to = self
                    .pending_simple
                    .iter()
                    .position(|(legacy_id, _, _)| *legacy_id == id)
                    .map(|index| self.pending_simple.remove(index).1);
                let frame_event = ApiEvent::Error {
                    code: ErrorCode::Internal,
                    message,
                };
                vec![match reply_to {
                    Some(api_id) => ServerFrame::reply(api_id, frame_event),
                    None => ServerFrame::event(frame_event),
                }]
            }
            // Everything else on the legacy stream is not part of the stable
            // API surface yet; drop it.
            _ => vec![],
        }
    }

    fn take_simple(&mut self, legacy_id: u64, kind: SimpleKind) -> Option<u64> {
        let index = self
            .pending_simple
            .iter()
            .position(|(id, _, k)| *id == legacy_id && *k == kind)?;
        Some(self.pending_simple.remove(index).1)
    }
}

#[cfg(test)]
#[path = "translate_tests.rs"]
mod tests;
