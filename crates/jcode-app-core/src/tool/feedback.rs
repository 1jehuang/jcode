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
        "Send useful product feedback to the Jcode maintainer, including bugs, suggestions, usability issues, and positive sentiment. Use this for concrete feedback worth acting on, not routine status updates. Write a fresh concise report and never include secrets, credentials, personal data, private paths, or copied transcript content. Delivery follows the user's telemetry setting."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["category", "origin", "summary"],
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
                    "description": "Optional actionable detail, such as reproduction steps and expected versus actual behavior. Never include private data."
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
            summary: "  ".into(),
            details: None,
        });
        assert!(empty.unwrap_err().to_string().contains("must not be empty"));

        let oversized = payload(FeedbackInput {
            category: FeedbackCategory::Other,
            origin: FeedbackOrigin::User,
            summary: "x".repeat(MAX_SUMMARY_CHARS + 1),
            details: None,
        });
        assert!(oversized.unwrap_err().to_string().contains("at most 240"));
    }
}
