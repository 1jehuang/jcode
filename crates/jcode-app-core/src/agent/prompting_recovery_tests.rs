use super::Agent;
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use crate::provider::{EventStream, Provider};
use crate::tool::Registry;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

const CUSTOM_PROMPT: &str = "  Custom instructions, unchanged.\n";
const RECOVERY_REMINDER: &str = "Resume from the last saved step after provider overload.";

#[derive(Clone, Default)]
struct PromptCaptureProvider {
    requests: Arc<Mutex<Vec<(String, String)>>>,
}

#[async_trait]
impl Provider for PromptCaptureProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        anyhow::bail!("agent must use the split request boundary")
    }

    async fn complete_split(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        system_static: &str,
        system_dynamic: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        self.requests
            .lock()
            .unwrap()
            .push((system_static.into(), system_dynamic.into()));
        Ok(Box::pin(futures::stream::iter([
            Ok(StreamEvent::TextDelta("continued".into())),
            Ok(StreamEvent::MessageEnd {
                stop_reason: Some("end_turn".into()),
            }),
        ])))
    }

    fn name(&self) -> &str {
        "custom-prompt-recovery-test"
    }

    fn supports_compaction(&self) -> bool {
        false
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn custom_prompt_agent(provider: Arc<PromptCaptureProvider>) -> Agent {
    let mut agent = Agent::new(provider, Registry::empty());
    agent.session.system_prompt = Some(CUSTOM_PROMPT.into());
    agent.memory_enabled = false;
    agent
}

#[tokio::test]
async fn custom_prompt_recovery_reminder_reaches_both_agent_request_paths() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let provider = Arc::new(PromptCaptureProvider::default());
    for streaming in [false, true] {
        let mut agent = custom_prompt_agent(provider.clone());
        let reminder = Some(format!("  {RECOVERY_REMINDER}\n"));
        // Recovery resumes saved history without supplying another user instruction.
        agent.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "Finish the saved task".into(),
                cache_control: None,
            }],
        );
        agent.current_turn_system_reminder = reminder;
        if streaming {
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            agent.run_turn_streaming_mpsc(tx).await.unwrap();
        } else {
            agent.run_turn(false).await.unwrap();
        }
    }
    assert_eq!(
        *provider.requests.lock().unwrap(),
        vec![
            (
                CUSTOM_PROMPT.into(),
                format!("# System Reminder\n\n{RECOVERY_REMINDER}"),
            );
            2
        ]
    );
}

#[test]
fn custom_prompt_without_meaningful_reminder_keeps_dynamic_section_empty() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let mut agent = custom_prompt_agent(Arc::new(PromptCaptureProvider::default()));
    let mut prompts = Vec::new();
    for reminder in [None, Some(""), Some(" \n\t ")] {
        agent.current_turn_system_reminder = reminder.map(str::to_owned);
        let split = agent.build_system_prompt_split(Some("ignored memory prompt"));
        prompts.push((split.static_part, split.dynamic_part));
    }
    assert_eq!(prompts, vec![(CUSTOM_PROMPT.into(), String::new()); 3]);
}

#[test]
fn changing_recovery_reminders_never_pollutes_custom_static_prompt() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let mut agent = custom_prompt_agent(Arc::new(PromptCaptureProvider::default()));
    let mut prompts = Vec::new();
    for reminder in [
        Some(RECOVERY_REMINDER),
        Some("Resume the next saved step."),
        None,
    ] {
        agent.current_turn_system_reminder = reminder.map(str::to_owned);
        let split = agent.build_system_prompt_split(None);
        prompts.push((split.static_part, split.dynamic_part));
    }
    assert_eq!(
        (prompts, agent.session.system_prompt.clone()),
        (
            vec![
                (
                    CUSTOM_PROMPT.into(),
                    format!("# System Reminder\n\n{RECOVERY_REMINDER}"),
                ),
                (
                    CUSTOM_PROMPT.into(),
                    "# System Reminder\n\nResume the next saved step.".into(),
                ),
                (CUSTOM_PROMPT.into(), String::new()),
            ],
            Some(CUSTOM_PROMPT.into()),
        )
    );
}
