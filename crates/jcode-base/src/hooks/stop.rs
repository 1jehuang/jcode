//! `stop`: a blocking hook that can keep a turn going.
//!
//! Fires when the model ends a turn with no tool calls. A command that blocks
//! (`decision: "block"` with `reason`, exit 2, or `additionalContext`) makes
//! the turn continue in place: the text becomes a hidden reminder and the
//! model answers again, so `turn_end` still fires once and
//! `user_prompt_submit` never fires for the continuation. `continue: false`
//! ends the turn and wins. A consecutive-continuation cap, reset whenever the
//! model calls a tool, guarantees termination.

use super::{
    HookEvent, Verdict, common_payload, current_prompt_origin, hook_commands, interpret,
    jcode_object, swarm_identity, truncate_marked,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

const DEFAULT_TIMEOUT_MS: u64 = 10_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
/// Chars of the final assistant message passed to the hook.
const LAST_MESSAGE_LIMIT: usize = 10_000;

static CONTINUATIONS: LazyLock<Mutex<HashMap<String, u32>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Consecutive stop-hook continuations in `session_id`.
pub fn stop_continuations(session_id: &str) -> u32 {
    CONTINUATIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_id)
        .copied()
        .unwrap_or(0)
}

/// The model called a tool (or a new turn began): the streak is over.
pub fn reset_stop_continuations(session_id: &str) {
    let mut map = CONTINUATIONS.lock().unwrap_or_else(|e| e.into_inner());
    if map.contains_key(session_id) {
        map.remove(session_id);
    }
}

fn bump(session_id: &str) {
    *CONTINUATIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(session_id.to_owned())
        .or_insert(0) += 1;
}

/// What the agent should do after the stop hook ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopOutcome {
    /// End the turn.
    Stop,
    /// Keep going: add `reminder` as hidden context and ask the model again.
    Continue { reminder: String },
    /// A command asked to continue but the cap was reached: end the turn.
    CapReached { max: u32 },
}

/// Fold per-command verdicts. The first command with a decision wins.
pub fn decide(verdicts: &[Verdict]) -> Option<Result<String, ()>> {
    for verdict in verdicts {
        if verdict.halt.is_some() {
            return Some(Err(()));
        }
        if let Some(reason) = verdict
            .block
            .as_ref()
            .or(verdict.additional_context.as_ref())
        {
            return Some(Ok(reason.clone()));
        }
    }
    None
}

/// Inputs describing the turn that is about to end.
pub struct StopInput<'a> {
    pub session_id: &'a str,
    pub cwd: Option<&'a str>,
    pub last_assistant_message: &'a str,
}

/// Build the stdin document.
pub fn stop_payload(input: &StopInput<'_>, continuations: u32) -> Value {
    let mut payload = common_payload("Stop", "stop", input.session_id, input.cwd);
    payload.insert("stop_hook_active".into(), (continuations > 0).into());
    payload.insert(
        "last_assistant_message".into(),
        truncate_marked(input.last_assistant_message, LAST_MESSAGE_LIMIT).into(),
    );
    let swarm = swarm_identity(input.session_id);
    if let Some(identity) = &swarm {
        payload.insert("agent_id".into(), input.session_id.into());
        if let Some(agent_type) = identity.agent_type.as_ref().or(identity.label.as_ref()) {
            payload.insert("agent_type".into(), agent_type.as_str().into());
        }
    }
    let origin = current_prompt_origin(input.session_id).map(|o| o.as_str());
    let jcode = jcode_object(&mut payload);
    jcode.insert("continuations".into(), continuations.into());
    jcode.insert("prompt_origin".into(), json!(origin));
    jcode.insert("swarm".into(), swarm.map_or(Value::Null, |s| s.to_json()));
    Value::Object(payload)
}

fn timeout() -> Duration {
    let configured = crate::config::config().hooks.stop_timeout_ms;
    let ms = if configured == 0 {
        DEFAULT_TIMEOUT_MS
    } else {
        configured.min(MAX_TIMEOUT_MS)
    };
    Duration::from_millis(ms)
}

/// Run the `stop` hook. `None` means no hook is configured. Counts a
/// continuation when one is granted.
pub async fn run_stop_hook(input: &StopInput<'_>) -> Option<StopOutcome> {
    let commands = hook_commands("stop");
    if commands.is_empty() {
        return None;
    }
    let continuations = stop_continuations(input.session_id);
    let max = crate::config::config().hooks.stop_max_continuations;
    let stdin = stop_payload(input, continuations).to_string();
    let mut event = HookEvent::new("stop")
        .session_id(input.session_id)
        .field("STOP_HOOK_ACTIVE", (continuations > 0).to_string())
        .field("CONTINUATIONS", continuations.to_string());
    if let Some(cwd) = input.cwd {
        event = event.cwd(cwd);
    }
    let results = super::run_json_commands(&commands, &event, &stdin, timeout(), |result| {
        let verdict = interpret(result, "stop");
        verdict.halt.is_some() || verdict.block.is_some() || verdict.additional_context.is_some()
    })
    .await;
    let verdicts: Vec<Verdict> = results.iter().map(|r| interpret(r, "stop")).collect();
    Some(match decide(&verdicts) {
        Some(Ok(reason)) => {
            if continuations >= max {
                StopOutcome::CapReached { max }
            } else {
                bump(input.session_id);
                StopOutcome::Continue { reminder: reason }
            }
        }
        Some(Err(())) | None => StopOutcome::Stop,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(f: impl FnOnce(&mut Verdict)) -> Verdict {
        let mut v = Verdict::default();
        f(&mut v);
        v
    }

    #[test]
    fn first_decision_wins_and_halt_ends() {
        assert_eq!(decide(&[]), None);
        assert_eq!(decide(&[v(|x| x.plain_stdout = Some("hi".into()))]), None);
        assert_eq!(
            decide(&[
                v(|x| x.block = Some("more".into())),
                v(|x| x.halt = Some("h".into()))
            ]),
            Some(Ok("more".into()))
        );
        assert_eq!(
            decide(&[
                v(|x| x.halt = Some("h".into())),
                v(|x| x.block = Some("m".into()))
            ]),
            Some(Err(()))
        );
        assert_eq!(
            decide(&[v(|x| x.additional_context = Some("ctx".into()))]),
            Some(Ok("ctx".into()))
        );
        // halt and block in one command: halt wins.
        assert_eq!(
            decide(&[v(|x| {
                x.block = Some("b".into());
                x.halt = Some("h".into());
            })]),
            Some(Err(()))
        );
    }

    #[test]
    fn counter_bumps_and_resets() {
        let id = "stop-counter-test";
        assert_eq!(stop_continuations(id), 0);
        bump(id);
        bump(id);
        assert_eq!(stop_continuations(id), 2);
        reset_stop_continuations(id);
        assert_eq!(stop_continuations(id), 0);
    }

    #[test]
    fn payload_marks_active_after_first_continuation() {
        let input = StopInput {
            session_id: "stop-payload-test",
            cwd: Some("/w"),
            last_assistant_message: "done",
        };
        let first = stop_payload(&input, 0);
        assert_eq!(first["hook_event_name"], "Stop");
        assert_eq!(first["stop_hook_active"], false);
        assert_eq!(first["last_assistant_message"], "done");
        let second = stop_payload(&input, 2);
        assert_eq!(second["stop_hook_active"], true);
        assert_eq!(second["jcode"]["continuations"], 2);
    }
}
