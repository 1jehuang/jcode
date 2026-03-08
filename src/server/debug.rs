use super::debug_testers::execute_tester_command;
use super::{
    broadcast_swarm_status, create_headless_session, debug_control_allowed, git_common_dir_for,
    record_swarm_event, server_has_newer_binary, swarm_id_for_dir, FileAccess, ServerIdentity,
    SharedContext, SwarmEvent, SwarmEventType, SwarmMember, VersionedPlan, MAX_EVENT_HISTORY,
};
use crate::agent::Agent;
use crate::ambient_runner::AmbientRunnerHandle;
use crate::build;
use crate::id;
use crate::mcp::McpConfig;
use crate::plan::PlanItem;
use crate::protocol::{decode_request, encode_event, NotificationType, Request, ServerEvent};
use crate::provider::Provider;
use crate::transport::Stream;
use anyhow::Result;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};

#[derive(Default)]
pub(super) struct ClientDebugState {
    pub(super) active_id: Option<String>,
    pub(super) clients: HashMap<String, mpsc::UnboundedSender<(u64, String)>>,
}

#[derive(Clone, Debug)]
pub(super) struct ClientConnectionInfo {
    pub(super) client_id: String,
    pub(super) session_id: String,
    pub(super) connected_at: Instant,
    pub(super) last_seen: Instant,
}

#[derive(Clone, Debug)]
pub(super) enum DebugJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

impl DebugJobStatus {
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            DebugJobStatus::Queued => "queued",
            DebugJobStatus::Running => "running",
            DebugJobStatus::Completed => "completed",
            DebugJobStatus::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct DebugJob {
    pub(super) id: String,
    pub(super) status: DebugJobStatus,
    pub(super) command: String,
    pub(super) session_id: Option<String>,
    pub(super) created_at: Instant,
    pub(super) started_at: Option<Instant>,
    pub(super) finished_at: Option<Instant>,
    pub(super) output: Option<String>,
    pub(super) error: Option<String>,
}

impl DebugJob {
    pub(super) fn summary_payload(&self) -> Value {
        let now = Instant::now();
        let elapsed_secs = now.duration_since(self.created_at).as_secs_f64();
        let run_secs = self.started_at.map(|s| now.duration_since(s).as_secs_f64());
        let total_secs = self
            .finished_at
            .map(|f| f.duration_since(self.created_at).as_secs_f64());

        serde_json::json!({
            "id": self.id.clone(),
            "status": self.status.as_str(),
            "command": self.command.clone(),
            "session_id": self.session_id.clone(),
            "elapsed_secs": elapsed_secs,
            "run_secs": run_secs,
            "total_secs": total_secs,
        })
    }

    pub(super) fn status_payload(&self) -> Value {
        let mut payload = self.summary_payload();
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("output".to_string(), serde_json::json!(self.output.clone()));
            obj.insert("error".to_string(), serde_json::json!(self.error.clone()));
        }
        payload
    }
}

impl ClientDebugState {
    pub(super) fn register(&mut self, client_id: String, tx: mpsc::UnboundedSender<(u64, String)>) {
        self.active_id = Some(client_id.clone());
        self.clients.insert(client_id, tx);
    }

    pub(super) fn unregister(&mut self, client_id: &str) {
        self.clients.remove(client_id);
        if self.active_id.as_deref() == Some(client_id) {
            self.active_id = self.clients.keys().next().cloned();
        }
    }

    pub(super) fn active_sender(
        &mut self,
    ) -> Option<(String, mpsc::UnboundedSender<(u64, String)>)> {
        if let Some(active_id) = self.active_id.clone() {
            if let Some(tx) = self.clients.get(&active_id) {
                return Some((active_id, tx.clone()));
            }
        }
        if let Some((id, tx)) = self.clients.iter().next() {
            let id = id.clone();
            self.active_id = Some(id.clone());
            return Some((id, tx.clone()));
        }
        None
    }
}

/// Execute a client debug command (visual debug, TUI state, etc.)
/// These commands access the TUI's visual debug module which uses global state.
pub(super) fn execute_client_debug_command(command: &str) -> String {
    use crate::tui::{markdown, mermaid, visual_debug};

    let trimmed = command.trim();

    // Visual debug commands
    if trimmed == "frame" || trimmed == "screen-json" {
        visual_debug::enable(); // Ensure enabled
        return visual_debug::latest_frame_json().unwrap_or_else(|| {
            "No frames captured yet. Try again after some UI activity.".to_string()
        });
    }

    if trimmed == "frame-normalized" || trimmed == "screen-json-normalized" {
        visual_debug::enable();
        return visual_debug::latest_frame_json_normalized()
            .unwrap_or_else(|| "No frames captured yet.".to_string());
    }

    if trimmed == "screen" {
        visual_debug::enable();
        match visual_debug::dump_to_file() {
            Ok(path) => return format!("Frames written to: {}", path.display()),
            Err(e) => return format!("Error dumping frames: {}", e),
        }
    }

    if trimmed == "enable" || trimmed == "debug-enable" {
        visual_debug::enable();
        return "Visual debugging enabled.".to_string();
    }

    if trimmed == "disable" || trimmed == "debug-disable" {
        visual_debug::disable();
        return "Visual debugging disabled.".to_string();
    }

    if trimmed == "status" {
        let enabled = visual_debug::is_enabled();
        let overlay = visual_debug::overlay_enabled();
        return serde_json::json!({
            "visual_debug_enabled": enabled,
            "visual_debug_overlay": overlay,
        })
        .to_string();
    }

    if trimmed == "overlay" || trimmed == "overlay:status" {
        let overlay = visual_debug::overlay_enabled();
        return serde_json::json!({
            "visual_debug_overlay": overlay,
        })
        .to_string();
    }

    if trimmed == "overlay:on" || trimmed == "overlay:enable" {
        visual_debug::set_overlay(true);
        return "Visual debug overlay enabled.".to_string();
    }

    if trimmed == "overlay:off" || trimmed == "overlay:disable" {
        visual_debug::set_overlay(false);
        return "Visual debug overlay disabled.".to_string();
    }

    if trimmed == "layout" {
        visual_debug::enable();
        return visual_debug::latest_frame().map_or_else(
            || "layout: no frames captured".to_string(),
            |frame| {
                serde_json::to_string_pretty(&serde_json::json!({
                    "frame_id": frame.frame_id,
                    "terminal_size": frame.terminal_size,
                    "layout": frame.layout,
                }))
                .unwrap_or_else(|_| "{}".to_string())
            },
        );
    }

    if trimmed == "margins" {
        visual_debug::enable();
        return visual_debug::latest_frame().map_or_else(
            || "margins: no frames captured".to_string(),
            |frame| {
                serde_json::to_string_pretty(&serde_json::json!({
                    "frame_id": frame.frame_id,
                    "margins": frame.layout.margins,
                }))
                .unwrap_or_else(|_| "{}".to_string())
            },
        );
    }

    if trimmed == "widgets" || trimmed == "info-widgets" {
        visual_debug::enable();
        return visual_debug::latest_frame().map_or_else(
            || "widgets: no frames captured".to_string(),
            |frame| {
                serde_json::to_string_pretty(&serde_json::json!({
                    "frame_id": frame.frame_id,
                    "info_widgets": frame.info_widgets,
                }))
                .unwrap_or_else(|_| "{}".to_string())
            },
        );
    }

    if trimmed == "render-stats" {
        visual_debug::enable();
        return visual_debug::latest_frame().map_or_else(
            || "render-stats: no frames captured".to_string(),
            |frame| {
                serde_json::to_string_pretty(&serde_json::json!({
                    "frame_id": frame.frame_id,
                    "render_timing": frame.render_timing,
                    "render_order": frame.render_order,
                }))
                .unwrap_or_else(|_| "{}".to_string())
            },
        );
    }

    if trimmed == "render-order" {
        visual_debug::enable();
        return visual_debug::latest_frame().map_or_else(
            || "render-order: no frames captured".to_string(),
            |frame| {
                serde_json::to_string_pretty(&frame.render_order)
                    .unwrap_or_else(|_| "[]".to_string())
            },
        );
    }

    if trimmed == "anomalies" {
        visual_debug::enable();
        return visual_debug::latest_frame().map_or_else(
            || "anomalies: no frames captured".to_string(),
            |frame| {
                serde_json::to_string_pretty(&frame.anomalies).unwrap_or_else(|_| "[]".to_string())
            },
        );
    }

    if trimmed == "theme" {
        visual_debug::enable();
        return visual_debug::latest_frame().map_or_else(
            || "theme: no frames captured".to_string(),
            |frame| {
                serde_json::to_string_pretty(&frame.theme).unwrap_or_else(|_| "null".to_string())
            },
        );
    }

    if trimmed == "mermaid:stats" {
        let stats = mermaid::debug_stats();
        return serde_json::to_string_pretty(&stats).unwrap_or_else(|_| "{}".to_string());
    }

    if trimmed == "mermaid:memory" {
        let profile = mermaid::debug_memory_profile();
        return serde_json::to_string_pretty(&profile).unwrap_or_else(|_| "{}".to_string());
    }

    if trimmed == "mermaid:memory-bench" {
        let result = mermaid::debug_memory_benchmark(40);
        return serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
    }

    if let Some(raw_iterations) = trimmed.strip_prefix("mermaid:memory-bench ") {
        let iterations = match raw_iterations.trim().parse::<usize>() {
            Ok(v) => v,
            Err(_) => return "Invalid iterations (expected integer)".to_string(),
        };
        let result = mermaid::debug_memory_benchmark(iterations);
        return serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
    }

    if trimmed == "mermaid:cache" {
        let entries = mermaid::debug_cache();
        return serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string());
    }

    if trimmed == "mermaid:evict" || trimmed == "mermaid:clear-cache" {
        return match mermaid::clear_cache() {
            Ok(_) => "mermaid: cache cleared".to_string(),
            Err(e) => format!("mermaid: cache clear failed: {}", e),
        };
    }

    if trimmed == "mermaid:state" {
        let state = mermaid::debug_image_state();
        return serde_json::to_string_pretty(&state).unwrap_or_else(|_| "[]".to_string());
    }

    if trimmed == "mermaid:test" {
        let result = mermaid::debug_test_render();
        return serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
    }

    if trimmed == "mermaid:scroll" {
        let result = mermaid::debug_test_scroll(None);
        return serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
    }

    if let Some(content) = trimmed.strip_prefix("mermaid:render ") {
        let result = mermaid::debug_render(content);
        return serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
    }

    if let Some(hash_str) = trimmed.strip_prefix("mermaid:stability ") {
        if let Ok(hash) = u64::from_str_radix(hash_str, 16) {
            let result = mermaid::debug_test_resize_stability(hash);
            return serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
        }
        return "Invalid hash (expected hex)".to_string();
    }

    if trimmed == "mermaid:active" {
        let diagrams = mermaid::get_active_diagrams();
        let info: Vec<serde_json::Value> = diagrams
            .iter()
            .map(|d| {
                serde_json::json!({
                    "hash": format!("{:016x}", d.hash),
                    "width": d.width,
                    "height": d.height,
                    "label": d.label,
                })
            })
            .collect();
        return serde_json::to_string_pretty(&serde_json::json!({
            "count": diagrams.len(),
            "diagrams": info,
        }))
        .unwrap_or_else(|_| "{}".to_string());
    }

    if trimmed == "markdown:stats" {
        let stats = markdown::debug_stats();
        return serde_json::to_string_pretty(&stats).unwrap_or_else(|_| "{}".to_string());
    }

    if trimmed == "help" {
        return r#"Client debug commands:
  frame / screen-json      - Get latest visual debug frame (JSON)
  frame-normalized         - Get normalized frame (for diffs)
  screen                   - Dump visual debug frames to file
  layout                   - Get latest layout JSON
  margins                  - Get layout margins JSON
  widgets                  - Get info widget summary/placements
  render-stats             - Get render timing + order JSON
  render-order             - Get render order list
  anomalies                - Get latest visual debug anomalies
  theme                    - Get palette snapshot
  mermaid:stats            - Get mermaid render/cache stats
  mermaid:memory           - Mermaid memory profile (RSS + cache estimates)
  mermaid:memory-bench [n] - Run synthetic Mermaid memory benchmark
  mermaid:cache            - List mermaid cache entries
  mermaid:state            - Get image state (resize modes, areas)
  mermaid:test             - Render test diagram, return results
  mermaid:scroll           - Run scroll simulation test
  mermaid:render <content> - Render arbitrary mermaid content
  mermaid:stability <hash> - Test resize mode stability for hash
  mermaid:active           - List active diagrams (for pinned widget)
  mermaid:evict            - Clear mermaid cache
  markdown:stats           - Get markdown render stats
  overlay:on/off/status    - Toggle overlay boxes
  enable                   - Enable visual debug capture
  disable                  - Disable visual debug capture
  status                   - Get client debug status
  help                     - Show this help

Note: Visual debug captures TUI rendering state for debugging UI issues.
Frames are captured automatically when visual debug is enabled."#
            .to_string();
    }

    format!(
        "Unknown client command: {}. Use client:help for available commands.",
        trimmed
    )
}

/// Parse namespaced debug command (e.g., "server:state", "client:frame", "tester:list")
pub(super) fn parse_namespaced_command(command: &str) -> (&str, &str) {
    let trimmed = command.trim();
    if let Some(idx) = trimmed.find(':') {
        let namespace = &trimmed[..idx];
        let rest = &trimmed[idx + 1..];
        // Only recognize known namespaces
        match namespace {
            "server" | "client" | "tester" => (namespace, rest),
            _ => ("server", trimmed), // Default to server namespace
        }
    } else {
        ("server", trimmed) // No namespace = server
    }
}

/// Generate help text for debug commands
pub(super) fn debug_help_text() -> String {
    r#"Debug socket commands (namespaced):

SERVER COMMANDS (server: prefix or no prefix):
  state                    - Get agent state
  history                  - Get conversation history
  tools                    - List available tools (names only)
  tools:full               - List tools with full definitions (input_schema)
  mcp:servers              - List configured + connected MCP servers
  last_response            - Get last assistant response
  message:<text>           - Send message to agent
  message_async:<text>     - Send message async (returns job id)
  swarm_message:<text>     - Plan and run subtasks via swarm workers, then integrate
  swarm_message_async:<text> - Async swarm message (returns job id)
  tool:<name> <json>       - Execute tool directly
  cancel                   - Cancel in-flight generation (urgent interrupt)
  clear                    - Clear conversation history
  agent:info               - Get comprehensive agent internal state
  jobs                     - List async debug jobs
  job_status:<id>          - Get async job status/output
  job_wait:<id>            - Wait for async job to finish
  job_cancel:<id>          - Cancel a running job
  jobs:purge               - Remove completed/failed jobs
  jobs:session:<id>        - List jobs for a session
  background:tasks         - List background tasks
  sessions                 - List all sessions (with full metadata)
  clients                  - List connected TUI clients
  clients:map              - Map connected clients to sessions
  server:info              - Server identity, health, uptime
  swarm                    - List swarm members + status (alias: swarm:members)
  swarm:help               - Full swarm command reference
  create_session           - Create headless session
  create_session:<path>    - Create session with working dir
  destroy_session:<id>     - Destroy a session
  set_model:<model>        - Switch model (may change provider)
  set_provider:<name>      - Switch provider (claude/openai/openrouter/cursor/copilot/antigravity)
  trigger_extraction       - Force end-of-session memory extraction
  available_models         - List all available models
  reload                   - Trigger server reload with current binary

SWARM COMMANDS (swarm: prefix):
  swarm:members            - List all swarm members with details
  swarm:list               - List all swarms with member counts
  swarm:info:<swarm_id>    - Full info for a swarm
  swarm:coordinators       - List all coordinators
  swarm:roles              - List all members with roles
  swarm:plans              - List all swarm plans
  swarm:plan_version:<id>  - Show plan version for a swarm
  swarm:proposals          - List pending plan proposals
  swarm:context            - List all shared context
  swarm:touches            - List all file touches
  swarm:conflicts          - Files touched by multiple sessions
  swarm:channels           - List channel subscriptions
  swarm:broadcast:<msg>    - Broadcast to swarm members
  swarm:notify:<sid> <msg> - Send DM to specific session
  swarm:help               - Full swarm command reference

AMBIENT COMMANDS (ambient: prefix):
  ambient:status              - Current ambient state, cycle count, last run
  ambient:queue               - Scheduled queue contents
  ambient:trigger             - Manually trigger an ambient cycle
  ambient:log                 - Recent transcript summaries
  ambient:permissions         - List pending permission requests
  ambient:approve:<id>        - Approve a permission request
  ambient:deny:<id> [reason]  - Deny a permission request (optional reason)
  ambient:start               - Start/restart ambient mode
  ambient:stop                - Stop ambient mode
  ambient:help                - Ambient command reference

EVENTS COMMANDS (events: prefix):
  events:recent            - Get recent events (default 50)
  events:recent:<N>        - Get recent N events
  events:since:<id>        - Get events since event ID
  events:count             - Event count and latest ID
  events:types             - List available event types
  events:subscribe         - Subscribe to all events (streaming)
  events:subscribe:<types> - Subscribe filtered (e.g. status_change,member_change)

CLIENT COMMANDS (client: prefix):
  client:state             - Get TUI state
  client:frame             - Get latest visual debug frame (JSON)
  client:frame-normalized  - Get normalized frame (for diffs)
  client:screen            - Dump visual debug to file
  client:layout            - Get latest layout JSON
  client:margins           - Get layout margins JSON
  client:widgets           - Get info widget summary/placements
  client:render-stats      - Get render timing + order JSON
  client:render-order      - Get render order list
  client:anomalies         - Get latest visual debug anomalies
  client:theme             - Get palette snapshot
  client:mermaid:stats     - Get mermaid render/cache stats
  client:mermaid:memory    - Mermaid memory profile (RSS + cache estimates)
  client:mermaid:memory-bench [n] - Synthetic Mermaid memory benchmark
  client:mermaid:cache     - List mermaid cache entries
  client:mermaid:state     - Get image state (resize modes)
  client:mermaid:test      - Render test diagram
  client:mermaid:scroll    - Run scroll simulation test
  client:mermaid:render <c> - Render arbitrary mermaid
  client:mermaid:evict     - Clear mermaid cache
  client:markdown:stats    - Get markdown render stats
  client:overlay:on/off    - Toggle overlay boxes
  client:input             - Get current input buffer
  client:set_input:<text>  - Set input buffer
  client:keys:<keyspec>    - Inject key events
  client:message:<text>    - Inject and submit message
  client:inject:<role>:<t> - Inject display message (no send)
  client:scroll:<dir>      - Scroll (up/down/top/bottom)
  client:scroll-test[:<j>] - Run offscreen scroll+diagram test
  client:scroll-suite[:<j>] - Run scroll+diagram test suite
  client:wait              - Check if processing
  client:history           - Get display messages
  client:help              - Client command help

TESTER COMMANDS (tester: prefix):
  tester:spawn             - Spawn new tester instance
  tester:list              - List active testers
  tester:<id>:frame        - Get frame from tester
  tester:<id>:message:<t>  - Send message to tester
  tester:<id>:inject:<t>   - Inject display message (no send)
  tester:<id>:state        - Get tester state
  tester:<id>:scroll-test  - Run offscreen scroll+diagram test
  tester:<id>:scroll-suite - Run scroll+diagram test suite
  tester:<id>:stop         - Stop tester

Examples:
  {"type":"debug_command","id":1,"command":"state"}
  {"type":"debug_command","id":2,"command":"client:frame"}
  {"type":"debug_command","id":3,"command":"tester:list"}
  {"type":"debug_command","id":4,"command":"set_provider:openai","session_id":"..."}
  {"type":"debug_command","id":5,"command":"swarm:info:/home/user/project"}"#
        .to_string()
}

/// Generate help text for swarm debug commands
pub(super) fn swarm_debug_help_text() -> String {
    r#"Swarm debug commands (swarm: prefix):

MEMBERS & STRUCTURE:
  swarm                    - List all swarm members (alias for swarm:members)
  swarm:members            - List all swarm members with full details
  swarm:list               - List all swarm IDs with member counts and coordinators
  swarm:info:<swarm_id>    - Full info: members, coordinator, plan, context, conflicts

COORDINATORS & ROLES:
  swarm:coordinators            - List all coordinators (swarm_id -> session_id)
  swarm:coordinator:<id>        - Get coordinator for specific swarm
  swarm:clear_coordinator:<id>  - Admin: forcibly clear coordinator so any session can self-promote
  swarm:roles                   - List all members with their roles

PLANS (server-scoped plan items):
  swarm:plans              - List all swarm plans with item counts
  swarm:plan:<swarm_id>    - Get plan items for specific swarm
  swarm:plan_version:<id>  - Show current plan version for a swarm

PLAN PROPOSALS (pending approval):
  swarm:proposals          - List all pending proposals across swarms
  swarm:proposals:<swarm>  - List proposals for a specific swarm (with items)
  swarm:proposals:<sess>   - Get detailed proposal from a session

SHARED CONTEXT (key-value store):
  swarm:context            - List all shared context entries
  swarm:context:<swarm_id> - List context for specific swarm
  swarm:context:<swarm_id>:<key> - Get specific context value

FILE TOUCHES (conflict detection):
  swarm:touches            - List all file touches (path, session, op, age, timestamp)
  swarm:touches:<path>     - Get touches for specific file
  swarm:touches:swarm:<id> - Get touches filtered by swarm members
  swarm:conflicts          - List files touched by multiple sessions

NOTIFICATIONS:
  swarm:broadcast:<msg>    - Broadcast message to all members of your swarm
  swarm:broadcast:<swarm_id> <msg> - Broadcast to specific swarm
  swarm:notify:<session_id> <msg> - Send direct message to specific session

EXECUTION STATE:
  swarm:session:<id>       - Detailed session state (interrupts, provider, usage)
  swarm:interrupts         - List pending interrupts across all sessions

CHANNELS:
  swarm:channels           - List channel subscriptions per swarm

OPERATIONS (debug-only, bypass tool:communicate):
  swarm:set_context:<sess> <key> <value> - Set shared context as session
  swarm:approve_plan:<coord> <proposer>  - Approve plan proposal (coordinator only)
  swarm:reject_plan:<coord> <proposer> [reason] - Reject plan proposal

UTILITIES:
  swarm:id:<path>          - Compute swarm_id for a path and show provenance

REAL-TIME EVENTS:
  events:recent            - Get recent 50 events
  events:recent:<N>        - Get recent N events
  events:since:<id>        - Get events since event ID (for polling)
  events:count             - Get event count and latest ID
  events:types             - List available event types
  events:subscribe         - Subscribe to all events (streaming, keeps connection open)
  events:subscribe:<types> - Subscribe filtered (e.g. events:subscribe:status_change,member_change)

Examples:
  {"type":"debug_command","id":1,"command":"swarm:list"}
  {"type":"debug_command","id":2,"command":"swarm:info:/home/user/myproject"}
  {"type":"debug_command","id":3,"command":"swarm:plan:/home/user/myproject"}
  {"type":"debug_command","id":4,"command":"swarm:broadcast:Build complete, ready for review"}
  {"type":"debug_command","id":5,"command":"swarm:notify:session_fox_123 Please review PR #42"}"#
        .to_string()
}

pub(super) async fn resolve_debug_session(
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    session_id: &Arc<RwLock<String>>,
    requested: Option<String>,
) -> Result<(String, Arc<Mutex<Agent>>)> {
    let mut target = requested;
    if target.is_none() {
        let current = session_id.read().await.clone();
        if !current.is_empty() {
            target = Some(current);
        }
    }

    let sessions_guard = sessions.read().await;
    if let Some(id) = target {
        let agent = sessions_guard
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Unknown session_id '{}'", id))?;
        return Ok((id, agent));
    }

    if sessions_guard.len() == 1 {
        let (id, agent) = sessions_guard.iter().next().unwrap();
        return Ok((id.clone(), Arc::clone(agent)));
    }

    Err(anyhow::anyhow!(
        "No active session found. Connect a client or provide session_id."
    ))
}

pub(super) fn debug_message_timeout_secs() -> Option<u64> {
    let raw = std::env::var("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let secs = trimmed.parse::<u64>().ok()?;
    if secs == 0 {
        None
    } else {
        Some(secs)
    }
}

pub(super) async fn run_debug_message_with_timeout(
    agent: Arc<Mutex<Agent>>,
    msg: &str,
    timeout_secs: u64,
) -> Result<String> {
    let msg = msg.to_string();
    let mut handle = tokio::spawn(async move {
        let mut agent = agent.lock().await;
        agent.run_once_capture(&msg).await
    });

    tokio::select! {
        join_result = &mut handle => {
            match join_result {
                Ok(result) => result,
                Err(e) => Err(anyhow::anyhow!("debug message task failed: {}", e)),
            }
        }
        _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
            handle.abort();
            Err(anyhow::anyhow!(
                "debug message timed out after {}s",
                timeout_secs
            ))
        }
    }
}

pub(super) async fn execute_debug_command(
    agent: Arc<Mutex<Agent>>,
    command: &str,
    debug_jobs: Arc<RwLock<HashMap<String, DebugJob>>>,
    server_identity: Option<&ServerIdentity>,
) -> Result<String> {
    let trimmed = command.trim();

    if trimmed.starts_with("swarm_message_async:") {
        let msg = trimmed
            .strip_prefix("swarm_message_async:")
            .unwrap_or("")
            .trim();
        if msg.is_empty() {
            return Err(anyhow::anyhow!("swarm_message_async: requires content"));
        }
        let session = {
            let agent = agent.lock().await;
            agent.session_id().to_string()
        };
        let job_id = id::new_id("job");
        {
            let mut jobs = debug_jobs.write().await;
            jobs.insert(
                job_id.clone(),
                DebugJob {
                    id: job_id.clone(),
                    status: DebugJobStatus::Queued,
                    command: format!("swarm_message:{}", msg),
                    session_id: Some(session),
                    created_at: Instant::now(),
                    started_at: None,
                    finished_at: None,
                    output: None,
                    error: None,
                },
            );
        }

        let jobs = Arc::clone(&debug_jobs);
        let agent = Arc::clone(&agent);
        let msg = msg.to_string();
        let job_id_inner = job_id.clone();
        tokio::spawn(async move {
            {
                let mut jobs = jobs.write().await;
                if let Some(job) = jobs.get_mut(&job_id_inner) {
                    job.status = DebugJobStatus::Running;
                    job.started_at = Some(Instant::now());
                }
            }

            let result = super::run_swarm_message(agent.clone(), &msg).await;
            let partial_output = if result.is_err() {
                let agent = agent.lock().await;
                agent.last_assistant_text()
            } else {
                None
            };

            let mut jobs = jobs.write().await;
            if let Some(job) = jobs.get_mut(&job_id_inner) {
                job.finished_at = Some(Instant::now());
                match result {
                    Ok(output) => {
                        job.status = DebugJobStatus::Completed;
                        job.output = Some(output);
                    }
                    Err(e) => {
                        job.status = DebugJobStatus::Failed;
                        job.error = Some(e.to_string());
                        if let Some(output) = partial_output {
                            job.output = Some(output);
                        }
                    }
                }
            }
        });

        return Ok(serde_json::json!({ "job_id": job_id }).to_string());
    }

    if trimmed.starts_with("message_async:") {
        let msg = trimmed.strip_prefix("message_async:").unwrap_or("").trim();
        if msg.is_empty() {
            return Err(anyhow::anyhow!("message_async: requires content"));
        }
        let session = {
            let agent = agent.lock().await;
            agent.session_id().to_string()
        };
        let job_id = id::new_id("job");
        {
            let mut jobs = debug_jobs.write().await;
            jobs.insert(
                job_id.clone(),
                DebugJob {
                    id: job_id.clone(),
                    status: DebugJobStatus::Queued,
                    command: format!("message:{}", msg),
                    session_id: Some(session),
                    created_at: Instant::now(),
                    started_at: None,
                    finished_at: None,
                    output: None,
                    error: None,
                },
            );
        }

        let jobs = Arc::clone(&debug_jobs);
        let agent = Arc::clone(&agent);
        let msg = msg.to_string();
        let job_id_inner = job_id.clone();
        tokio::spawn(async move {
            {
                let mut jobs = jobs.write().await;
                if let Some(job) = jobs.get_mut(&job_id_inner) {
                    job.status = DebugJobStatus::Running;
                    job.started_at = Some(Instant::now());
                }
            }

            let result = {
                let mut agent = agent.lock().await;
                agent.run_once_capture(&msg).await
            };
            let partial_output = if result.is_err() {
                let agent = agent.lock().await;
                agent.last_assistant_text()
            } else {
                None
            };

            let mut jobs = jobs.write().await;
            if let Some(job) = jobs.get_mut(&job_id_inner) {
                job.finished_at = Some(Instant::now());
                match result {
                    Ok(output) => {
                        job.status = DebugJobStatus::Completed;
                        job.output = Some(output);
                    }
                    Err(e) => {
                        job.status = DebugJobStatus::Failed;
                        job.error = Some(e.to_string());
                        if let Some(output) = partial_output {
                            job.output = Some(output);
                        }
                    }
                }
            }
        });

        return Ok(serde_json::json!({ "job_id": job_id }).to_string());
    }

    if trimmed.starts_with("swarm_message:") {
        let msg = trimmed.strip_prefix("swarm_message:").unwrap_or("").trim();
        if msg.is_empty() {
            return Err(anyhow::anyhow!("swarm_message: requires content"));
        }

        let final_text = super::run_swarm_message(agent.clone(), msg).await?;
        return Ok(final_text);
    }

    if trimmed.starts_with("message:") {
        let msg = trimmed.strip_prefix("message:").unwrap_or("").trim();
        if let Some(timeout_secs) = debug_message_timeout_secs() {
            return run_debug_message_with_timeout(agent, msg, timeout_secs).await;
        }
        let mut agent = agent.lock().await;
        let output = agent.run_once_capture(msg).await?;
        return Ok(output);
    }

    // queue_interrupt:<content> - Queue soft interrupt (for testing)
    // This adds a message to the agent's soft interrupt queue without blocking
    if trimmed.starts_with("queue_interrupt:") {
        let content = trimmed
            .strip_prefix("queue_interrupt:")
            .unwrap_or("")
            .trim();
        if content.is_empty() {
            return Err(anyhow::anyhow!("queue_interrupt: requires content"));
        }
        let agent = agent.lock().await;
        agent.queue_soft_interrupt(content.to_string(), false);
        return Ok("queued".to_string());
    }

    // queue_interrupt_urgent:<content> - Queue urgent soft interrupt (can skip tools)
    if trimmed.starts_with("queue_interrupt_urgent:") {
        let content = trimmed
            .strip_prefix("queue_interrupt_urgent:")
            .unwrap_or("")
            .trim();
        if content.is_empty() {
            return Err(anyhow::anyhow!("queue_interrupt_urgent: requires content"));
        }
        let agent = agent.lock().await;
        agent.queue_soft_interrupt(content.to_string(), true);
        return Ok("queued (urgent)".to_string());
    }

    if trimmed.starts_with("tool:") {
        let raw = trimmed.strip_prefix("tool:").unwrap_or("").trim();
        if raw.is_empty() {
            return Err(anyhow::anyhow!("tool: requires a tool name"));
        }
        let mut parts = raw.splitn(2, |c: char| c.is_whitespace());
        let name = parts.next().unwrap_or("").trim();
        let input_raw = parts.next().unwrap_or("").trim();
        let input = if input_raw.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str::<serde_json::Value>(input_raw)?
        };
        let agent = agent.lock().await;
        let output = agent.execute_tool(name, input).await?;
        let payload = serde_json::json!({
            "output": output.output,
            "title": output.title,
            "metadata": output.metadata,
        });
        return Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()));
    }

    if trimmed == "history" {
        let agent = agent.lock().await;
        let history = agent.get_history();
        return Ok(serde_json::to_string_pretty(&history).unwrap_or_else(|_| "[]".to_string()));
    }

    if trimmed == "tools" {
        let agent = agent.lock().await;
        let tools = agent.tool_names().await;
        return Ok(serde_json::to_string_pretty(&tools).unwrap_or_else(|_| "[]".to_string()));
    }

    if trimmed == "tools:full" {
        let agent = agent.lock().await;
        let definitions = agent.tool_definitions_for_debug().await;
        return Ok(serde_json::to_string_pretty(&definitions).unwrap_or_else(|_| "[]".to_string()));
    }

    if trimmed == "mcp" || trimmed == "mcp:servers" {
        let agent = agent.lock().await;
        let tool_names = agent.tool_names().await;
        let mut connected: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for name in tool_names {
            if let Some(rest) = name.strip_prefix("mcp__") {
                let mut parts = rest.splitn(2, "__");
                if let (Some(server), Some(tool)) = (parts.next(), parts.next()) {
                    connected
                        .entry(server.to_string())
                        .or_default()
                        .push(tool.to_string());
                }
            }
        }
        for tools in connected.values_mut() {
            tools.sort();
        }
        let connected_servers: Vec<String> = connected.keys().cloned().collect();

        // Load merged MCP config (handles ~/.jcode/mcp.json + project-local configs)
        let config = McpConfig::load();
        let config_path = if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let path = jcode_dir.join("mcp.json");
            if path.exists() {
                Some(path.to_string_lossy().to_string())
            } else {
                None
            }
        } else {
            None
        };
        let mut configured_servers: Vec<String> = config.servers.keys().cloned().collect();
        configured_servers.sort();

        return Ok(serde_json::to_string_pretty(&serde_json::json!({
            "config_path": config_path,
            "configured_servers": configured_servers,
            "connected_servers": connected_servers,
            "connected_tools": connected,
        }))
        .unwrap_or_else(|_| "{}".to_string()));
    }

    // mcp:tools - list all registered MCP tools
    if trimmed == "mcp:tools" {
        let agent = agent.lock().await;
        let tool_names = agent.tool_names().await;
        let mcp_tools: Vec<&str> = tool_names
            .iter()
            .filter(|n| n.starts_with("mcp__"))
            .map(|n| n.as_str())
            .collect();
        return Ok(serde_json::to_string_pretty(&mcp_tools).unwrap_or_else(|_| "[]".to_string()));
    }

    // mcp:connect:<server> <json> - connect to an MCP server
    if let Some(rest) = trimmed.strip_prefix("mcp:connect:") {
        let (server_name, config_json) = match rest.find(' ') {
            Some(idx) => (rest[..idx].trim(), &rest[idx + 1..]),
            None => {
                return Err(anyhow::anyhow!(
                    "Usage: mcp:connect:<server> {{\"command\":\"...\",\"args\":[...]}}"
                ))
            }
        };
        let mut input: serde_json::Value = serde_json::from_str(config_json)
            .map_err(|e| anyhow::anyhow!("Invalid JSON: {}", e))?;
        input["action"] = serde_json::json!("connect");
        input["server"] = serde_json::json!(server_name);
        let agent = agent.lock().await;
        let result = agent.execute_tool("mcp", input).await?;
        return Ok(result.output);
    }

    // mcp:disconnect:<server> - disconnect from an MCP server
    if let Some(server_name) = trimmed.strip_prefix("mcp:disconnect:") {
        let server_name = server_name.trim();
        let input = serde_json::json!({"action": "disconnect", "server": server_name});
        let agent = agent.lock().await;
        let result = agent.execute_tool("mcp", input).await?;
        return Ok(result.output);
    }

    // mcp:reload - reload MCP config and reconnect
    if trimmed == "mcp:reload" {
        let input = serde_json::json!({"action": "reload"});
        let mut agent = agent.lock().await;
        let result = agent.execute_tool("mcp", input).await?;
        // Unlock tool list so next request picks up new MCP tools
        agent.unlock_tools();
        return Ok(result.output);
    }

    // mcp:call:<server>:<tool> <json> - call an MCP tool directly
    if let Some(rest) = trimmed.strip_prefix("mcp:call:") {
        let (tool_path, args_json) = match rest.find(' ') {
            Some(idx) => (rest[..idx].trim(), rest[idx + 1..].trim()),
            None => (rest.trim(), "{}"),
        };
        let mut parts = tool_path.splitn(2, ':');
        let server = parts.next().unwrap_or("");
        let tool = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("Usage: mcp:call:<server>:<tool> <json>"))?;
        let tool_name = format!("mcp__{}__{}", server, tool);
        let input: serde_json::Value =
            serde_json::from_str(args_json).map_err(|e| anyhow::anyhow!("Invalid JSON: {}", e))?;
        let agent = agent.lock().await;
        let result = agent.execute_tool(&tool_name, input).await?;
        return Ok(result.output);
    }

    if trimmed == "cancel" {
        // Queue an urgent interrupt to cancel in-flight generation
        let agent = agent.lock().await;
        agent.queue_soft_interrupt(
            "[CANCELLED] Generation cancelled via debug socket".to_string(),
            true,
        );
        return Ok(serde_json::json!({
            "status": "cancel_queued",
            "message": "Urgent interrupt queued - will cancel at next tool boundary"
        })
        .to_string());
    }

    if trimmed == "clear" || trimmed == "clear_history" {
        // Clear conversation history
        let mut agent = agent.lock().await;
        agent.clear();
        return Ok(serde_json::json!({
            "status": "cleared",
            "message": "Conversation history cleared"
        })
        .to_string());
    }

    if trimmed == "agent:info" {
        // Get comprehensive agent internal state
        let agent = agent.lock().await;
        let info = agent.debug_info();
        return Ok(serde_json::to_string_pretty(&info).unwrap_or_else(|_| "{}".to_string()));
    }

    if trimmed == "last_response" {
        let agent = agent.lock().await;
        return Ok(agent
            .last_assistant_text()
            .unwrap_or_else(|| "last_response: none".to_string()));
    }

    if trimmed == "state" {
        let agent = agent.lock().await;
        let mut payload = serde_json::json!({
            "session_id": agent.session_id(),
            "messages": agent.message_count(),
            "is_canary": agent.is_canary(),
            "provider": agent.provider_name(),
            "model": agent.provider_model(),
            "upstream_provider": agent.last_upstream_provider(),
        });
        if let Some(identity) = server_identity {
            payload["server_name"] = serde_json::json!(identity.name);
            payload["server_icon"] = serde_json::json!(identity.icon);
            payload["server_version"] = serde_json::json!(identity.version);
        }
        return Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()));
    }

    if trimmed == "usage" {
        let agent = agent.lock().await;
        let usage = agent.last_usage();
        return Ok(serde_json::to_string_pretty(&usage).unwrap_or_else(|_| "{}".to_string()));
    }

    if trimmed == "help" {
        return Ok(
            "debug commands: state, usage, history, tools, tools:full, mcp:servers, mcp:tools, mcp:connect:<server> <json>, mcp:disconnect:<server>, mcp:reload, mcp:call:<server>:<tool> <json>, last_response, message:<text>, message_async:<text>, swarm_message:<text>, swarm_message_async:<text>, tool:<name> <json>, queue_interrupt:<content>, queue_interrupt_urgent:<content>, jobs, job_status:<id>, job_wait:<id>, sessions, create_session, create_session:<path>, set_model:<model>, set_provider:<name>, trigger_extraction, available_models, reload, help".to_string()
        );
    }

    // set_model:<model> - Switch to a different model (may change provider)
    if trimmed.starts_with("set_model:") {
        let model = trimmed.strip_prefix("set_model:").unwrap_or("").trim();
        if model.is_empty() {
            return Err(anyhow::anyhow!("set_model: requires a model name"));
        }
        let mut agent = agent.lock().await;
        agent.set_model(model)?;
        let payload = serde_json::json!({
            "model": agent.provider_model(),
            "provider": agent.provider_name(),
        });
        return Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()));
    }

    // set_provider:<name> - Switch to a provider with default model
    if trimmed.starts_with("set_provider:") {
        let provider = trimmed
            .strip_prefix("set_provider:")
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let claude_usage = crate::usage::get_sync();
        let claude_usage_exhausted =
            claude_usage.five_hour >= 0.99 && claude_usage.seven_day >= 0.99;
        let default_model = match provider.as_str() {
            "claude" | "anthropic" => {
                if claude_usage_exhausted {
                    "claude-sonnet-4-6"
                } else {
                    "claude-opus-4-6"
                }
            }
            "openai" | "codex" => "gpt-5.4",
            "openrouter" => "anthropic/claude-sonnet-4",
            "cursor" => "gpt-5",
            "copilot" => "copilot:claude-sonnet-4",
            "antigravity" => "default",
            _ => {
                return Err(anyhow::anyhow!(
                    "Unknown provider '{}'. Use: claude, openai, openrouter, cursor, copilot, antigravity",
                    provider
                ))
            }
        };
        let mut agent = agent.lock().await;
        agent.set_model(default_model)?;
        let payload = serde_json::json!({
            "model": agent.provider_model(),
            "provider": agent.provider_name(),
        });
        return Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()));
    }

    // trigger_extraction - Force end-of-session memory extraction
    if trimmed == "trigger_extraction" {
        let agent = agent.lock().await;
        let count = agent.extract_session_memories().await;
        let payload = serde_json::json!({
            "extracted": count,
            "message_count": agent.message_count(),
        });
        return Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()));
    }

    // available_models - List all available models
    if trimmed == "available_models" {
        let agent = agent.lock().await;
        let models = agent.available_models_display();
        return Ok(serde_json::to_string_pretty(&models).unwrap_or_else(|_| "[]".to_string()));
    }

    // reload - Trigger server reload with current binary (direct signal, bypasses tool system)
    if trimmed == "reload" {
        // Get repo directory and check for binary
        let repo_dir = crate::build::get_repo_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find jcode repository directory"))?;

        let target_binary = crate::build::find_dev_binary(&repo_dir)
            .unwrap_or_else(|| build::release_binary_path(&repo_dir));
        if !target_binary.exists() {
            return Err(anyhow::anyhow!(format!(
                "No binary found at {}. Run 'cargo build --release' first.",
                target_binary.display()
            )));
        }

        let hash = crate::build::current_git_hash(&repo_dir)?;

        // Install version and update canary symlink
        crate::build::install_version(&repo_dir, &hash)?;
        crate::build::update_canary_symlink(&hash)?;

        // Update manifest
        let mut manifest = crate::build::BuildManifest::load()?;
        manifest.canary = Some(hash.clone());
        manifest.canary_status = Some(crate::build::CanaryStatus::Testing);
        manifest.save()?;

        // Write reload info for post-restart display
        let jcode_dir = crate::storage::jcode_dir()?;
        let info_path = jcode_dir.join("reload-info");
        std::fs::write(&info_path, format!("reload:{}", hash))?;

        // Signal reload via in-process channel
        super::send_reload_signal(hash.clone(), None);

        return Ok(format!(
            "Reload signal sent for build {}. Server will restart.",
            hash
        ));
    }

    Err(anyhow::anyhow!("Unknown debug command '{}'", trimmed))
}

pub(super) async fn handle_debug_client(
    stream: Stream,
    sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    is_processing: Arc<RwLock<bool>>,
    session_id: Arc<RwLock<String>>,
    provider: Arc<dyn Provider>,
    client_connections: Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
    file_touches: Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    channel_subscriptions: Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    client_debug_state: Arc<RwLock<ClientDebugState>>,
    client_debug_response_tx: broadcast::Sender<(u64, String)>,
    debug_jobs: Arc<RwLock<HashMap<String, DebugJob>>>,
    event_history: Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
    server_identity: ServerIdentity,
    server_start_time: std::time::Instant,
    ambient_runner: Option<AmbientRunnerHandle>,
    mcp_pool: Option<Arc<crate::mcp::SharedMcpPool>>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }

        let request = match decode_request(&line) {
            Ok(r) => r,
            Err(e) => {
                let event = ServerEvent::Error {
                    id: 0,
                    message: format!("Invalid request: {}", e),
                    retry_after_secs: None,
                };
                let json = encode_event(&event);
                writer.write_all(json.as_bytes()).await?;
                continue;
            }
        };

        match request {
            Request::Ping { id } => {
                let event = ServerEvent::Pong { id };
                let json = encode_event(&event);
                writer.write_all(json.as_bytes()).await?;
            }

            Request::GetState { id } => {
                let current_session_id = session_id.read().await.clone();
                let sessions = sessions.read().await;
                let message_count = sessions.len();

                let event = ServerEvent::State {
                    id,
                    session_id: current_session_id,
                    message_count,
                    is_processing: *is_processing.read().await,
                };
                let json = encode_event(&event);
                writer.write_all(json.as_bytes()).await?;
            }

            Request::DebugCommand {
                id,
                command,
                session_id: requested_session,
            } => {
                if !debug_control_allowed() {
                    let event = ServerEvent::Error {
                        id,
                        message: "Debug control is disabled. Set JCODE_DEBUG_CONTROL=1 or run in self-dev mode.".to_string(),
                        retry_after_secs: None,
                    };
                    let json = encode_event(&event);
                    writer.write_all(json.as_bytes()).await?;
                    continue;
                }

                // Parse namespaced command
                let (namespace, cmd) = parse_namespaced_command(&command);

                let result = match namespace {
                    "client" => {
                        // Forward to TUI client
                        let mut response_rx = client_debug_response_tx.subscribe();
                        let mut attempts = 0usize;

                        loop {
                            let (client_id, tx) = {
                                let mut debug_state = client_debug_state.write().await;
                                match debug_state.active_sender() {
                                    Some(active) => active,
                                    None => {
                                        break Err(anyhow::anyhow!("No TUI client connected"));
                                    }
                                }
                            };

                            if tx.send((id, cmd.to_string())).is_ok() {
                                // Wait for response with timeout
                                let timeout = tokio::time::Duration::from_secs(30);
                                match tokio::time::timeout(timeout, async {
                                    loop {
                                        if let Ok((resp_id, output)) = response_rx.recv().await {
                                            if resp_id == id {
                                                return Ok(output);
                                            }
                                        }
                                    }
                                })
                                .await
                                {
                                    Ok(result) => break result,
                                    Err(_) => {
                                        break Err(anyhow::anyhow!(
                                            "Timeout waiting for client response"
                                        ));
                                    }
                                }
                            } else {
                                let mut debug_state = client_debug_state.write().await;
                                debug_state.unregister(&client_id);
                                attempts += 1;
                                if debug_state.clients.is_empty() || attempts > 8 {
                                    break Err(anyhow::anyhow!("No TUI client connected"));
                                }
                            }
                        }
                    }
                    "tester" => {
                        // Handle tester commands
                        execute_tester_command(cmd).await
                    }
                    _ => {
                        // Server commands (default)
                        if cmd == "jobs" {
                            let jobs_guard = debug_jobs.read().await;
                            let payload: Vec<Value> = jobs_guard
                                .values()
                                .map(|job| job.summary_payload())
                                .collect();
                            Ok(serde_json::to_string_pretty(&payload)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("job_status:") {
                            let job_id = cmd.strip_prefix("job_status:").unwrap_or("").trim();
                            if job_id.is_empty() {
                                Err(anyhow::anyhow!("job_status: requires a job id"))
                            } else {
                                let jobs_guard = debug_jobs.read().await;
                                if let Some(job) = jobs_guard.get(job_id) {
                                    Ok(serde_json::to_string_pretty(&job.status_payload())
                                        .unwrap_or_else(|_| "{}".to_string()))
                                } else {
                                    Err(anyhow::anyhow!("Unknown job id '{}'", job_id))
                                }
                            }
                        } else if cmd.starts_with("job_cancel:") {
                            // Cancel a running job
                            let job_id = cmd.strip_prefix("job_cancel:").unwrap_or("").trim();
                            if job_id.is_empty() {
                                Err(anyhow::anyhow!("job_cancel: requires a job id"))
                            } else {
                                let mut jobs_guard = debug_jobs.write().await;
                                if let Some(job) = jobs_guard.get_mut(job_id) {
                                    if matches!(
                                        job.status,
                                        DebugJobStatus::Running | DebugJobStatus::Queued
                                    ) {
                                        job.status = DebugJobStatus::Failed;
                                        job.output = Some("[CANCELLED]".to_string());
                                        Ok(serde_json::json!({
                                            "status": "cancelled",
                                            "job_id": job_id,
                                        })
                                        .to_string())
                                    } else {
                                        Err(anyhow::anyhow!("Job '{}' is not running", job_id))
                                    }
                                } else {
                                    Err(anyhow::anyhow!("Unknown job id '{}'", job_id))
                                }
                            }
                        } else if cmd == "jobs:purge" {
                            // Remove all completed/failed jobs
                            let mut jobs_guard = debug_jobs.write().await;
                            let before = jobs_guard.len();
                            jobs_guard.retain(|_, job| {
                                matches!(
                                    job.status,
                                    DebugJobStatus::Running | DebugJobStatus::Queued
                                )
                            });
                            let removed = before - jobs_guard.len();
                            Ok(serde_json::json!({
                                "status": "purged",
                                "removed": removed,
                                "remaining": jobs_guard.len(),
                            })
                            .to_string())
                        } else if cmd.starts_with("jobs:session:") {
                            // List jobs for a specific session
                            let sess_id = cmd.strip_prefix("jobs:session:").unwrap_or("").trim();
                            let jobs_guard = debug_jobs.read().await;
                            let payload: Vec<Value> = jobs_guard
                                .values()
                                .filter(|job| job.session_id.as_deref() == Some(sess_id))
                                .map(|job| job.summary_payload())
                                .collect();
                            Ok(serde_json::to_string_pretty(&payload)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("job_wait:") {
                            let job_id = cmd.strip_prefix("job_wait:").unwrap_or("").trim();
                            if job_id.is_empty() {
                                Err(anyhow::anyhow!("job_wait: requires a job id"))
                            } else {
                                let timeout = Duration::from_secs(900);
                                let start = Instant::now();
                                loop {
                                    {
                                        let jobs_guard = debug_jobs.read().await;
                                        if let Some(job) = jobs_guard.get(job_id) {
                                            if matches!(
                                                job.status,
                                                DebugJobStatus::Completed | DebugJobStatus::Failed
                                            ) {
                                                break Ok(serde_json::to_string_pretty(
                                                    &job.status_payload(),
                                                )
                                                .unwrap_or_else(|_| "{}".to_string()));
                                            }
                                        } else {
                                            break Err(anyhow::anyhow!(
                                                "Unknown job id '{}'",
                                                job_id
                                            ));
                                        }
                                    }
                                    if start.elapsed() > timeout {
                                        break Err(anyhow::anyhow!(
                                            "Timeout waiting for job '{}'",
                                            job_id
                                        ));
                                    }
                                    tokio::time::sleep(Duration::from_millis(500)).await;
                                }
                            }
                        } else if cmd == "create_session" || cmd.starts_with("create_session:") {
                            create_headless_session(
                                &sessions,
                                &session_id,
                                &provider,
                                cmd,
                                &swarm_members,
                                &swarms_by_id,
                                &swarm_coordinators,
                                &swarm_plans,
                                None,
                                mcp_pool.clone(),
                            )
                            .await
                        } else if cmd.starts_with("destroy_session:") {
                            let target_id =
                                cmd.strip_prefix("destroy_session:").unwrap_or("").trim();
                            if target_id.is_empty() {
                                Err(anyhow::anyhow!("destroy_session: requires a session_id"))
                            } else {
                                // Remove session first, extract transcript for final memory extraction
                                let removed_agent = {
                                    let mut sessions_guard = sessions.write().await;
                                    sessions_guard.remove(target_id)
                                };
                                if let Some(ref agent_arc) = removed_agent {
                                    let agent = agent_arc.lock().await;
                                    let memory_enabled = agent.memory_enabled();
                                    let transcript = if memory_enabled {
                                        Some(agent.build_transcript_for_extraction())
                                    } else {
                                        None
                                    };
                                    let sid = target_id.to_string();
                                    drop(agent);
                                    if let Some(transcript) = transcript {
                                        crate::memory_agent::trigger_final_extraction(
                                            transcript, sid,
                                        );
                                    }
                                }
                                let removed = removed_agent.is_some();
                                if removed {
                                    // Clean up swarm membership
                                    let (swarm_id, friendly_name) = {
                                        let mut members = swarm_members.write().await;
                                        let info = members
                                            .remove(target_id)
                                            .map(|m| (m.swarm_id, m.friendly_name));
                                        info.map(|(sid, name)| (sid, name)).unwrap_or((None, None))
                                    };
                                    if let Some(ref id) = swarm_id {
                                        // Fire status change event before removing from swarm
                                        record_swarm_event(
                                            &event_history,
                                            &event_counter,
                                            &swarm_event_tx,
                                            target_id.to_string(),
                                            friendly_name.clone(),
                                            Some(id.clone()),
                                            SwarmEventType::StatusChange {
                                                old_status: "ready".to_string(),
                                                new_status: "stopped".to_string(),
                                            },
                                        )
                                        .await;
                                        record_swarm_event(
                                            &event_history,
                                            &event_counter,
                                            &swarm_event_tx,
                                            target_id.to_string(),
                                            friendly_name,
                                            Some(id.clone()),
                                            SwarmEventType::MemberChange {
                                                action: "left".to_string(),
                                            },
                                        )
                                        .await;
                                        // Remove from swarm (scoped to drop write guard)
                                        {
                                            let mut swarms = swarms_by_id.write().await;
                                            if let Some(swarm) = swarms.get_mut(id) {
                                                swarm.remove(target_id);
                                                if swarm.is_empty() {
                                                    swarms.remove(id);
                                                }
                                            }
                                        }
                                        // Handle coordinator change if needed
                                        let was_coordinator = {
                                            let coordinators = swarm_coordinators.read().await;
                                            coordinators
                                                .get(id)
                                                .map(|c| c == target_id)
                                                .unwrap_or(false)
                                        };
                                        if was_coordinator {
                                            let new_coordinator = {
                                                let swarms = swarms_by_id.read().await;
                                                swarms.get(id).and_then(|s| s.iter().min().cloned())
                                            };
                                            let mut coordinators = swarm_coordinators.write().await;
                                            coordinators.remove(id);
                                            if let Some(new_id) = new_coordinator {
                                                coordinators.insert(id.clone(), new_id);
                                            }
                                        }
                                        broadcast_swarm_status(id, &swarm_members, &swarms_by_id)
                                            .await;
                                    }
                                    Ok(format!("Session '{}' destroyed", target_id))
                                } else {
                                    Err(anyhow::anyhow!("Unknown session_id '{}'", target_id))
                                }
                            }
                        } else if cmd == "sessions" {
                            // Return full session metadata
                            let sessions_guard = sessions.read().await;
                            let members = swarm_members.read().await;
                            let connections = client_connections.read().await;
                            let connected_sessions: std::collections::HashSet<String> =
                                connections.values().map(|c| c.session_id.clone()).collect();
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (sid, agent_arc) in sessions_guard.iter() {
                                if !connected_sessions.contains(sid) {
                                    continue;
                                }
                                let member_info = members.get(sid);
                                let member_status = member_info.map(|m| m.status.as_str());
                                let (provider, model, is_processing, working_dir_str, token_usage): (
                                    Option<String>,
                                    Option<String>,
                                    bool,
                                    Option<String>,
                                    Option<serde_json::Value>,
                                ) = if let Ok(agent) = agent_arc.try_lock() {
                                    let usage = agent.last_usage();
                                    (
                                        Some(agent.provider_name()),
                                        Some(agent.provider_model()),
                                        member_status == Some("running"),
                                        agent.working_dir().map(|p| p.to_string()),
                                        Some(serde_json::json!({
                                            "input": usage.input_tokens,
                                            "output": usage.output_tokens,
                                            "cache_read": usage.cache_read_input_tokens,
                                            "cache_write": usage.cache_creation_input_tokens,
                                        })),
                                    )
                                } else {
                                    (None, None, member_status == Some("running"), None, None)
                                };
                                let final_working_dir: Option<String> =
                                    working_dir_str.or_else(|| {
                                        member_info.and_then(|m| {
                                            m.working_dir
                                                .as_ref()
                                                .map(|p| p.to_string_lossy().to_string())
                                        })
                                    });
                                out.push(serde_json::json!({
                                    "session_id": sid,
                                    "friendly_name": member_info.and_then(|m| m.friendly_name.clone()),
                                    "provider": provider,
                                    "model": model,
                                    "is_processing": is_processing,
                                    "working_dir": final_working_dir,
                                    "swarm_id": member_info.and_then(|m| m.swarm_id.clone()),
                                    "status": member_info.map(|m| m.status.clone()),
                                    "detail": member_info.and_then(|m| m.detail.clone()),
                                    "token_usage": token_usage,
                                    "server_name": server_identity.name,
                                    "server_icon": server_identity.icon,
                                }));
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd == "swarm" || cmd == "swarm_status" || cmd == "swarm:members"
                        {
                            // List all swarm members with full details
                            let members = swarm_members.read().await;
                            let sessions_guard = sessions.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for member in members.values() {
                                // Get provider/model from the agent if session exists
                                let (provider, model) = if let Some(agent_arc) =
                                    sessions_guard.get(&member.session_id)
                                {
                                    if let Ok(agent) = agent_arc.try_lock() {
                                        (Some(agent.provider_name()), Some(agent.provider_model()))
                                    } else {
                                        (None, None)
                                    }
                                } else {
                                    (None, None)
                                };
                                out.push(serde_json::json!({
                                    "session_id": member.session_id,
                                    "friendly_name": member.friendly_name,
                                    "swarm_id": member.swarm_id,
                                    "working_dir": member.working_dir,
                                    "status": member.status,
                                    "detail": member.detail,
                                    "joined_secs_ago": member.joined_at.elapsed().as_secs(),
                                    "status_changed_secs_ago": member.last_status_change.elapsed().as_secs(),
                                    "provider": provider,
                                    "model": model,
                                    "server_name": server_identity.name,
                                    "server_icon": server_identity.icon,
                                }));
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd == "swarm:list" {
                            // List all swarm IDs with member counts
                            let swarms = swarms_by_id.read().await;
                            let coordinators = swarm_coordinators.read().await;
                            let members = swarm_members.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (swarm_id, session_ids) in swarms.iter() {
                                let coordinator = coordinators.get(swarm_id);
                                let coordinator_name = coordinator.and_then(|cid| {
                                    members.get(cid).and_then(|m| m.friendly_name.clone())
                                });
                                out.push(serde_json::json!({
                                    "swarm_id": swarm_id,
                                    "member_count": session_ids.len(),
                                    "members": session_ids.iter().collect::<Vec<_>>(),
                                    "coordinator": coordinator,
                                    "coordinator_name": coordinator_name,
                                }));
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd == "swarm:coordinators" {
                            // List all coordinators
                            let coordinators = swarm_coordinators.read().await;
                            let members = swarm_members.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (swarm_id, session_id) in coordinators.iter() {
                                let name = members
                                    .get(session_id)
                                    .and_then(|m| m.friendly_name.clone());
                                out.push(serde_json::json!({
                                    "swarm_id": swarm_id,
                                    "coordinator_session": session_id,
                                    "coordinator_name": name,
                                }));
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("swarm:coordinator:") {
                            // Get coordinator for specific swarm
                            let swarm_id =
                                cmd.strip_prefix("swarm:coordinator:").unwrap_or("").trim();
                            let coordinators = swarm_coordinators.read().await;
                            let members = swarm_members.read().await;
                            if let Some(session_id) = coordinators.get(swarm_id) {
                                let name = members
                                    .get(session_id)
                                    .and_then(|m| m.friendly_name.clone());
                                Ok(serde_json::json!({
                                    "swarm_id": swarm_id,
                                    "coordinator_session": session_id,
                                    "coordinator_name": name,
                                })
                                .to_string())
                            } else {
                                Err(anyhow::anyhow!("No coordinator for swarm '{}'", swarm_id))
                            }
                        } else if cmd.starts_with("swarm:clear_coordinator:") {
                            // Admin: forcibly clear the coordinator for a swarm so a new one can be elected.
                            let swarm_id = cmd
                                .strip_prefix("swarm:clear_coordinator:")
                                .unwrap_or("")
                                .trim();
                            let mut coordinators = swarm_coordinators.write().await;
                            if coordinators.remove(swarm_id).is_some() {
                                // Demote the old coordinator member to "agent" role
                                let mut members = swarm_members.write().await;
                                for m in members.values_mut() {
                                    if m.swarm_id.as_deref() == Some(swarm_id)
                                        && m.role == "coordinator"
                                    {
                                        m.role = "agent".to_string();
                                    }
                                }
                                Ok(format!(
                                    "Coordinator cleared for swarm '{}'. Any session can now self-promote.",
                                    swarm_id
                                ))
                            } else {
                                Err(anyhow::anyhow!(
                                    "No coordinator set for swarm '{}'",
                                    swarm_id
                                ))
                            }
                        } else if cmd == "swarm:roles" {
                            // List all members with their roles
                            let members = swarm_members.read().await;
                            let coordinators = swarm_coordinators.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (sid, m) in members.iter() {
                                let is_coordinator = m
                                    .swarm_id
                                    .as_ref()
                                    .map(|swid| {
                                        coordinators.get(swid).map(|c| c == sid).unwrap_or(false)
                                    })
                                    .unwrap_or(false);
                                out.push(serde_json::json!({
                                    "session_id": sid,
                                    "friendly_name": m.friendly_name,
                                    "role": m.role,
                                    "swarm_id": m.swarm_id,
                                    "status": m.status,
                                    "is_coordinator": is_coordinator,
                                }));
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd == "swarm:channels" {
                            // List channel subscriptions per swarm
                            let subs = channel_subscriptions.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (swarm_id, channels) in subs.iter() {
                                let mut channel_data: Vec<serde_json::Value> = Vec::new();
                                for (channel, session_ids) in channels.iter() {
                                    channel_data.push(serde_json::json!({
                                        "channel": channel,
                                        "subscribers": session_ids.iter().collect::<Vec<_>>(),
                                        "count": session_ids.len(),
                                    }));
                                }
                                out.push(serde_json::json!({
                                    "swarm_id": swarm_id,
                                    "channels": channel_data,
                                }));
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("swarm:plan_version:") {
                            // Get plan version for a specific swarm
                            let swarm_id =
                                cmd.strip_prefix("swarm:plan_version:").unwrap_or("").trim();
                            let plans = swarm_plans.read().await;
                            if let Some(vp) = plans.get(swarm_id) {
                                Ok(serde_json::json!({
                                    "swarm_id": swarm_id,
                                    "version": vp.version,
                                    "item_count": vp.items.len(),
                                })
                                .to_string())
                            } else {
                                Ok(serde_json::json!({
                                    "swarm_id": swarm_id,
                                    "version": 0,
                                    "item_count": 0,
                                })
                                .to_string())
                            }
                        } else if cmd == "swarm:plans" {
                            // List all swarm plans
                            let plans = swarm_plans.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (swarm_id, vp) in plans.iter() {
                                out.push(serde_json::json!({
                                    "swarm_id": swarm_id,
                                    "item_count": vp.items.len(),
                                    "version": vp.version,
                                    "participants": vp.participants,
                                    "items": vp.items,
                                }));
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("swarm:plan:") {
                            // Get plan for specific swarm
                            let swarm_id = cmd.strip_prefix("swarm:plan:").unwrap_or("").trim();
                            let plans = swarm_plans.read().await;
                            if let Some(vp) = plans.get(swarm_id) {
                                Ok(serde_json::json!({
                                    "version": vp.version,
                                    "participants": vp.participants,
                                    "items": vp.items,
                                })
                                .to_string())
                            } else {
                                Ok("[]".to_string())
                            }
                        } else if cmd == "swarm:context" {
                            // List all shared context
                            let ctx = shared_context.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (swarm_id, entries) in ctx.iter() {
                                for (key, context) in entries.iter() {
                                    out.push(serde_json::json!({
                                        "swarm_id": swarm_id,
                                        "key": key,
                                        "value": context.value,
                                        "from_session": context.from_session,
                                        "from_name": context.from_name,
                                        "created_secs_ago": context.created_at.elapsed().as_secs(),
                                        "updated_secs_ago": context.updated_at.elapsed().as_secs(),
                                    }));
                                }
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("swarm:context:") {
                            // Get context for specific swarm or key
                            let arg = cmd.strip_prefix("swarm:context:").unwrap_or("").trim();
                            let ctx = shared_context.read().await;
                            // Check if arg contains a key separator
                            if let Some((swarm_id, key)) = arg.split_once(':') {
                                // Get specific key in specific swarm
                                if let Some(entries) = ctx.get(swarm_id) {
                                    if let Some(context) = entries.get(key) {
                                        Ok(serde_json::json!({
                                            "swarm_id": swarm_id,
                                            "key": key,
                                            "value": context.value,
                                            "from_session": context.from_session,
                                            "from_name": context.from_name,
                                            "created_secs_ago": context.created_at.elapsed().as_secs(),
                                            "updated_secs_ago": context.updated_at.elapsed().as_secs(),
                                        }).to_string())
                                    } else {
                                        Err(anyhow::anyhow!(
                                            "No context key '{}' in swarm '{}'",
                                            key,
                                            swarm_id
                                        ))
                                    }
                                } else {
                                    Err(anyhow::anyhow!("No context for swarm '{}'", swarm_id))
                                }
                            } else {
                                // Get all context for swarm
                                if let Some(entries) = ctx.get(arg) {
                                    let mut out: Vec<serde_json::Value> = Vec::new();
                                    for (key, context) in entries.iter() {
                                        out.push(serde_json::json!({
                                            "key": key,
                                            "value": context.value,
                                            "from_session": context.from_session,
                                            "from_name": context.from_name,
                                            "created_secs_ago": context.created_at.elapsed().as_secs(),
                                            "updated_secs_ago": context.updated_at.elapsed().as_secs(),
                                        }));
                                    }
                                    Ok(serde_json::to_string_pretty(&out)
                                        .unwrap_or_else(|_| "[]".to_string()))
                                } else {
                                    Ok("[]".to_string())
                                }
                            }
                        } else if cmd == "swarm:touches" {
                            // List all file touches
                            let touches = file_touches.read().await;
                            let members = swarm_members.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (path, accesses) in touches.iter() {
                                for access in accesses.iter() {
                                    let name = members
                                        .get(&access.session_id)
                                        .and_then(|m| m.friendly_name.clone());
                                    let timestamp_iso = access
                                        .absolute_time
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs())
                                        .unwrap_or(0);
                                    out.push(serde_json::json!({
                                        "path": path.to_string_lossy(),
                                        "session_id": access.session_id,
                                        "session_name": name,
                                        "op": access.op.as_str(),
                                        "summary": access.summary,
                                        "age_secs": access.timestamp.elapsed().as_secs(),
                                        "timestamp_unix": timestamp_iso,
                                    }));
                                }
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("swarm:touches:") {
                            // Get touches for specific path or filter by swarm
                            let arg = cmd.strip_prefix("swarm:touches:").unwrap_or("").trim();
                            let touches = file_touches.read().await;
                            let members = swarm_members.read().await;

                            // Check if filtering by swarm
                            if arg.starts_with("swarm:") {
                                let swarm_id = arg.strip_prefix("swarm:").unwrap_or("");
                                // Get session IDs for this swarm
                                let swarm_sessions: HashSet<String> = members
                                    .iter()
                                    .filter(|(_, m)| m.swarm_id.as_deref() == Some(swarm_id))
                                    .map(|(id, _)| id.clone())
                                    .collect();

                                let mut out: Vec<serde_json::Value> = Vec::new();
                                for (path, accesses) in touches.iter() {
                                    for access in accesses.iter() {
                                        if swarm_sessions.contains(&access.session_id) {
                                            let name = members
                                                .get(&access.session_id)
                                                .and_then(|m| m.friendly_name.clone());
                                            let timestamp_unix = access
                                                .absolute_time
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .map(|d| d.as_secs())
                                                .unwrap_or(0);
                                            out.push(serde_json::json!({
                                                "path": path.to_string_lossy(),
                                                "session_id": access.session_id,
                                                "session_name": name,
                                                "op": access.op.as_str(),
                                                "summary": access.summary,
                                                "age_secs": access.timestamp.elapsed().as_secs(),
                                                "timestamp_unix": timestamp_unix,
                                            }));
                                        }
                                    }
                                }
                                Ok(serde_json::to_string_pretty(&out)
                                    .unwrap_or_else(|_| "[]".to_string()))
                            } else {
                                // Get touches for specific path
                                let path = PathBuf::from(arg);
                                if let Some(accesses) = touches.get(&path) {
                                    let mut out: Vec<serde_json::Value> = Vec::new();
                                    for access in accesses.iter() {
                                        let name = members
                                            .get(&access.session_id)
                                            .and_then(|m| m.friendly_name.clone());
                                        let timestamp_unix = access
                                            .absolute_time
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .map(|d| d.as_secs())
                                            .unwrap_or(0);
                                        out.push(serde_json::json!({
                                            "session_id": access.session_id,
                                            "session_name": name,
                                            "op": access.op.as_str(),
                                            "summary": access.summary,
                                            "age_secs": access.timestamp.elapsed().as_secs(),
                                            "timestamp_unix": timestamp_unix,
                                        }));
                                    }
                                    Ok(serde_json::to_string_pretty(&out)
                                        .unwrap_or_else(|_| "[]".to_string()))
                                } else {
                                    Ok("[]".to_string())
                                }
                            }
                        } else if cmd == "swarm:conflicts" {
                            // List files touched by multiple sessions
                            let touches = file_touches.read().await;
                            let members = swarm_members.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (path, accesses) in touches.iter() {
                                // Get unique session IDs
                                let unique_sessions: HashSet<_> =
                                    accesses.iter().map(|a| &a.session_id).collect();
                                if unique_sessions.len() > 1 {
                                    // Build full access history for this conflicting file
                                    let access_history: Vec<_> = accesses
                                        .iter()
                                        .map(|a| {
                                            let name = members
                                                .get(&a.session_id)
                                                .and_then(|m| m.friendly_name.clone());
                                            let timestamp_unix = a
                                                .absolute_time
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .map(|d| d.as_secs())
                                                .unwrap_or(0);
                                            serde_json::json!({
                                                "session_id": a.session_id,
                                                "session_name": name,
                                                "op": a.op.as_str(),
                                                "summary": a.summary,
                                                "age_secs": a.timestamp.elapsed().as_secs(),
                                                "timestamp_unix": timestamp_unix,
                                            })
                                        })
                                        .collect();
                                    out.push(serde_json::json!({
                                        "path": path.to_string_lossy(),
                                        "session_count": unique_sessions.len(),
                                        "accesses": access_history,
                                    }));
                                }
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd == "swarm:proposals" {
                            // List all pending plan proposals across all swarms
                            let ctx = shared_context.read().await;
                            let members = swarm_members.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (swarm_id, swarm_ctx) in ctx.iter() {
                                for (key, context) in swarm_ctx.iter() {
                                    if key.starts_with("plan_proposal:") {
                                        let proposer_id =
                                            key.strip_prefix("plan_proposal:").unwrap_or("");
                                        let proposer_name = members
                                            .get(proposer_id)
                                            .and_then(|m| m.friendly_name.clone());
                                        let item_count =
                                            serde_json::from_str::<Vec<serde_json::Value>>(
                                                &context.value,
                                            )
                                            .map(|v| v.len())
                                            .unwrap_or(0);
                                        out.push(serde_json::json!({
                                            "swarm_id": swarm_id,
                                            "proposer_session": proposer_id,
                                            "proposer_name": proposer_name,
                                            "item_count": item_count,
                                            "age_secs": context.created_at.elapsed().as_secs(),
                                            "status": "pending",
                                        }));
                                    }
                                }
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("swarm:proposals:") {
                            // Get proposals for specific swarm or specific proposal
                            let arg = cmd.strip_prefix("swarm:proposals:").unwrap_or("").trim();
                            let ctx = shared_context.read().await;
                            let members = swarm_members.read().await;

                            // Check if this is a session ID (get specific proposal details)
                            if arg.starts_with("session_") {
                                // Find proposal from this session across all swarms
                                let proposal_key = format!("plan_proposal:{}", arg);
                                let mut found_proposal: Option<String> = None;
                                for (swarm_id, swarm_ctx) in ctx.iter() {
                                    if let Some(context) = swarm_ctx.get(&proposal_key) {
                                        let proposer_name =
                                            members.get(arg).and_then(|m| m.friendly_name.clone());
                                        let items: Vec<serde_json::Value> =
                                            serde_json::from_str(&context.value)
                                                .unwrap_or_default();
                                        found_proposal = Some(
                                            serde_json::json!({
                                                "swarm_id": swarm_id,
                                                "proposer_session": arg,
                                                "proposer_name": proposer_name,
                                                "status": "pending",
                                                "age_secs": context.created_at.elapsed().as_secs(),
                                                "items": items,
                                            })
                                            .to_string(),
                                        );
                                        break;
                                    }
                                }
                                if let Some(result) = found_proposal {
                                    Ok(result)
                                } else {
                                    Err(anyhow::anyhow!("No proposal found from session '{}'", arg))
                                }
                            } else {
                                // Filter by swarm ID
                                let mut out: Vec<serde_json::Value> = Vec::new();
                                if let Some(swarm_ctx) = ctx.get(arg) {
                                    for (key, context) in swarm_ctx.iter() {
                                        if key.starts_with("plan_proposal:") {
                                            let proposer_id =
                                                key.strip_prefix("plan_proposal:").unwrap_or("");
                                            let proposer_name = members
                                                .get(proposer_id)
                                                .and_then(|m| m.friendly_name.clone());
                                            let items: Vec<serde_json::Value> =
                                                serde_json::from_str(&context.value)
                                                    .unwrap_or_default();
                                            out.push(serde_json::json!({
                                                "proposer_session": proposer_id,
                                                "proposer_name": proposer_name,
                                                "status": "pending",
                                                "age_secs": context.created_at.elapsed().as_secs(),
                                                "items": items,
                                            }));
                                        }
                                    }
                                }
                                Ok(serde_json::to_string_pretty(&out)
                                    .unwrap_or_else(|_| "[]".to_string()))
                            }
                        } else if cmd.starts_with("swarm:info:") {
                            // Get full info for a specific swarm
                            let swarm_id = cmd.strip_prefix("swarm:info:").unwrap_or("").trim();
                            let swarms = swarms_by_id.read().await;
                            let coordinators = swarm_coordinators.read().await;
                            let members = swarm_members.read().await;
                            let plans = swarm_plans.read().await;
                            let ctx = shared_context.read().await;
                            let touches = file_touches.read().await;

                            if let Some(session_ids) = swarms.get(swarm_id) {
                                let coordinator = coordinators.get(swarm_id);
                                let coordinator_name = coordinator.and_then(|cid| {
                                    members.get(cid).and_then(|m| m.friendly_name.clone())
                                });

                                // Get member details
                                let member_details: Vec<_> = session_ids
                                    .iter()
                                    .filter_map(|sid| {
                                        members.get(sid).map(|m| {
                                            serde_json::json!({
                                                "session_id": m.session_id,
                                                "friendly_name": m.friendly_name,
                                                "status": m.status,
                                                "detail": m.detail,
                                                "working_dir": m.working_dir,
                                            })
                                        })
                                    })
                                    .collect();

                                // Get plan
                                let plan = plans
                                    .get(swarm_id)
                                    .map(|vp| &vp.items)
                                    .cloned()
                                    .unwrap_or_default();

                                // Get context keys
                                let context_keys: Vec<_> = ctx
                                    .get(swarm_id)
                                    .map(|entries| entries.keys().cloned().collect())
                                    .unwrap_or_default();

                                // Get files with conflicts in this swarm
                                let conflicts: Vec<_> = touches
                                    .iter()
                                    .filter_map(|(path, accesses)| {
                                        let swarm_accesses: Vec<_> = accesses
                                            .iter()
                                            .filter(|a| session_ids.contains(&a.session_id))
                                            .collect();
                                        let unique: HashSet<_> =
                                            swarm_accesses.iter().map(|a| &a.session_id).collect();
                                        if unique.len() > 1 {
                                            Some(path.to_string_lossy().to_string())
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();

                                Ok(serde_json::json!({
                                    "swarm_id": swarm_id,
                                    "member_count": session_ids.len(),
                                    "members": member_details,
                                    "coordinator": coordinator,
                                    "coordinator_name": coordinator_name,
                                    "plan": plan,
                                    "context_keys": context_keys,
                                    "conflict_files": conflicts,
                                })
                                .to_string())
                            } else {
                                Err(anyhow::anyhow!("No swarm with id '{}'", swarm_id))
                            }
                        } else if cmd.starts_with("swarm:broadcast:") {
                            // Broadcast a message to all members of a swarm
                            let rest = cmd.strip_prefix("swarm:broadcast:").unwrap_or("").trim();
                            // Parse: swarm_id message or just message (uses requester's swarm)
                            let (target_swarm_id, message) = if let Some(space_idx) = rest.find(' ')
                            {
                                let potential_id = &rest[..space_idx];
                                let msg = rest[space_idx + 1..].trim();
                                // Check if potential_id looks like a swarm_id (contains /)
                                if potential_id.contains('/') {
                                    (Some(potential_id.to_string()), msg.to_string())
                                } else {
                                    (None, rest.to_string())
                                }
                            } else {
                                (None, rest.to_string())
                            };

                            if message.is_empty() {
                                Err(anyhow::anyhow!("swarm:broadcast requires a message"))
                            } else {
                                // Find the swarm to broadcast to
                                let swarm_id = if let Some(id) = target_swarm_id {
                                    Some(id)
                                } else {
                                    // Try to find requester's swarm
                                    let members = swarm_members.read().await;
                                    let current_session = session_id.read().await;
                                    members
                                        .get(&*current_session)
                                        .and_then(|m| m.swarm_id.clone())
                                };

                                if let Some(swarm_id) = swarm_id {
                                    let swarms = swarms_by_id.read().await;
                                    let members = swarm_members.read().await;
                                    let current_session = session_id.read().await;
                                    let from_name = members
                                        .get(&*current_session)
                                        .and_then(|m| m.friendly_name.clone());

                                    if let Some(member_ids) = swarms.get(&swarm_id) {
                                        let mut sent_count = 0;
                                        for member_id in member_ids {
                                            if let Some(member) = members.get(member_id) {
                                                let notification = ServerEvent::Notification {
                                                    from_session: current_session.clone(),
                                                    from_name: from_name.clone(),
                                                    notification_type: NotificationType::Message {
                                                        scope: Some("broadcast".to_string()),
                                                        channel: None,
                                                    },
                                                    message: message.clone(),
                                                };
                                                if member.event_tx.send(notification).is_ok() {
                                                    sent_count += 1;
                                                }
                                            }
                                        }
                                        Ok(serde_json::json!({
                                            "swarm_id": swarm_id,
                                            "message": message,
                                            "sent_to": sent_count,
                                        })
                                        .to_string())
                                    } else {
                                        Err(anyhow::anyhow!("No members in swarm '{}'", swarm_id))
                                    }
                                } else {
                                    Err(anyhow::anyhow!("No swarm found. Specify swarm_id: swarm:broadcast:<swarm_id> <message>"))
                                }
                            }
                        } else if cmd.starts_with("swarm:notify:") {
                            // Send notification to a specific session
                            let rest = cmd.strip_prefix("swarm:notify:").unwrap_or("").trim();
                            // Parse: session_id message
                            if let Some(space_idx) = rest.find(' ') {
                                let target_session = &rest[..space_idx];
                                let message = rest[space_idx + 1..].trim();

                                if message.is_empty() {
                                    Err(anyhow::anyhow!("swarm:notify requires a message"))
                                } else {
                                    let members = swarm_members.read().await;
                                    let current_session = session_id.read().await;
                                    let from_name = members
                                        .get(&*current_session)
                                        .and_then(|m| m.friendly_name.clone());

                                    if let Some(target) = members.get(target_session) {
                                        let notification = ServerEvent::Notification {
                                            from_session: current_session.clone(),
                                            from_name: from_name.clone(),
                                            notification_type: NotificationType::Message {
                                                scope: Some("dm".to_string()),
                                                channel: None,
                                            },
                                            message: message.to_string(),
                                        };
                                        if target.event_tx.send(notification).is_ok() {
                                            let target_name = target.friendly_name.clone();
                                            Ok(serde_json::json!({
                                                "sent_to": target_session,
                                                "sent_to_name": target_name,
                                                "message": message,
                                            })
                                            .to_string())
                                        } else {
                                            Err(anyhow::anyhow!("Failed to send notification"))
                                        }
                                    } else {
                                        Err(anyhow::anyhow!("Unknown session '{}'", target_session))
                                    }
                                }
                            } else {
                                Err(anyhow::anyhow!(
                                    "Usage: swarm:notify:<session_id> <message>"
                                ))
                            }
                        } else if cmd.starts_with("swarm:session:") {
                            // Get detailed execution state for a specific session
                            let target_session =
                                cmd.strip_prefix("swarm:session:").unwrap_or("").trim();
                            if target_session.is_empty() {
                                Err(anyhow::anyhow!("swarm:session requires a session_id"))
                            } else {
                                let sessions_guard = sessions.read().await;
                                let members = swarm_members.read().await;

                                if let Some(agent_arc) = sessions_guard.get(target_session) {
                                    let member_info = members.get(target_session);

                                    // Try to get agent state (may fail if agent is busy)
                                    let agent_state = if let Ok(agent) = agent_arc.try_lock() {
                                        Some(serde_json::json!({
                                            "provider": agent.provider_name(),
                                            "model": agent.provider_model(),
                                            "message_count": agent.message_count(),
                                            "pending_alert_count": agent.pending_alert_count(),
                                            "pending_alerts": agent.pending_alerts_preview(),
                                            "soft_interrupt_count": agent.soft_interrupt_count(),
                                            "soft_interrupts": agent.soft_interrupts_preview(),
                                            "has_urgent_interrupt": agent.has_urgent_interrupt(),
                                            "last_usage": agent.last_usage(),
                                        }))
                                    } else {
                                        None
                                    };

                                    let is_processing = member_info
                                        .map(|m| m.status == "running")
                                        .unwrap_or(agent_state.is_none());

                                    Ok(serde_json::json!({
                                        "session_id": target_session,
                                        "friendly_name": member_info.and_then(|m| m.friendly_name.clone()),
                                        "swarm_id": member_info.and_then(|m| m.swarm_id.clone()),
                                        "status": member_info.map(|m| m.status.clone()),
                                        "detail": member_info.and_then(|m| m.detail.clone()),
                                        "joined_secs_ago": member_info.map(|m| m.joined_at.elapsed().as_secs()),
                                        "status_changed_secs_ago": member_info.map(|m| m.last_status_change.elapsed().as_secs()),
                                        "is_processing": is_processing,
                                        "agent_state": agent_state,
                                    }).to_string())
                                } else {
                                    Err(anyhow::anyhow!("Unknown session '{}'", target_session))
                                }
                            }
                        } else if cmd == "swarm:interrupts" {
                            // List all pending interrupts across all sessions
                            let sessions_guard = sessions.read().await;
                            let members = swarm_members.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();

                            for (session_id, agent_arc) in sessions_guard.iter() {
                                if let Ok(agent) = agent_arc.try_lock() {
                                    let alert_count = agent.pending_alert_count();
                                    let interrupt_count = agent.soft_interrupt_count();

                                    if alert_count > 0 || interrupt_count > 0 {
                                        let name = members
                                            .get(session_id)
                                            .and_then(|m| m.friendly_name.clone());
                                        out.push(serde_json::json!({
                                            "session_id": session_id,
                                            "session_name": name,
                                            "pending_alert_count": alert_count,
                                            "pending_alerts": agent.pending_alerts_preview(),
                                            "soft_interrupt_count": interrupt_count,
                                            "soft_interrupts": agent.soft_interrupts_preview(),
                                            "has_urgent": agent.has_urgent_interrupt(),
                                        }));
                                    }
                                }
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("swarm:id:") {
                            // Compute swarm_id for a path and show provenance
                            let path_str = cmd.strip_prefix("swarm:id:").unwrap_or("").trim();
                            if path_str.is_empty() {
                                Err(anyhow::anyhow!("swarm:id requires a path"))
                            } else {
                                let path = PathBuf::from(path_str);

                                // Check env override first
                                let env_override = std::env::var("JCODE_SWARM_ID")
                                    .ok()
                                    .filter(|s| !s.trim().is_empty());

                                // Try to get git common dir
                                let git_common = git_common_dir_for(&path);

                                // Compute final swarm_id
                                let swarm_id = swarm_id_for_dir(Some(path.clone()));

                                let is_git_repo = git_common.is_some();
                                Ok(serde_json::json!({
                                    "path": path_str,
                                    "swarm_id": swarm_id,
                                    "source": if env_override.is_some() { "env:JCODE_SWARM_ID" }
                                              else if is_git_repo { "git_common_dir" }
                                              else { "none" },
                                    "env_override": env_override,
                                    "git_common_dir": git_common.clone(),
                                    "git_root": git_common,
                                    "is_git_repo": is_git_repo,
                                })
                                .to_string())
                            }
                        } else if cmd.starts_with("swarm:set_context:") {
                            // Set shared context: swarm:set_context:<session_id> <key> <value>
                            let rest = cmd.strip_prefix("swarm:set_context:").unwrap_or("").trim();
                            // Parse: session_id key value
                            let parts: Vec<&str> = rest.splitn(3, ' ').collect();
                            if parts.len() < 3 {
                                Err(anyhow::anyhow!(
                                    "Usage: swarm:set_context:<session_id> <key> <value>"
                                ))
                            } else {
                                let acting_session = parts[0];
                                let key = parts[1].to_string();
                                let value = parts[2].to_string();

                                // Find swarm_id for acting session
                                let (swarm_id, friendly_name) = {
                                    let members = swarm_members.read().await;
                                    let swarm_id = members
                                        .get(acting_session)
                                        .and_then(|m| m.swarm_id.clone());
                                    let name = members
                                        .get(acting_session)
                                        .and_then(|m| m.friendly_name.clone());
                                    (swarm_id, name)
                                };

                                if let Some(swarm_id) = swarm_id {
                                    // Store context
                                    {
                                        let mut ctx = shared_context.write().await;
                                        let swarm_ctx = ctx
                                            .entry(swarm_id.clone())
                                            .or_insert_with(HashMap::new);
                                        let now = Instant::now();
                                        let created_at = swarm_ctx
                                            .get(&key)
                                            .map(|c| c.created_at)
                                            .unwrap_or(now);
                                        swarm_ctx.insert(
                                            key.clone(),
                                            SharedContext {
                                                key: key.clone(),
                                                value: value.clone(),
                                                from_session: acting_session.to_string(),
                                                from_name: friendly_name.clone(),
                                                created_at,
                                                updated_at: now,
                                            },
                                        );
                                    }

                                    // Notify other swarm members
                                    let swarm_session_ids: Vec<String> = {
                                        let swarms = swarms_by_id.read().await;
                                        swarms
                                            .get(&swarm_id)
                                            .map(|s| s.iter().cloned().collect())
                                            .unwrap_or_default()
                                    };
                                    let members = swarm_members.read().await;
                                    for sid in &swarm_session_ids {
                                        if sid != acting_session {
                                            if let Some(member) = members.get(sid) {
                                                let _ = member.event_tx.send(
                                                    ServerEvent::Notification {
                                                        from_session: acting_session.to_string(),
                                                        from_name: friendly_name.clone(),
                                                        notification_type:
                                                            NotificationType::SharedContext {
                                                                key: key.clone(),
                                                                value: value.clone(),
                                                            },
                                                        message: format!(
                                                            "Shared context: {} = {}",
                                                            key, value
                                                        ),
                                                    },
                                                );
                                            }
                                        }
                                    }
                                    Ok(serde_json::json!({
                                        "swarm_id": swarm_id,
                                        "key": key,
                                        "value": value,
                                        "from_session": acting_session,
                                    })
                                    .to_string())
                                } else {
                                    Err(anyhow::anyhow!(
                                        "Session '{}' is not in a swarm",
                                        acting_session
                                    ))
                                }
                            }
                        } else if cmd.starts_with("swarm:approve_plan:") {
                            // Approve plan: swarm:approve_plan:<coordinator_session> <proposer_session>
                            let rest = cmd.strip_prefix("swarm:approve_plan:").unwrap_or("").trim();
                            let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                            if parts.len() < 2 {
                                Err(anyhow::anyhow!("Usage: swarm:approve_plan:<coordinator_session> <proposer_session>"))
                            } else {
                                let coord_session = parts[0];
                                let proposer_session = parts[1];

                                // Check coordinator status
                                let (swarm_id, is_coordinator) = {
                                    let members = swarm_members.read().await;
                                    let swarm_id =
                                        members.get(coord_session).and_then(|m| m.swarm_id.clone());
                                    let is_coord = if let Some(ref sid) = swarm_id {
                                        let coordinators = swarm_coordinators.read().await;
                                        coordinators
                                            .get(sid)
                                            .map(|c| c == coord_session)
                                            .unwrap_or(false)
                                    } else {
                                        false
                                    };
                                    (swarm_id, is_coord)
                                };

                                if !is_coordinator {
                                    Err(anyhow::anyhow!(
                                        "Only the coordinator can approve plan proposals."
                                    ))
                                } else if let Some(swarm_id) = swarm_id {
                                    // Read proposal
                                    let proposal_key =
                                        format!("plan_proposal:{}", proposer_session);
                                    let proposal_value = {
                                        let ctx = shared_context.read().await;
                                        ctx.get(&swarm_id)
                                            .and_then(|sc| sc.get(&proposal_key))
                                            .map(|c| c.value.clone())
                                    };

                                    match proposal_value {
                                        None => Err(anyhow::anyhow!(
                                            "No pending plan proposal from session '{}'",
                                            proposer_session
                                        )),
                                        Some(proposal) => {
                                            if let Ok(items) =
                                                serde_json::from_str::<Vec<PlanItem>>(&proposal)
                                            {
                                                let version = {
                                                    let mut plans = swarm_plans.write().await;
                                                    let vp = plans
                                                        .entry(swarm_id.clone())
                                                        .or_insert_with(VersionedPlan::new);
                                                    vp.items.extend(items.clone());
                                                    vp.version += 1;
                                                    vp.participants
                                                        .insert(coord_session.to_string());
                                                    vp.participants
                                                        .insert(proposer_session.to_string());
                                                    vp.version
                                                };
                                                // Remove proposal
                                                {
                                                    let mut ctx = shared_context.write().await;
                                                    if let Some(swarm_ctx) = ctx.get_mut(&swarm_id)
                                                    {
                                                        swarm_ctx.remove(&proposal_key);
                                                    }
                                                }
                                                Ok(serde_json::json!({
                                                    "approved": true,
                                                    "items_added": items.len(),
                                                    "plan_version": version,
                                                    "swarm_id": swarm_id,
                                                })
                                                .to_string())
                                            } else {
                                                Err(anyhow::anyhow!("Failed to parse plan proposal as Vec<PlanItem>"))
                                            }
                                        }
                                    }
                                } else {
                                    Err(anyhow::anyhow!("Not in a swarm."))
                                }
                            }
                        } else if cmd.starts_with("swarm:reject_plan:") {
                            // Reject plan: swarm:reject_plan:<coordinator_session> <proposer_session> [reason]
                            let rest = cmd.strip_prefix("swarm:reject_plan:").unwrap_or("").trim();
                            let parts: Vec<&str> = rest.splitn(3, ' ').collect();
                            if parts.len() < 2 {
                                Err(anyhow::anyhow!("Usage: swarm:reject_plan:<coordinator_session> <proposer_session> [reason]"))
                            } else {
                                let coord_session = parts[0];
                                let proposer_session = parts[1];
                                let reason = if parts.len() >= 3 {
                                    Some(parts[2].to_string())
                                } else {
                                    None
                                };

                                // Check coordinator status
                                let (swarm_id, is_coordinator) = {
                                    let members = swarm_members.read().await;
                                    let swarm_id =
                                        members.get(coord_session).and_then(|m| m.swarm_id.clone());
                                    let is_coord = if let Some(ref sid) = swarm_id {
                                        let coordinators = swarm_coordinators.read().await;
                                        coordinators
                                            .get(sid)
                                            .map(|c| c == coord_session)
                                            .unwrap_or(false)
                                    } else {
                                        false
                                    };
                                    (swarm_id, is_coord)
                                };

                                if !is_coordinator {
                                    Err(anyhow::anyhow!(
                                        "Only the coordinator can reject plan proposals."
                                    ))
                                } else if let Some(swarm_id) = swarm_id {
                                    let proposal_key =
                                        format!("plan_proposal:{}", proposer_session);
                                    let proposal_exists = {
                                        let ctx = shared_context.read().await;
                                        ctx.get(&swarm_id)
                                            .and_then(|sc| sc.get(&proposal_key))
                                            .is_some()
                                    };
                                    if !proposal_exists {
                                        Err(anyhow::anyhow!(
                                            "No pending plan proposal from session '{}'",
                                            proposer_session
                                        ))
                                    } else {
                                        // Remove proposal
                                        {
                                            let mut ctx = shared_context.write().await;
                                            if let Some(swarm_ctx) = ctx.get_mut(&swarm_id) {
                                                swarm_ctx.remove(&proposal_key);
                                            }
                                        }
                                        let reason_msg = reason
                                            .as_ref()
                                            .map(|r| format!(": {}", r))
                                            .unwrap_or_default();
                                        Ok(serde_json::json!({
                                            "rejected": true,
                                            "proposer_session": proposer_session,
                                            "reason": reason_msg,
                                            "swarm_id": swarm_id,
                                        })
                                        .to_string())
                                    }
                                } else {
                                    Err(anyhow::anyhow!("Not in a swarm."))
                                }
                            }
                        } else if false {
                            // Placeholder (duplicates removed — swarm:roles, swarm:channels,
                            // swarm:plan_version are handled earlier in the chain)
                            Ok("unreachable".to_string())
                        } else if cmd == "ambient:status" {
                            // Get ambient mode status
                            if let Some(ref runner) = ambient_runner {
                                Ok(runner.status_json().await)
                            } else {
                                Ok(serde_json::json!({
                                    "enabled": false,
                                    "status": "disabled",
                                    "message": "Ambient mode is not enabled in config"
                                })
                                .to_string())
                            }
                        } else if cmd == "ambient:queue" {
                            if let Some(ref runner) = ambient_runner {
                                Ok(runner.queue_json().await)
                            } else {
                                Ok("[]".to_string())
                            }
                        } else if cmd == "ambient:trigger" {
                            if let Some(ref runner) = ambient_runner {
                                runner.trigger().await;
                                Ok("Ambient cycle triggered".to_string())
                            } else {
                                Err(anyhow::anyhow!("Ambient mode is not enabled"))
                            }
                        } else if cmd == "ambient:log" {
                            if let Some(ref runner) = ambient_runner {
                                Ok(runner.log_json().await)
                            } else {
                                Ok("[]".to_string())
                            }
                        } else if cmd == "ambient:permissions" {
                            if let Some(ref runner) = ambient_runner {
                                let _ = runner
                                    .safety()
                                    .expire_dead_session_requests("debug_socket_gc");
                                let pending = runner.safety().pending_requests();
                                let items: Vec<serde_json::Value> = pending
                                    .iter()
                                    .map(|r| {
                                        let review_summary = r
                                            .context
                                            .as_ref()
                                            .and_then(|ctx| ctx.get("review"))
                                            .and_then(|review| review.get("summary"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or(&r.description);
                                        let review_why = r
                                            .context
                                            .as_ref()
                                            .and_then(|ctx| ctx.get("review"))
                                            .and_then(|review| review.get("why_permission_needed"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or(&r.rationale);
                                        serde_json::json!({
                                            "id": r.id,
                                            "action": r.action,
                                            "description": r.description,
                                            "rationale": r.rationale,
                                            "summary": review_summary,
                                            "why_permission_needed": review_why,
                                            "urgency": format!("{:?}", r.urgency),
                                            "wait": r.wait,
                                            "created_at": r.created_at.to_rfc3339(),
                                            "context": r.context,
                                        })
                                    })
                                    .collect();
                                Ok(serde_json::to_string_pretty(&items)
                                    .unwrap_or_else(|_| "[]".to_string()))
                            } else {
                                Ok("[]".to_string())
                            }
                        } else if cmd.starts_with("ambient:approve:") {
                            let request_id =
                                cmd.strip_prefix("ambient:approve:").unwrap_or("").trim();
                            if request_id.is_empty() {
                                Err(anyhow::anyhow!("Usage: ambient:approve:<request_id>"))
                            } else if let Some(ref runner) = ambient_runner {
                                runner.safety().record_decision(
                                    request_id,
                                    true,
                                    "debug_socket",
                                    None,
                                )?;
                                Ok(format!("Approved: {}", request_id))
                            } else {
                                Err(anyhow::anyhow!("Ambient mode is not enabled"))
                            }
                        } else if cmd.starts_with("ambient:deny:") {
                            let rest = cmd.strip_prefix("ambient:deny:").unwrap_or("").trim();
                            if rest.is_empty() {
                                Err(anyhow::anyhow!("Usage: ambient:deny:<request_id> [reason]"))
                            } else if let Some(ref runner) = ambient_runner {
                                let mut parts = rest.splitn(2, char::is_whitespace);
                                let request_id = parts.next().unwrap_or("").trim();
                                let message = parts
                                    .next()
                                    .map(|s| s.trim().to_string())
                                    .filter(|s| !s.is_empty());
                                runner.safety().record_decision(
                                    request_id,
                                    false,
                                    "debug_socket",
                                    message,
                                )?;
                                Ok(format!("Denied: {}", request_id))
                            } else {
                                Err(anyhow::anyhow!("Ambient mode is not enabled"))
                            }
                        } else if cmd == "ambient:stop" {
                            if let Some(ref runner) = ambient_runner {
                                runner.stop().await;
                                Ok("Ambient mode stopped".to_string())
                            } else {
                                Err(anyhow::anyhow!("Ambient mode is not enabled"))
                            }
                        } else if cmd == "ambient:start" {
                            if let Some(ref runner) = ambient_runner {
                                if runner.start(Arc::clone(&provider)).await {
                                    Ok("Ambient mode started".to_string())
                                } else {
                                    Ok("Ambient mode is already running".to_string())
                                }
                            } else {
                                Err(anyhow::anyhow!("Ambient mode is not enabled in config"))
                            }
                        } else if cmd == "ambient:help" {
                            Ok(r#"Ambient mode debug commands (ambient: prefix):
  ambient:status              - Current ambient state, cycle count, last run
  ambient:queue               - Scheduled queue contents
  ambient:trigger             - Manually trigger an ambient cycle
  ambient:log                 - Recent transcript summaries
  ambient:permissions         - List pending permission requests
  ambient:approve:<id>        - Approve a permission request
  ambient:deny:<id> [reason]  - Deny a permission request (optional reason)
  ambient:start               - Start/restart ambient mode
  ambient:stop                - Stop ambient mode"#
                                .to_string())
                        } else if cmd == "events:subscribe" || cmd.starts_with("events:subscribe:")
                        {
                            let type_filter: Option<Vec<String>> = cmd
                                .strip_prefix("events:subscribe:")
                                .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());

                            let ack = ServerEvent::DebugResponse {
                                id,
                                ok: true,
                                output: serde_json::json!({
                                    "subscribed": true,
                                    "filter": type_filter.as_ref().map(|f| f.join(",")),
                                })
                                .to_string(),
                            };
                            let json = encode_event(&ack);
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
                                            SwarmEventType::ContextUpdate { .. } => {
                                                "context_update"
                                            }
                                            SwarmEventType::StatusChange { .. } => "status_change",
                                            SwarmEventType::MemberChange { .. } => "member_change",
                                        };
                                        if let Some(ref filter) = type_filter {
                                            if !filter.iter().any(|f| f == event_type) {
                                                continue;
                                            }
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
                                        let mut line =
                                            serde_json::to_string(&event_json).unwrap_or_default();
                                        line.push('\n');
                                        if writer.write_all(line.as_bytes()).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(broadcast::error::RecvError::Lagged(n)) => {
                                        let lag_json = serde_json::json!({
                                            "type": "lag",
                                            "missed": n,
                                        });
                                        let mut line =
                                            serde_json::to_string(&lag_json).unwrap_or_default();
                                        line.push('\n');
                                        if writer.write_all(line.as_bytes()).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(broadcast::error::RecvError::Closed) => {
                                        break;
                                    }
                                }
                            }
                            return Ok(());
                        } else if cmd == "events:recent" || cmd.starts_with("events:recent:") {
                            // Get recent events (default 50, or specify count)
                            let count: usize = cmd
                                .strip_prefix("events:recent:")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(50);

                            let history = event_history.read().await;
                            let events: Vec<serde_json::Value> = history
                                .iter()
                                .rev()
                                .take(count)
                                .map(|e| {
                                    let timestamp_unix = e
                                        .absolute_time
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs())
                                        .unwrap_or(0);
                                    serde_json::json!({
                                        "id": e.id,
                                        "session_id": e.session_id,
                                        "session_name": e.session_name,
                                        "swarm_id": e.swarm_id,
                                        "event": e.event,
                                        "age_secs": e.timestamp.elapsed().as_secs(),
                                        "timestamp_unix": timestamp_unix,
                                    })
                                })
                                .collect();
                            Ok(serde_json::to_string_pretty(&events)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("events:since:") {
                            // Get events since a specific event ID
                            let since_id: u64 = cmd
                                .strip_prefix("events:since:")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0);

                            let history = event_history.read().await;
                            let events: Vec<serde_json::Value> = history
                                .iter()
                                .filter(|e| e.id > since_id)
                                .map(|e| {
                                    let timestamp_unix = e
                                        .absolute_time
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs())
                                        .unwrap_or(0);
                                    serde_json::json!({
                                        "id": e.id,
                                        "session_id": e.session_id,
                                        "session_name": e.session_name,
                                        "swarm_id": e.swarm_id,
                                        "event": e.event,
                                        "age_secs": e.timestamp.elapsed().as_secs(),
                                        "timestamp_unix": timestamp_unix,
                                    })
                                })
                                .collect();
                            Ok(serde_json::to_string_pretty(&events)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd == "events:types" {
                            // List available event types
                            Ok(serde_json::json!({
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
                            }).to_string())
                        } else if cmd == "events:count" {
                            // Get current event count and latest ID
                            let history = event_history.read().await;
                            let latest_id = history.last().map(|e| e.id).unwrap_or(0);
                            Ok(serde_json::json!({
                                "count": history.len(),
                                "latest_id": latest_id,
                                "max_history": MAX_EVENT_HISTORY,
                            })
                            .to_string())
                        } else if cmd == "background" || cmd == "background:tasks" {
                            // List background tasks (running + completed on disk)
                            let tasks = crate::background::global().list().await;
                            Ok(serde_json::json!({
                                "count": tasks.len(),
                                "tasks": tasks,
                            })
                            .to_string())
                        } else if cmd == "server:info" {
                            // Return server identity, health, and uptime
                            let uptime_secs = server_start_time.elapsed().as_secs();
                            let session_count = sessions.read().await.len();
                            let member_count = swarm_members.read().await.len();
                            let has_update = server_has_newer_binary();
                            Ok(serde_json::json!({
                                "id": server_identity.id,
                                "name": server_identity.name,
                                "icon": server_identity.icon,
                                "version": server_identity.version,
                                "git_hash": server_identity.git_hash,
                                "uptime_secs": uptime_secs,
                                "session_count": session_count,
                                "swarm_member_count": member_count,
                                "has_update": has_update,
                                "debug_control_enabled": debug_control_allowed(),
                            })
                            .to_string())
                        } else if cmd == "clients:map" || cmd == "clients:mapping" {
                            // Map connected clients to sessions
                            let connections = client_connections.read().await;
                            let members = swarm_members.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for info in connections.values() {
                                let member = members.get(&info.session_id);
                                out.push(serde_json::json!({
                                    "client_id": info.client_id,
                                    "session_id": info.session_id,
                                    "friendly_name": member.and_then(|m| m.friendly_name.clone()),
                                    "working_dir": member.and_then(|m| m.working_dir.clone()),
                                    "swarm_id": member.and_then(|m| m.swarm_id.clone()),
                                    "status": member.map(|m| m.status.clone()),
                                    "detail": member.and_then(|m| m.detail.clone()),
                                    "connected_secs_ago": info.connected_at.elapsed().as_secs(),
                                    "last_seen_secs_ago": info.last_seen.elapsed().as_secs(),
                                }));
                            }
                            Ok(serde_json::json!({
                                "count": out.len(),
                                "clients": out,
                            })
                            .to_string())
                        } else if cmd == "clients" {
                            // List connected TUI clients
                            let debug_state = client_debug_state.read().await;
                            let client_ids: Vec<&String> = debug_state.clients.keys().collect();
                            Ok(serde_json::json!({
                                "count": debug_state.clients.len(),
                                "active_id": debug_state.active_id,
                                "client_ids": client_ids,
                            })
                            .to_string())
                        } else if cmd == "swarm:help" {
                            Ok(swarm_debug_help_text())
                        } else if cmd == "help" {
                            Ok(debug_help_text())
                        } else {
                            match resolve_debug_session(&sessions, &session_id, requested_session)
                                .await
                            {
                                Ok((_session, agent)) => {
                                    execute_debug_command(
                                        agent,
                                        cmd,
                                        Arc::clone(&debug_jobs),
                                        Some(&server_identity),
                                    )
                                    .await
                                }
                                Err(e) => Err(e),
                            }
                        }
                    }
                };

                let (ok, output) = match result {
                    Ok(output) => (true, output),
                    Err(e) => (false, e.to_string()),
                };
                let event = ServerEvent::DebugResponse { id, ok, output };
                let json = encode_event(&event);
                writer.write_all(json.as_bytes()).await?;
            }

            _ => {
                // Debug socket only allows ping, state, and debug_command
                let event = ServerEvent::Error {
                    id: request.id(),
                    message: "Debug socket only allows ping, state, and debug_command".to_string(),
                    retry_after_secs: None,
                };
                let json = encode_event(&event);
                writer.write_all(json.as_bytes()).await?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        crate::storage::lock_test_env()
    }

    struct TestHomeGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev_home: Option<OsString>,
        _temp_home: tempfile::TempDir,
    }

    impl TestHomeGuard {
        fn new() -> Self {
            let lock = lock_env();
            let temp_home = tempfile::Builder::new()
                .prefix("jcode-server-debug-test-home-")
                .tempdir()
                .expect("create temp home");
            let prev_home = std::env::var_os("JCODE_HOME");
            std::env::set_var("JCODE_HOME", temp_home.path());
            Self {
                _lock: lock,
                prev_home,
                _temp_home: temp_home,
            }
        }
    }

    impl Drop for TestHomeGuard {
        fn drop(&mut self) {
            if let Some(prev_home) = &self.prev_home {
                std::env::set_var("JCODE_HOME", prev_home);
            } else {
                std::env::remove_var("JCODE_HOME");
            }
        }
    }

    #[test]
    fn client_debug_state_registers_unregisters_and_falls_back() {
        let mut state = ClientDebugState::default();
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();

        state.register("client-a".to_string(), tx1.clone());
        state.register("client-b".to_string(), tx2.clone());

        let (active_id, _sender) = state.active_sender().expect("active sender present");
        assert_eq!(active_id, "client-b");

        state.unregister("client-b");
        let (fallback_id, _sender) = state.active_sender().expect("fallback sender present");
        assert_eq!(fallback_id, "client-a");

        state.unregister("client-a");
        assert!(state.active_sender().is_none());
    }

    #[test]
    fn debug_job_payloads_include_expected_fields() {
        let now = Instant::now();
        let job = DebugJob {
            id: "job_123".to_string(),
            status: DebugJobStatus::Completed,
            command: "message:hello".to_string(),
            session_id: Some("session_abc".to_string()),
            created_at: now,
            started_at: Some(now),
            finished_at: Some(now),
            output: Some("done".to_string()),
            error: None,
        };

        let summary = job.summary_payload();
        assert_eq!(summary.get("id").and_then(|v| v.as_str()), Some("job_123"));
        assert_eq!(
            summary.get("status").and_then(|v| v.as_str()),
            Some("completed")
        );
        assert_eq!(
            summary.get("session_id").and_then(|v| v.as_str()),
            Some("session_abc")
        );

        let status = job.status_payload();
        assert_eq!(status.get("output").and_then(|v| v.as_str()), Some("done"));
        assert!(status.get("error").is_some());
    }

    #[test]
    fn debug_help_text_mentions_key_namespaces_and_commands() {
        let help = debug_help_text();
        assert!(help.contains("SERVER COMMANDS"));
        assert!(help.contains("CLIENT COMMANDS"));
        assert!(help.contains("TESTER COMMANDS"));
        assert!(help.contains("message_async:<text>"));
        assert!(help.contains("client:frame"));
    }

    #[test]
    fn swarm_debug_help_text_mentions_core_swarm_sections() {
        let help = swarm_debug_help_text();
        assert!(help.contains("MEMBERS & STRUCTURE"));
        assert!(help.contains("PLAN PROPOSALS"));
        assert!(help.contains("REAL-TIME EVENTS"));
        assert!(help.contains("swarm:list"));
    }

    #[test]
    fn parse_namespaced_command_defaults_to_server_namespace() {
        assert_eq!(parse_namespaced_command("state"), ("server", "state"));
        assert_eq!(
            parse_namespaced_command("swarm:list"),
            ("server", "swarm:list")
        );
    }

    #[test]
    fn parse_namespaced_command_recognizes_known_namespaces() {
        assert_eq!(
            parse_namespaced_command("client:frame"),
            ("client", "frame")
        );
        assert_eq!(parse_namespaced_command("tester:list"), ("tester", "list"));
        assert_eq!(
            parse_namespaced_command("server:state"),
            ("server", "state")
        );
    }
}

#[cfg(test)]
mod debug_execution_tests {
    use super::{debug_message_timeout_secs, resolve_debug_session};
    use crate::agent::Agent;
    use crate::provider;
    use crate::tool::Registry;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::sync::Arc;
    use tokio::sync::{Mutex as AsyncMutex, RwLock};

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        crate::storage::lock_test_env()
    }

    struct EnvVarGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let lock = lock_env();
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self {
                _lock: lock,
                key,
                previous,
            }
        }

        fn remove(key: &'static str) -> Self {
            let lock = lock_env();
            let previous = std::env::var_os(key);
            std::env::remove_var(key);
            Self {
                _lock: lock,
                key,
                previous,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(prev) = &self.previous {
                std::env::set_var(self.key, prev);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    struct TestProvider;

    #[async_trait::async_trait]
    impl provider::Provider for TestProvider {
        fn name(&self) -> &str {
            "test"
        }

        fn model(&self) -> String {
            "test".to_string()
        }

        fn available_models(&self) -> Vec<&'static str> {
            vec![]
        }

        fn available_models_display(&self) -> Vec<String> {
            vec![]
        }

        async fn prefetch_models(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_model(&self, _model: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn handles_tools_internally(&self) -> bool {
            false
        }

        async fn complete(
            &self,
            _messages: &[crate::message::Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _session_id: Option<&str>,
        ) -> anyhow::Result<crate::provider::EventStream> {
            unimplemented!()
        }

        fn fork(&self) -> Arc<dyn provider::Provider> {
            Arc::new(TestProvider)
        }
    }

    async fn test_agent() -> Arc<AsyncMutex<Agent>> {
        let provider = Arc::new(TestProvider) as Arc<dyn provider::Provider>;
        let registry = Registry::new(provider.clone()).await;
        Arc::new(AsyncMutex::new(Agent::new(provider, registry)))
    }

    #[tokio::test]
    async fn resolve_debug_session_uses_requested_session_when_present() {
        let agent = test_agent().await;
        let session_id = {
            let agent = agent.lock().await;
            agent.session_id().to_string()
        };
        let sessions = Arc::new(RwLock::new(HashMap::from([(
            session_id.clone(),
            agent.clone(),
        )])));
        let current = Arc::new(RwLock::new(String::new()));

        let (resolved_id, resolved_agent) =
            resolve_debug_session(&sessions, &current, Some(session_id.clone()))
                .await
                .expect("resolve requested session");

        assert_eq!(resolved_id, session_id);
        assert!(Arc::ptr_eq(&resolved_agent, &agent));
    }

    #[tokio::test]
    async fn resolve_debug_session_falls_back_to_current_session() {
        let agent = test_agent().await;
        let session_id = {
            let agent = agent.lock().await;
            agent.session_id().to_string()
        };
        let sessions = Arc::new(RwLock::new(HashMap::from([(
            session_id.clone(),
            agent.clone(),
        )])));
        let current = Arc::new(RwLock::new(session_id.clone()));

        let (resolved_id, resolved_agent) = resolve_debug_session(&sessions, &current, None)
            .await
            .expect("resolve current session");

        assert_eq!(resolved_id, session_id);
        assert!(Arc::ptr_eq(&resolved_agent, &agent));
    }

    #[tokio::test]
    async fn resolve_debug_session_uses_only_session_when_singleton() {
        let agent = test_agent().await;
        let session_id = {
            let agent = agent.lock().await;
            agent.session_id().to_string()
        };
        let sessions = Arc::new(RwLock::new(HashMap::from([(
            session_id.clone(),
            agent.clone(),
        )])));
        let current = Arc::new(RwLock::new(String::new()));

        let (resolved_id, _) = resolve_debug_session(&sessions, &current, None)
            .await
            .expect("resolve single session");

        assert_eq!(resolved_id, session_id);
    }

    #[tokio::test]
    async fn resolve_debug_session_errors_for_unknown_or_missing_session() {
        let agent_a = test_agent().await;
        let id_a = {
            let agent = agent_a.lock().await;
            agent.session_id().to_string()
        };
        let agent_b = test_agent().await;
        let id_b = {
            let agent = agent_b.lock().await;
            agent.session_id().to_string()
        };

        let sessions = Arc::new(RwLock::new(HashMap::from([
            (id_a.clone(), agent_a),
            (id_b.clone(), agent_b),
        ])));
        let current = Arc::new(RwLock::new(String::new()));

        let unknown = resolve_debug_session(&sessions, &current, Some("missing".to_string())).await;
        let unknown_err = match unknown {
            Ok(_) => panic!("expected unknown session to error"),
            Err(err) => err,
        };
        assert!(unknown_err.to_string().contains("Unknown session_id"));

        let missing = resolve_debug_session(&sessions, &current, None).await;
        let missing_err = match missing {
            Ok(_) => panic!("expected missing active session to error"),
            Err(err) => err,
        };
        assert!(missing_err.to_string().contains("No active session found"));
    }

    #[test]
    fn debug_message_timeout_secs_reads_valid_env_values() {
        let _guard = EnvVarGuard::set("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS", "17");
        assert_eq!(debug_message_timeout_secs(), Some(17));
    }

    #[test]
    fn debug_message_timeout_secs_ignores_missing_empty_invalid_and_zero() {
        let _guard = EnvVarGuard::remove("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS");
        assert_eq!(debug_message_timeout_secs(), None);
        drop(_guard);

        let _guard = EnvVarGuard::set("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS", "   ");
        assert_eq!(debug_message_timeout_secs(), None);
        drop(_guard);

        let _guard = EnvVarGuard::set("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS", "abc");
        assert_eq!(debug_message_timeout_secs(), None);
        drop(_guard);

        let _guard = EnvVarGuard::set("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS", "0");
        assert_eq!(debug_message_timeout_secs(), None);
    }
}
