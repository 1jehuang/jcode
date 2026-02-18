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

/// Convert a timeline into a sequence of (delay_ms, ServerEvent) pairs for playback.
pub fn timeline_to_server_events(timeline: &[TimelineEvent]) -> Vec<(u64, ServerEvent)> {
    let mut out = Vec::new();
    let mut prev_t: u64 = 0;
    let mut turn_id: u64 = 1;
    let mut tool_id_counter: u64 = 0;

    for event in timeline {
        let delay = event.t.saturating_sub(prev_t);
        prev_t = event.t;

        match &event.kind {
            TimelineEventKind::UserMessage { .. } => {
                // User messages are handled by pushing to display_messages directly,
                // not via ServerEvent. We emit a marker that the player will handle.
                out.push((delay, ServerEvent::Done { id: turn_id }));
                turn_id += 1;
            }
            TimelineEventKind::Thinking { duration } => {
                // Emit a tiny text delta to trigger Streaming state, then wait
                out.push((delay, ServerEvent::TextDelta { text: String::new() }));
                // The thinking duration is baked into the gap before the next event
                let _ = duration; // Duration is already encoded in the timeline gaps
            }
            TimelineEventKind::StreamText { text, speed } => {
                // Split text into chunks and emit as TextDelta events
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
                    out.push((chunk_delay, ServerEvent::TextDelta { text: chunk.clone() }));
                }
            }
            TimelineEventKind::ToolStart { name, input } => {
                tool_id_counter += 1;
                let id = format!("replay_tool_{}", tool_id_counter);

                // ToolStart
                out.push((
                    delay,
                    ServerEvent::ToolStart {
                        id: id.clone(),
                        name: name.clone(),
                    },
                ));

                // Stream tool input as JSON
                let input_str = serde_json::to_string(input).unwrap_or_default();
                if !input_str.is_empty() && input_str != "null" {
                    out.push((
                        0,
                        ServerEvent::ToolInput {
                            delta: input_str,
                        },
                    ));
                }

                // ToolExec (transition from input streaming to execution)
                out.push((
                    50,
                    ServerEvent::ToolExec {
                        id: id.clone(),
                        name: name.clone(),
                    },
                ));
            }
            TimelineEventKind::ToolDone {
                name,
                output,
                is_error,
            } => {
                tool_id_counter += 1;
                let id = format!("replay_tool_{}", tool_id_counter);
                out.push((
                    delay,
                    ServerEvent::ToolDone {
                        id,
                        name: name.clone(),
                        output: output.clone(),
                        error: if *is_error {
                            Some(output.clone())
                        } else {
                            None
                        },
                    },
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
                    ServerEvent::TokenUsage {
                        input: *input,
                        output: *output,
                        cache_read_input: *cache_read,
                        cache_creation_input: *cache_creation,
                    },
                ));
            }
            TimelineEventKind::Done => {
                out.push((delay, ServerEvent::Done { id: turn_id }));
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

fn truncate_for_timeline(s: &str) -> String {
    if s.len() > 500 {
        format!("{}...", &s[..497])
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
    fn test_timeline_to_server_events() {
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

        let server_events = timeline_to_server_events(&events);
        assert!(!server_events.is_empty());

        // First event should be a TextDelta
        match &server_events[0].1 {
            ServerEvent::TextDelta { text } => assert!(!text.is_empty()),
            _ => panic!("Expected TextDelta"),
        }

        // Last event should be Done
        match &server_events.last().unwrap().1 {
            ServerEvent::Done { .. } => {}
            _ => panic!("Expected Done"),
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

        let server_events = timeline_to_server_events(&events);
        // Should have: ToolStart, ToolInput, ToolExec, ToolDone
        let types: Vec<&str> = server_events
            .iter()
            .map(|(_, e)| match e {
                ServerEvent::ToolStart { .. } => "start",
                ServerEvent::ToolInput { .. } => "input",
                ServerEvent::ToolExec { .. } => "exec",
                ServerEvent::ToolDone { .. } => "done",
                _ => "other",
            })
            .collect();
        assert!(types.contains(&"start"));
        assert!(types.contains(&"exec"));
        assert!(types.contains(&"done"));
    }
}
