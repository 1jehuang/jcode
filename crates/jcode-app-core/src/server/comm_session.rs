use super::ClientConnectionInfo;
use super::client_lifecycle::process_message_streaming_mpsc;
use super::swarm_mutation_state::{
    PersistedSwarmMutationResponse, SwarmMutationRuntime, begin_or_replay, finish_request,
    request_key,
};
use super::{
    SessionInterruptQueues, SwarmEvent, SwarmEventType, SwarmMember, SwarmState, VersionedPlan,
    append_swarm_completion_report_instructions, broadcast_swarm_plan, broadcast_swarm_status,
    create_headless_session, fanout_session_event, persist_swarm_state_for, record_swarm_event,
    record_swarm_event_for_session, remove_background_tool_signal,
    remove_session_channel_subscriptions, remove_session_from_swarm,
    remove_session_interrupt_queue, set_member_task_label, truncate_detail, update_member_status,
    update_member_status_with_report,
};
use crate::agent::Agent;
use crate::config::SwarmSpawnMode;
use crate::protocol::{NotificationType, ServerEvent};
use crate::provider::Provider;
use crate::session::Session;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;
type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;
type ClientConnections = Arc<RwLock<HashMap<String, ClientConnectionInfo>>>;

/// Serialize spawn admission through member registration within one swarm.
/// Without a reservation or lock, many recursive agents can all observe the same
/// free slot and burst past the configured limit before any child is registered.
/// Weak entries avoid retaining locks for swarms that are no longer active.
fn spawn_admission_lock(swarm_id: &str) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<StdMutex<HashMap<String, Weak<Mutex<()>>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut locks = locks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(swarm_id).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(swarm_id.to_string(), Arc::downgrade(&lock));
    lock
}

/// Look up the most recent terminal env snapshot for the live client connection
/// driving `session_id`, so spawn hooks target that client's terminal instead
/// of the long-lived server's stale startup env (#405). Prefers the most
/// recently seen connection when a session has more than one client attached.
async fn client_terminal_env_for_session(
    session_id: &str,
    client_connections: &ClientConnections,
) -> Vec<(String, String)> {
    let connections = client_connections.read().await;
    connections
        .values()
        .filter(|info| info.session_id == session_id && !info.terminal_env.is_empty())
        .max_by_key(|info| info.last_seen)
        .map(|info| info.terminal_env.clone())
        .unwrap_or_default()
}

fn create_visible_spawn_session(
    working_dir: Option<&str>,
    model_override: Option<&str>,
    provider_key_override: Option<&str>,
    route_api_method_override: Option<&str>,
    effort_override: Option<&str>,
    selfdev_requested: bool,
) -> anyhow::Result<(String, PathBuf)> {
    let cwd = working_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let mut session = Session::create(None, None);
    session.working_dir = Some(cwd.display().to_string());
    if let Some(model) = model_override {
        session.model = Some(model.to_string());
    }
    if let Some(provider_key) = provider_key_override {
        session.provider_key = Some(provider_key.to_string());
    }
    if let Some(route_api_method) = route_api_method_override
        .map(str::trim)
        .filter(|route| !route.is_empty())
    {
        session.route_api_method = Some(route_api_method.to_string());
    }
    if let Some(effort) = effort_override.map(str::trim).filter(|e| !e.is_empty()) {
        // Persisted effort is restored (and validated against the resolved
        // provider/model) by `restore_reasoning_effort_from_session` when the
        // headed client attaches to this session.
        session.reasoning_effort = Some(effort.to_string());
    }
    if selfdev_requested {
        session.set_canary("self-dev");
    }
    session.save()?;

    Ok((session.id.clone(), cwd))
}

async fn resolve_spawn_working_dir(
    requested_working_dir: Option<String>,
    req_session_id: &str,
    sessions: &SessionAgents,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> Option<String> {
    if requested_working_dir
        .as_deref()
        .is_some_and(|dir| !dir.trim().is_empty())
    {
        return requested_working_dir;
    }

    if let Some(agent_dir) = {
        let agent_sessions = sessions.read().await;
        agent_sessions.get(req_session_id).and_then(|agent| {
            agent
                .try_lock()
                .ok()
                .and_then(|agent_guard| agent_guard.working_dir().map(str::to_string))
        })
    } && !agent_dir.trim().is_empty()
    {
        return Some(agent_dir);
    }

    swarm_members
        .read()
        .await
        .get(req_session_id)
        .and_then(|member| member.working_dir.as_ref())
        .map(|dir| dir.display().to_string())
        .filter(|dir| !dir.trim().is_empty())
}

/// Launch a headed window for `session_id`, exporting the given spawn context
/// (`JCODE_SPAWN_KIND`, swarm/coordinator ids, ...) to spawn hooks and
/// spawned terminals so external programs can reroute the window.
fn spawn_visible_session_window_with_context(
    session_id: &str,
    cwd: &std::path::Path,
    selfdev_requested: bool,
    provider_key: Option<&str>,
    context: &crate::session_launch::SessionSpawnContext,
) -> anyhow::Result<bool> {
    let exe = crate::build::client_update_candidate(selfdev_requested)
        .map(|(path, _label)| path)
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from("jcode"));
    if selfdev_requested {
        crate::session_launch::spawn_selfdev_in_new_terminal_with_context(
            &exe,
            session_id,
            cwd,
            provider_key,
            context,
        )
    } else {
        crate::session_launch::spawn_resume_in_new_terminal_with_context(
            &exe,
            session_id,
            cwd,
            provider_key,
            context,
        )
    }
}

fn provider_key_for_spawn_model(
    model: Option<&str>,
    provider_key_override: Option<&str>,
) -> Option<String> {
    if let Some(provider_key) = provider_key_override
        .map(str::trim)
        .filter(|provider_key| !provider_key.is_empty())
    {
        return Some(provider_key.to_string());
    }

    let model = model?.trim();
    if model.is_empty() {
        return None;
    }

    if let Some((prefix, _rest)) = model.split_once(':') {
        let prefix = prefix.trim();
        if crate::provider::provider_from_model_key(prefix).is_some()
            || crate::provider_catalog::resolve_openai_compatible_profile_selection(prefix)
                .is_some()
            || crate::config::config().providers.contains_key(prefix)
        {
            return Some(prefix.to_string());
        }
    }

    crate::provider::provider_for_model(model).map(str::to_string)
}

/// The model/auth identity a spawned swarm agent should inherit from its
/// coordinator. Resolved with a persisted-session fallback so it stays correct
/// even when the coordinator agent is mid-turn (its mutex held), which is the
/// common case because spawns are issued from inside the coordinator's turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CoordinatorSpawnIdentity {
    pub model: Option<String>,
    pub provider_key: Option<String>,
    pub route_api_method: Option<String>,
    pub is_canary: bool,
}

/// The resolved model + auth route a spawned swarm agent should be created
/// with, after reconciling `agents.swarm_model` config against the
/// coordinator's identity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct SwarmSpawnSelection {
    pub model: Option<String>,
    pub provider_key: Option<String>,
    pub route_api_method: Option<String>,
}

/// Resolve the coordinator's model/auth identity without blocking on its agent
/// mutex. During an active coordinator turn the agent lock is held for the
/// whole turn, so `try_lock` fails exactly when a spawn is issued. We fall back
/// to the persisted session snapshot so spawned agents still inherit the
/// coordinator's model, provider key, and auth route instead of silently
/// dropping to the config default (e.g. Claude OAuth instead of the API route).
async fn resolve_coordinator_spawn_identity(
    req_session_id: &str,
    sessions: &SessionAgents,
) -> CoordinatorSpawnIdentity {
    if let Some(agent) = {
        let agent_sessions = sessions.read().await;
        agent_sessions.get(req_session_id).cloned()
    } && let Ok(agent_guard) = agent.try_lock()
    {
        return CoordinatorSpawnIdentity {
            model: Some(agent_guard.provider_model()),
            provider_key: agent_guard.session_provider_key(),
            route_api_method: agent_guard.session_route_api_method(),
            is_canary: agent_guard.is_canary(),
        };
    }

    // Agent busy (mid-turn) or not resident: read the authoritative persisted
    // session snapshot instead of falling back to config defaults.
    match Session::load_startup_stub(req_session_id) {
        Ok(session) => {
            let identity = CoordinatorSpawnIdentity {
                model: session.model.clone(),
                provider_key: session.provider_key.clone(),
                route_api_method: session.route_api_method.clone(),
                is_canary: session.is_canary,
            };
            crate::logging::info(&format!(
                "Swarm spawn: coordinator {} agent busy/unavailable, inheriting identity from persisted session (model={:?} provider_key={:?} route={:?} canary={})",
                req_session_id,
                identity.model,
                identity.provider_key,
                identity.route_api_method,
                identity.is_canary,
            ));
            identity
        }
        Err(error) => {
            crate::logging::warn(&format!(
                "Swarm spawn: failed to load persisted coordinator session {} for model inheritance: {} (spawned agent will use server defaults)",
                req_session_id, error
            ));
            CoordinatorSpawnIdentity::default()
        }
    }
}

/// Split a configured swarm model that carries an explicit auth-route prefix
/// (`openai-api:`, `openai-oauth:`, `claude-api:`, `claude-oauth:`) into a
/// structured selection so spawned sessions pin the exact provider + auth
/// method instead of guessing from the bare model name.
///
/// Example: `agents.swarm_model = "openai-api:gpt-5.5"` resolves to
/// `model = gpt-5.5`, `provider_key = openai-api-key`,
/// `route_api_method = openai-api-key`, which makes every spawned agent use
/// GPT-5.5 on the OpenAI API key route regardless of the coordinator's model.
///
/// Returns `None` for models without such a prefix, or for prefixes that carry
/// no API-vs-OAuth decision (bare provider aliases, OpenRouter, Copilot, ...).
/// Those keep their prefixed model and route correctly via the existing
/// session-restore path.
fn explicit_route_for_configured_model(model: &str) -> Option<SwarmSpawnSelection> {
    let (_, prefix, bare) = crate::provider::explicit_model_provider_prefix(model)?;
    let bare = bare.trim();
    if bare.is_empty() {
        return None;
    }
    // Only the dual-auth (Anthropic/OpenAI OAuth-vs-API) prefixes carry an
    // explicit credential decision worth pinning. The canonical parser maps the
    // prefix to its stable route id, which `ModelRouteApiMethod::parse` round-
    // trips back to the exact auth method when the spawned session is restored.
    let route_id = jcode_provider_core::AuthRoute::parse_explicit_credential_prefix(prefix)?
        .route_api_method();
    Some(SwarmSpawnSelection {
        model: Some(bare.to_string()),
        provider_key: Some(route_id.to_string()),
        route_api_method: Some(route_id.to_string()),
    })
}

/// True when a model string is one of the "inherit the coordinator" sentinels.
fn is_inherit_sentinel(model: &str) -> bool {
    let trimmed = model.trim();
    trimmed.eq_ignore_ascii_case("inherit") || trimmed.eq_ignore_ascii_case("coordinator")
}

/// Selection that inherits the coordinator's model, provider key, and route.
fn inherit_coordinator_selection(coordinator: &CoordinatorSpawnIdentity) -> SwarmSpawnSelection {
    SwarmSpawnSelection {
        model: coordinator.model.clone(),
        provider_key: coordinator
            .provider_key
            .clone()
            .or_else(|| provider_key_for_spawn_model(coordinator.model.as_deref(), None)),
        route_api_method: coordinator.route_api_method.clone(),
    }
}

/// Selection for a concrete model string (optionally route-prefixed like
/// `openai-api:gpt-5.5`), reconciled against the coordinator's identity.
fn selection_for_concrete_model(
    model: String,
    coordinator: &CoordinatorSpawnIdentity,
) -> SwarmSpawnSelection {
    // A model may pin an explicit provider + auth route via a prefix
    // (e.g. "openai-api:gpt-5.5"). Honor it directly so spawned agents do
    // NOT inherit the coordinator's model/auth and instead use the
    // requested model on the requested API route.
    if let Some(selection) = explicit_route_for_configured_model(&model) {
        return selection;
    }

    // A concrete model only inherits the coordinator's provider_key/route
    // when it targets the same model; otherwise the route would point at
    // the wrong provider/auth mode.
    if coordinator.model.as_deref() == Some(model.as_str()) {
        SwarmSpawnSelection {
            model: Some(model.clone()),
            provider_key: coordinator
                .provider_key
                .clone()
                .or_else(|| provider_key_for_spawn_model(Some(&model), None)),
            route_api_method: coordinator.route_api_method.clone(),
        }
    } else {
        SwarmSpawnSelection {
            provider_key: provider_key_for_spawn_model(Some(&model), None),
            model: Some(model),
            route_api_method: None,
        }
    }
}

fn resolve_swarm_spawn_selection(
    requested_model: Option<String>,
    configured_swarm_model: Option<String>,
    coordinator: &CoordinatorSpawnIdentity,
) -> SwarmSpawnSelection {
    // A per-spawn requested model (the `model` param on `swarm spawn`) takes
    // precedence over the `agents.swarm_model` config pin. An explicit
    // `inherit`/`coordinator` request forces coordinator inheritance even when
    // the config pins a different model.
    let requested_model = requested_model
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty());
    if let Some(requested) = requested_model {
        if is_inherit_sentinel(&requested) {
            return inherit_coordinator_selection(coordinator);
        }
        return selection_for_concrete_model(requested, coordinator);
    }

    // Treat empty strings and the explicit "inherit"/"coordinator" sentinels as
    // "no override": spawned swarm agents should inherit the coordinator's model
    // unless `agents.swarm_model` is deliberately set to a concrete model. This
    // avoids the surprising case where a stale `swarm_model` config pins every
    // spawned agent to an unrelated model/provider.
    let configured_swarm_model = configured_swarm_model
        .filter(|model| !model.trim().is_empty() && !is_inherit_sentinel(model));

    match configured_swarm_model {
        Some(model) => selection_for_concrete_model(model, coordinator),
        None => inherit_coordinator_selection(coordinator),
    }
}

fn persist_headed_startup_message(session_id: &str, message: &str) {
    crate::logging::info(&format!(
        "Headed spawn: persisting startup submission for {session_id} (chars={}) to client-input handoff file",
        message.chars().count(),
    ));
    crate::client_input::save_startup_submission_for_session(
        session_id,
        message.to_string(),
        Vec::new(),
    );
}

fn clear_headed_startup_message(session_id: &str) {
    if let Ok(jcode_dir) = crate::storage::jcode_dir() {
        let path = jcode_dir.join(format!("client-input-{}", session_id));
        let _ = std::fs::remove_file(path);
    }
}

fn cleanup_prepared_visible_spawn_session(session_id: &str) {
    clear_headed_startup_message(session_id);
    if let Ok(path) = crate::session::session_path(session_id) {
        let _ = std::fs::remove_file(path);
    }
    if let Ok(path) = crate::session::session_journal_path(session_id) {
        let _ = std::fs::remove_file(path);
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_visible_spawn_session<F>(
    working_dir: Option<&str>,
    model_override: Option<&str>,
    provider_key_override: Option<&str>,
    route_api_method_override: Option<&str>,
    effort_override: Option<&str>,
    selfdev_requested: bool,
    startup_message: Option<&str>,
    launch_visible: F,
) -> anyhow::Result<(String, bool)>
where
    F: FnOnce(&str, &std::path::Path, bool, Option<&str>) -> anyhow::Result<bool>,
{
    let provider_key = provider_key_for_spawn_model(model_override, provider_key_override);
    let (new_session_id, cwd) = create_visible_spawn_session(
        working_dir,
        model_override,
        provider_key.as_deref(),
        route_api_method_override,
        effort_override,
        selfdev_requested,
    )?;

    if let Some(message) = startup_message {
        persist_headed_startup_message(&new_session_id, message);
    }

    match launch_visible(
        &new_session_id,
        &cwd,
        selfdev_requested,
        provider_key.as_deref(),
    ) {
        Ok(launched) => {
            if !launched {
                cleanup_prepared_visible_spawn_session(&new_session_id);
            }
            Ok((new_session_id, launched))
        }
        Err(error) => {
            cleanup_prepared_visible_spawn_session(&new_session_id);
            Err(error)
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "visible spawn registration updates swarm state, event history, and UI delivery metadata together"
)]
async fn register_visible_spawned_member(
    session_id: &str,
    swarm_id: &str,
    working_dir: Option<&str>,
    has_startup_message: bool,
    report_back_to_session_id: Option<&str>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let now = Instant::now();
    let friendly_name = crate::id::extract_session_name(session_id)
        .map(|name| name.to_string())
        .unwrap_or_else(|| session_id.to_string());
    let (status, detail) = if has_startup_message {
        ("running".to_string(), Some("startup queued".to_string()))
    } else {
        ("spawned".to_string(), Some("launching client".to_string()))
    };

    {
        let mut members = swarm_members.write().await;
        members.insert(
            session_id.to_string(),
            SwarmMember {
                session_id: session_id.to_string(),
                event_tx,
                event_txs: HashMap::new(),
                working_dir: working_dir.map(PathBuf::from),
                swarm_id: Some(swarm_id.to_string()),
                swarm_enabled: true,
                status,
                detail,
                task_label: None,
                friendly_name: Some(friendly_name),
                report_back_to_session_id: report_back_to_session_id.map(str::to_string),
                latest_completion_report: None,
                role: "agent".to_string(),
                joined_at: now,
                last_status_change: now,
                is_headless: false,
                output_tail: None,
                todo_progress: None,
                todo_items: Vec::new(),
                runtime: crate::protocol::SwarmMemberRuntime::default(),
            },
        );
    }

    {
        let mut swarms = swarms_by_id.write().await;
        swarms
            .entry(swarm_id.to_string())
            .or_insert_with(HashSet::new)
            .insert(session_id.to_string());
    }

    record_swarm_event_for_session(
        session_id,
        SwarmEventType::MemberChange {
            action: "joined".to_string(),
        },
        swarm_members,
        event_history,
        event_counter,
        swarm_event_tx,
    )
    .await;
    broadcast_swarm_status(swarm_id, swarm_members, swarms_by_id).await;
}

#[expect(
    clippy::too_many_arguments,
    reason = "server-side swarm spawning needs session, swarm state, provider, and event sinks together"
)]
pub(super) async fn spawn_swarm_agent(
    req_session_id: &str,
    swarm_id: &str,
    working_dir: Option<String>,
    initial_message: Option<String>,
    spawn_mode: Option<SwarmSpawnMode>,
    requested_model: Option<String>,
    requested_effort: Option<String>,
    label: Option<String>,
    sessions: &SessionAgents,
    global_session_id: &Arc<RwLock<String>>,
    provider_template: &Arc<dyn Provider>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    mcp_pool: &Arc<crate::mcp::SharedMcpPool>,
    soft_interrupt_queues: &SessionInterruptQueues,
    client_connections: &ClientConnections,
) -> anyhow::Result<String> {
    let resolved_working_dir =
        resolve_spawn_working_dir(working_dir, req_session_id, sessions, swarm_members).await;
    let coordinator = resolve_coordinator_spawn_identity(req_session_id, sessions).await;
    let coordinator_is_canary = coordinator.is_canary;
    // Capture the requesting client's terminal env so spawn hooks place the new
    // window in the terminal the user is attached to, not the server's stale
    // startup env (#405).
    let client_terminal_env =
        client_terminal_env_for_session(req_session_id, client_connections).await;
    let agents_config = &crate::config::config().agents;
    let configured_swarm_model = agents_config.swarm_model.clone();
    let resolved_spawn_mode = spawn_mode.unwrap_or(agents_config.swarm_spawn_mode);
    let selection = resolve_swarm_spawn_selection(
        requested_model.clone(),
        configured_swarm_model.clone(),
        &coordinator,
    );
    let spawn_model = selection.model.clone();
    let spawn_provider_key = selection.provider_key.clone();
    let spawn_route_api_method = selection.route_api_method.clone();
    let spawn_effort = requested_effort
        .as_deref()
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
        .map(str::to_string);
    crate::logging::info(&format!(
        "Swarm spawn model resolution: requested_model={:?} requested_effort={:?} configured_swarm_model={:?} coordinator_model={:?} coordinator_provider_key={:?} coordinator_route={:?} -> spawn_model={:?} spawn_provider_key={:?} spawn_route={:?}",
        requested_model,
        spawn_effort,
        configured_swarm_model,
        coordinator.model,
        coordinator.provider_key,
        coordinator.route_api_method,
        spawn_model,
        spawn_provider_key,
        spawn_route_api_method,
    ));

    let startup_message = initial_message
        .as_deref()
        .map(append_swarm_completion_report_instructions);

    let visible_spawn = match resolved_spawn_mode {
        // Inline workers run in-process like headless ones; the difference is
        // purely how the coordinator renders them (a live inline gallery).
        SwarmSpawnMode::Headless | SwarmSpawnMode::Inline => {
            Err(anyhow::anyhow!("headless spawn requested"))
        }
        SwarmSpawnMode::Visible | SwarmSpawnMode::Auto => prepare_visible_spawn_session(
            resolved_working_dir.as_deref(),
            spawn_model.as_deref(),
            spawn_provider_key.as_deref(),
            spawn_route_api_method.as_deref(),
            spawn_effort.as_deref(),
            coordinator_is_canary,
            startup_message.as_deref(),
            |session_id, cwd, selfdev_requested, provider_key| {
                // Tag the headed window as a swarm-agent spawn so spawn hooks
                // and terminals can identify and reroute it (JCODE_SPAWN_*).
                let context = crate::session_launch::SessionSpawnContext::kind("swarm-agent")
                    .env("JCODE_SPAWN_SWARM_ID", swarm_id)
                    .env("JCODE_SPAWN_COORDINATOR_SESSION_ID", req_session_id)
                    .with_client_terminal_env(client_terminal_env.clone());
                spawn_visible_session_window_with_context(
                    session_id,
                    cwd,
                    selfdev_requested,
                    provider_key,
                    &context,
                )
            },
        ),
    };

    let (new_session_id, is_headless_fallback) = match visible_spawn {
        Ok((new_session_id, true)) => Ok((new_session_id, false)),
        Ok((_, false)) | Err(_) => {
            let cmd = if let Some(ref dir) = resolved_working_dir {
                format!("create_session:{dir}")
            } else {
                "create_session".to_string()
            };
            create_headless_session(
                sessions,
                global_session_id,
                provider_template,
                &cmd,
                swarm_members,
                swarms_by_id,
                swarm_coordinators,
                swarm_plans,
                soft_interrupt_queues,
                coordinator_is_canary,
                spawn_model.clone(),
                spawn_provider_key.clone(),
                spawn_route_api_method.clone(),
                spawn_effort.clone(),
                Some(Arc::clone(mcp_pool)),
                Some(req_session_id.to_string()),
            )
            .await
            .and_then(|result_json| {
                serde_json::from_str::<serde_json::Value>(&result_json)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("session_id")
                            .and_then(|session_id| session_id.as_str())
                            .map(|session_id| session_id.to_string())
                    })
                    .map(|session_id| (session_id, true))
                    .ok_or_else(|| anyhow::anyhow!("Failed to parse spawned session id"))
            })
        }
    }?;

    let startup_message = startup_message.clone();
    {
        let mut plans = swarm_plans.write().await;
        if let Some(plan) = plans.get_mut(swarm_id)
            && (!plan.items.is_empty() || !plan.participants.is_empty())
        {
            plan.participants.insert(req_session_id.to_string());
            plan.participants.insert(new_session_id.clone());
        }
    }

    broadcast_swarm_plan(
        swarm_id,
        Some("participant_spawned".to_string()),
        swarm_plans,
        swarm_members,
        swarms_by_id,
    )
    .await;
    if !is_headless_fallback {
        register_visible_spawned_member(
            &new_session_id,
            swarm_id,
            resolved_working_dir.as_deref(),
            startup_message.is_some(),
            Some(req_session_id),
            swarm_members,
            swarms_by_id,
            event_history,
            event_counter,
            swarm_event_tx,
        )
        .await;
    }
    // Label the worker with what it was spawned for so the swarm strip and
    // member lists can show the task, not just the animal name. An explicit
    // spawn `label` wins; otherwise the label is derived from the raw prompt
    // (before completion-report boilerplate is appended).
    if let Some(label_text) = label.as_deref().or(initial_message.as_deref()) {
        set_member_task_label(&new_session_id, label_text, swarm_members).await;
    }
    let swarm_state = SwarmState {
        members: Arc::clone(swarm_members),
        swarms_by_id: Arc::clone(swarms_by_id),
        plans: Arc::clone(swarm_plans),
        coordinators: Arc::clone(swarm_coordinators),
    };
    persist_swarm_state_for(swarm_id, &swarm_state).await;

    if let Some(initial_msg) = startup_message
        && is_headless_fallback
    {
        record_swarm_event_for_session(
            &new_session_id,
            SwarmEventType::MemberChange {
                action: "joined".to_string(),
            },
            swarm_members,
            event_history,
            event_counter,
            swarm_event_tx,
        )
        .await;

        let agent_arc = {
            let agent_sessions = sessions.read().await;
            agent_sessions.get(&new_session_id).cloned()
        };
        if let Some(agent_arc) = agent_arc {
            let sid_clone = new_session_id.clone();
            let swarm_members2 = Arc::clone(swarm_members);
            let swarms_by_id2 = Arc::clone(swarms_by_id);
            let event_history2 = Arc::clone(event_history);
            let event_counter2 = Arc::clone(event_counter);
            let swarm_event_tx2 = swarm_event_tx.clone();
            tokio::spawn(async move {
                update_member_status(
                    &sid_clone,
                    "running",
                    Some(truncate_detail(&initial_msg, 120)),
                    &swarm_members2,
                    &swarms_by_id2,
                    Some(&event_history2),
                    Some(&event_counter2),
                    Some(&swarm_event_tx2),
                )
                .await;
                let event_tx = super::session_event_fanout_sender(
                    sid_clone.clone(),
                    Arc::clone(&swarm_members2),
                );
                let start_message_index = {
                    let agent = agent_arc.lock().await;
                    agent.message_count()
                };
                let result = process_message_streaming_mpsc(
                    Arc::clone(&agent_arc),
                    &initial_msg,
                    vec![],
                    None,
                    event_tx,
                )
                .await;
                let completion_report = if result.is_ok() {
                    let agent = agent_arc.lock().await;
                    agent.latest_assistant_text_after(start_message_index)
                } else {
                    None
                };
                let (new_status, new_detail) = match result {
                    Ok(()) => ("ready", None),
                    Err(ref error) => ("failed", Some(truncate_detail(&error.to_string(), 120))),
                };
                update_member_status_with_report(
                    &sid_clone,
                    new_status,
                    new_detail,
                    completion_report,
                    &swarm_members2,
                    &swarms_by_id2,
                    Some(&event_history2),
                    Some(&event_counter2),
                    Some(&swarm_event_tx2),
                )
                .await;
            });
        }
    }

    Ok(new_session_id)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_comm_spawn(
    id: u64,
    req_session_id: String,
    working_dir: Option<String>,
    initial_message: Option<String>,
    request_nonce: Option<String>,
    spawn_mode: Option<SwarmSpawnMode>,
    model: Option<String>,
    effort: Option<String>,
    label: Option<String>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    sessions: &SessionAgents,
    global_session_id: &Arc<RwLock<String>>,
    provider_template: &Arc<dyn Provider>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    _channel_subscriptions: &ChannelSubscriptions,
    _channel_subscriptions_by_session: &ChannelSubscriptions,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    mcp_pool: &Arc<crate::mcp::SharedMcpPool>,
    soft_interrupt_queues: &SessionInterruptQueues,
    swarm_mutation_runtime: &SwarmMutationRuntime,
    client_connections: &ClientConnections,
) {
    // Hold this swarm's admission through member registration so concurrent
    // recursive requests cannot all pass the population check against stale
    // state. Unrelated swarms retain independent spawn throughput.
    let admission_key = swarm_members
        .read()
        .await
        .get(&req_session_id)
        .and_then(|member| member.swarm_id.clone())
        .unwrap_or_else(|| req_session_id.clone());
    let admission_lock = spawn_admission_lock(&admission_key);
    let admission_guard = admission_lock.lock().await;
    let swarm_id = match ensure_spawn_coordinator_swarm(
        id,
        &req_session_id,
        client_event_tx,
        swarm_members,
        swarms_by_id,
        swarm_coordinators,
        swarm_plans,
        crate::config::config().agents.swarm_max_concurrent_agents,
    )
    .await
    {
        Some(swarm_id) => swarm_id,
        None => return,
    };

    let mutation_key = request_key(
        &req_session_id,
        "spawn",
        &[
            swarm_id.clone(),
            working_dir.clone().unwrap_or_default(),
            initial_message.clone().unwrap_or_default(),
            request_nonce.clone().unwrap_or_default(),
            spawn_mode
                .map(|mode| format!("{mode:?}"))
                .unwrap_or_default(),
            model.clone().unwrap_or_default(),
            effort.clone().unwrap_or_default(),
            label.clone().unwrap_or_default(),
        ],
    );
    let Some(mutation_state) = begin_or_replay(
        swarm_mutation_runtime,
        &mutation_key,
        "spawn",
        &req_session_id,
        id,
        client_event_tx,
    )
    .await
    else {
        return;
    };

    let response = match spawn_swarm_agent(
        &req_session_id,
        &swarm_id,
        working_dir,
        initial_message,
        spawn_mode,
        model,
        effort,
        label,
        sessions,
        global_session_id,
        provider_template,
        swarm_members,
        swarms_by_id,
        swarm_coordinators,
        swarm_plans,
        event_history,
        event_counter,
        swarm_event_tx,
        mcp_pool,
        soft_interrupt_queues,
        client_connections,
    )
    .await
    {
        Ok(new_session_id) => PersistedSwarmMutationResponse::Spawn { new_session_id },
        Err(error) => PersistedSwarmMutationResponse::Error {
            message: format!("Failed to spawn agent: {error}"),
            retry_after_secs: None,
        },
    };

    // The new member is registered (or spawning failed), so the next admission
    // check can safely observe the updated population.
    drop(admission_guard);

    finish_request(swarm_mutation_runtime, &mutation_state, response).await;
}

/// Handle `comm_list_models`: report the model routes available for spawning
/// swarm agents, plus the requester's current model (the spawn default) and
/// any `agents.swarm_model` config pin. Read-only, so it needs no coordinator
/// check or mutation dedup. Uses the requester's live agent catalog when its
/// lock is free, otherwise falls back to the provider template's catalog.
pub(super) async fn handle_comm_list_models(
    id: u64,
    req_session_id: &str,
    sessions: &SessionAgents,
    provider_template: &Arc<dyn Provider>,
    send_event: impl FnOnce(ServerEvent),
) {
    let coordinator = resolve_coordinator_spawn_identity(req_session_id, sessions).await;

    let agent = {
        let agent_sessions = sessions.read().await;
        agent_sessions.get(req_session_id).cloned()
    };
    let model_routes = match agent.as_ref().and_then(|agent| agent.try_lock().ok()) {
        Some(agent_guard) => agent_guard.model_routes(),
        // Agent busy (mid-turn, the common case for tool calls) or not
        // resident: the provider template exposes the same route catalog.
        None => provider_template.model_routes(),
    };

    send_event(ServerEvent::CommListModelsResponse {
        id,
        current_model: coordinator.model,
        configured_swarm_model: crate::config::config().agents.swarm_model.clone(),
        model_routes,
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_comm_stop(
    id: u64,
    req_session_id: String,
    target_session: String,
    force: bool,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    sessions: &SessionAgents,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    soft_interrupt_queues: &SessionInterruptQueues,
    swarm_mutation_runtime: &SwarmMutationRuntime,
) {
    // Stopping is authorized per-target by ownership (the requester is the
    // target's spawner or a transitive ancestor) rather than by the swarm-level
    // coordinator slot, so that any parent can stop agents in its own subtree.
    // We only require the requester to be a member of a swarm here; the concrete
    // permission check happens below via `stop_allowed`.
    let swarm_id = {
        let members = swarm_members.read().await;
        members
            .get(&req_session_id)
            .and_then(|member| member.swarm_id.clone())
    };
    let Some(swarm_id) = swarm_id else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm.".to_string(),
            retry_after_secs: None,
        });
        return;
    };

    let target_session =
        match resolve_stop_target_session(&swarm_id, &target_session, swarm_members).await {
            Ok(target_session) => target_session,
            Err(message) => {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message,
                    retry_after_secs: None,
                });
                return;
            }
        };

    let stop_allowed = {
        let members = swarm_members.read().await;
        members
            .get(&target_session)
            .map(|member| {
                swarm_stop_allowed_by_owner(&req_session_id, member, force)
                    || (!force
                        && super::swarm_is_self_or_ancestor(
                            &members,
                            &req_session_id,
                            &target_session,
                        ))
            })
            .unwrap_or(false)
    };
    if !stop_allowed {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: format!(
                "Refusing to stop session '{target_session}' because it was not spawned by this coordinator. Pass force=true to stop a non-owned/user-created swarm session explicitly."
            ),
            retry_after_secs: None,
        });
        return;
    }

    let _ = fanout_session_event(
        swarm_members,
        &target_session,
        ServerEvent::SessionCloseRequested {
            reason: format!("Stopped by coordinator {req_session_id}"),
        },
    )
    .await;

    let mutation_key = request_key(&req_session_id, "stop", &[swarm_id, target_session.clone()]);
    let Some(mutation_state) = begin_or_replay(
        swarm_mutation_runtime,
        &mutation_key,
        "stop",
        &req_session_id,
        id,
        client_event_tx,
    )
    .await
    else {
        return;
    };

    let removed_agent = super::remove_session_entry(sessions, &target_session).await;
    let removed_live_agent = removed_agent.is_some();
    if let Some(agent_arc) = removed_agent {
        remove_session_interrupt_queue(soft_interrupt_queues, &target_session).await;
        remove_background_tool_signal(&target_session);
        if let Ok(mut agent) = agent_arc.try_lock() {
            agent.mark_closed();
            let memory_enabled = agent.memory_enabled();
            let transcript = if memory_enabled {
                Some(agent.build_transcript_for_extraction())
            } else {
                None
            };
            let sid = target_session.clone();
            let working_dir = agent.working_dir().map(|dir| dir.to_string());
            drop(agent);
            if let Some(transcript) = transcript {
                crate::memory_agent::trigger_final_extraction_with_dir(
                    transcript,
                    sid,
                    working_dir,
                );
            }
        }
    }

    let (removed_swarm_id, removed_name) = {
        let mut members = swarm_members.write().await;
        if let Some(member) = members.remove(&target_session) {
            (member.swarm_id, member.friendly_name)
        } else {
            (None, None)
        }
    };
    if let Some(ref swarm_id) = removed_swarm_id {
        record_swarm_event(
            event_history,
            event_counter,
            swarm_event_tx,
            target_session.clone(),
            removed_name.clone(),
            Some(swarm_id.clone()),
            SwarmEventType::MemberChange {
                action: "left".to_string(),
            },
        )
        .await;
        remove_session_from_swarm(
            &target_session,
            swarm_id,
            swarm_members,
            swarms_by_id,
            swarm_coordinators,
            swarm_plans,
        )
        .await;
    }
    remove_session_channel_subscriptions(
        &target_session,
        channel_subscriptions,
        channel_subscriptions_by_session,
    )
    .await;

    let response = if removed_live_agent || removed_swarm_id.is_some() {
        PersistedSwarmMutationResponse::Done
    } else {
        PersistedSwarmMutationResponse::Error {
            message: format!("Unknown session '{target_session}'"),
            retry_after_secs: None,
        }
    };
    finish_request(swarm_mutation_runtime, &mutation_state, response).await;
}

fn swarm_stop_allowed_by_owner(
    req_session_id: &str,
    target_member: &SwarmMember,
    force: bool,
) -> bool {
    force || target_member.report_back_to_session_id.as_deref() == Some(req_session_id)
}

async fn resolve_stop_target_session(
    swarm_id: &str,
    target: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> std::result::Result<String, String> {
    let target = target.trim();
    if target.is_empty() {
        return Err("target_session is required.".to_string());
    }

    let members = swarm_members.read().await;
    if members
        .get(target)
        .is_some_and(|member| member.swarm_id.as_deref() == Some(swarm_id))
    {
        return Ok(target.to_string());
    }

    let mut matches = members
        .iter()
        .filter(|(_, member)| member.swarm_id.as_deref() == Some(swarm_id))
        .filter(|(session_id, member)| {
            member.friendly_name.as_deref() == Some(target)
                || session_id.starts_with(target)
                || session_id.ends_with(target)
        })
        .map(|(session_id, member)| {
            (
                session_id.clone(),
                member
                    .friendly_name
                    .as_deref()
                    .unwrap_or(session_id)
                    .to_string(),
            )
        })
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| a.0.cmp(&b.0));

    match matches.len() {
        0 => Err(format!(
            "Unknown swarm session '{target}'. Use an exact session ID, unique friendly name, or unique session ID prefix/suffix."
        )),
        1 => Ok(matches.remove(0).0),
        _ => Err(format!(
            "Ambiguous swarm session '{target}' matched: {}. Use an exact session ID.",
            matches
                .iter()
                .map(|(session_id, friendly)| format!("{friendly} [{session_id}]"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn swarm_member_status_is_stale_for_coordination(status: &str) -> bool {
    matches!(
        status,
        "crashed" | "failed" | "stopped" | "closed" | "disconnected"
    )
}

async fn ensure_spawn_coordinator_swarm(
    id: u64,
    req_session_id: &str,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    configured_live_agent_limit: usize,
) -> Option<String> {
    let (
        swarm_id,
        from_name,
        is_root,
        coordinator_id,
        coordinator_is_stale,
        live_member_count,
        live_spawned_agent_count,
    ) = {
        let members = swarm_members.read().await;
        let swarm_id = members
            .get(req_session_id)
            .and_then(|member| member.swarm_id.clone());
        let from_name = members
            .get(req_session_id)
            .and_then(|member| member.friendly_name.clone());
        // A session is a "root" when it has no spawner/owner above it.
        let is_root = members
            .get(req_session_id)
            .and_then(|member| member.report_back_to_session_id.clone())
            .is_none();
        // Count both all live members for the absolute hard cap and live spawned
        // agents for the configurable RAM-safety cap. User-created roots do not
        // consume worker slots; every recursively spawned descendant does.
        let (live_member_count, live_spawned_agent_count) = swarm_id
            .as_ref()
            .map(|swarm_id| {
                members
                    .values()
                    .filter(|member| member.swarm_id.as_deref() == Some(swarm_id.as_str()))
                    .filter(|member| super::member_consumes_swarm_capacity(member))
                    .fold((0usize, 0usize), |(members, spawned), member| {
                        (
                            members + 1,
                            spawned + usize::from(member.report_back_to_session_id.is_some()),
                        )
                    })
            })
            .unwrap_or_default();
        let coordinator_id = if let Some(ref swarm_id) = swarm_id {
            let coordinators = swarm_coordinators.read().await;
            coordinators.get(swarm_id).cloned()
        } else {
            None
        };
        let coordinator_is_stale = coordinator_id.as_ref().is_some_and(|coordinator| {
            !members.get(coordinator).is_some_and(|member| {
                // A coordinator is stale for slot-reclaim purposes when it left
                // the swarm, reached a terminal status, or can no longer be
                // reached at all (every event channel closed). The last case
                // catches a wedged coordinator whose client died without a
                // clean status transition; without it the slot stays blocked
                // until the status sweep happens to notice.
                let unreachable = member.event_tx.is_closed()
                    && member.event_txs.values().all(|tx| tx.is_closed());
                member.swarm_id.as_deref() == swarm_id.as_deref()
                    && !swarm_member_status_is_stale_for_coordination(&member.status)
                    && !unreachable
            })
        });
        (
            swarm_id,
            from_name,
            is_root,
            coordinator_id,
            coordinator_is_stale,
            live_member_count,
            live_spawned_agent_count,
        )
    };

    let Some(swarm_id) = swarm_id else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm.".to_string(),
            retry_after_secs: None,
        });
        return None;
    };

    // Keep an absolute hard ceiling even when the configurable limit is disabled.
    if live_member_count >= super::MAX_SWARM_MEMBERS {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: format!(
                "Swarm member limit reached (hard max {}). This swarm already has {live_member_count} live members; it cannot spawn more. Let existing agents finish and free up capacity, or narrow the task decomposition before spawning further.",
                super::MAX_SWARM_MEMBERS
            ),
            retry_after_secs: None,
        });
        return None;
    }

    // `swarm_max_concurrent_agents` is the machine-safety budget shared by
    // run_plan and ad hoc recursive spawning. Previously only run_plan obeyed it,
    // so nested agents could grow to the 1000-member hard cap and exhaust RAM.
    let live_agent_limit = (configured_live_agent_limit > 0)
        .then(|| configured_live_agent_limit.min(super::MAX_SWARM_MEMBERS));
    if live_agent_limit.is_some_and(|limit| live_spawned_agent_count >= limit) {
        let limit = live_agent_limit.unwrap_or(super::MAX_SWARM_MEMBERS);
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: format!(
                "Swarm live-agent limit reached (max {limit}, configured by agents.swarm_max_concurrent_agents). This swarm already has {live_spawned_agent_count} active spawned agents. Let existing agents finish or stop them before spawning more."
            ),
            retry_after_secs: None,
        });
        return None;
    }

    // Coordinator-slot election is now only about the swarm-level coordinator used
    // for shared plan operations (propose/approve/assign). Only a root session
    // (depth 0, no spawner) claims it, and only when the slot is empty or stale.
    // Non-root spawners coordinate their own subtree via report-back ownership and
    // never disturb the swarm-level coordinator slot. Crucially, the presence of a
    // live coordinator no longer blocks anyone from spawning.
    if is_root && coordinator_id.as_deref() != Some(req_session_id) {
        let should_claim = coordinator_id.is_none() || coordinator_is_stale;
        if should_claim {
            let promoted = {
                let mut coordinators = swarm_coordinators.write().await;
                match coordinators.get(&swarm_id) {
                    Some(existing) if existing == req_session_id => false,
                    Some(_) if !coordinator_is_stale => false,
                    _ => {
                        coordinators.insert(swarm_id.clone(), req_session_id.to_string());
                        true
                    }
                }
            };

            if promoted {
                {
                    let mut members = swarm_members.write().await;
                    if let Some(member) = members.get_mut(req_session_id) {
                        member.role = "coordinator".to_string();
                    }
                }
                let swarm_state = SwarmState {
                    members: Arc::clone(swarm_members),
                    swarms_by_id: Arc::clone(swarms_by_id),
                    plans: Arc::clone(swarm_plans),
                    coordinators: Arc::clone(swarm_coordinators),
                };
                persist_swarm_state_for(&swarm_id, &swarm_state).await;
                broadcast_swarm_status(&swarm_id, swarm_members, swarms_by_id).await;
                let _ = client_event_tx.send(ServerEvent::Notification {
                    from_session: req_session_id.to_string(),
                    from_name,
                    notification_type: NotificationType::Message {
                        scope: Some("swarm".to_string()),
                        channel: None,
                        tldr: None,
                    },
                    message: "You are the coordinator for this swarm.".to_string(),
                });
            }
        }
    }

    Some(swarm_id)
}

#[cfg(test)]
#[path = "comm_session_tests.rs"]
mod comm_session_tests;
