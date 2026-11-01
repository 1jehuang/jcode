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

    let sessions: Arc<RwLock<HashMap<String, Arc<tokio::sync::Mutex<crate::agent::Agent>>>>> =
        Arc::new(RwLock::new(HashMap::new()));
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

    let sessions: Arc<RwLock<HashMap<String, Arc<tokio::sync::Mutex<crate::agent::Agent>>>>> =
        Arc::new(RwLock::new(HashMap::new()));
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
