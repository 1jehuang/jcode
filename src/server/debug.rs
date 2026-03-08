use super::debug_ambient::maybe_handle_ambient_command;
use super::debug_events::{
    maybe_handle_event_query_command, maybe_handle_event_subscription_command,
};
use super::debug_jobs::{maybe_handle_job_command, maybe_start_async_debug_job, DebugJob};
use super::debug_server_state::maybe_handle_server_state_command;
use super::debug_session_admin::maybe_handle_session_admin_command;
use super::debug_swarm_read::maybe_handle_swarm_read_command;
use super::debug_swarm_write::maybe_handle_swarm_write_command;
use super::debug_testers::execute_tester_command;
use super::{
    debug_control_allowed, FileAccess, ServerIdentity, SharedContext, SwarmEvent, SwarmMember,
    VersionedPlan,
};
use crate::agent::Agent;
use crate::ambient_runner::AmbientRunnerHandle;
use crate::build;
use crate::mcp::McpConfig;
use crate::protocol::{decode_request, encode_event, Request, ServerEvent};
use crate::provider::Provider;
use crate::transport::Stream;
use anyhow::Result;
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

    if let Some(output) =
        maybe_start_async_debug_job(Arc::clone(&agent), trimmed, Arc::clone(&debug_jobs)).await?
    {
        return Ok(output);
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
                        if let Some(output) = maybe_handle_job_command(cmd, &debug_jobs).await? {
                            Ok(output)
                        } else if let Some(output) = maybe_handle_session_admin_command(
                            cmd,
                            &sessions,
                            &session_id,
                            &provider,
                            &swarm_members,
                            &swarms_by_id,
                            &swarm_coordinators,
                            &swarm_plans,
                            &event_history,
                            &event_counter,
                            &swarm_event_tx,
                            mcp_pool.clone(),
                        )
                        .await?
                        {
                            Ok(output)
                        } else if let Some(output) = maybe_handle_server_state_command(
                            cmd,
                            &sessions,
                            &client_connections,
                            &swarm_members,
                            &client_debug_state,
                            &server_identity,
                            server_start_time,
                        )
                        .await?
                        {
                            Ok(output)
                        } else if let Some(output) = maybe_handle_swarm_read_command(
                            cmd,
                            &sessions,
                            &swarm_members,
                            &swarms_by_id,
                            &shared_context,
                            &swarm_plans,
                            &swarm_coordinators,
                            &file_touches,
                            &channel_subscriptions,
                            &server_identity,
                        )
                        .await?
                        {
                            Ok(output)
                        } else if let Some(output) = maybe_handle_swarm_write_command(
                            cmd,
                            &session_id,
                            &swarm_members,
                            &swarms_by_id,
                            &shared_context,
                            &swarm_plans,
                            &swarm_coordinators,
                        )
                        .await?
                        {
                            Ok(output)
                        } else if let Some(output) =
                            maybe_handle_ambient_command(cmd, &ambient_runner, &provider).await?
                        {
                            Ok(output)
                        } else if maybe_handle_event_subscription_command(
                            id,
                            cmd,
                            &swarm_event_tx,
                            &mut writer,
                        )
                        .await?
                        {
                            return Ok(());
                        } else if let Some(output) =
                            maybe_handle_event_query_command(cmd, &event_history).await
                        {
                            Ok(output)
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
