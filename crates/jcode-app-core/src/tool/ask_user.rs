use super::{Tool, ToolContext, ToolOutput};
use crate::tool::{DecisionInputOption, DecisionInputRequest, DecisionInputResponse};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

/// Ask the user to decide at a decision point. Blocks the turn until the
/// user answers. Raised only when the next step genuinely depends on the
/// user or the options are materially different and a rule or prompt
/// requires user input.
pub struct AskUserTool;

impl AskUserTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct AskUserInput {
    /// What is being decided, phrased for the user
    question: String,
    /// 2-5 offered options
    options: Vec<AskUserOption>,
    /// Optional short context on why this decision point is being raised
    #[serde(default)]
    context: Option<String>,
}

#[derive(Deserialize)]
struct AskUserOption {
    /// Short label shown in the chooser
    label: String,
    /// Optional one-line explanation of what this option implies
    #[serde(default)]
    detail: Option<String>,
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user to choose among 2-5 options when their input is required."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["intent", "question", "options"],
            "properties": {
                "intent": super::intent_schema_property(),
                "question": {
                    "type": "string",
                    "description": "What is being decided, phrased for the user. One sentence."
                },
                "options": {
                    "type": "array",
                    "minItems": 2,
                    "maxItems": 5,
                    "description": "2-5 offered options. Materially different choices only.",
                    "items": {
                        "type": "object",
                        "required": ["label"],
                        "properties": {
                            "label": {
                                "type": "string",
                                "description": "Short label shown in the chooser."
                            },
                            "detail": {
                                "type": "string",
                                "description": "Optional one-line explanation of what this option implies."
                            }
                        }
                    }
                },
                "context": {
                    "type": "string",
                    "description": "Optional short context on why this decision point is being raised (e.g. which rule requires asking)."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: AskUserInput = serde_json::from_value(input)?;

        // Config off-switch: never block, never forward to a client. Tell the
        // agent to fall back to plain text so the turn keeps moving.
        if !crate::config::config().features.decisions {
            return Ok(ToolOutput::new(
                "The decision chooser is disabled in config (features.decisions = false). Ask the \
                 question in plain text instead and end the turn."
                    .to_string(),
            ));
        }

        if params.options.len() < 2 {
            return Ok(ToolOutput::new(
                "ask_user requires at least 2 options. If there is only one way forward, \
                 do not ask; proceed and state what you did."
                    .to_string(),
            ));
        }
        if params.options.len() > 5 {
            return Ok(ToolOutput::new(
                "ask_user accepts at most 5 options. Consolidate materially similar \
                 options before asking."
                    .to_string(),
            ));
        }

        let Some(decision_tx) = ctx.decision_request_tx.clone() else {
            // No interactive channel (headless run, subcall, or direct mode).
            // Never block forever: tell the agent no chooser is available.
            return Ok(ToolOutput::new(
                "No interactive chooser is available in this execution context. Ask the \
                 question in plain text instead and end the turn."
                    .to_string(),
            ));
        };

        let request_id = format!("decision-{}", ctx.tool_call_id);
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        let request = DecisionInputRequest {
            request_id: request_id.clone(),
            tool_call_id: ctx.tool_call_id.clone(),
            question: params.question.clone(),
            options: params
                .options
                .iter()
                .map(|opt| DecisionInputOption {
                    label: opt.label.clone(),
                    detail: opt.detail.clone(),
                })
                .collect(),
            response_tx,
        };

        if decision_tx.send(request).is_err() {
            return Ok(ToolOutput::new(
                "The decision channel is closed (client disconnected). Ask the question in \
                 plain text instead and end the turn."
                    .to_string(),
            ));
        }

        // Block until the user answers, the client disconnects, or the turn is
        // gracefully shut down. With `features.decision_timeout_secs` > 0 the
        // whole wait is bounded: on expiry the first option is auto-picked so
        // the agent proceeds with a sensible default instead of hanging the
        // turn. `0` (default) blocks forever until one of those happens.
        let timeout_secs = crate::config::config().features.decision_timeout_secs;
        let mut timed_out = false;
        let wait_and_report = async {
            tokio::select! {
                biased;
                response = response_rx => match response {
                    Ok(answer) => Ok(answer),
                    Err(_) => Err(ToolOutput::new(
                        "The chooser was closed before you answered. Ask the question in \
                         plain text instead and end the turn."
                            .to_string(),
                    )),
                },
                _ = wait_for_shutdown(&ctx) => Err(ToolOutput::new(
                    "The turn was interrupted before the user answered.".to_string(),
                )),
            }
        };
        let answer = if timeout_secs > 0 {
            match tokio::time::timeout(
                std::time::Duration::from_secs(timeout_secs),
                wait_and_report,
            )
            .await
            {
                Ok(result) => match result {
                    Ok(answer) => answer,
                    Err(output) => return Ok(output),
                },
                Err(_) => {
                    timed_out = true;
                    DecisionInputResponse::Option(1)
                }
            }
        } else {
            match wait_and_report.await {
                Ok(answer) => answer,
                Err(output) => return Ok(output),
            }
        };

        let mut out = String::new();
        out.push_str(&format!("Decision: {}\n", params.question));
        match answer {
            DecisionInputResponse::Option(index) => {
                if index == 1 && timed_out {
                    // Timeout auto-pick: say so explicitly so the agent knows
                    // the user never actually chose this.
                    if let Some(opt) = option_labels(&params, index) {
                        out.push_str(&format!(
                            "No answer within the configured timeout \
                             (features.decision_timeout_secs = {timeout_secs}s); auto-picked \
                             option 1: {}\n",
                            opt
                        ));
                    } else {
                        out.push_str(
                            "No answer within the configured timeout; auto-picked option 1.\n",
                        );
                    }
                } else if let Some(opt) = option_labels(&params, index) {
                    out.push_str(&format!("User chose option {}: {}\n", index, opt));
                } else {
                    out.push_str(&format!(
                        "User chose option {} (out of range; treat as free-form)\n",
                        index
                    ));
                }
            }
            DecisionInputResponse::Text(text) => {
                out.push_str(&format!("User answered (free form): {}\n", text));
            }
            DecisionInputResponse::Dismissed => {
                out.push_str(
                    "User dismissed the chooser without answering. Ask the question in \
                     plain text instead and end the turn.\n",
                );
            }
        }
        if let Some(context) = params.context {
            out.push_str(&format!("Context: {}", context));
        }
        Ok(ToolOutput::new(out).with_title(format!("Ask user: {}", params.question)))
    }
}

fn option_labels(params: &AskUserInput, index: u32) -> Option<String> {
    params
        .options
        .get(index.saturating_sub(1) as usize)
        .map(|opt| {
            if let Some(ref detail) = opt.detail {
                format!("{} (value: {})", opt.label, detail)
            } else {
                opt.label.clone()
            }
        })
}

async fn wait_for_shutdown(ctx: &ToolContext) {
    match ctx.graceful_shutdown_signal.clone() {
        Some(signal) => {
            if !signal.is_set() {
                signal.notified().await;
            }
        }
        None => {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
#[path = "ask_user_tests.rs"]
mod tests;
