use super::Agent;
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use crate::provider::{EventStream, Provider};
use crate::tool::Registry;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

const CUSTOM_PROMPT: &str = "  Custom instructions, unchanged.\n";
const RECOVERY_REMINDER: &str = "Resume from the last saved step after provider overload.";

#[test]
fn profile_is_additive_to_normal_override_and_own_provider_prompts() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    for own_system_prompt in [false, true] {
        for override_prompt in [None, Some(""), Some(CUSTOM_PROMPT)] {
            let provider = Arc::new(PromptCaptureProvider {
                own_system_prompt,
                ..Default::default()
            });
            let mut agent = Agent::new(provider, Registry::empty());
            agent.session.system_prompt = override_prompt.map(str::to_owned);
            let before = agent.build_system_prompt_split(None);
            agent.session.agent_profile = Some(crate::agent_profile::AgentProfile {
                name: "writer".into(),
                instructions: "PROFILE_SENTINEL\n".into(),
            });
            let after = agent.build_system_prompt_split(None);
            let separator = if before.static_part.is_empty() {
                ""
            } else {
                "\n\n"
            };
            assert_eq!(
                after.static_part,
                format!("{}{separator}PROFILE_SENTINEL\n", before.static_part)
            );
            assert_eq!(after.dynamic_part, before.dynamic_part);
            assert_eq!(agent.session.system_prompt.as_deref(), override_prompt);
            agent.session.agent_profile = None;
            assert_eq!(
                agent.build_system_prompt_split(None).static_part,
                before.static_part
            );
        }
    }
}

#[derive(Clone, Default)]
struct PromptCaptureProvider {
    requests: Arc<Mutex<Vec<(String, String)>>>,
    own_system_prompt: bool,
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

    fn uses_own_system_prompt(&self) -> bool {
        self.own_system_prompt
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
        if streaming {
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            agent
                .resume_turn_streaming_mpsc(reminder, tx)
                .await
                .unwrap();
        } else {
            agent.current_turn_system_reminder = reminder;
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

#[test]
fn own_prompt_provider_gets_only_working_dir_and_custom_prompt() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let provider = PromptCaptureProvider {
        own_system_prompt: true,
        ..Default::default()
    };
    let mut agent = Agent::new(Arc::new(provider), Registry::empty());
    agent.session.working_dir = Some("/work/project".into());
    let context = "# Session Context\nWorking directory: /work/project";
    assert_eq!(agent.build_system_prompt_split(None).static_part, context);
    agent.session.system_prompt = Some(CUSTOM_PROMPT.into());
    assert_eq!(
        agent.build_system_prompt_split(None).static_part,
        format!("{context}\n\n{CUSTOM_PROMPT}")
    );
}
