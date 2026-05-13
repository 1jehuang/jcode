use super::{Tool, ToolContext, ToolOutput};
use crate::bus::{Bus, BusEvent, SidePanelUpdated};
use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
    /// Optional side panel page id. Defaults to `ask-user-question` so repeated calls update the same page.
    #[serde(default)]
    page_id: Option<String>,
    /// Optional side panel title.
    #[serde(default)]
    title: Option<String>,
    /// Focus the generated side-panel page. Defaults to true.
    #[serde(default)]
    focus: Option<bool>,
    /// Optional reply instructions. Defaults to a concise option-id reply format.
    #[serde(default)]
    reply_instructions: Option<String>,
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
            "Ask the user a structured multiple-choice question by creating a focused side-panel quiz. ",
            "Use this when user confirmation or preference selection would be easier as choices. ",
            "Highlight one recommended option and explain why. The user answers in chat with the option id or custom value."
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
                    "description": "Optional context shown before the choices."
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
                                "description": "Exact value to apply if selected."
                            },
                            "description": {
                                "type": "string",
                                "description": "Optional explanation/notes for this option."
                            },
                            "recommended": {
                                "type": "boolean",
                                "description": "Whether this option is recommended. Prefer exactly one recommended option."
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
                "page_id": {
                    "type": "string",
                    "description": "Side panel page id. Defaults to ask-user-question."
                },
                "title": {
                    "type": "string",
                    "description": "Side panel title. Defaults to Question."
                },
                "focus": {
                    "type": "boolean",
                    "description": "Focus the side panel page. Defaults to true."
                },
                "reply_instructions": {
                    "type": "string",
                    "description": "Optional instructions for how the user should answer."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: AskUserQuestionInput = serde_json::from_value(input)?;
        if params.options.is_empty() {
            bail!("askUserQuestion requires at least one option");
        }

        let page_id = params
            .page_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .unwrap_or("ask-user-question");
        let title = params
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or("Question");
        let focus = params.focus.unwrap_or(true);
        let content = render_question_markdown(&params);

        let snapshot = crate::side_panel::write_markdown_page(
            &ctx.session_id,
            page_id,
            Some(title),
            &content,
            focus,
        )?;
        Bus::global().publish(BusEvent::SidePanelUpdated(SidePanelUpdated {
            session_id: ctx.session_id.clone(),
            snapshot: snapshot.clone(),
        }));

        let recommended = params
            .options
            .iter()
            .enumerate()
            .filter(|(_, option)| option.recommended)
            .map(|(idx, option)| option_id(idx, option))
            .collect::<Vec<_>>();
        let response_hint = params.reply_instructions.clone().unwrap_or_else(|| {
            if params.allow_multiple {
                "Ask the user to reply with one or more option ids, or a custom value.".to_string()
            } else {
                "Ask the user to reply with one option id, or a custom value.".to_string()
            }
        });

        Ok(ToolOutput::new(format!(
            "Question displayed in side panel page `{page_id}`. Recommended: {}. {response_hint}",
            if recommended.is_empty() {
                "none".to_string()
            } else {
                recommended.join(", ")
            }
        ))
        .with_title("askUserQuestion")
        .with_metadata(json!({
            "page_id": page_id,
            "title": title,
            "recommended": recommended,
            "allow_multiple": params.allow_multiple,
            "question": params.question,
            "options": params.options,
        })))
    }
}

fn render_question_markdown(params: &AskUserQuestionInput) -> String {
    let mut out = String::new();
    out.push_str("# Question\n\n");
    out.push_str(&params.question);
    out.push_str("\n\n");

    if let Some(context) = params
        .context
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push_str("## Context\n\n");
        out.push_str(context);
        out.push_str("\n\n");
    }

    out.push_str("## Options\n\n");
    for (idx, option) in params.options.iter().enumerate() {
        let id = option_id(idx, option);
        if option.recommended {
            out.push_str(&format!(
                "### ✅ {id}. {} **(recommended)**\n\n",
                option.label
            ));
        } else {
            out.push_str(&format!("### {id}. {}\n\n", option.label));
        }

        if let Some(value) = option
            .value
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            out.push_str(&format!("- Value: `{value}`\n"));
        }
        if let Some(description) = option
            .description
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            out.push_str(&format!("- Notes: {description}\n"));
        }
        if option.recommended {
            if let Some(reason) = option
                .recommendation_reason
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                out.push_str(&format!("- Why recommended: {reason}\n"));
            }
        }
        out.push('\n');
    }

    out.push_str("## How to answer\n\n");
    if let Some(instructions) = params
        .reply_instructions
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push_str(instructions);
    } else if params.allow_multiple {
        out.push_str("Reply in chat with one or more option IDs, for example: `A C`, or provide a custom value.");
    } else {
        out.push_str(
            "Reply in chat with one option ID, for example: `A`, or provide a custom value.",
        );
    }
    out.push('\n');
    out
}

fn option_id(idx: usize, option: &QuestionOption) -> String {
    option
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| auto_option_id(idx))
}

fn auto_option_id(idx: usize) -> String {
    // A..Z, then 27, 28, ... to avoid surprising AA-style ids in a compact UI.
    if idx < 26 {
        ((b'A' + idx as u8) as char).to_string()
    } else {
        (idx + 1).to_string()
    }
}

#[cfg(test)]
#[path = "ask_user_question_tests.rs"]
mod ask_user_question_tests;
