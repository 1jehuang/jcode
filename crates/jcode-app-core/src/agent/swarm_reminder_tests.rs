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
