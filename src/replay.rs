use crate::message::{ContentBlock, Role};
use crate::protocol::ServerEvent;
use crate::session::Session;
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// A single event in a replay timeline.
///
/// The `t` field is milliseconds from the start of the replay.
/// Edit this value to change pacing in post-production.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    /// Milliseconds from replay start
    pub t: u64,
    /// The event payload
    #[serde(flatten)]
    pub kind: TimelineEventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum TimelineEventKind {
    /// User message appears instantly
    #[serde(rename = "user_message")]
    UserMessage { text: String },

    /// Assistant starts streaming (sets processing state)
    #[serde(rename = "thinking")]
    Thinking {
        /// How long to show the thinking spinner (ms)
        #[serde(default = "default_thinking_duration")]
        duration: u64,
    },

    /// Stream a chunk of assistant text
    #[serde(rename = "stream_text")]
    StreamText {
        text: String,
        /// Tokens per second for streaming speed (default 80)
        #[serde(default = "default_stream_speed")]
        speed: u64,
    },

    /// Tool call starts
    #[serde(rename = "tool_start")]
    ToolStart {
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },

    /// Tool execution completes
    #[serde(rename = "tool_done")]
    ToolDone {
        name: String,
        output: String,
        #[serde(default)]
        is_error: bool,
    },

    /// Token usage update (drives context bar)
    #[serde(rename = "token_usage")]
    TokenUsage {
        input: u64,
        output: u64,
        #[serde(default)]
        cache_read: Option<u64>,
        #[serde(default)]
        cache_creation: Option<u64>,
    },

    /// Turn complete (commits streaming text, resets to idle)
    #[serde(rename = "done")]
    Done,
}

fn default_thinking_duration() -> u64 {
    1200
}
fn default_stream_speed() -> u64 {
    80
}

/// Export a session to a replay timeline.
///
/// Uses stored timestamps for real pacing, falls back to estimates.
pub fn export_timeline(session: &Session) -> Vec<TimelineEvent> {
    let mut events = Vec::new();
    let mut t: u64 = 0;
    let session_start = session.created_at;

    // Track tool IDs for pairing ToolUse → ToolResult
    let mut pending_tools: Vec<(String, String, serde_json::Value)> = Vec::new(); // (id, name, input)

    for msg in &session.messages {
        // Advance time based on stored timestamp
        if let Some(ts) = msg.timestamp {
            let offset = ts
                .signed_duration_since(session_start)
                .num_milliseconds()
                .max(0) as u64;
            if offset > t {
                t = offset;
            }
        }

        match msg.role {
            Role::User => {
                // Check if this is a tool result
                let mut has_tool_result = false;
                for block in &msg.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = block
                    {
                        has_tool_result = true;
                        // Find matching tool start
                        let tool_name = pending_tools
                            .iter()
                            .find(|(id, _, _)| id == tool_use_id)
                            .map(|(_, name, _)| name.clone())
                            .unwrap_or_else(|| "tool".to_string());

                        // Use stored duration or estimate
                        let duration_ms = msg.tool_duration_ms.unwrap_or(500);

                        events.push(TimelineEvent {
                            t,
                            kind: TimelineEventKind::ToolDone {
                                name: tool_name,
                                output: truncate_for_timeline(content),
                                is_error: is_error.unwrap_or(false),
                            },
                        });
                        t += duration_ms.min(100); // Small gap after tool result
                        pending_tools.retain(|(id, _, _)| id != tool_use_id);
                    }
                }

                if !has_tool_result {
                    // Regular user message
                    let text = extract_text(&msg.content);
                    if !text.is_empty() {
                        events.push(TimelineEvent {
                            t,
                            kind: TimelineEventKind::UserMessage { text },
                        });
                        t += 300; // Brief pause after user message
                    }
                }
            }
            Role::Assistant => {
                let text = extract_text(&msg.content);
                let tool_uses: Vec<_> = msg
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::ToolUse { id, name, input } = b {
                            Some((id.clone(), name.clone(), input.clone()))
                        } else {
                            None
                        }
                    })
                    .collect();

                // Thinking phase
                if !text.is_empty() || !tool_uses.is_empty() {
                    events.push(TimelineEvent {
                        t,
                        kind: TimelineEventKind::Thinking { duration: 800 },
                    });
                    t += 800;
                }

                // Stream text
                if !text.is_empty() {
                    let speed = 80;
                    let stream_duration_ms = (text.len() as u64 * 1000) / (speed * 4); // ~4 chars/token
                    events.push(TimelineEvent {
                        t,
                        kind: TimelineEventKind::StreamText {
                            text: text.clone(),
                            speed,
                        },
                    });
                    t += stream_duration_ms;
                }

                // Token usage
                if let Some(ref usage) = msg.token_usage {
                    events.push(TimelineEvent {
                        t,
                        kind: TimelineEventKind::TokenUsage {
                            input: usage.input_tokens,
                            output: usage.output_tokens,
                            cache_read: usage.cache_read_input_tokens,
                            cache_creation: usage.cache_creation_input_tokens,
                        },
                    });
                }

                // Tool calls
                for (id, name, input) in &tool_uses {
                    events.push(TimelineEvent {
                        t,
                        kind: TimelineEventKind::ToolStart {
                            name: name.clone(),
                            input: input.clone(),
                        },
                    });
                    pending_tools.push((id.clone(), name.clone(), input.clone()));
                    t += 200; // Small gap between tool starts
                }

                // Done if no pending tools
                if tool_uses.is_empty() {
                    events.push(TimelineEvent {
                        t,
                        kind: TimelineEventKind::Done,
                    });
                    t += 200;
                }
            }
        }
    }

    // Final done if we haven't emitted one
    if !events.is_empty() {
        let last_is_done = events.last().map_or(false, |e| {
            matches!(e.kind, TimelineEventKind::Done)
        });
        if !last_is_done {
            events.push(TimelineEvent {
                t,
                kind: TimelineEventKind::Done,
            });
        }
    }

    events
}

/// Replay-specific server events that don't exist in the normal protocol.
/// These are handled specially in `run_replay`.
#[derive(Debug, Clone)]
pub enum ReplayEvent {
    /// A normal server event
    Server(ServerEvent),
    /// User message (displayed directly, not via server event)
    UserMessage { text: String },
    /// Start processing state (shows thinking spinner)
    StartProcessing,
}

/// Convert a timeline into a sequence of (delay_ms, ReplayEvent) pairs for playback.
pub fn timeline_to_replay_events(timeline: &[TimelineEvent]) -> Vec<(u64, ReplayEvent)> {
    let mut out = Vec::new();
    let mut prev_t: u64 = 0;
    let mut turn_id: u64 = 1;
    let mut tool_id_counter: u64 = 0;
    let mut pending_tool_ids: Vec<String> = Vec::new();

    for event in timeline {
        let delay = event.t.saturating_sub(prev_t);
        prev_t = event.t;

        match &event.kind {
            TimelineEventKind::UserMessage { text } => {
                out.push((delay, ReplayEvent::UserMessage { text: text.clone() }));
            }
            TimelineEventKind::Thinking { .. } => {
                out.push((delay, ReplayEvent::StartProcessing));
            }
            TimelineEventKind::StreamText { text, speed } => {
                let chars_per_chunk = 4; // ~1 token
                let ms_per_chunk = if *speed > 0 { 1000 / speed } else { 12 };
                let chunks: Vec<String> = text
                    .chars()
                    .collect::<Vec<_>>()
                    .chunks(chars_per_chunk)
                    .map(|c| c.iter().collect::<String>())
                    .collect();

                for (i, chunk) in chunks.iter().enumerate() {
                    let chunk_delay = if i == 0 { delay } else { ms_per_chunk };
                    out.push((chunk_delay, ReplayEvent::Server(ServerEvent::TextDelta { text: chunk.clone() })));
                }
            }
            TimelineEventKind::ToolStart { name, input } => {
                tool_id_counter += 1;
                let id = format!("replay_tool_{}", tool_id_counter);
                pending_tool_ids.push(id.clone());

                out.push((
                    delay,
                    ReplayEvent::Server(ServerEvent::ToolStart {
                        id: id.clone(),
                        name: name.clone(),
                    }),
                ));

                let input_str = serde_json::to_string(input).unwrap_or_default();
                if !input_str.is_empty() && input_str != "null" {
                    out.push((
                        0,
                        ReplayEvent::Server(ServerEvent::ToolInput {
                            delta: input_str,
                        }),
                    ));
                }

                out.push((
                    50,
                    ReplayEvent::Server(ServerEvent::ToolExec {
                        id: id.clone(),
                        name: name.clone(),
                    }),
                ));
            }
            TimelineEventKind::ToolDone {
                name,
                output,
                is_error,
            } => {
                let id = pending_tool_ids.pop().unwrap_or_else(|| {
                    tool_id_counter += 1;
                    format!("replay_tool_{}", tool_id_counter)
                });
                out.push((
                    delay,
                    ReplayEvent::Server(ServerEvent::ToolDone {
                        id,
                        name: name.clone(),
                        output: output.clone(),
                        error: if *is_error {
                            Some(output.clone())
                        } else {
                            None
                        },
                    }),
                ));
            }
            TimelineEventKind::TokenUsage {
                input,
                output,
                cache_read,
                cache_creation,
            } => {
                out.push((
                    delay,
                    ReplayEvent::Server(ServerEvent::TokenUsage {
                        input: *input,
                        output: *output,
                        cache_read_input: *cache_read,
                        cache_creation_input: *cache_creation,
                    }),
                ));
            }
            TimelineEventKind::Done => {
                out.push((delay, ReplayEvent::Server(ServerEvent::Done { id: turn_id })));
                turn_id += 1;
            }
        }
    }

    out
}

/// Load a session by ID or path
pub fn load_session(id_or_path: &str) -> Result<Session> {
    use std::path::Path;

    // Try as file path first
    let path = Path::new(id_or_path);
    if path.exists() {
        let data = std::fs::read_to_string(path)?;
        let session: Session = serde_json::from_str(&data)?;
        return Ok(session);
    }

    // Try as session ID in the sessions directory
    let sessions_dir = crate::storage::jcode_dir()?.join("sessions");
    // Try exact match
    let exact = sessions_dir.join(format!("{}.json", id_or_path));
    if exact.exists() {
        let data = std::fs::read_to_string(&exact)?;
        let session: Session = serde_json::from_str(&data)?;
        return Ok(session);
    }

    // Try prefix match (session_<id>.json or session_<name>_<ts>.json)
    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.contains(id_or_path) && name.ends_with(".json") {
            let data = std::fs::read_to_string(entry.path())?;
            let session: Session = serde_json::from_str(&data)?;
            return Ok(session);
        }
    }

    anyhow::bail!(
        "Session not found: '{}'. Provide a session ID, name, or file path.",
        id_or_path
    );
}

fn extract_text(blocks: &[ContentBlock]) -> String {
    let mut text = String::new();
    for block in blocks {
        if let ContentBlock::Text { text: t, .. } = block {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(t);
        }
    }
    text
}

/// Auto-edit a timeline for demo-quality pacing.
///
/// Compresses dead time so the replay feels snappy:
/// - Tool call execution (tool_start → tool_done): capped to `tool_max_ms`
/// - Gaps between turns (done → next user_message): capped to `gap_max_ms`
/// - Thinking duration: capped to `think_max_ms`
/// - Streaming text and everything else: preserved as-is
pub fn auto_edit_timeline(timeline: &[TimelineEvent], opts: &AutoEditOpts) -> Vec<TimelineEvent> {
    if timeline.is_empty() {
        return vec![];
    }

    let mut out: Vec<TimelineEvent> = Vec::with_capacity(timeline.len());
    let mut time_shift: i64 = 0; // accumulated shift (negative = earlier)

    // Track tool nesting for compressing tool_start→tool_done spans
    let mut tool_depth: u32 = 0;
    let mut tool_span_start_t: Option<u64> = None;

    // Track done→user_message gaps
    let mut last_done_t: Option<u64> = None;
    // Track user_message→thinking gaps
    let mut last_user_msg_t: Option<u64> = None;

    for event in timeline {
        let orig_t = event.t;
        let mut new_t = (orig_t as i64 + time_shift).max(0) as u64;

        match &event.kind {
            TimelineEventKind::Thinking { duration } => {
                // Clamp gap from done→thinking
                if let Some(done_t) = last_done_t.take() {
                    let gap = orig_t.saturating_sub(done_t);
                    if gap > opts.gap_max_ms {
                        time_shift -= (gap - opts.gap_max_ms) as i64;
                        new_t = (orig_t as i64 + time_shift).max(0) as u64;
                    }
                }
                // Clamp gap from user_message→thinking (model response delay)
                if let Some(user_t) = last_user_msg_t.take() {
                    let gap = orig_t.saturating_sub(user_t);
                    if gap > opts.response_delay_max_ms {
                        time_shift -= (gap - opts.response_delay_max_ms) as i64;
                        new_t = (orig_t as i64 + time_shift).max(0) as u64;
                    }
                }

                let clamped = (*duration).min(opts.think_max_ms);
                out.push(TimelineEvent {
                    t: new_t,
                    kind: TimelineEventKind::Thinking {
                        duration: clamped,
                    },
                });
                continue;
            }
            TimelineEventKind::UserMessage { .. } => {
                // Compress gap after last done
                if let Some(done_t) = last_done_t.take() {
                    let gap = orig_t.saturating_sub(done_t);
                    if gap > opts.gap_max_ms {
                        time_shift -= (gap - opts.gap_max_ms) as i64;
                        new_t = (orig_t as i64 + time_shift).max(0) as u64;
                    }
                }
                last_user_msg_t = Some(orig_t);
            }
            TimelineEventKind::ToolStart { .. } => {
                if tool_depth == 0 {
                    tool_span_start_t = Some(orig_t);
                }
                tool_depth += 1;
            }
            TimelineEventKind::ToolDone { .. } => {
                tool_depth = tool_depth.saturating_sub(1);
                if tool_depth == 0 {
                    if let Some(start_t) = tool_span_start_t.take() {
                        let span = orig_t.saturating_sub(start_t);
                        if span > opts.tool_max_ms {
                            time_shift -= (span - opts.tool_max_ms) as i64;
                            new_t = (orig_t as i64 + time_shift).max(0) as u64;
                        }
                    }
                }
            }
            TimelineEventKind::Done => {
                last_done_t = Some(orig_t);
            }
            _ => {}
        }

        out.push(TimelineEvent {
            t: new_t,
            kind: event.kind.clone(),
        });
    }

    out
}

/// Options for [`auto_edit_timeline`].
pub struct AutoEditOpts {
    /// Max ms for a tool_start→tool_done span (default: 800)
    pub tool_max_ms: u64,
    /// Max ms gap between done→next user_message (default: 2000)
    pub gap_max_ms: u64,
    /// Max ms for thinking duration (default: 1200)
    pub think_max_ms: u64,
    /// Max ms between user_message→thinking (model response delay, default: 1000)
    pub response_delay_max_ms: u64,
}

impl Default for AutoEditOpts {
    fn default() -> Self {
        Self {
            tool_max_ms: 800,
            gap_max_ms: 2000,
            think_max_ms: 1200,
            response_delay_max_ms: 1000,
        }
    }
}

fn truncate_for_timeline(s: &str) -> String {
    if s.len() > 500 {
        let mut end = 497;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeline_roundtrip() {
        let events = vec![
            TimelineEvent {
                t: 0,
                kind: TimelineEventKind::UserMessage {
                    text: "hello".to_string(),
                },
            },
            TimelineEvent {
                t: 500,
                kind: TimelineEventKind::Thinking { duration: 1000 },
            },
            TimelineEvent {
                t: 1500,
                kind: TimelineEventKind::StreamText {
                    text: "Hi there!".to_string(),
                    speed: 80,
                },
            },
            TimelineEvent {
                t: 2000,
                kind: TimelineEventKind::Done,
            },
        ];

        // Serialize to JSON
        let json = serde_json::to_string_pretty(&events).unwrap();
        assert!(json.contains("user_message"));
        assert!(json.contains("stream_text"));

        // Deserialize back
        let parsed: Vec<TimelineEvent> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed[0].t, 0);
        assert_eq!(parsed[2].t, 1500);
    }

    #[test]
    fn test_timeline_to_replay_events() {
        let events = vec![
            TimelineEvent {
                t: 0,
                kind: TimelineEventKind::StreamText {
                    text: "Hello world".to_string(),
                    speed: 80,
                },
            },
            TimelineEvent {
                t: 500,
                kind: TimelineEventKind::Done,
            },
        ];

        let replay_events = timeline_to_replay_events(&events);
        assert!(!replay_events.is_empty());

        // First event should be a Server(TextDelta)
        match &replay_events[0].1 {
            ReplayEvent::Server(ServerEvent::TextDelta { text }) => assert!(!text.is_empty()),
            _ => panic!("Expected Server(TextDelta)"),
        }

        // Last event should be Server(Done)
        match &replay_events.last().unwrap().1 {
            ReplayEvent::Server(ServerEvent::Done { .. }) => {}
            _ => panic!("Expected Server(Done)"),
        }
    }

    #[test]
    fn test_tool_events() {
        let events = vec![
            TimelineEvent {
                t: 0,
                kind: TimelineEventKind::ToolStart {
                    name: "file_read".to_string(),
                    input: serde_json::json!({"file_path": "/tmp/test.rs"}),
                },
            },
            TimelineEvent {
                t: 500,
                kind: TimelineEventKind::ToolDone {
                    name: "file_read".to_string(),
                    output: "fn main() {}".to_string(),
                    is_error: false,
                },
            },
        ];

        let replay_events = timeline_to_replay_events(&events);
        let types: Vec<&str> = replay_events
            .iter()
            .filter_map(|(_, e)| match e {
                ReplayEvent::Server(se) => Some(match se {
                    ServerEvent::ToolStart { .. } => "start",
                    ServerEvent::ToolInput { .. } => "input",
                    ServerEvent::ToolExec { .. } => "exec",
                    ServerEvent::ToolDone { .. } => "done",
                    _ => "other",
                }),
                _ => None,
            })
            .collect();
        assert!(types.contains(&"start"));
        assert!(types.contains(&"exec"));
        assert!(types.contains(&"done"));
    }

    #[test]
    fn test_user_message_and_thinking() {
        let events = vec![
            TimelineEvent {
                t: 0,
                kind: TimelineEventKind::UserMessage {
                    text: "hello".to_string(),
                },
            },
            TimelineEvent {
                t: 500,
                kind: TimelineEventKind::Thinking { duration: 800 },
            },
            TimelineEvent {
                t: 1300,
                kind: TimelineEventKind::StreamText {
                    text: "Hi!".to_string(),
                    speed: 80,
                },
            },
        ];

        let replay_events = timeline_to_replay_events(&events);

        // First should be UserMessage
        assert!(matches!(&replay_events[0].1, ReplayEvent::UserMessage { .. }));

        // Second should be StartProcessing
        assert!(matches!(&replay_events[1].1, ReplayEvent::StartProcessing));

        // Third should be Server(TextDelta)
        assert!(matches!(
            &replay_events[2].1,
            ReplayEvent::Server(ServerEvent::TextDelta { .. })
        ));
    }

    #[test]
    fn test_tool_ids_match_between_start_and_done() {
        let events = vec![
            TimelineEvent {
                t: 0,
                kind: TimelineEventKind::ToolStart {
                    name: "file_read".to_string(),
                    input: serde_json::json!({"file_path": "/tmp/test.rs"}),
                },
            },
            TimelineEvent {
                t: 500,
                kind: TimelineEventKind::ToolDone {
                    name: "file_read".to_string(),
                    output: "fn main() {}".to_string(),
                    is_error: false,
                },
            },
        ];

        let replay_events = timeline_to_replay_events(&events);

        let start_id = replay_events.iter().find_map(|(_, e)| match e {
            ReplayEvent::Server(ServerEvent::ToolStart { id, .. }) => Some(id.clone()),
            _ => None,
        });
        let exec_id = replay_events.iter().find_map(|(_, e)| match e {
            ReplayEvent::Server(ServerEvent::ToolExec { id, .. }) => Some(id.clone()),
            _ => None,
        });
        let done_id = replay_events.iter().find_map(|(_, e)| match e {
            ReplayEvent::Server(ServerEvent::ToolDone { id, .. }) => Some(id.clone()),
            _ => None,
        });

        assert!(start_id.is_some(), "Should have ToolStart");
        assert_eq!(start_id, exec_id, "ToolStart and ToolExec IDs must match");
        assert_eq!(start_id, done_id, "ToolStart and ToolDone IDs must match");
    }

    #[test]
    fn test_batch_tool_input_preserved() {
        let batch_input = serde_json::json!({
            "tool_calls": [
                {"tool": "file_read", "parameters": {"file_path": "/tmp/a.rs"}},
                {"tool": "file_read", "parameters": {"file_path": "/tmp/b.rs"}},
                {"tool": "file_grep", "parameters": {"pattern": "foo"}},
            ]
        });

        let events = vec![
            TimelineEvent {
                t: 0,
                kind: TimelineEventKind::ToolStart {
                    name: "batch".to_string(),
                    input: batch_input.clone(),
                },
            },
            TimelineEvent {
                t: 1000,
                kind: TimelineEventKind::ToolDone {
                    name: "batch".to_string(),
                    output: "--- [1] file_read ---\nok\n--- [2] file_read ---\nok\n--- [3] file_grep ---\nok".to_string(),
                    is_error: false,
                },
            },
        ];

        let replay_events = timeline_to_replay_events(&events);

        // Verify the ToolInput delta contains the batch input
        let input_delta = replay_events.iter().find_map(|(_, e)| match e {
            ReplayEvent::Server(ServerEvent::ToolInput { delta }) => Some(delta.clone()),
            _ => None,
        });
        assert!(input_delta.is_some(), "Should have ToolInput with batch params");
        let parsed: serde_json::Value = serde_json::from_str(&input_delta.unwrap()).unwrap();
        let tool_calls = parsed.get("tool_calls").and_then(|v| v.as_array());
        assert_eq!(tool_calls.map(|a| a.len()), Some(3), "Batch should have 3 tool calls");

        // Verify IDs match
        let start_id = replay_events.iter().find_map(|(_, e)| match e {
            ReplayEvent::Server(ServerEvent::ToolStart { id, .. }) => Some(id.clone()),
            _ => None,
        });
        let done_id = replay_events.iter().find_map(|(_, e)| match e {
            ReplayEvent::Server(ServerEvent::ToolDone { id, .. }) => Some(id.clone()),
            _ => None,
        });
        assert_eq!(start_id, done_id, "Batch ToolStart and ToolDone IDs must match");
    }

    #[test]
    fn test_auto_edit_compresses_tool_spans() {
        let events = vec![
            TimelineEvent { t: 0, kind: TimelineEventKind::UserMessage { text: "hi".into() } },
            TimelineEvent { t: 500, kind: TimelineEventKind::Thinking { duration: 800 } },
            TimelineEvent { t: 1300, kind: TimelineEventKind::StreamText { text: "Let me check.".into(), speed: 80 } },
            TimelineEvent { t: 2000, kind: TimelineEventKind::ToolStart { name: "file_read".into(), input: serde_json::json!({}) } },
            TimelineEvent { t: 12000, kind: TimelineEventKind::ToolDone { name: "file_read".into(), output: "ok".into(), is_error: false } },
            TimelineEvent { t: 13000, kind: TimelineEventKind::StreamText { text: "Done!".into(), speed: 80 } },
            TimelineEvent { t: 14000, kind: TimelineEventKind::Done },
        ];

        let opts = AutoEditOpts { tool_max_ms: 800, gap_max_ms: 2000, think_max_ms: 1200, response_delay_max_ms: 1000 };
        let edited = auto_edit_timeline(&events, &opts);

        assert_eq!(edited.len(), events.len());

        let tool_start_t = edited[3].t;
        let tool_done_t = edited[4].t;
        let tool_span = tool_done_t - tool_start_t;
        assert!(tool_span <= 800, "Tool span should be compressed to ≤800ms, got {tool_span}ms");

        assert!(edited[5].t > tool_done_t, "Events after tool_done should still be ordered");
    }

    #[test]
    fn test_auto_edit_compresses_inter_prompt_gaps() {
        let events = vec![
            TimelineEvent { t: 0, kind: TimelineEventKind::UserMessage { text: "first".into() } },
            TimelineEvent { t: 500, kind: TimelineEventKind::Thinking { duration: 800 } },
            TimelineEvent { t: 1500, kind: TimelineEventKind::StreamText { text: "response".into(), speed: 80 } },
            TimelineEvent { t: 2000, kind: TimelineEventKind::Done },
            TimelineEvent { t: 30000, kind: TimelineEventKind::UserMessage { text: "second".into() } },
            TimelineEvent { t: 30500, kind: TimelineEventKind::Thinking { duration: 800 } },
            TimelineEvent { t: 31500, kind: TimelineEventKind::StreamText { text: "response2".into(), speed: 80 } },
            TimelineEvent { t: 32000, kind: TimelineEventKind::Done },
        ];

        let opts = AutoEditOpts::default();
        let edited = auto_edit_timeline(&events, &opts);

        let done_t = edited[3].t;
        let next_user_t = edited[4].t;
        let gap = next_user_t - done_t;
        assert!(gap <= 2000, "Gap between turns should be compressed to ≤2000ms, got {gap}ms");

        let total_original = events.last().unwrap().t;
        let total_edited = edited.last().unwrap().t;
        assert!(total_edited < total_original, "Total time should be shorter: {total_edited} < {total_original}");
    }

    #[test]
    fn test_auto_edit_clamps_thinking() {
        let events = vec![
            TimelineEvent { t: 0, kind: TimelineEventKind::UserMessage { text: "hi".into() } },
            TimelineEvent { t: 500, kind: TimelineEventKind::Thinking { duration: 5000 } },
            TimelineEvent { t: 5500, kind: TimelineEventKind::StreamText { text: "ok".into(), speed: 80 } },
        ];

        let opts = AutoEditOpts { think_max_ms: 1200, ..Default::default() };
        let edited = auto_edit_timeline(&events, &opts);

        match &edited[1].kind {
            TimelineEventKind::Thinking { duration } => {
                assert_eq!(*duration, 1200, "Thinking should be clamped to 1200ms");
            }
            _ => panic!("Expected Thinking event"),
        }
    }

    #[test]
    fn test_auto_edit_preserves_already_fast_timeline() {
        let events = vec![
            TimelineEvent { t: 0, kind: TimelineEventKind::UserMessage { text: "hi".into() } },
            TimelineEvent { t: 200, kind: TimelineEventKind::Thinking { duration: 500 } },
            TimelineEvent { t: 700, kind: TimelineEventKind::StreamText { text: "hello!".into(), speed: 80 } },
            TimelineEvent { t: 900, kind: TimelineEventKind::Done },
            TimelineEvent { t: 1500, kind: TimelineEventKind::UserMessage { text: "bye".into() } },
        ];

        let opts = AutoEditOpts::default();
        let edited = auto_edit_timeline(&events, &opts);

        for (orig, ed) in events.iter().zip(edited.iter()) {
            assert_eq!(orig.t, ed.t, "Fast timeline should not be modified");
        }
    }

    #[test]
    fn test_auto_edit_empty_timeline() {
        let edited = auto_edit_timeline(&[], &AutoEditOpts::default());
        assert!(edited.is_empty());
    }
}
