//! One user turn against a live `claude` child.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use jcode_message_types::{StreamEvent, ToolDefinition};
use jcode_provider_core::NativeToolResult;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::control::{self, McpAction};
use crate::process::{ChildEvent, ChildHandle, INTERRUPT_GRACE};
use crate::translate::{ClaudeCodeRateLimits, Translator};

/// Emit a status heartbeat after this much silence so long tool runs do not
/// look stalled.
const HEARTBEAT_AFTER: Duration = Duration::from_secs(30);

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEnd {
    /// The CLI sent the turn's `result`.
    Finished,
    /// The consumer dropped the stream (user cancelled).
    Cancelled,
    /// A rejected rate limit parked the turn; it was interrupted.
    RateLimited,
    /// The child exited before the turn finished. Carries recent stderr.
    Exited(String),
}

/// Result of [`drive_turn`].
pub struct TurnOutcome {
    pub end: TurnEnd,
    /// Whether any assistant output reached the consumer.
    pub produced_output: bool,
    /// Whether the child is still usable for the next turn.
    pub child_usable: bool,
    /// Session id the CLI reported, when it differs from the child's.
    pub session_id: Option<String>,
}

/// Inputs of one turn.
pub struct TurnRequest<'a> {
    pub child: &'a Arc<ChildHandle>,
    pub user_line: Value,
    pub turn_uuid: String,
    pub tools: &'a [ToolDefinition],
    pub permission_mode: &'a str,
    pub login_hint: String,
    pub out: &'a mpsc::Sender<Result<StreamEvent>>,
    pub results: &'a mut mpsc::Receiver<NativeToolResult>,
    pub on_rate_limits: &'a (dyn Fn(ClaudeCodeRateLimits) + Send + Sync),
}

/// Drive one turn: write the user line, translate output, answer control
/// requests and bridge jcode tool calls until the turn's `result` arrives.
pub async fn drive_turn(req: TurnRequest<'_>) -> TurnOutcome {
    let TurnRequest {
        child,
        user_line,
        turn_uuid,
        tools,
        permission_mode,
        login_hint,
        out,
        results,
        on_rate_limits,
    } = req;

    let (line_tx, mut line_rx) = mpsc::unbounded_channel();
    child.attach_turn(line_tx);
    child.set_tools(tools);
    // Drop results left over from an earlier, cancelled turn.
    while results.try_recv().is_ok() {}

    let mut translator = Translator::new(login_hint, Some(turn_uuid));
    let mut pending_tools: HashMap<String, Value> = HashMap::new();
    let mut reported_session: Option<String> = None;

    if let Err(err) = child.send(&user_line).await {
        child.detach_turn();
        let tail = child.stderr_tail();
        return TurnOutcome {
            end: TurnEnd::Exited(if tail.is_empty() {
                err.to_string()
            } else {
                tail
            }),
            produced_output: false,
            child_usable: false,
            session_id: None,
        };
    }

    let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_activity = Instant::now();

    let end = loop {
        tokio::select! {
            biased;
            _ = out.closed() => break TurnEnd::Cancelled,
            event = line_rx.recv() => {
                let line = match event {
                    Some(ChildEvent::Line(line)) => line,
                    Some(ChildEvent::Exited(tail)) => break TurnEnd::Exited(tail),
                    None => break TurnEnd::Exited(child.stderr_tail()),
                };
                last_activity = Instant::now();
                if line.get("type").and_then(Value::as_str) == Some("control_request") {
                    let events = handle_control(child, &line, tools, permission_mode, &mut pending_tools).await;
                    if !emit_all(out, events).await {
                        break TurnEnd::Cancelled;
                    }
                    continue;
                }
                let outcome = translator.handle(&line);
                if let Some(limits) = outcome.rate_limits {
                    on_rate_limits(limits);
                }
                let mut events = Vec::new();
                if let Some(session_id) = outcome.session_id
                    && session_id != child.session_id()
                    && reported_session.as_deref() != Some(session_id.as_str())
                {
                    reported_session = Some(session_id.clone());
                    events.push(StreamEvent::SessionId(session_id));
                }
                events.extend(outcome.events);
                if !emit_all(out, events).await {
                    break TurnEnd::Cancelled;
                }
                if outcome.rate_limit_rejected {
                    break TurnEnd::RateLimited;
                }
                if outcome.turn_finished {
                    break TurnEnd::Finished;
                }
            }
            Some(result) = results.recv() => {
                last_activity = Instant::now();
                answer_tool_call(child, &mut pending_tools, result).await;
            }
            _ = heartbeat.tick() => {
                if last_activity.elapsed() >= HEARTBEAT_AFTER {
                    last_activity = Instant::now();
                    let _ = out
                        .send(Ok(StreamEvent::StatusDetail {
                            detail: "Claude Code is working".into(),
                        }))
                        .await;
                }
            }
        }
    };

    let mut child_usable = !matches!(end, TurnEnd::Exited(_));
    if matches!(end, TurnEnd::Cancelled | TurnEnd::RateLimited) {
        child_usable = interrupt(child, &mut line_rx, &mut pending_tools, permission_mode).await;
    }
    child.detach_turn();
    TurnOutcome {
        end,
        produced_output: translator.produced_output(),
        child_usable,
        session_id: reported_session,
    }
}

async fn emit_all(out: &mpsc::Sender<Result<StreamEvent>>, events: Vec<StreamEvent>) -> bool {
    for event in events {
        if out.send(Ok(event)).await.is_err() {
            return false;
        }
    }
    true
}

/// Answer one CLI -> host control request. Returns events for the consumer.
async fn handle_control(
    child: &Arc<ChildHandle>,
    line: &Value,
    tools: &[ToolDefinition],
    permission_mode: &str,
    pending_tools: &mut HashMap<String, Value>,
) -> Vec<StreamEvent> {
    let request_id = line
        .get("request_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let request = line.get("request").cloned().unwrap_or(Value::Null);
    let subtype = request.get("subtype").and_then(Value::as_str).unwrap_or("");
    let mut events = Vec::new();
    let reply = match subtype {
        "can_use_tool" => {
            let tool_name = request
                .get("tool_name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let input = request.get("input").cloned().unwrap_or_else(|| json!({}));
            let decision = control::decide_permission(permission_mode, tool_name, &input);
            jcode_base::logging::info(&format!(
                "Claude Code permission: {tool_name} -> {} ({})",
                if decision.allow { "allow" } else { "deny" },
                decision.reason
            ));
            if let Some(plan) = decision.captured_plan {
                events.push(StreamEvent::TextDelta(format!("\n\n{plan}\n")));
            }
            control::control_success(&request_id, decision.response)
        }
        "mcp_message" => {
            let message = request.get("message").cloned().unwrap_or(Value::Null);
            match control::handle_mcp_message(&message, tools) {
                McpAction::Reply(reply) => {
                    control::control_success(&request_id, control::mcp_response_body(reply))
                }
                McpAction::CallTool {
                    jsonrpc_id,
                    tool_name,
                    arguments,
                } => {
                    pending_tools.insert(request_id.clone(), jsonrpc_id);
                    events.push(StreamEvent::NativeToolCall {
                        request_id,
                        tool_name,
                        input: arguments,
                    });
                    return events;
                }
            }
        }
        other => match control::simple_control_answer(other) {
            Some(Ok(body)) => control::control_success(&request_id, body),
            Some(Err(error)) => control::control_error(&request_id, &error),
            None => return events,
        },
    };
    if let Err(err) = child.send(&reply).await {
        jcode_base::logging::warn(&format!("Claude Code: control reply failed: {err}"));
    }
    events
}

async fn answer_tool_call(
    child: &Arc<ChildHandle>,
    pending_tools: &mut HashMap<String, Value>,
    result: NativeToolResult,
) {
    let Some(jsonrpc_id) = pending_tools.remove(&result.request_id) else {
        jcode_base::logging::debug(&format!(
            "Claude Code: dropping tool result for unknown request {}",
            result.request_id
        ));
        return;
    };
    let text = result
        .result
        .output
        .or(result.result.error)
        .unwrap_or_default();
    let reply = control::control_success(
        &result.request_id,
        control::mcp_response_body(control::mcp_tool_result(
            &jsonrpc_id,
            &text,
            result.is_error,
        )),
    );
    if let Err(err) = child.send(&reply).await {
        jcode_base::logging::warn(&format!("Claude Code: tool result reply failed: {err}"));
    }
}

/// Stop the running turn. Returns whether the child can be reused.
async fn interrupt(
    child: &Arc<ChildHandle>,
    line_rx: &mut mpsc::UnboundedReceiver<ChildEvent>,
    pending_tools: &mut HashMap<String, Value>,
    permission_mode: &str,
) -> bool {
    // Unblock tool calls jcode will no longer answer.
    for (request_id, jsonrpc_id) in pending_tools.drain() {
        let reply = control::control_success(
            &request_id,
            control::mcp_response_body(control::mcp_tool_result(
                &jsonrpc_id,
                "Cancelled by the user.",
                true,
            )),
        );
        let _ = child.send(&reply).await;
    }
    let request_id = format!("jcode-interrupt-{}", uuid::Uuid::new_v4());
    if child
        .send(&control::control_request(
            &request_id,
            json!({"subtype": "interrupt"}),
        ))
        .await
        .is_err()
    {
        child.shutdown().await;
        return false;
    }
    let deadline = tokio::time::Instant::now() + INTERRUPT_GRACE;
    loop {
        match tokio::time::timeout_at(deadline, line_rx.recv()).await {
            Ok(Some(ChildEvent::Line(line))) => {
                match line.get("type").and_then(Value::as_str) {
                    Some("result") => return true,
                    Some("control_request") => {
                        // Deny anything new while the turn winds down.
                        let mut ignored = HashMap::new();
                        let request = line.get("request").cloned().unwrap_or(Value::Null);
                        if request.get("subtype").and_then(Value::as_str) == Some("can_use_tool") {
                            let request_id =
                                line.get("request_id").and_then(Value::as_str).unwrap_or("");
                            let _ = child
                                .send(&control::control_success(
                                    request_id,
                                    json!({"behavior": "deny", "message": "Cancelled by the user."}),
                                ))
                                .await;
                        } else {
                            let _ =
                                handle_control(child, &line, &[], permission_mode, &mut ignored)
                                    .await;
                        }
                    }
                    _ => {}
                }
            }
            Ok(Some(ChildEvent::Exited(_))) | Ok(None) => return false,
            Err(_) => {
                jcode_base::logging::warn(
                    "Claude Code: turn did not stop after interrupt; restarting the CLI",
                );
                child.shutdown().await;
                return false;
            }
        }
    }
}
