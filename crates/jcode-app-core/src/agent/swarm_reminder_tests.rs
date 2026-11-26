use super::*;
use async_trait::async_trait;

#[derive(Clone)]
struct EffortProvider {
    effort: Option<String>,
}

#[async_trait]
impl Provider for EffortProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        anyhow::bail!("not used")
    }
    fn name(&self) -> &str {
        "test"
    }
    fn model(&self) -> String {
        "test-model".into()
    }
    fn reasoning_effort(&self) -> Option<String> {
        self.effort.clone()
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

async fn last_user_text(effort: Option<&str>, message: &str) -> String {
    let provider: Arc<dyn Provider> = Arc::new(EffortProvider {
        effort: effort.map(str::to_string),
    });
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);
    agent
        .append_user_context_message(message, Vec::new())
        .expect("append user message");
    agent
        .messages()
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .and_then(|m| {
            m.content.iter().find_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
        })
        .expect("user text")
}

#[tokio::test]
async fn jarvis_delegation_policy_only_changes_the_attached_root() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let provider: Arc<dyn Provider> = Arc::new(EffortProvider { effort: None });
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);
    let id = agent.session_id().to_owned();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let _guard = crate::tool::sdk::ConnectionGuard(id.clone());
    let swarm = |tools: Vec<ToolDefinition>| {
        tools
            .into_iter()
            .find(|tool| tool.name == "swarm")
            .unwrap()
            .description
    };
    let baseline = swarm(agent.tool_definitions_for_debug().await);
    assert!(baseline.contains("keep working yourself"));
    assert!(
        !agent
            .build_system_prompt_split(None)
            .static_part
            .contains("## Jarvis root coordination")
    );
    crate::tool::sdk::configure(
        &id,
        &id,
        crate::protocol::SessionToolConfig {
            system_prompt_append: Some("## Jarvis voice interaction\nSpeak briefly.".into()),
            custom: vec![crate::protocol::SessionToolDefinition {
                name: "speak".into(),
                description: "Speak".into(),
                parameters: serde_json::json!({"type":"object"}),
            }],
            ..Default::default()
        },
        tx,
    )
    .unwrap();
    let root = agent.build_system_prompt_split(None).static_part;
    for rule in [
        "only one sequential task",
        "explicitly requested workspace script directly",
        "Never pass a raw transcript to the shell",
        "do not also call `swarm spawn`",
        "do not dispatch workers",
        "Never delegate around an approval",
        "You own the result",
    ] {
        assert!(root.contains(rule), "missing {rule}");
    }
    let description = swarm(agent.tool_definitions_for_debug().await);
    assert!(description.contains("single-lane work"));
    assert!(!description.contains("keep working yourself"));
    agent.session.parent_id = Some("coordinator".into());
    assert_eq!(swarm(agent.tool_definitions_for_debug().await), baseline);
    assert!(
        !agent
            .build_system_prompt_split(None)
            .static_part
            .contains("## Jarvis root coordination")
    );
}

#[tokio::test]
async fn swarm_effort_appends_reminder_to_user_message() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let text = last_user_text(Some("swarm"), "fix the parser").await;
    assert_eq!(
        text,
        "fix the parser\n\nUse swarms to get things done in parallel."
    );
}

#[tokio::test]
async fn non_swarm_effort_leaves_user_message_unchanged() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    for effort in [None, Some("high"), Some("swarm-deep")] {
        assert_eq!(
            last_user_text(effort, "fix the parser").await,
            "fix the parser"
        );
    }
}

#[tokio::test]
async fn swarm_effort_does_not_touch_system_prompt() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let provider: Arc<dyn Provider> = Arc::new(EffortProvider {
        effort: Some("swarm".into()),
    });
    let registry = Registry::new(provider.clone()).await;
    let agent = Agent::new(provider, registry);
    let split = agent.build_system_prompt_split(None);
    let full = format!("{}{}", split.static_part, split.dynamic_part);
    assert!(!full.contains("Use swarms to get things done in parallel."));
    assert!(!full.contains("# Swarm Effort"));
    assert!(!full.contains("Deep Task Graph"));
}

#[tokio::test]
async fn run_once_path_also_appends_reminder() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let provider: Arc<dyn Provider> = Arc::new(EffortProvider {
        effort: Some("swarm".into()),
    });
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);
    // The provider errors on complete; we only care about the stored message.
    let _ = agent.run_once_capture("ship it").await;
    let text = agent
        .messages()
        .iter()
        .filter(|m| m.role == Role::User)
        .flat_map(|m| m.content.iter())
        .find_map(|block| match block {
            ContentBlock::Text { text, .. } if text.starts_with("ship it") => Some(text.clone()),
            _ => None,
        })
        .expect("user text");
    assert_eq!(
        text,
        "ship it\n\nUse swarms to get things done in parallel."
    );
}

#[derive(Clone)]
struct MutableEffortProvider {
    effort: Arc<StdMutex<Option<String>>>,
    reject_none: bool,
}

impl MutableEffortProvider {
    fn new(initial: Option<&str>, reject_none: bool) -> Self {
        Self {
            effort: Arc::new(StdMutex::new(initial.map(str::to_string))),
            reject_none,
        }
    }
}

#[async_trait]
impl Provider for MutableEffortProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        anyhow::bail!("not used")
    }
    fn name(&self) -> &str {
        "test"
    }
    fn model(&self) -> String {
        "test-model".into()
    }
    fn reasoning_effort(&self) -> Option<String> {
        self.effort.lock().unwrap().clone()
    }
    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        if self.reject_none && effort == "none" {
            anyhow::bail!("none unsupported");
        }
        *self.effort.lock().unwrap() = Some(effort.to_string());
        Ok(())
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn jarvis_tool_config() -> crate::protocol::SessionToolConfig {
    crate::protocol::SessionToolConfig {
        system_prompt_append: Some("## Jarvis voice interaction\nSpeak briefly.".into()),
        custom: vec![crate::protocol::SessionToolDefinition {
            name: "speak".into(),
            description: "Speak".into(),
            parameters: serde_json::json!({"type":"object"}),
        }],
        ..Default::default()
    }
}

#[cfg(unix)]
#[tokio::test]
async fn jarvis_workspace_script_uses_session_directory_and_respects_disabled_bash() {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let workspace = sandbox.external_dir();
    std::fs::write(
        workspace.join("check.sh"),
        "echo jarvis-fixture-ok > result\n",
    )
    .unwrap();
    let provider: Arc<dyn Provider> = Arc::new(EffortProvider { effort: None });
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);
    agent.set_working_dir(workspace.to_str().unwrap());
    let id = agent.session_id().to_owned();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let _guard = crate::tool::sdk::ConnectionGuard(id.clone());
    let mut config = jarvis_tool_config();
    crate::tool::sdk::configure(&id, &id, config.clone(), tx.clone()).unwrap();

    let args = serde_json::json!({"command":"sh ./check.sh","intent":"Run the workspace fixture"});
    let result = workspace.join("result");
    agent.execute_tool("bash", args.clone()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(&result).unwrap(),
        "jarvis-fixture-ok\n"
    );

    std::fs::remove_file(&result).unwrap();
    config.disabled.push("bash".into());
    crate::tool::sdk::configure(&id, &id, config, tx).unwrap();
    assert!(
        agent
            .execute_tool("bash", args)
            .await
            .unwrap_err()
            .to_string()
            .contains("disabled")
    );
    assert!(!result.exists());
}

#[tokio::test]
async fn jarvis_root_defaults_reasoning_effort_to_none_and_allows_user_override() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let provider: Arc<dyn Provider> = Arc::new(MutableEffortProvider::new(Some("high"), false));
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider.clone(), registry);
    let id = agent.session_id().to_owned();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let _guard = crate::tool::sdk::ConnectionGuard(id.clone());

    assert_eq!(agent.provider_reasoning_effort().as_deref(), Some("high"));

    crate::tool::sdk::configure(&id, &id, jarvis_tool_config(), tx).unwrap();
    let applied = agent.apply_jarvis_default_reasoning_effort();
    assert_eq!(applied, Some(Some("none".to_string())));
    assert_eq!(agent.provider_reasoning_effort().as_deref(), Some("none"));
    assert_eq!(agent.session.reasoning_effort.as_deref(), Some("none"));

    // Explicit user override is preserved even if Jarvis config is re-applied.
    agent.set_reasoning_effort("high").unwrap();
    assert_eq!(agent.provider_reasoning_effort().as_deref(), Some("high"));
    assert_eq!(agent.apply_jarvis_default_reasoning_effort(), None);
    assert_eq!(agent.provider_reasoning_effort().as_deref(), Some("high"));
}

#[tokio::test]
async fn jarvis_default_reasoning_effort_respects_persisted_session_and_non_root() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let provider: Arc<dyn Provider> = Arc::new(MutableEffortProvider::new(Some("medium"), false));
    let registry = Registry::new(provider.clone()).await;
    let mut session = Session::create(None, None);
    session.reasoning_effort = Some("high".to_string());
    let mut agent = Agent::new_with_session(provider.clone(), registry.clone(), session, None);
    let id = agent.session_id().to_owned();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let _guard = crate::tool::sdk::ConnectionGuard(id.clone());

    crate::tool::sdk::configure(&id, &id, jarvis_tool_config(), tx.clone()).unwrap();
    assert_eq!(agent.apply_jarvis_default_reasoning_effort(), None);
    assert_eq!(agent.provider_reasoning_effort().as_deref(), Some("high"));

    // Non-root (child) session is not defaulted to none.
    let child_provider: Arc<dyn Provider> =
        Arc::new(MutableEffortProvider::new(Some("medium"), false));
    let mut child = Agent::new(child_provider, registry.clone());
    child.session.parent_id = Some("parent".into());
    let child_id = child.session_id().to_owned();
    let _child_guard = crate::tool::sdk::ConnectionGuard(child_id.clone());
    crate::tool::sdk::configure(&child_id, &child_id, jarvis_tool_config(), tx.clone()).unwrap();
    assert_eq!(child.apply_jarvis_default_reasoning_effort(), None);
    assert_eq!(child.provider_reasoning_effort().as_deref(), Some("medium"));

    // Provider that rejects "none" fails gracefully without erroring.
    let rejecting_provider: Arc<dyn Provider> =
        Arc::new(MutableEffortProvider::new(Some("medium"), true));
    let mut rejecting = Agent::new(rejecting_provider, registry);
    let rej_id = rejecting.session_id().to_owned();
    let _rej_guard = crate::tool::sdk::ConnectionGuard(rej_id.clone());
    crate::tool::sdk::configure(&rej_id, &rej_id, jarvis_tool_config(), tx).unwrap();
    assert_eq!(rejecting.apply_jarvis_default_reasoning_effort(), None);
    assert_eq!(
        rejecting.provider_reasoning_effort().as_deref(),
        Some("medium")
    );
}
