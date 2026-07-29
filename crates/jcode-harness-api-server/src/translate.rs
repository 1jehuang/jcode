//! Pure JSON-to-JSON translation between the harness API and the legacy
//! internal protocol. Kept side-effect free so it is trivially unit-testable.

use jcode_harness_api::{ApiEvent, ErrorCode, HistoryMessage, ServerFrame, SessionInfo};

/// Default number of messages a `peek_session` returns. A preview is a glance,
/// so this is a tail rather than a transcript: enough to recognise which
/// conversation it is, few enough that peeking a dozen sessions stays cheap.
const PEEK_LIMIT: u64 = 12;

/// Flatten a stored message's `content` to plain text.
///
/// The daemon writes content either as a bare string or as an array of typed
/// blocks, so both shapes are accepted; anything without text (a tool call, an
/// image) contributes nothing rather than a placeholder.
fn flatten_content(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    let Some(blocks) = content.as_array() else {
        return String::new();
    };
    blocks
        .iter()
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("")
}
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
    /// Legacy id of the unsolicited model-catalog probe sent after attach. Its
    /// reply becomes a `model_info` event rather than a request reply, so it is
    /// tracked apart from `pending_simple`.
    pending_model_probe: Option<u64>,
    /// Legacy id -> API id for simple acked requests (ping, clear, ...).
    pending_simple: Vec<(u64, u64, SimpleKind)>,
    /// Every session the daemon has told us about, newest snapshot wins.
    ///
    /// The legacy protocol has no session-list request, but it volunteers the
    /// full set on every `state` event, so the bridge remembers it rather than
    /// answering `list_sessions` with only the one session this connection
    /// happens to be attached to.
    known_sessions: Vec<String>,
    /// Working directory per session, as far as it is known.
    session_dirs: std::collections::BTreeMap<String, String>,
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
                let catalog_id = self.legacy_id();
                self.pending_attach_id = Some((state_id, api_id));
                self.pending_model_probe = Some(catalog_id);
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
                // Sessions rooted inside a jcode checkout are self-dev
                // sessions: the daemon only enables the self-dev tools and
                // prompt when the subscribe says so, and a client that opens
                // the repo without saying so gets an agent that cannot build
                // the very app it is running in.
                if working_dir
                    .as_deref()
                    .is_some_and(Self::path_is_inside_jcode_repo)
                {
                    subscribe["selfdev"] = json!(true);
                }
                if req == "attach_session"
                    && let Some(target) = request["session_id"].as_str()
                {
                    subscribe["target_session_id"] = json!(target);
                }
                // The daemon assigns the session during subscribe but reports
                // the id via `state`, so chase the subscribe with get_state.
                // The model identity arrives the same way, via the catalog
                // reply, so ask for it now rather than making the client poll.
                vec![
                    Outbound::Legacy(subscribe),
                    Outbound::Legacy(json!({"type": "state", "id": state_id})),
                    Outbound::Legacy(json!({"type": "get_model_catalog", "id": catalog_id})),
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
            // Answered from the stored record rather than the daemon: the
            // legacy protocol can only speak about the attached session, and
            // attaching to a session merely to read it would disturb the very
            // thing being previewed.
            "peek_session" => {
                let session_id = request["session_id"].as_str().unwrap_or_default();
                let limit = request["limit"].as_u64().unwrap_or(PEEK_LIMIT) as usize;
                vec![Outbound::Reply(ServerFrame::reply(
                    api_id,
                    ApiEvent::History {
                        session_id: session_id.to_string(),
                        messages: Self::stored_tail(session_id, limit),
                    },
                ))]
            }
            "ping" => {
                let id = self.legacy_id();
                self.pending_simple.push((id, api_id, SimpleKind::Ping));
                vec![Outbound::Legacy(json!({"type": "ping", "id": id}))]
            }
            "list_sessions" => {
                // The legacy protocol has no list request, but it reports the
                // full set on every `state` event, so answer from that rather
                // than pretending only the attached session exists.
                let mut ids = self.known_sessions.clone();
                if let Some(attached) = self.session_id.clone()
                    && !ids.contains(&attached)
                {
                    ids.push(attached);
                }
                for id in &ids {
                    if !self.session_dirs.contains_key(id)
                        && let Some(dir) = Self::resolve_working_dir(id)
                    {
                        self.session_dirs.insert(id.clone(), dir);
                    }
                }
                let sessions = ids
                    .iter()
                    .map(|session_id| SessionInfo {
                        session_id: session_id.clone(),
                        working_dir: self.session_dirs.get(session_id).cloned(),
                        title: None,
                        status: if self.session_id.as_ref() == Some(session_id) {
                            "attached".into()
                        } else {
                            "idle".into()
                        },
                        transcript_bytes: Self::transcript_bytes(session_id),
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
                                transcript_bytes: Self::transcript_bytes(&session_id),
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
                // The daemon volunteers the full session set on `history`,
                // which is the only place it appears: remember it so
                // `list_sessions` can answer with more than this connection.
                self.note_sessions(event);
                // The catalog probe rides the same `history` reply shape but
                // carries no messages: it is model identity, not transcript.
                if self.pending_model_probe == Some(id) {
                    self.pending_model_probe = None;
                    return vec![ServerFrame::event(self.model_info(session(self), event))];
                }
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
            // The model can change mid-session (`/model`, a cycle, or an auth
            // change re-resolving the route), so both pushes are forwarded.
            "model_changed" => {
                if event["error"].is_string() {
                    return vec![];
                }
                vec![ServerFrame::event(ApiEvent::ModelInfo {
                    session_id: session(self),
                    provider: event["provider_name"].as_str().map(str::to_string),
                    model: event["model"].as_str().map(str::to_string),
                })]
            }
            "available_models_updated" => {
                vec![ServerFrame::event(self.model_info(session(self), event))]
            }
            "ack" => {
                let id = event["id"].as_u64().unwrap_or(0);
                // The daemon acking the in-flight `message` is the proof the
                // agent has the text: report it as its own event so a client
                // can move a message from "sent" to "acknowledged" without
                // waiting for the first token of the reply.
                if self.pending_message_id == Some(id) {
                    return vec![ServerFrame::event(ApiEvent::MessageAccepted {
                        session_id: session(self),
                    })];
                }
                self.take_simple(id, SimpleKind::Ok)
                    .map(|api_id| vec![ServerFrame::reply(api_id, ApiEvent::Ok)])
                    .unwrap_or_default()
            }
            "error" => {
                let id = event["id"].as_u64().unwrap_or(0);
                let message = event["message"].as_str().unwrap_or("").to_string();
                // A turn that fails ends with `error` *instead of* `done`, so
                // the turn is over: forget the pending message, or a later
                // unrelated `done` carrying the same id would be reported as
                // this turn finishing.
                if self.pending_message_id == Some(id) {
                    self.pending_message_id = None;
                }
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

    /// Read provider/model identity out of any legacy event that carries the
    /// `provider_name`/`provider_model` pair (the catalog reply and the
    /// available-models push both do).
    fn model_info(&self, session_id: String, event: &Value) -> ApiEvent {
        ApiEvent::ModelInfo {
            session_id,
            provider: event["provider_name"].as_str().map(str::to_string),
            model: event["provider_model"].as_str().map(str::to_string),
        }
    }

    /// True when `path`, or any ancestor, looks like a jcode source checkout.
    ///
    /// Matched by content (a workspace manifest next to the crates directory)
    /// rather than by name, so a clone in any directory is recognised.
    fn path_is_inside_jcode_repo(path: &str) -> bool {
        let mut current = Some(std::path::Path::new(path));
        while let Some(dir) = current {
            if dir.join("Cargo.toml").is_file() && dir.join("crates/jcode-base").is_dir() {
                return true;
            }
            current = dir.parent();
        }
        false
    }

    /// Working directory of a session, read from its persisted record.
    ///
    /// The legacy `history` event lists session *ids* only, but the strip
    /// groups by directory, so the bridge resolves them from the same files
    /// the daemon persists. Best-effort by design: an unreadable or missing
    /// record simply leaves the session ungrouped rather than failing the
    /// list, and results are cached because this is on a poll path.
    fn resolve_working_dir(session_id: &str) -> Option<String> {
        let home = std::env::var_os("HOME")?;
        let path = std::path::Path::new(&home)
            .join(".jcode")
            .join("sessions")
            .join(format!("{session_id}.json"));
        // A missing or malformed record is expected (a session may predate the
        // field, or be mid-write), and the only cost is an ungrouped bar, so
        // this degrades rather than failing the whole session list.
        let text = std::fs::read_to_string(path).ok()?;
        let value: Value = serde_json::from_str(&text).ok()?;
        value["working_dir"].as_str().map(str::to_string)
    }

    /// Size of a session's stored record, in bytes.
    ///
    /// A stat rather than a parse: this runs for every session on every list
    /// request, and deserializing a dozen multi-megabyte transcripts to count
    /// their characters would make the cheap call expensive. The file is
    /// almost entirely message content, so its size tracks the conversation
    /// closely enough for a client to size or sort by.
    fn transcript_bytes(session_id: &str) -> Option<u64> {
        let home = std::env::var_os("HOME")?;
        let path = std::path::Path::new(&home)
            .join(".jcode")
            .join("sessions")
            .join(format!("{session_id}.json"));
        std::fs::metadata(path).ok().map(|meta| meta.len())
    }

    /// The last `limit` messages of a session, read from its stored record.
    ///
    /// Content blocks are flattened to their text, which is what a preview
    /// wants: a reader glancing at another session needs the words, not the
    /// tool-call structure around them.
    fn stored_tail(session_id: &str, limit: usize) -> Vec<HistoryMessage> {
        let Some(home) = std::env::var_os("HOME") else {
            return vec![];
        };
        let path = std::path::Path::new(&home)
            .join(".jcode")
            .join("sessions")
            .join(format!("{session_id}.json"));
        let Ok(text) = std::fs::read_to_string(path) else {
            return vec![];
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return vec![];
        };
        let Some(messages) = value["messages"].as_array() else {
            return vec![];
        };
        messages
            .iter()
            .rev()
            .filter_map(|message| {
                let role = message["role"].as_str()?;
                // Only the conversation: a preview of tool traffic would be
                // noise where the point is to recognise which conversation
                // this is.
                if role != "user" && role != "assistant" {
                    return None;
                }
                let content = flatten_content(&message["content"]);
                (!content.trim().is_empty()).then(|| HistoryMessage {
                    role: role.to_string(),
                    content,
                })
            })
            .take(limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// Record the session set the daemon reported, plus any working
    /// directory it mentioned. Kept separate so both the attach probe and an
    /// explicit history request feed the same list.
    fn note_sessions(&mut self, event: &Value) {
        if let Some(all) = event["all_sessions"].as_array() {
            let listed: Vec<String> = all
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect();
            if !listed.is_empty() {
                self.known_sessions = listed;
            }
        }
        if let Some(dir) = event["working_dir"].as_str()
            && let Some(session_id) = event["session_id"].as_str()
        {
            self.session_dirs
                .insert(session_id.to_string(), dir.to_string());
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
