//! Multiple-choice questions answered in the attached TUI.
use super::{AskUserInputRequest, ClientInputRequest, Tool, ToolContext, ToolOutput};
use anyhow::{Result, bail};
use async_trait::async_trait;
use jcode_message_types::ask_user::{AskUserAnswer, AskUserQuestion};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;

pub(super) struct AskUserTool;

pub(super) const DISMISSED: &str = "The user dismissed the questions without answering. Stop and wait for the user to tell you how to proceed.";
pub(super) const NO_USER: &str = "No interactive user is available to answer. Proceed with your best judgment and state the assumptions you made.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    questions: Vec<AskUserQuestion>,
    #[serde(rename = "intent")]
    _intent: Option<String>,
    #[serde(rename = "accept_large_output")]
    _accept_large_output: Option<bool>,
}

fn validate(questions: &[AskUserQuestion]) -> Result<()> {
    if !(1..=4).contains(&questions.len()) {
        bail!("ask_user takes 1-4 questions, got {}", questions.len());
    }
    let mut texts = HashSet::new();
    for question in questions {
        if !texts.insert(question.question.as_str()) {
            bail!("duplicate question text: {:?}", question.question);
        }
        if !(2..=4).contains(&question.options.len()) {
            bail!(
                "question {:?} needs 2-4 options, got {}",
                question.question,
                question.options.len()
            );
        }
        let mut labels = HashSet::new();
        for option in &question.options {
            if !labels.insert(option.label.as_str()) {
                bail!(
                    "duplicate option label {:?} in question {:?}",
                    option.label,
                    question.question
                );
            }
        }
    }
    Ok(())
}

pub(super) fn format_answers(questions: &[AskUserQuestion], answers: &[AskUserAnswer]) -> String {
    let pairs: Vec<String> = questions
        .iter()
        .map(|question| {
            let answer = answers
                .iter()
                .find(|answer| answer.question == question.question)
                .filter(|answer| !answer.answers.is_empty());
            match answer {
                Some(answer) => format!("{:?}={:?}", question.question, answer.answers.join(", ")),
                None => format!("{:?}=(no answer)", question.question),
            }
        })
        .collect();
    format!(
        "The user answered: {}. Follow what they actually said.",
        pairs.join(", ")
    )
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user 1-4 multiple-choice questions and wait for their answers. Use only when blocked on a decision that is genuinely the user's to make and that you cannot resolve from the request, the code, or sensible defaults. The user can always pick \"Other\" and type a custom answer, so never include an Other option. Put a recommended option first and suffix its label with \"(Recommended)\". Each question must stand on its own; the user may see only the question dialog."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "question": {"type": "string", "description": "Full question, ending with ?"},
                            "header": {"type": "string", "description": "Chip label, at most 12 chars."},
                            "options": {
                                "type": "array",
                                "minItems": 2,
                                "maxItems": 4,
                                "items": {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "properties": {
                                        "label": {"type": "string", "description": "1-5 words."},
                                        "description": {"type": "string"}
                                    },
                                    "required": ["label", "description"]
                                }
                            },
                            "multiSelect": {"type": "boolean", "default": false}
                        },
                        "required": ["question", "header", "options"]
                    }
                },
                "intent": super::intent_schema_property()
            },
            "required": ["questions"]
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let Input { questions, .. } = serde_json::from_value(input)?;
        validate(&questions)?;
        let output = |text: &str| Ok(ToolOutput::new(text).with_title("ask_user"));
        let Some(client) = ctx.stdin_request_tx.as_ref() else {
            return output(NO_USER);
        };
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        let request = AskUserInputRequest {
            request_id: format!("ask-{}", ctx.tool_call_id),
            tool_call_id: ctx.tool_call_id.clone(),
            questions: questions.clone(),
            response_tx,
        };
        if client.send(ClientInputRequest::AskUser(request)).is_err() {
            return output(NO_USER);
        }
        let interrupted = async {
            match &ctx.graceful_shutdown_signal {
                Some(signal) if !signal.is_set() => signal.notified().await,
                Some(_) => {}
                None => std::future::pending::<()>().await,
            }
        };
        let reply = tokio::select! {
            reply = response_rx => reply,
            _ = interrupted => Ok(None),
        };
        match reply {
            Ok(Some(answers)) => output(&format_answers(&questions, &answers)),
            Ok(None) => output(DISMISSED),
            Err(_) => output(NO_USER),
        }
    }
}

#[cfg(test)]
#[path = "ask_user_tests.rs"]
mod tests;
