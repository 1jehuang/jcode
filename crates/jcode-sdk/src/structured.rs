//! Schema-validated structured model turns.
//!
//! This is an SDK-level contract rather than a harness protocol feature. The
//! client asks for JSON, validates the response locally, and gives the model a
//! bounded number of correction opportunities when parsing or validation
//! fails.

use crate::{ApiEvent, Error, JcodeClient, RunOptions, ToolCall, TurnResult, Usage};
use jsonschema::Validator;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::fmt;
use std::sync::Arc;

/// JSON Schema accepted by [`JcodeClient::run_structured`].
pub type StructuredOutputSchema = Value;

/// Callback invoked for every event in every structured-output attempt.
pub type StructuredEventCallback = Arc<dyn Fn(&ApiEvent) + Send + Sync>;

/// Options for a schema-validated turn.
pub struct RunStructuredOptions {
    /// JSON Schema the assistant's JSON response must satisfy.
    pub schema: StructuredOutputSchema,
    /// Corrective retries after the first invalid response. Defaults to two.
    pub max_retries: usize,
    /// Images attached to every attempt, matching [`RunOptions::images`].
    pub images: Vec<(String, String)>,
    /// Called for every event in every attempt.
    pub on_event: Option<StructuredEventCallback>,
    /// Auto-answer permission prompts, matching [`RunOptions::auto_approve`].
    pub auto_approve: bool,
}

impl RunStructuredOptions {
    /// Create options with the TypeScript SDK's default of two corrective
    /// retries and otherwise-default turn options.
    pub fn new(schema: StructuredOutputSchema) -> Self {
        Self {
            schema,
            max_retries: 2,
            images: Vec::new(),
            on_event: None,
            auto_approve: false,
        }
    }

    fn run_options(&self) -> RunOptions {
        let on_event = self.on_event.as_ref().map(|callback| {
            let callback = Arc::clone(callback);
            Box::new(move |event: &ApiEvent| callback(event)) as Box<dyn Fn(&ApiEvent) + Send>
        });
        RunOptions {
            images: self.images.clone(),
            on_event,
            auto_approve: self.auto_approve,
        }
    }
}

/// A normalized JSON parse, schema validation, or typed-decoding problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredValidationIssue {
    /// JSON Pointer path to the invalid value. `/` means the root.
    pub path: String,
    /// JSON Schema keyword, `parse`, or `deserialize`.
    pub keyword: String,
    /// Human-readable validation message.
    pub message: String,
    /// Keyword-specific validation metadata, when available.
    pub params: Option<Map<String, Value>>,
}

/// One model attempt made by [`JcodeClient::run_structured`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredOutputAttempt {
    /// One-based attempt number.
    pub attempt: usize,
    /// Raw assistant text produced for this attempt.
    pub text: String,
    /// Empty for the successful attempt.
    pub errors: Vec<StructuredValidationIssue>,
}

/// A model response parsed and validated against the requested schema.
#[derive(Debug, Clone, PartialEq)]
pub struct StructuredTurnResult<T> {
    /// Parsed JSON value, decoded as the caller's requested Rust type.
    pub data: T,
    /// All attempts, including invalid retries and the successful response.
    pub attempts: Vec<StructuredOutputAttempt>,
    pub text: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
}

impl<T> StructuredTurnResult<T> {
    fn from_turn(data: T, attempts: Vec<StructuredOutputAttempt>, turn: TurnResult) -> Self {
        Self {
            data,
            attempts,
            text: turn.text,
            reasoning: turn.reasoning,
            tool_calls: turn.tool_calls,
            usage: turn.usage,
        }
    }
}

/// An invalid JSON Schema supplied to [`JcodeClient::run_structured`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredSchemaError {
    pub message: String,
}

impl fmt::Display for StructuredSchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StructuredSchemaError {}

/// Raised after every bounded structured-output attempt fails validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredOutputError {
    pub attempts: Vec<StructuredOutputAttempt>,
    pub validation_errors: Vec<StructuredValidationIssue>,
    pub last_text: String,
}

impl StructuredOutputError {
    fn new(attempts: Vec<StructuredOutputAttempt>) -> Self {
        let validation_errors = attempts
            .last()
            .map(|attempt| attempt.errors.clone())
            .unwrap_or_default();
        let last_text = attempts
            .last()
            .map(|attempt| attempt.text.clone())
            .unwrap_or_default();
        Self {
            attempts,
            validation_errors,
            last_text,
        }
    }

    /// Stable error code shared with the TypeScript SDK.
    pub fn code(&self) -> &'static str {
        "structured_output_invalid"
    }
}

impl fmt::Display for StructuredOutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.attempts.len();
        let summary = self
            .validation_errors
            .iter()
            .map(format_issue)
            .collect::<Vec<_>>()
            .join("; ");
        let summary = if summary.is_empty() {
            "no structured output attempts ran"
        } else {
            &summary
        };
        write!(
            f,
            "model did not produce valid structured output after {count} attempt{}: {summary}",
            if count == 1 { "" } else { "s" }
        )
    }
}

impl std::error::Error for StructuredOutputError {}

/// Failure modes specific to a structured turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStructuredError {
    /// The underlying harness turn failed.
    Client(Error),
    /// The caller supplied an invalid JSON Schema.
    InvalidSchema(StructuredSchemaError),
    /// All model attempts failed parsing, validation, or typed decoding.
    InvalidOutput(StructuredOutputError),
}

impl RunStructuredError {
    /// Stable machine-readable error code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Client(error) => error.code(),
            Self::InvalidSchema(_) => "structured_schema_invalid",
            Self::InvalidOutput(error) => error.code(),
        }
    }
}

impl fmt::Display for RunStructuredError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(error) => error.fmt(f),
            Self::InvalidSchema(error) => {
                write!(f, "structured_schema_invalid: {error}")
            }
            Self::InvalidOutput(error) => {
                write!(f, "{}: {error}", error.code())
            }
        }
    }
}

impl std::error::Error for RunStructuredError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
            Self::InvalidSchema(error) => Some(error),
            Self::InvalidOutput(error) => Some(error),
        }
    }
}

impl From<Error> for RunStructuredError {
    fn from(error: Error) -> Self {
        Self::Client(error)
    }
}

impl JcodeClient {
    /// Send a message and parse the assistant answer as schema-validated JSON.
    ///
    /// Schema compilation happens before the first model turn. Invalid model
    /// output is followed by at most `max_retries` corrective turns containing
    /// the normalized errors and a truncated copy of the previous response.
    pub fn run_structured<T: DeserializeOwned>(
        &self,
        session_id: &str,
        content: &str,
        options: RunStructuredOptions,
    ) -> Result<StructuredTurnResult<T>, RunStructuredError> {
        let validator = jsonschema::validator_for(&options.schema).map_err(|error| {
            RunStructuredError::InvalidSchema(StructuredSchemaError {
                message: error.to_string(),
            })
        })?;
        let mut attempts = Vec::new();
        let mut prompt = build_structured_prompt(content, &options.schema);

        for attempt_number in 1..=options.max_retries.saturating_add(1) {
            let turn = self.run(session_id, &prompt, options.run_options())?;
            let validation = validate_structured_text::<T>(&turn.text, &validator, &options.schema);
            let errors = match &validation {
                Ok(_) => Vec::new(),
                Err(errors) => errors.clone(),
            };
            let attempt = StructuredOutputAttempt {
                attempt: attempt_number,
                text: turn.text.clone(),
                errors,
            };
            attempts.push(attempt.clone());

            match validation {
                Ok(data) => return Ok(StructuredTurnResult::from_turn(data, attempts, turn)),
                Err(_) if attempt_number <= options.max_retries => {
                    prompt = build_structured_correction_prompt(&options.schema, &attempt);
                }
                Err(_) => break,
            }
        }

        Err(RunStructuredError::InvalidOutput(
            StructuredOutputError::new(attempts),
        ))
    }
}

fn validate_structured_text<T: DeserializeOwned>(
    text: &str,
    validator: &Validator,
    schema: &Value,
) -> Result<T, Vec<StructuredValidationIssue>> {
    let value = parse_json_from_text(text).map_err(|issue| vec![issue])?;
    let errors: Vec<_> = validator
        .iter_errors(&value)
        .map(|error| normalize_validation_error(&error, schema))
        .collect();
    if !errors.is_empty() {
        return Err(errors);
    }

    serde_json::from_value(value).map_err(|error| {
        vec![StructuredValidationIssue {
            path: "/".to_string(),
            keyword: "deserialize".to_string(),
            message: format!("valid JSON did not match the requested Rust type: {error}"),
            params: None,
        }]
    })
}

fn normalize_validation_error(
    error: &jsonschema::ValidationError<'_>,
    schema: &Value,
) -> StructuredValidationIssue {
    let path = error.instance_path().to_string();
    let keyword = error.kind().keyword().to_string();
    let (message, params) = ajv_style_details(error, schema);
    StructuredValidationIssue {
        path: if path.is_empty() {
            "/".to_string()
        } else {
            path
        },
        keyword,
        message,
        params,
    }
}

fn ajv_style_details(
    error: &jsonschema::ValidationError<'_>,
    schema: &Value,
) -> (String, Option<Map<String, Value>>) {
    use jsonschema::error::ValidationErrorKind;

    let mut params = Map::new();
    let message = match error.kind() {
        ValidationErrorKind::Type { .. } => {
            let expected = schema
                .pointer(&error.schema_path().to_string())
                .cloned()
                .unwrap_or(Value::Null);
            params.insert("type".to_string(), expected.clone());
            let expected = expected
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| expected.to_string());
            format!("must be {expected}")
        }
        ValidationErrorKind::Required { property } => {
            params.insert("missingProperty".to_string(), property.clone());
            format!("must have required property {property}")
        }
        ValidationErrorKind::AdditionalProperties { unexpected } => {
            params.insert(
                "additionalProperties".to_string(),
                Value::Array(unexpected.iter().cloned().map(Value::String).collect()),
            );
            "must NOT have additional properties".to_string()
        }
        ValidationErrorKind::Minimum { limit } => {
            params.insert("comparison".to_string(), Value::String(">=".to_string()));
            params.insert("limit".to_string(), limit.clone());
            format!("must be >= {limit}")
        }
        ValidationErrorKind::Maximum { limit } => {
            params.insert("comparison".to_string(), Value::String("<=".to_string()));
            params.insert("limit".to_string(), limit.clone());
            format!("must be <= {limit}")
        }
        _ => error.to_string(),
    };
    (message, Some(params))
}

fn parse_json_from_text(text: &str) -> Result<Value, StructuredValidationIssue> {
    let candidates = json_candidates(text);
    let mut last_error = "input was empty".to_string();
    for candidate in candidates {
        match serde_json::from_str(&candidate) {
            Ok(value) => return Ok(value),
            Err(error) => last_error = error.to_string(),
        }
    }
    Err(StructuredValidationIssue {
        path: "/".to_string(),
        keyword: "parse".to_string(),
        message: format!("invalid JSON: {last_error}"),
        params: None,
    })
}

fn json_candidates(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let mut candidates = vec![trimmed.to_string()];
    if let Some(inner) = trimmed
        .strip_prefix("```")
        .and_then(|value| value.strip_suffix("```"))
    {
        let inner = inner.trim();
        let body = if inner
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("json"))
        {
            inner[4..].trim()
        } else {
            inner
        };
        if !body.is_empty() && !candidates.iter().any(|candidate| candidate == body) {
            candidates.push(body.to_string());
        }
    }
    if let Some(container) = first_json_container(trimmed)
        && !candidates.iter().any(|candidate| candidate == container)
    {
        candidates.push(container.to_string());
    }
    candidates
}

fn first_json_container(text: &str) -> Option<&str> {
    let (start, first) = text
        .char_indices()
        .find(|(_, character)| matches!(character, '{' | '['))?;
    let mut stack = vec![if first == '{' { '}' } else { ']' }];
    let mut in_string = false;
    let mut escaped = false;

    for (offset, character) in text[start + first.len_utf8()..].char_indices() {
        let index = start + first.len_utf8() + offset;
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                if stack.pop() != Some(character) {
                    return None;
                }
                if stack.is_empty() {
                    return Some(&text[start..index + character.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    None
}

fn build_structured_prompt(content: &str, schema: &Value) -> String {
    format!("{content}\n\n{}", structured_instructions(schema))
}

fn build_structured_correction_prompt(schema: &Value, attempt: &StructuredOutputAttempt) -> String {
    let errors = attempt
        .errors
        .iter()
        .map(|issue| format!("- {}", format_issue(issue)))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Your previous response did not satisfy the required structured-output contract.\n\
         Return a corrected response as JSON only, with no markdown, prose, code fences, or comments.\n\
         It must validate against this JSON Schema:\n```json\n{}\n```\n\
         Validation errors:\n{errors}\nPrevious response:\n```\n{}\n```",
        stable_stringify(schema),
        truncate(&attempt.text, 4_000)
    )
}

fn structured_instructions(schema: &Value) -> String {
    format!(
        "Return the answer as JSON only, with no markdown, prose, code fences, or comments.\n\
         The JSON value must validate against this JSON Schema:\n```json\n{}\n```",
        stable_stringify(schema)
    )
}

fn stable_stringify(value: &Value) -> String {
    serde_json::to_string_pretty(&sort_json(value)).expect("JSON values always serialize")
}

fn sort_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(sort_json).collect()),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.iter().collect();
            entries.sort_by_key(|(left, _)| *left);
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.clone(), sort_json(value)))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

fn format_issue(issue: &StructuredValidationIssue) -> String {
    format!("{} {}", issue.path, issue.message)
}

fn truncate(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_string();
    }
    let prefix: String = value.chars().take(max_chars).collect();
    format!("{prefix}\n… truncated {} chars", count - max_chars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_fenced_and_embedded_json_like_typescript() {
        assert_eq!(
            parse_json_from_text("```json\n{\"ok\": true}\n```").unwrap(),
            json!({"ok": true})
        );
        assert_eq!(
            parse_json_from_text("Result: [1, {\"text\": \"a ] brace\"}] done").unwrap(),
            json!([1, {"text": "a ] brace"}])
        );
    }

    #[test]
    fn correction_text_truncation_is_unicode_safe() {
        let text = "🦀".repeat(4_001);
        let truncated = truncate(&text, 4_000);
        assert_eq!(truncated.matches('🦀').count(), 4_000);
        assert!(truncated.ends_with("… truncated 1 chars"));
    }
}
