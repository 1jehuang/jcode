//! Session turn orchestration, including retained-child history reconciliation.

use std::path::PathBuf;

use anyhow::Result;
use jcode_message_types::{Message, StreamEvent, ToolDefinition};
use tokio::sync::mpsc;

use super::{ClaudeCodeProvider, ClaudeCodeRateLimits};
use crate::history::SeenHistory;
use crate::prompt;
use crate::turn::{self, TurnEnd, TurnRequest};

impl ClaudeCodeProvider {
    pub(super) async fn run_turn(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        cwd: Option<PathBuf>,
        resume_hint: Option<String>,
        append_system_prompt: String,
        out: mpsc::Sender<Result<StreamEvent>>,
    ) {
        let _turn_guard = self.turn_lock.lock().await;
        let mut results = self.result_rx.lock().await;
        let instance = self.active_instance();
        let login_hint = self.login_hint(&instance);
        let rate_limits = self.rate_limits.clone();
        let on_rate_limits = move |limits: ClaudeCodeRateLimits| {
            *rate_limits.lock().unwrap_or_else(|p| p.into_inner()) = Some(limits);
        };

        let missing_start = {
            let mut state = self.state.lock().await;
            match state
                .history
                .as_ref()
                .map(|history| history.missing_start(&messages))
            {
                Some(Ok(start)) => start,
                Some(Err(())) => {
                    // Resuming this CLI transcript would retain an edited or
                    // compacted-away branch. A fresh session gets current history.
                    if let Some(child) = state.child.take() {
                        child.shutdown().await;
                    }
                    state.resume_id = None;
                    state.seeded = false;
                    state.hint_spent = true;
                    state.history = None;
                    0
                }
                None => 0,
            }
        };

        // One automatic restart when the child died before producing output.
        for attempt in 0..2 {
            let mut state = self.state.lock().await;
            let child = match self
                .ensure_child(
                    &mut state,
                    &tools,
                    cwd.clone(),
                    resume_hint.as_deref(),
                    &append_system_prompt,
                )
                .await
            {
                Ok(child) => child,
                Err(err) => {
                    let _ = out
                        .send(Ok(StreamEvent::Error {
                            message: format!("{err:#}"),
                            retry_after_secs: None,
                        }))
                        .await;
                    return;
                }
            };
            if attempt == 0 {
                let _ = out
                    .send(Ok(StreamEvent::ConnectionType {
                        connection: "claude code cli (stream-json)".into(),
                    }))
                    .await;
                let _ = out
                    .send(Ok(StreamEvent::SessionId(child.session_id().to_string())))
                    .await;
            }

            // Split only the unseen suffix. Consecutive user messages from a
            // previous request must not be resent as new input. Fresh children
            // instead seed from the entire (bounded) conversation transcript.
            let start = if state.seeded { missing_start } else { 0 };
            let input = prompt::turn_input(&messages[start..]);
            let transcript = if state.seeded && state.history.is_none() {
                // A persisted resume hint has no local checkpoint yet. Preserve
                // the native resume contract for that first request.
                None
            } else {
                prompt::transcript(&messages[start..start + input.history_len])
            };
            let turn_uuid = uuid::Uuid::new_v4().to_string();
            let user_line = prompt::user_line(&input, transcript.as_deref(), &turn_uuid);
            let permission_mode = self.settings().permission_mode;

            let outcome = turn::drive_turn(TurnRequest {
                child: &child,
                user_line,
                turn_uuid,
                tools: &tools,
                permission_mode: &permission_mode,
                login_hint: login_hint.clone(),
                out: &out,
                results: &mut results,
                on_rate_limits: &on_rate_limits,
            })
            .await;

            state.seeded = true;
            state.history = Some(SeenHistory::new(&messages, outcome.native_history));
            if let Some(session_id) = outcome.session_id {
                state.resume_id = Some(session_id);
            }
            if !outcome.child_usable
                && let Some(dead) = state.child.take()
            {
                state.resume_id = Some(dead.session_id().to_string());
                dead.shutdown().await;
            }
            match outcome.end {
                TurnEnd::Finished | TurnEnd::Cancelled | TurnEnd::RateLimited => return,
                TurnEnd::Exited(tail) => {
                    if attempt == 0 && !outcome.produced_output {
                        jcode_base::logging::warn(&format!(
                            "Claude Code exited mid-turn, restarting with --resume: {tail}"
                        ));
                        continue;
                    }
                    let detail = if tail.trim().is_empty() {
                        String::new()
                    } else {
                        format!(": {}", tail.trim())
                    };
                    let _ = out
                        .send(Ok(StreamEvent::Error {
                            message: format!("Claude Code exited unexpectedly{detail}"),
                            retry_after_secs: None,
                        }))
                        .await;
                    return;
                }
            }
        }
    }
}
