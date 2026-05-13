use super::{Tool, ToolContext, ToolOutput};
use crate::ask_user::{
    AskUserAnswerKind, AskUserOption, AskUserQuestion, register_pending,
};
use crate::bus::{Bus, BusEvent};
use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

/// Maximum time the tool will wait for the user to respond before giving up
/// and returning a "no response" tool result. Generous because we expect the
/// user to genuinely answer; the tool also returns early on Esc / disconnect.
const ASK_USER_TIMEOUT: Duration = Duration::from_secs(60 * 60);

pub struct AskUserQuestionTool;

impl AskUserQuestionTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct AskUserQuestionInput {
    /// Short natural-language label for compact tool display.
    #[serde(default)]
    intent: Option<String>,
    /// The question to ask the user.
    question: String,
    /// Optional context shown above the choices.
    #[serde(default)]
    context: Option<String>,
    /// Candidate answers. Exactly one should normally be marked recommended.
    options: Vec<QuestionOption>,
    /// Allow the user to select more than one option.
    #[serde(default)]
    allow_multiple: bool,
    /// Optional reply instructions / hint shown in the modal footer.
    #[serde(default)]
    reply_instructions: Option<String>,
    /// Optional modal title.
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct QuestionOption {
    /// Stable choice id shown to the user, such as `A`, `B`, `keep`, or `rec`.
    #[serde(default)]
    id: Option<String>,
    /// Human-readable option label.
    label: String,
    /// Optional exact value the agent should apply if this option is selected.
    #[serde(default)]
    value: Option<String>,
    /// Optional explanation/notes for this option.
    #[serde(default)]
    description: Option<String>,
    /// Whether this is the agent's recommended option.
    #[serde(default)]
    recommended: bool,
    /// Why this option is recommended. Displayed only for recommended options.
    #[serde(default)]
    recommendation_reason: Option<String>,
}

#[async_trait]
impl Tool for AskUserQuestionTool {
    fn name(&self) -> &str {
        "askUserQuestion"
    }

    fn description(&self) -> &str {
        concat!(
            "Ask the user an interactive multiple-choice question via a TUI modal overlay. ",
            "Use this when user confirmation or preference selection would be clearer as a ",
            "small set of choices rather than free-form chat. The user navigates with the ",
            "arrow keys, presses Enter to pick an option, or selects \"Other\" to type a ",
            "custom free-form answer. The tool blocks until the user responds or cancels. ",
            "Mark exactly one option `recommended:true` when you have a preferred answer; ",
            "the modal highlights it and pre-selects it for fast Enter-to-confirm."
        )
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["question", "options"],
            "properties": {
                "intent": super::intent_schema_property(),
                "question": {
                    "type": "string",
                    "description": "The question to ask the user."
                },
                "context": {
                    "type": "string",
                    "description": "Optional context shown above the choices."
                },
                "options": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "required": ["label"],
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Stable option id, e.g. A, B, keep, rec. Auto-generated as A/B/C if omitted."
                            },
                            "label": {
                                "type": "string",
                                "description": "Human-readable option label."
                            },
                            "value": {
                                "type": "string",
                                "description": "Exact value the agent receives if this option is selected."
                            },
                            "description": {
                                "type": "string",
                                "description": "Optional explanation shown under the option label."
                            },
                            "recommended": {
                                "type": "boolean",
                                "description": "Whether this is the agent's recommended option. Prefer exactly one recommended option."
                            },
                            "recommendation_reason": {
                                "type": "string",
                                "description": "Why this option is recommended."
                            }
                        }
                    }
                },
                "allow_multiple": {
                    "type": "boolean",
                    "description": "Allow multiple options to be selected. Defaults to false."
                },
                "reply_instructions": {
                    "type": "string",
                    "description": "Optional hint shown in the modal footer."
                },
                "title": {
                    "type": "string",
                    "description": "Optional modal title. Defaults to Question."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: AskUserQuestionInput = serde_json::from_value(input)?;
        if params.options.is_empty() {
            bail!("askUserQuestion requires at least one option");
        }

        // Normalize options: assign auto ids where missing, capture parallel
        // data for the response mapping.
        let normalized: Vec<AskUserOption> = params
            .options
            .iter()
            .enumerate()
            .map(|(idx, option)| AskUserOption {
                id: assigned_option_id(idx, option),
                label: option.label.clone(),
                description: option
                    .description
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned),
                value: option
                    .value
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned),
                recommended: option.recommended,
                recommendation_reason: option
                    .recommendation_reason
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned),
            })
            .collect();

        let request_id = format!(
            "ask-user-{}-{}",
            ctx.tool_call_id,
            chrono::Utc::now().timestamp_millis()
        );
        let receiver = register_pending(request_id.clone());

        let question = AskUserQuestion {
            request_id: request_id.clone(),
            session_id: ctx.session_id.clone(),
            question: params.question.clone(),
            context: params
                .context
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned),
            options: normalized.clone(),
            allow_multiple: params.allow_multiple,
            reply_instructions: params
                .reply_instructions
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned),
            title: params
                .title
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned),
        };

        Bus::global().publish(BusEvent::AskUserQuestionOpened(question));

        let answer = match tokio::time::timeout(ASK_USER_TIMEOUT, receiver).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_recv_err)) => {
                // Sender dropped (e.g. session reset) before answering.
                return Ok(ToolOutput::new(
                    "User did not answer (modal was closed without selection).",
                )
                .with_title("askUserQuestion")
                .with_metadata(json!({
                    "request_id": request_id,
                    "outcome": "closed",
                })));
            }
            Err(_timeout) => {
                // Try to clean up the pending entry to avoid leaking.
                crate::ask_user::drop_pending(&request_id);
                return Ok(ToolOutput::new(
                    "User did not answer within the timeout. Continue with a sensible default or ask again.",
                )
                .with_title("askUserQuestion")
                .with_metadata(json!({
                    "request_id": request_id,
                    "outcome": "timeout",
                })));
            }
        };

        match &answer.kind {
            AskUserAnswerKind::Options { ids, labels, values } => {
                let pretty_choices = ids
                    .iter()
                    .zip(labels.iter())
                    .map(|(id, label)| format!("{id} ({label})"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let text = format!("User chose: {}", pretty_choices);
                Ok(ToolOutput::new(text)
                    .with_title("askUserQuestion")
                    .with_metadata(json!({
                        "request_id": request_id,
                        "outcome": "selected",
                        "selected_ids": ids,
                        "selected_labels": labels,
                        "selected_values": values,
                    })))
            }
            AskUserAnswerKind::Custom { text } => Ok(ToolOutput::new(format!(
                "User typed a custom answer:\n{}",
                text
            ))
            .with_title("askUserQuestion")
            .with_metadata(json!({
                "request_id": request_id,
                "outcome": "custom",
                "custom_text": text,
            }))),
            AskUserAnswerKind::Canceled => Ok(ToolOutput::new(
                "User canceled the question (pressed Esc). Proceed without an answer or ask again with different framing.",
            )
            .with_title("askUserQuestion")
            .with_metadata(json!({
                "request_id": request_id,
                "outcome": "canceled",
            }))),
        }
    }
}

fn assigned_option_id(idx: usize, option: &QuestionOption) -> String {
    option
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| auto_option_id(idx))
}

fn auto_option_id(idx: usize) -> String {
    if idx < 26 {
        ((b'A' + idx as u8) as char).to_string()
    } else {
        (idx + 1).to_string()
    }
}

#[cfg(test)]
#[path = "ask_user_question_tests.rs"]
mod ask_user_question_tests;
