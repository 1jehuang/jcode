use crate::message::{ContentBlock, Role};
use crate::protocol::ServerEvent;
use crate::session::{Session, StoredReplayEventKind};
use anyhow::Result;
use chrono::Duration;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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

    /// Memory injection from auto-recall
    #[serde(rename = "memory_injection")]
    MemoryInjection {
        summary: String,
        content: String,
        count: u32,
    },
    /// A persisted non-provider display message.
    #[serde(rename = "display_message")]
    DisplayMessage {
        role: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        content: String,
    },
    /// Historical swarm status snapshot.
    #[serde(rename = "swarm_status")]
    SwarmStatus {
        members: Vec<crate::protocol::SwarmMemberStatus>,
    },
    /// Historical swarm plan snapshot.
    #[serde(rename = "swarm_plan")]
    SwarmPlan {
        swarm_id: String,
        version: u64,
        items: Vec<crate::plan::PlanItem>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        participants: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

fn default_thinking_duration() -> u64 {
    1200
}
fn default_stream_speed() -> u64 {
    80
}

const MAX_INITIAL_REPLAY_IDLE_MS: u64 = 0;

fn cap_initial_replay_idle(events: &mut [TimelineEvent]) {
    let Some(first_t) = events.first().map(|event| event.t) else {
        return;
    };
    let shift = first_t.saturating_sub(MAX_INITIAL_REPLAY_IDLE_MS);
    if shift == 0 {
        return;
    }
    for event in events {
        event.t = event.t.saturating_sub(shift);
    }
}

/// Export a session to a replay timeline.
///
/// Uses stored timestamps for real pacing, falls back to estimates.
/// Memory injections from `session.memory_injections` are inserted at the
/// correct positions based on their `before_message` index.
pub fn export_timeline(session: &Session) -> Vec<TimelineEvent> {
    let mut events = Vec::new();
    let mut t: u64 = 0;
    let session_start = session.created_at;

    // Track tool IDs for pairing ToolUse → ToolResult
    let mut pending_tools: Vec<(String, String, serde_json::Value)> = Vec::new(); // (id, name, input)

    // Track memory injections by message index
    let mut memory_by_msg: std::collections::HashMap<usize, Vec<_>> =
        std::collections::HashMap::new();
    for inj in &session.memory_injections {
        if let Some(idx) = inj.before_message {
            memory_by_msg.entry(idx).or_default().push(inj);
        }
    }

    for (msg_idx, msg) in session.messages.iter().enumerate() {
        // Insert memory injections before this message
        if let Some(injs) = memory_by_msg.get(&msg_idx) {
            for inj in injs {
                events.push(TimelineEvent {
                    t,
                    kind: TimelineEventKind::MemoryInjection {
                        summary: inj.summary.clone(),
                        content: inj.content.clone(),
                        count: inj.count,
                    },
                });
                t += 500; // Brief pause after memory injection
            }
        }

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
        let last_is_done = events
            .last()
            .is_some_and(|e| matches!(e.kind, TimelineEventKind::Done));
        if !last_is_done {
            events.push(TimelineEvent {
                t,
                kind: TimelineEventKind::Done,
            });
        }
    }

    for replay_event in &session.replay_events {
        let offset = replay_event
            .timestamp
            .signed_duration_since(session_start)
            .num_milliseconds()
            .max(0) as u64;
        let kind = match &replay_event.kind {
            StoredReplayEventKind::DisplayMessage {
                role,
                title,
                content,
            } => TimelineEventKind::DisplayMessage {
                role: role.clone(),
                title: title.clone(),
                content: content.clone(),
            },
            StoredReplayEventKind::SwarmStatus { members } => TimelineEventKind::SwarmStatus {
                members: members.clone(),
            },
            StoredReplayEventKind::SwarmPlan {
                swarm_id,
                version,
                items,
                participants,
                reason,
            } => TimelineEventKind::SwarmPlan {
                swarm_id: swarm_id.clone(),
                version: *version,
                items: items.clone(),
                participants: participants.clone(),
                reason: reason.clone(),
            },
        };
        events.push(TimelineEvent { t: offset, kind });
    }

    events.sort_by_key(|event| event.t);
    cap_initial_replay_idle(&mut events);

    events
}

/// Replay-specific server events that don't exist in the normal protocol.
/// These are handled specially in `run_replay`.
#[derive(Debug, Clone)]
#[expect(
    clippy::large_enum_variant,
    reason = "replay events mirror protocol events directly for simpler playback serialization and handling"
)]
pub enum ReplayEvent {
    /// A normal server event
    Server(ServerEvent),
    /// User message (displayed directly, not via server event)
    UserMessage { text: String },
    /// Start processing state (shows thinking spinner)
    StartProcessing,
    /// Memory injection from auto-recall
    MemoryInjection {
        summary: String,
        content: String,
        count: u32,
    },
    /// Persisted non-provider display message.
    DisplayMessage {
        role: String,
        title: Option<String>,
        content: String,
    },
    /// Historical swarm status snapshot.
    SwarmStatus {
        members: Vec<crate::protocol::SwarmMemberStatus>,
    },
    /// Historical swarm plan snapshot.
    SwarmPlan {
        swarm_id: String,
        version: u64,
        items: Vec<crate::plan::PlanItem>,
    },
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
        let delay = if out.is_empty() {
            MAX_INITIAL_REPLAY_IDLE_MS
        } else {
            delay
        };
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
                    out.push((
                        chunk_delay,
                        ReplayEvent::Server(ServerEvent::TextDelta {
                            text: chunk.clone(),
                        }),
                    ));
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
                        ReplayEvent::Server(ServerEvent::ToolInput { delta: input_str }),
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
                out.push((
                    delay,
                    ReplayEvent::Server(ServerEvent::Done { id: turn_id }),
                ));
                turn_id += 1;
            }
            TimelineEventKind::MemoryInjection {
                summary,
                content,
                count,
            } => {
                out.push((
                    delay,
                    ReplayEvent::MemoryInjection {
                        summary: summary.clone(),
                        content: content.clone(),
                        count: *count,
                    },
                ));
            }
            TimelineEventKind::DisplayMessage {
                role,
                title,
                content,
            } => {
                out.push((
                    delay,
                    ReplayEvent::DisplayMessage {
                        role: role.clone(),
                        title: title.clone(),
                        content: content.clone(),
                    },
                ));
            }
            TimelineEventKind::SwarmStatus { members } => {
                out.push((
                    delay,
                    ReplayEvent::SwarmStatus {
                        members: members.clone(),
                    },
                ));
            }
            TimelineEventKind::SwarmPlan {
                swarm_id,
                version,
                items,
                ..
            } => {
                out.push((
                    delay,
                    ReplayEvent::SwarmPlan {
                        swarm_id: swarm_id.clone(),
                        version: *version,
                        items: items.clone(),
                    },
                ));
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
        return Session::load_from_path(path);
    }

    // Try as session ID in the sessions directory
    let sessions_dir = crate::storage::jcode_dir()?.join("sessions");
    // Try exact match
    let exact = sessions_dir.join(format!("{}.json", id_or_path));
    if exact.exists() {
        return Session::load_from_path(&exact);
    }

    // Try prefix match (session_<id>.json or session_<name>_<ts>.json)
    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.contains(id_or_path) && name.ends_with(".json") {
            return Session::load_from_path(&entry.path());
        }
    }

    anyhow::bail!(
        "Session not found: '{}'. Provide a session ID, name, or file path.",
        id_or_path
    );
}

#[derive(Debug, Clone)]
pub struct SwarmReplaySession {
    pub session: Session,
    pub timeline: Vec<TimelineEvent>,
}

pub fn load_swarm_sessions(
    seed_id_or_path: &str,
    auto_edit: bool,
) -> Result<Vec<SwarmReplaySession>> {
    let seed = load_session(seed_id_or_path)?;
    let seed_working_dir = seed.working_dir.clone();
    let lower_bound = seed.created_at - Duration::hours(6);
    let upper_bound = seed.updated_at + Duration::hours(6);

    let sessions_dir = crate::storage::jcode_dir()?.join("sessions");
    if !sessions_dir.exists() {
        return Ok(vec![SwarmReplaySession {
            timeline: maybe_auto_edit(&seed, auto_edit),
            session: seed,
        }]);
    }

    let mut all_sessions: Vec<Session> = Vec::new();
    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.extension().map(|e| e == "json").unwrap_or(false) {
            continue;
        }
        let Ok(session) = Session::load_from_path(&path) else {
            continue;
        };
        all_sessions.push(session);
    }

    let mut selected_ids: BTreeSet<String> = BTreeSet::new();
    selected_ids.insert(seed.id.clone());

    for session in &all_sessions {
        if session.id == seed.id {
            continue;
        }
        let same_working_dir =
            seed_working_dir.is_some() && session.working_dir == seed_working_dir;
        let linked_parent = session.parent_id.as_deref() == Some(seed.id.as_str())
            || seed.parent_id.as_deref() == Some(session.id.as_str())
            || (seed.parent_id.is_some() && session.parent_id == seed.parent_id);
        let overlapping_time =
            session.updated_at >= lower_bound && session.created_at <= upper_bound;
        let has_swarm_events = session.replay_events.iter().any(|evt| {
            matches!(
                evt.kind,
                StoredReplayEventKind::SwarmStatus { .. } | StoredReplayEventKind::SwarmPlan { .. }
            )
        });

        if overlapping_time && (same_working_dir || linked_parent || has_swarm_events) {
            selected_ids.insert(session.id.clone());
        }
    }

    let mut selected: Vec<Session> = all_sessions
        .into_iter()
        .filter(|session| selected_ids.contains(&session.id))
        .collect();
    if !selected.iter().any(|session| session.id == seed.id) {
        selected.push(seed.clone());
    }

    selected.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(selected
        .into_iter()
        .map(|session| {
            let timeline = maybe_auto_edit(&session, auto_edit);
            SwarmReplaySession { session, timeline }
        })
        .collect())
}

fn maybe_auto_edit(session: &Session, auto_edit: bool) -> Vec<TimelineEvent> {
    let timeline = export_timeline(session);
    if auto_edit {
        auto_edit_timeline(&timeline, &AutoEditOpts::default())
    } else {
        timeline
    }
}

#[derive(Debug, Clone)]
pub struct PaneReplayInput {
    pub session: Session,
    pub timeline: Vec<TimelineEvent>,
}

#[derive(Debug, Clone)]
pub struct SwarmPaneFrames {
    pub session_id: String,
    pub title: String,
    pub frames: Vec<(f64, ratatui::buffer::Buffer)>,
}

pub fn compose_swarm_buffers(
    pane_frames: &[SwarmPaneFrames],
    width: u16,
    height: u16,
    fps: u32,
    cols: u16,
) -> Vec<(f64, ratatui::buffer::Buffer)> {
    use ratatui::{buffer::Buffer, layout::Rect};

    if pane_frames.is_empty() {
        return Vec::new();
    }

    let fps = fps.max(1);
    let frame_step = 1.0 / fps as f64;
    let end_time = pane_frames
        .iter()
        .filter_map(|pane| pane.frames.last().map(|(t, _)| *t))
        .fold(0.0, f64::max);

    let pane_count = pane_frames.len() as u16;
    let cols = cols.clamp(1, pane_count.max(1));
    let rows = pane_count.div_ceil(cols).max(1);
    let pane_width = (width / cols).max(1);
    let pane_height = (height / rows).max(1);

    let mut output = Vec::new();
    let mut t = 0.0;
    while t <= end_time + frame_step {
        let mut canvas = Buffer::empty(Rect::new(0, 0, width, height));
        for (idx, pane) in pane_frames.iter().enumerate() {
            let idx = idx as u16;
            let col = idx % cols;
            let row = idx / cols;
            let x = col * pane_width;
            let y = row * pane_height;
            let area = Rect::new(
                x,
                y,
                if col == cols - 1 {
                    width - x
                } else {
                    pane_width
                },
                if row == rows - 1 {
                    height - y
                } else {
                    pane_height
                },
            );
            if let Some(buf) = buffer_at_time(&pane.frames, t) {
                blit_buffer(&mut canvas, area, buf);
            }
        }
        output.push((t, canvas));
        t += frame_step;
    }

    output
}

fn buffer_at_time(
    frames: &[(f64, ratatui::buffer::Buffer)],
    t: f64,
) -> Option<&ratatui::buffer::Buffer> {
    let mut current = None;
    for (frame_t, buf) in frames {
        if *frame_t <= t {
            current = Some(buf);
        } else {
            break;
        }
    }
    current.or_else(|| frames.first().map(|(_, buf)| buf))
}

fn blit_buffer(
    dst: &mut ratatui::buffer::Buffer,
    area: ratatui::layout::Rect,
    src: &ratatui::buffer::Buffer,
) {
    for sy in 0..area.height.min(src.area.height) {
        for sx in 0..area.width.min(src.area.width) {
            let dx = area.x + sx;
            let dy = area.y + sy;
            if let (Some(src_cell), Some(dst_cell)) = (src.cell((sx, sy)), dst.cell_mut((dx, dy))) {
                *dst_cell = src_cell.clone();
            }
        }
    }
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
    // Track the end of the most recent top-level tool span so we can
    // compress any long idle wait before the assistant resumes.
    let mut last_tool_done_t: Option<u64> = None;

    // Track done→user_message gaps
    let mut last_done_t: Option<u64> = None;
    // Track user_message→thinking gaps
    let mut last_user_msg_t: Option<u64> = None;

    for event in timeline {
        let orig_t = event.t;
        let mut new_t = (orig_t as i64 + time_shift).max(0) as u64;

        // If the assistant sat idle for a long time after a tool completed
        // (for example during a selfdev reload), compress that post-tool gap
        // before the next later event.
        if let Some(tool_done_t) = last_tool_done_t
            && orig_t > tool_done_t
        {
            let gap = orig_t.saturating_sub(tool_done_t);
            if gap > opts.response_delay_max_ms {
                time_shift -= (gap - opts.response_delay_max_ms) as i64;
                new_t = (orig_t as i64 + time_shift).max(0) as u64;
            }
            last_tool_done_t = None;
        }

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
                    kind: TimelineEventKind::Thinking { duration: clamped },
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
                    last_tool_done_t = Some(orig_t);
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
mod tests;
