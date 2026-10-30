//! `user_prompt_submit`: a synchronous hook that sees each new user message.
//!
//! The prompt reaches the hook on stdin only, never in env. Plain stdout or
//! `hookSpecificOutput.additionalContext` becomes hidden context after the
//! message, `decision: "block"`, exit 2 or `continue: false` rejects the
//! message, and every failure fails open.

use super::origin::PromptOrigin;
use super::{HookEvent, Verdict, common_payload, hook_commands, interpret, jcode_object};
use serde_json::{Value, json};
use std::time::Duration;

/// Default and maximum wait per command.
const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const MAX_TIMEOUT_MS: u64 = 30_000;

/// What the agent is about to submit.
pub struct PromptSubmit<'a> {
    pub session_id: &'a str,
    pub cwd: Option<&'a str>,
    pub prompt: &'a str,
    pub origin: PromptOrigin,
    pub active_skill: Option<&'a str>,
    /// Entrypoint: `chat`, `run`, `capture`.
    pub source: &'a str,
}

/// Result of running the configured commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptSubmitOutcome {
    Allow {
        /// Hidden context to add after the user message.
        context: Option<String>,
        /// Notices for the user (`systemMessage`).
        notices: Vec<String>,
    },
    Block {
        reason: String,
    },
}

/// Build the stdin document for the hook.
pub fn prompt_payload(input: &PromptSubmit<'_>) -> Value {
    let mut payload = common_payload(
        "UserPromptSubmit",
        "user_prompt_submit",
        input.session_id,
        input.cwd,
    );
    payload.insert("prompt".into(), input.prompt.into());
    payload.insert("origin".into(), input.origin.turn_origin_json());
    if let Some(identity) = super::swarm_identity(input.session_id) {
        payload.insert("agent_id".into(), input.session_id.into());
        if let Some(agent_type) = identity.agent_type.as_ref().or(identity.label.as_ref()) {
            payload.insert("agent_type".into(), agent_type.as_str().into());
        }
        jcode_object(&mut payload).insert("swarm".into(), identity.to_json());
    }
    let jcode = jcode_object(&mut payload);
    jcode.insert("origin_detail".into(), input.origin.as_str().into());
    jcode.insert("active_skill".into(), json!(input.active_skill));
    jcode.insert("source".into(), input.source.into());
    Value::Object(payload)
}

/// Fold per-command verdicts into one outcome. The first block or halt wins.
pub fn combine(verdicts: &[Verdict]) -> PromptSubmitOutcome {
    let mut contexts = Vec::new();
    let mut notices = Vec::new();
    for verdict in verdicts {
        if let Some(reason) = verdict.block.as_ref().or(verdict.halt.as_ref()) {
            return PromptSubmitOutcome::Block {
                reason: reason.clone(),
            };
        }
        contexts.extend(
            verdict
                .additional_context
                .iter()
                .chain(verdict.plain_stdout.iter())
                .cloned(),
        );
        notices.extend(verdict.system_message.iter().cloned());
    }
    PromptSubmitOutcome::Allow {
        context: (!contexts.is_empty()).then(|| contexts.join("\n\n")),
        notices,
    }
}

fn timeout() -> Duration {
    let configured = crate::config::config().hooks.user_prompt_submit_timeout_ms;
    let ms = if configured == 0 {
        DEFAULT_TIMEOUT_MS
    } else {
        configured.min(MAX_TIMEOUT_MS)
    };
    Duration::from_millis(ms)
}

/// Run the configured `user_prompt_submit` commands. Returns `None` when none
/// are configured, so the hot path costs one lookup.
pub async fn run_user_prompt_submit(input: &PromptSubmit<'_>) -> Option<PromptSubmitOutcome> {
    let commands = hook_commands("user_prompt_submit");
    if commands.is_empty() {
        return None;
    }
    let stdin = prompt_payload(input).to_string();
    let mut event = HookEvent::new("user_prompt_submit")
        .session_id(input.session_id)
        .field("PROMPT_ORIGIN", input.origin.as_str())
        .field("SOURCE", input.source);
    if let Some(skill) = input.active_skill {
        event = event.field("ACTIVE_SKILL", skill);
    }
    if let Some(cwd) = input.cwd {
        event = event.cwd(cwd);
    }
    let results = super::run_json_commands(&commands, &event, &stdin, timeout(), |result| {
        let verdict = interpret(result, "user_prompt_submit");
        verdict.block.is_some() || verdict.halt.is_some()
    })
    .await;
    let verdicts: Vec<Verdict> = results
        .iter()
        .map(|result| interpret(result, "user_prompt_submit"))
        .collect();
    Some(combine(&verdicts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(f: impl FnOnce(&mut Verdict)) -> Verdict {
        let mut v = Verdict::default();
        f(&mut v);
        v
    }

    #[test]
    fn payload_has_prompt_origin_and_jcode_detail() {
        let payload = prompt_payload(&PromptSubmit {
            session_id: "s1",
            cwd: Some("/w"),
            prompt: "do it",
            origin: PromptOrigin::SwarmSpawn,
            active_skill: Some("aidlc"),
            source: "chat",
        });
        assert_eq!(payload["hook_event_name"], "UserPromptSubmit");
        assert_eq!(payload["prompt"], "do it");
        assert_eq!(payload["origin"]["kind"], "host");
        assert_eq!(payload["jcode"]["origin_detail"], "swarm_spawn");
        assert_eq!(payload["jcode"]["active_skill"], "aidlc");
        assert_eq!(payload["jcode"]["source"], "chat");
    }

    #[test]
    fn person_origin_for_user() {
        let payload = prompt_payload(&PromptSubmit {
            session_id: "s1",
            cwd: None,
            prompt: "hi",
            origin: PromptOrigin::User,
            active_skill: None,
            source: "run",
        });
        assert_eq!(payload["origin"], json!({"kind": "person"}));
        assert!(payload["jcode"]["active_skill"].is_null());
    }

    #[test]
    fn combine_joins_context_and_first_block_wins() {
        let outcome = combine(&[
            verdict(|v| v.plain_stdout = Some("a".into())),
            verdict(|v| v.additional_context = Some("b".into())),
        ]);
        assert_eq!(
            outcome,
            PromptSubmitOutcome::Allow {
                context: Some("a\n\nb".into()),
                notices: vec![]
            }
        );
        let outcome = combine(&[
            verdict(|v| v.plain_stdout = Some("a".into())),
            verdict(|v| v.halt = Some("stop".into())),
            verdict(|v| v.block = Some("later".into())),
        ]);
        assert_eq!(
            outcome,
            PromptSubmitOutcome::Block {
                reason: "stop".into()
            }
        );
    }
}
