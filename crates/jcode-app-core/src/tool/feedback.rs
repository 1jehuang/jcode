use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

const MAX_SUMMARY_CHARS: usize = 240;
const MAX_DETAILS_CHARS: usize = 1500;

pub struct MaintainerFeedbackTool;

impl MaintainerFeedbackTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct FeedbackInput {
    category: FeedbackCategory,
    origin: FeedbackOrigin,
    user_confirmed: bool,
    summary: String,
    #[serde(default)]
    details: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FeedbackCategory {
    Bug,
    Praise,
    Suggestion,
    Usability,
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FeedbackOrigin {
    User,
    Agent,
    Mixed,
}

fn limited(value: &str, label: &str, max: usize) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail!("{label} must not be empty");
    }
    if value.chars().count() > max {
        bail!("{label} must be at most {max} characters");
    }
    Ok(value.to_string())
}

fn payload(input: FeedbackInput) -> Result<String> {
    if matches!(input.origin, FeedbackOrigin::User | FeedbackOrigin::Mixed) && !input.user_confirmed
    {
        bail!("user_confirmed must be true for user or mixed-origin feedback");
    }
    let summary = limited(&input.summary, "summary", MAX_SUMMARY_CHARS)?;
    let details = input
        .details
        .as_deref()
        .map(|value| limited(value, "details", MAX_DETAILS_CHARS))
        .transpose()?;
    let category = format!("{:?}", input.category).to_ascii_lowercase();
    let origin = format!("{:?}", input.origin).to_ascii_lowercase();
    let mut text = format!("[agent feedback; category={category}; origin={origin}] {summary}");
    if let Some(details) = details {
        text.push_str("\n\n");
        text.push_str(&details);
    }
    Ok(text)
}

#[async_trait]
impl Tool for MaintainerFeedbackTool {
    fn name(&self) -> &str {
        "maintainer_feedback"
    }

    fn description(&self) -> &str {
        "Send product feedback to Jcode's maintainer. Respects telemetry settings."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["category", "origin", "user_confirmed", "summary"],
            "properties": {
                "intent": super::intent_schema_property(),
                "category": {
                    "type": "string",
                    "enum": ["bug", "praise", "suggestion", "usability", "other"],
                    "description": "Kind of feedback."
                },
                "origin": {
                    "type": "string",
                    "enum": ["user", "agent", "mixed"],
                    "description": "Whether this reflects the user's words, the agent's observation, or both."
                },
                "user_confirmed": {
                    "type": "boolean",
                    "description": "True only if the user approved sharing user-originated feedback. Agent observations may use false."
                },
                "summary": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": MAX_SUMMARY_CHARS,
                    "description": "Self-contained maintainer-facing summary. Paraphrase rather than quoting the user."
                },
                "details": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": MAX_DETAILS_CHARS,
                    "description": "Optional reproduction steps or expected versus actual behavior. Never include private data."
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let text = payload(serde_json::from_value(input)?)?;
        if !crate::telemetry::is_enabled() {
            return Ok(ToolOutput::new(
                "Feedback was not sent because telemetry is disabled. The user can use /telemetry to change that setting.",
            ));
        }
        crate::telemetry::record_feedback(&text);
        Ok(ToolOutput::new(
            "Feedback queued for the Jcode maintainer. Thank you.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_labels_and_formats_feedback() {
        let text = payload(FeedbackInput {
            category: FeedbackCategory::Praise,
            origin: FeedbackOrigin::Mixed,
            user_confirmed: true,
            summary: "The new picker is much easier to use".into(),
            details: Some("The labels make model differences clear.".into()),
        })
        .unwrap();
        assert_eq!(
            text,
            "[agent feedback; category=praise; origin=mixed] The new picker is much easier to use\n\nThe labels make model differences clear."
        );
    }

    #[test]
    fn payload_rejects_empty_and_oversized_fields() {
        let empty = payload(FeedbackInput {
            category: FeedbackCategory::Bug,
            origin: FeedbackOrigin::Agent,
            user_confirmed: false,
            summary: "  ".into(),
            details: None,
        });
        assert!(empty.unwrap_err().to_string().contains("must not be empty"));

        let oversized = payload(FeedbackInput {
            category: FeedbackCategory::Other,
            origin: FeedbackOrigin::User,
            user_confirmed: true,
            summary: "x".repeat(MAX_SUMMARY_CHARS + 1),
            details: None,
        });
        assert!(oversized.unwrap_err().to_string().contains("at most 240"));
    }

    #[test]
    fn schema_requires_provenance_and_carries_privacy_guidance() {
        let tool = MaintainerFeedbackTool::new();
        let schema = tool.parameters_schema();
        assert_eq!(
            schema["required"],
            json!(["category", "origin", "user_confirmed", "summary"])
        );
        assert_eq!(
            schema["properties"]["origin"]["enum"],
            json!(["user", "agent", "mixed"])
        );
        assert!(
            schema["properties"]["summary"]["description"]
                .as_str()
                .unwrap()
                .contains("Paraphrase")
        );
        assert!(
            schema["properties"]["details"]["description"]
                .as_str()
                .unwrap()
                .contains("Never include private data")
        );
        assert!(tool.description().contains("telemetry settings"));
    }

    #[test]
    fn user_origin_requires_explicit_confirmation() {
        let error = payload(FeedbackInput {
            category: FeedbackCategory::Praise,
            origin: FeedbackOrigin::User,
            user_confirmed: false,
            summary: "The user likes the new workflow".into(),
            details: None,
        })
        .unwrap_err();
        assert!(error.to_string().contains("user_confirmed must be true"));

        assert!(
            payload(FeedbackInput {
                category: FeedbackCategory::Bug,
                origin: FeedbackOrigin::Agent,
                user_confirmed: false,
                summary: "The agent observed a reproducible tool error".into(),
                details: None,
            })
            .is_ok()
        );
    }
}
