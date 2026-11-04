use crate::message::{Message, ToolDefinition};
use crate::provider::{EventStream, Provider};
use crate::server::comm_session::spawn_swarm_agent;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use tokio::sync::{RwLock, broadcast};

#[derive(Default)]
struct QuietProvider {
    model: std::sync::Mutex<String>,
    effort: std::sync::Mutex<Option<String>>,
}

#[async_trait]
impl Provider for QuietProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        anyhow::bail!("agent definition spawn test must not make model calls")
    }
    fn name(&self) -> &str {
        "mock"
    }
    fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }
    fn set_model(&self, model: &str) -> Result<()> {
        *self.model.lock().unwrap() = model
            .rsplit_once(':')
            .map(|(_, model)| model)
            .unwrap_or(model)
            .to_string();
        Ok(())
    }
    fn reasoning_effort(&self) -> Option<String> {
        self.effort.lock().unwrap().clone()
    }
    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        *self.effort.lock().unwrap() = Some(effort.to_string());
        Ok(())
    }
    fn available_efforts(&self) -> Vec<&'static str> {
        vec!["none", "minimal", "low", "medium", "high", "xhigh", "max"]
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            model: std::sync::Mutex::new(self.model()),
            effort: std::sync::Mutex::new(self.reasoning_effort()),
        })
    }
}

#[tokio::test]
async fn spawn_with_agent_delivers_persona_first_and_sets_agent_type() {
    let _env = crate::storage::lock_test_env();
    let home = tempfile::TempDir::new().unwrap();
    let project = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    std::fs::create_dir_all(project.path().join(".claude/agents")).unwrap();
    std::fs::write(
        project.path().join(".claude/agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\n---\nYou are a strict reviewer.\n",
    )
    .unwrap();
    let previous = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
        HashMap::new(),
    ));
    let provider: Arc<dyn Provider> = Arc::new(QuietProvider::default());
    let state = crate::server::SwarmState::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
    );
    let mcp = Arc::new(crate::mcp::SharedMcpPool::new(
        crate::mcp::McpConfig::default(),
    ));
    let cwd = project.path().to_string_lossy().into_owned();
    let spawn = |agent: Option<&str>| {
        let agent = agent.map(str::to_string);
        let cwd = cwd.clone();
        let (sessions, provider, state, mcp) = (&sessions, &provider, &state, &mcp);
        async move {
            spawn_swarm_agent(
                "coord",
                "agent-def-swarm",
                None,
                Some(cwd),
                None,
                Some(crate::config::SwarmSpawnMode::Headless),
                None,
                None,
                Some("review".into()),
                agent,
                sessions,
                &Arc::new(RwLock::new(String::new())),
                provider,
                &state.members,
                &state.swarms_by_id,
                &state.coordinators,
                &state.plans,
                &Arc::new(RwLock::new(VecDeque::new())),
                &Arc::new(AtomicU64::new(0)),
                &broadcast::channel(16).0,
                mcp,
                &Arc::new(RwLock::new(HashMap::new())),
                &Arc::new(RwLock::new(HashMap::new())),
            )
            .await
        }
    };

    let unknown = spawn(Some("nobody")).await.unwrap_err().to_string();
    let child_id = spawn(Some("reviewer")).await.unwrap();
    let plain_id = spawn(None).await.unwrap();

    let child = sessions.read().await.get(&child_id).cloned().unwrap();
    let (persona_index, first_persona, total) = {
        let child = child.lock().await;
        let texts: Vec<String> = child
            .messages()
            .iter()
            .map(|message| {
                message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        crate::message::ContentBlock::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<String>()
            })
            .collect();
        let index = texts
            .iter()
            .position(|text| text.contains("# Agent persona: reviewer"));
        let personas = texts
            .iter()
            .filter(|t| t.contains("# Agent persona"))
            .count();
        (
            index,
            texts.get(index.unwrap_or(0)).cloned().unwrap_or_default(),
            (texts.len(), personas),
        )
    };
    let first = first_persona;
    let identity = crate::hooks::swarm_identity(&child_id);
    let plain_identity = crate::hooks::swarm_identity(&plain_id);
    let plain = sessions.read().await.get(&plain_id).cloned().unwrap();
    let plain_has_persona = plain
        .lock()
        .await
        .messages()
        .iter()
        .any(|m| format!("{:?}", m.content).contains("Agent persona"));
    crate::hooks::forget_swarm_identity(&child_id);
    crate::hooks::forget_swarm_identity(&plain_id);
    match previous {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }

    assert!(unknown.contains("Available: reviewer"), "{unknown}");
    let persona_index = persona_index.expect("persona message present");
    assert!(
        persona_index <= 1,
        "persona must open the transcript, found at {persona_index}"
    );
    assert_eq!(total.1, 1, "persona delivered exactly once");
    assert!(first.contains("# Agent persona: reviewer"), "{first}");
    assert!(first.contains("You are a strict reviewer."));
    assert_eq!(identity.unwrap().agent_type.as_deref(), Some("reviewer"));
    assert_eq!(plain_identity.unwrap().agent_type, None);
    assert!(!plain_has_persona);
}

struct PolicyFx {
    sessions: Arc<RwLock<HashMap<String, Arc<tokio::sync::Mutex<crate::agent::Agent>>>>>,
    provider: Arc<dyn Provider>,
    state: crate::server::SwarmState,
    mcp: Arc<crate::mcp::SharedMcpPool>,
    cwd: String,
}

impl PolicyFx {
    fn new(cwd: String) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            provider: Arc::new(QuietProvider::default()),
            state: crate::server::SwarmState::new(
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
            ),
            mcp: Arc::new(crate::mcp::SharedMcpPool::new(
                crate::mcp::McpConfig::default(),
            )),
            cwd,
        }
    }

    async fn spawn(&self, agent: &str) -> String {
        spawn_swarm_agent(
            "coord",
            "policy-swarm",
            None,
            Some(self.cwd.clone()),
            None,
            Some(crate::config::SwarmSpawnMode::Headless),
            None,
            None,
            Some("policy".into()),
            Some(agent.to_string()),
            &self.sessions,
            &Arc::new(RwLock::new(String::new())),
            &self.provider,
            &self.state.members,
            &self.state.swarms_by_id,
            &self.state.coordinators,
            &self.state.plans,
            &Arc::new(RwLock::new(VecDeque::new())),
            &Arc::new(AtomicU64::new(0)),
            &broadcast::channel(16).0,
            &self.mcp,
            &Arc::new(RwLock::new(HashMap::new())),
            &Arc::new(RwLock::new(HashMap::new())),
        )
        .await
        .unwrap()
    }

    async fn worker_state(&self, id: &str) -> (String, Option<String>, Vec<String>) {
        let agent = self.sessions.read().await.get(id).cloned().unwrap();
        let agent = agent.lock().await;
        let mut tools = agent.tool_names().await;
        tools.sort();
        (
            agent.provider_model(),
            crate::session_effort::session_effort(id),
            tools,
        )
    }
}

fn write_definition(project: &std::path::Path, dir: &str, text: &str) {
    let path = project.join(dir).join("policy-agent.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[tokio::test]
async fn definition_model_effort_and_tools_apply_to_a_user_level_worker() {
    let _env = crate::storage::lock_test_env();
    let home = tempfile::TempDir::new().unwrap();
    let project = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    let previous = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());
    // A user-level definition needs no trust.
    write_definition(
        home.path(),
        "agents",
        "---\nname: policy-agent\ndescription: d\nmodel: gpt-5.5\neffort: high\ntools: [read, bash]\ndisallowedTools: Bash\n---\nbody\n",
    );
    let fx = PolicyFx::new(project.path().to_string_lossy().into_owned());
    let id = fx.spawn("policy-agent").await;
    let (model, effort, tools) = fx.worker_state(&id).await;
    crate::hooks::forget_swarm_identity(&id);
    match previous {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    assert_eq!(model, "gpt-5.5");
    assert_eq!(effort.as_deref(), Some("high"));
    assert!(tools.iter().all(|t| t == "read"), "tools: {tools:?}");
    assert!(tools.contains(&"read".to_string()), "tools: {tools:?}");
}

#[tokio::test]
async fn project_definition_policy_is_ignored_until_trusted() {
    let _env = crate::storage::lock_test_env();
    let home = tempfile::TempDir::new().unwrap();
    let project = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    let previous = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());
    write_definition(
        project.path(),
        ".claude/agents",
        "---\nname: policy-agent\ndescription: d\nmodel: gpt-5.5\neffort: max\ntools: [read]\n---\nbody\n",
    );
    let fx = PolicyFx::new(project.path().to_string_lossy().into_owned());
    let untrusted = fx.spawn("policy-agent").await;
    let before = fx.worker_state(&untrusted).await;

    let defs = crate::agent_def::project_policy_definitions(Some(project.path()));
    let refs: Vec<_> = defs.iter().collect();
    crate::project_trust::grant(
        project.path(),
        crate::project_trust::Scope::Agents,
        &crate::agent_def::agents_digest(&refs),
    )
    .unwrap();
    let trusted = fx.spawn("policy-agent").await;
    let after = fx.worker_state(&trusted).await;
    crate::hooks::forget_swarm_identity(&untrusted);
    crate::hooks::forget_swarm_identity(&trusted);
    match previous {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    assert_ne!(
        before.0, "gpt-5.5",
        "untrusted project policy must not set the model"
    );
    assert_eq!(after.0, "gpt-5.5");
    assert_eq!(after.1.as_deref(), Some("max"));
    assert!(after.2.iter().all(|t| t == "read"), "{:?}", after.2);
    assert!(
        before.2.len() > after.2.len(),
        "untrusted keeps the full tool set"
    );
}

#[tokio::test]
async fn force_swarm_defaults_makes_operator_defaults_win_over_a_definition() {
    let _env = crate::storage::lock_test_env();
    let home = tempfile::TempDir::new().unwrap();
    let project = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    let previous = std::env::var_os("JCODE_HOME");
    let previous_force = std::env::var_os("JCODE_SWARM_FORCE_DEFAULTS");
    crate::env::set_var("JCODE_HOME", home.path());
    write_definition(
        home.path(),
        "agents",
        "---\nname: policy-agent\ndescription: d\nmodel: gpt-5.5\neffort: max\ntools: [read]\n---\nbody\n",
    );
    crate::env::set_var("JCODE_SWARM_FORCE_DEFAULTS", "1");
    crate::config::invalidate_config_cache();
    let fx = PolicyFx::new(project.path().to_string_lossy().into_owned());
    let id = fx.spawn("policy-agent").await;
    let state = fx.worker_state(&id).await;
    crate::hooks::forget_swarm_identity(&id);
    match previous_force {
        Some(value) => crate::env::set_var("JCODE_SWARM_FORCE_DEFAULTS", value),
        None => crate::env::remove_var("JCODE_SWARM_FORCE_DEFAULTS"),
    }
    match previous {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    crate::config::invalidate_config_cache();
    assert_ne!(state.0, "gpt-5.5");
    assert_ne!(state.1.as_deref(), Some("max"));
    assert!(
        state.2.len() > 1,
        "force must leave the full tool set: {:?}",
        state.2
    );
}

#[tokio::test]
async fn definition_comes_from_the_coordinator_dir_not_the_worker_dir() {
    let _env = crate::storage::lock_test_env();
    let home = tempfile::TempDir::new().unwrap();
    let coordinator_project = tempfile::TempDir::new().unwrap();
    let other_checkout = tempfile::TempDir::new().unwrap();
    for dir in [coordinator_project.path(), other_checkout.path()] {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
    }
    std::fs::create_dir_all(coordinator_project.path().join(".claude/agents")).unwrap();
    std::fs::write(
        coordinator_project
            .path()
            .join(".claude/agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\n---\nCOORDINATOR_PERSONA\n",
    )
    .unwrap();
    let previous = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
        HashMap::new(),
    ));
    let provider: Arc<dyn Provider> = Arc::new(QuietProvider::default());
    let state = crate::server::SwarmState::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
    );
    let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
    state.members.write().await.insert(
        "coord".to_string(),
        crate::server::SwarmMember {
            session_id: "coord".to_string(),
            event_tx,
            event_txs: HashMap::new(),
            working_dir: Some(coordinator_project.path().to_path_buf()),
            swarm_id: Some("agent-def-swarm".to_string()),
            swarm_enabled: true,
            status: "ready".to_string(),
            detail: None,
            friendly_name: Some("coord".to_string()),
            report_back_to_session_id: None,
            latest_completion_report: None,
            role: "coordinator".to_string(),
            joined_at: std::time::Instant::now(),
            last_status_change: std::time::Instant::now(),
            is_headless: false,
            output_tail: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: crate::protocol::SwarmMemberRuntime::default(),
            task_label: None,
        },
    );
    let mcp = Arc::new(crate::mcp::SharedMcpPool::new(
        crate::mcp::McpConfig::default(),
    ));
    let worker_dir = other_checkout.path().to_string_lossy().into_owned();
    let child_id = spawn_swarm_agent(
        "coord",
        "agent-def-swarm",
        None,
        Some(worker_dir),
        None,
        Some(crate::config::SwarmSpawnMode::Headless),
        None,
        None,
        Some("review".into()),
        Some("reviewer".into()),
        &sessions,
        &Arc::new(RwLock::new(String::new())),
        &provider,
        &state.members,
        &state.swarms_by_id,
        &state.coordinators,
        &state.plans,
        &Arc::new(RwLock::new(VecDeque::new())),
        &Arc::new(AtomicU64::new(0)),
        &broadcast::channel(16).0,
        &mcp,
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
    )
    .await;
    let persona_text = match &child_id {
        Ok(id) => {
            let child = sessions.read().await.get(id).cloned().unwrap();
            let child = child.lock().await;
            format!("{:?}", child.messages())
        }
        Err(_) => String::new(),
    };
    if let Ok(id) = &child_id {
        crate::hooks::forget_swarm_identity(id);
    }
    match previous {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    assert!(
        child_id.is_ok(),
        "the coordinator's definition must resolve: {child_id:?}"
    );
    assert!(
        persona_text.contains("COORDINATOR_PERSONA"),
        "{persona_text}"
    );
}

/// Point `event_env` at a script that appends `$JCODE_HOOK_*` fields to a
/// record file, returning the file. Callers restore the variable with
/// [`unset_recording_hook`].
#[cfg(unix)]
fn set_recording_hook(
    dir: &std::path::Path,
    event_env: &str,
    body: &str,
) -> (std::path::PathBuf, Option<std::ffi::OsString>) {
    use std::os::unix::fs::PermissionsExt;
    let record = dir.join(format!("{event_env}.txt"));
    let script = dir.join(format!("{event_env}.sh"));
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' \"{body}\" >> {}\n",
            crate::terminal_launch::sh_escape(&record.to_string_lossy())
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let previous = std::env::var_os(event_env);
    crate::env::set_var(event_env, script.to_string_lossy().to_string());
    let event = event_env
        .trim_start_matches("JCODE_HOOK_")
        .to_ascii_lowercase();
    wait_for_hook_config(&event, true);
    (record, previous)
}

#[cfg(unix)]
fn unset_recording_hook(event_env: &str, previous: Option<std::ffi::OsString>) {
    match previous {
        Some(value) => crate::env::set_var(event_env, value),
        None => crate::env::remove_var(event_env),
    }
    let event = event_env
        .trim_start_matches("JCODE_HOOK_")
        .to_ascii_lowercase();
    wait_for_hook_config(&event, false);
}

/// The hook config cache re-checks the environment on a throttle.
#[cfg(unix)]
fn wait_for_hook_config(event: &str, configured: bool) {
    for _ in 0..200 {
        if crate::hooks::hook_configured(event) == configured {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[cfg(unix)]
fn read_record(record: &std::path::Path) -> String {
    for _ in 0..100 {
        if let Ok(data) = std::fs::read_to_string(record)
            && !data.is_empty()
        {
            return data;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    String::new()
}

#[cfg(unix)]
#[tokio::test]
async fn headless_session_start_hook_sees_worker_identity() {
    let _env = crate::storage::lock_test_env();
    let home = tempfile::TempDir::new().unwrap();
    let project = tempfile::TempDir::new().unwrap();
    let hooks_dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    std::fs::create_dir_all(project.path().join(".claude/agents")).unwrap();
    std::fs::write(
        project.path().join(".claude/agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\n---\nBe strict.\n",
    )
    .unwrap();
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());
    let (record, previous_hook) = set_recording_hook(
        hooks_dir.path(),
        "JCODE_HOOK_SESSION_START",
        "$JCODE_HOOK_SESSION_ID|$JCODE_HOOK_AGENT_TYPE|$JCODE_HOOK_SWARM_ID|$JCODE_HOOK_COORDINATOR_SESSION_ID",
    );

    let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
        HashMap::new(),
    ));
    let provider: Arc<dyn Provider> = Arc::new(QuietProvider::default());
    let state = crate::server::SwarmState::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
    );
    let mcp = Arc::new(crate::mcp::SharedMcpPool::new(
        crate::mcp::McpConfig::default(),
    ));
    let child_id = spawn_swarm_agent(
        "coord",
        "start-hook-swarm",
        None,
        Some(project.path().to_string_lossy().into_owned()),
        None,
        Some(crate::config::SwarmSpawnMode::Headless),
        None,
        None,
        Some("review".into()),
        Some("reviewer".into()),
        &sessions,
        &Arc::new(RwLock::new(String::new())),
        &provider,
        &state.members,
        &state.swarms_by_id,
        &state.coordinators,
        &state.plans,
        &Arc::new(RwLock::new(VecDeque::new())),
        &Arc::new(AtomicU64::new(0)),
        &broadcast::channel(16).0,
        &mcp,
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
    )
    .await;
    let recorded = read_record(&record);
    unset_recording_hook("JCODE_HOOK_SESSION_START", previous_hook);
    match previous_home {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    let child_id = child_id.unwrap();
    crate::hooks::forget_swarm_identity(&child_id);
    let first = recorded.lines().next().unwrap_or_default();
    assert_eq!(
        first,
        format!("{child_id}|reviewer|start-hook-swarm|coord"),
        "{recorded}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn stopping_a_running_worker_fires_swarm_member_end() {
    let _env = crate::storage::lock_test_env();
    let hooks_dir = tempfile::TempDir::new().unwrap();
    let coord_dir = tempfile::TempDir::new().unwrap();
    let state = crate::server::SwarmState::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
    );
    let member = |id: &str, role: &str, status: &str, dir: Option<&std::path::Path>| {
        let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
        crate::server::SwarmMember {
            session_id: id.to_string(),
            event_tx,
            event_txs: HashMap::new(),
            working_dir: dir.map(std::path::Path::to_path_buf),
            swarm_id: Some("stop-hook-swarm".to_string()),
            swarm_enabled: true,
            status: status.to_string(),
            detail: None,
            friendly_name: Some(id.to_string()),
            report_back_to_session_id: (role == "agent").then(|| "coord".to_string()),
            latest_completion_report: None,
            role: role.to_string(),
            joined_at: std::time::Instant::now(),
            last_status_change: std::time::Instant::now(),
            is_headless: true,
            output_tail: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: crate::protocol::SwarmMemberRuntime::default(),
            task_label: None,
        }
    };
    {
        let mut members = state.members.write().await;
        members.insert(
            "coord".into(),
            member("coord", "coordinator", "ready", Some(coord_dir.path())),
        );
        members.insert(
            "stop-hook-worker".into(),
            member("stop-hook-worker", "agent", "running", None),
        );
    }
    crate::hooks::register_swarm_identity(
        "stop-hook-worker",
        crate::hooks::SwarmIdentity {
            swarm_id: "stop-hook-swarm".into(),
            coordinator_session_id: "coord".into(),
            parent_session_id: "coord".into(),
            label: Some("task".into()),
            agent_type: Some("reviewer".into()),
        },
    );
    let (record, previous_hook) = set_recording_hook(
        hooks_dir.path(),
        "JCODE_HOOK_SWARM_MEMBER_END",
        "$JCODE_HOOK_AGENT_ID|$JCODE_HOOK_STATUS|$JCODE_HOOK_AGENT_TYPE|$(pwd -P)",
    );

    let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
        HashMap::new(),
    ));
    let (client_tx, _client_rx) = tokio::sync::mpsc::unbounded_channel();
    let subscriptions = Arc::new(RwLock::new(HashMap::new()));
    let by_session = Arc::new(RwLock::new(HashMap::new()));
    let interrupts = Arc::new(RwLock::new(HashMap::new()));
    crate::server::comm_session::handle_comm_stop(
        1,
        "coord".to_string(),
        "stop-hook-worker".to_string(),
        false,
        &client_tx,
        &sessions,
        &state.members,
        &state.swarms_by_id,
        &state.coordinators,
        &state.plans,
        &subscriptions,
        &by_session,
        &Arc::new(RwLock::new(VecDeque::new())),
        &Arc::new(AtomicU64::new(0)),
        &broadcast::channel(16).0,
        &interrupts,
        &crate::server::SwarmMutationRuntime::default(),
    )
    .await;
    let recorded = read_record(&record);
    unset_recording_hook("JCODE_HOOK_SWARM_MEMBER_END", previous_hook);
    crate::hooks::forget_swarm_identity("stop-hook-worker");
    let expected_dir = std::fs::canonicalize(coord_dir.path()).unwrap();
    assert_eq!(
        recorded.lines().next().unwrap_or_default(),
        format!("stop-hook-worker|stopped|reviewer|{}", expected_dir.display()),
        "{recorded}"
    );
}
