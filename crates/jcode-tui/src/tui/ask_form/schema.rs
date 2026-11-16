//! Parsing for `ask_user` form schemas.
//!
//! The tool emits a flat ACP form whose schema root is tagged with
//! [`TAG`](super::TAG). This module converts that schema into the small model
//! the TUI renders: question text, single/multi-select options, the "Other"
//! text field and the per-question notes field.
//!
//! v1 intentionally ignores every form that is not tagged, including applet
//! forms. See `plans/active/ASK_USER_FORM.md`.

use serde_json::Value;

use super::state::{Choice, OtherField, Question, TAG};

/// True when the schema root carries the `ask_user` tag.
pub(super) fn is_tagged(schema: &Value) -> bool {
    schema
        .get("_meta")
        .and_then(Value::as_object)
        .and_then(|meta| meta.get(TAG))
        .is_some()
}

/// Extract `q1`..`qN` questions in numeric order.
///
/// Returns `None` when the tagged schema contains no usable question, so a
/// malformed form is ignored rather than rendered empty.
pub(super) fn parse_questions(schema: &Value) -> Option<Vec<Question>> {
    if !is_tagged(schema) {
        return None;
    }
    let properties = schema.get("properties")?.as_object()?;
    let mut indexed: Vec<(u32, &str)> = properties
        .keys()
        .filter_map(|name| question_index(name).map(|index| (index, name.as_str())))
        .collect();
    indexed.sort_by_key(|(index, _)| *index);

    let mut questions = Vec::new();
    for (_, name) in indexed {
        let Some(property) = properties.get(name) else {
            continue;
        };
        if let Some(question) = parse_question(name, property, properties) {
            questions.push(question);
        }
    }
    (!questions.is_empty()).then_some(questions)
}

/// `q1` -> 1. Auxiliary fields (`q1_other`) deliberately do not match.
fn question_index(name: &str) -> Option<u32> {
    let digits = name.strip_prefix('q')?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|index| (1..=64).contains(index))
}

fn parse_question(
    name: &str,
    property: &Value,
    properties: &serde_json::Map<String, Value>,
) -> Option<Question> {
    let meta = property.get("_meta").and_then(Value::as_object);
    let tool_meta = meta
        .and_then(|meta| meta.get(TAG))
        .and_then(Value::as_object);

    let header = tool_meta
        .and_then(|meta| meta.get("header"))
        .and_then(Value::as_str)
        .or_else(|| property.get("title").and_then(Value::as_str))
        .unwrap_or(name)
        .to_string();
    let text = property
        .get("description")
        .and_then(Value::as_str)
        .or_else(|| property.get("title").and_then(Value::as_str))
        .unwrap_or(header.as_str())
        .to_string();

    let kind = property.get("type").and_then(Value::as_str)?;
    let (multi, choices) = match kind {
        "string" => (false, property.get("oneOf")?.as_array()?),
        "array" => {
            let items = property.get("items")?;
            let choices = items
                .get("anyOf")
                .or_else(|| items.get("oneOf"))?
                .as_array()?;
            (true, choices)
        }
        _ => return None,
    };

    let mut options = Vec::new();
    let mut other_marker = None;
    for choice in choices {
        let Some(value) = choice.get("const").and_then(Value::as_str) else {
            continue;
        };
        let label = choice
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(value)
            .to_string();
        if value == "__other__" {
            other_marker = Some(value.to_string());
            continue;
        }
        let preview = choice
            .get("_meta")
            .and_then(Value::as_object)
            .and_then(|meta| meta.get("preview"))
            .and_then(Value::as_str)
            .map(str::to_string);
        options.push(Choice {
            value: value.to_string(),
            label,
            description: choice
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
            preview,
        });
    }

    let other_field = tool_meta
        .and_then(|meta| meta.get("otherField"))
        .and_then(Value::as_str)
        .filter(|field| properties.contains_key(*field))
        .map(str::to_string);
    let other_value = other_marker.unwrap_or_else(|| "__other__".to_string());
    let other = other_field.map(|field| OtherField {
        value: other_value,
        field,
    });

    let notes_field = tool_meta
        .and_then(|meta| meta.get("notesField"))
        .and_then(Value::as_str)
        .filter(|field| properties.contains_key(*field))
        .map(str::to_string);

    if options.is_empty() && other.is_none() {
        return None;
    }

    Some(Question::new(
        name.to_string(),
        header,
        text,
        multi,
        options,
        other,
        notes_field,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_tagged_single_multi_other_and_notes() {
        let schema = json!({
            "_meta": { "jcode.dev/askUser": {"version": 1} },
            "type": "object",
            "properties": {
                "q1": {
                    "type": "string",
                    "title": "Pick",
                    "description": "Which one?",
                    "_meta": { "jcode.dev/askUser": {"header": "One", "otherField": "q1_other", "notesField": "q1_notes"} },
                    "oneOf": [
                        {"const": "a", "title": "A", "description": "first", "_meta": {"preview": "A preview"}},
                        {"const": "b", "title": "B"},
                        {"const": "__other__", "title": "Other"}
                    ]
                },
                "q1_other": {"type": "string"},
                "q1_notes": {"type": "string"},
                "q2": {
                    "type": "array",
                    "title": "Many",
                    "_meta": { "jcode.dev/askUser": {"header": "Two"} },
                    "items": {"anyOf": [
                        {"const": "x", "title": "X"},
                        {"const": "y", "title": "Y"}
                    ]}
                }
            }
        });
        assert!(is_tagged(&schema));
        let questions = parse_questions(&schema).expect("questions");
        assert_eq!(questions.len(), 2);
        assert_eq!(questions[0].header, "One");
        assert_eq!(questions[0].text, "Which one?");
        assert!(!questions[0].multi);
        assert_eq!(questions[0].options.len(), 2);
        assert_eq!(
            questions[0].options[0].preview.as_deref(),
            Some("A preview")
        );
        assert!(questions[0].other.is_some());
        assert_eq!(questions[0].notes_field.as_deref(), Some("q1_notes"));
        assert!(questions[1].multi);
        assert_eq!(questions[1].options.len(), 2);
        assert!(questions[1].other.is_none());
    }

    #[test]
    fn ignores_untagged_and_malformed_schemas() {
        let untagged = json!({"type": "object", "properties": {"q1": {"type": "string", "oneOf": [{"const": "a"}]}}});
        assert!(!is_tagged(&untagged));
        assert!(parse_questions(&untagged).is_none());
        let empty = json!({"type": "object", "properties": {}});
        assert!(parse_questions(&empty).is_none());
    }

    #[test]
    fn auxiliary_fields_are_not_questions() {
        assert_eq!(question_index("q1"), Some(1));
        assert_eq!(question_index("q12"), Some(12));
        assert_eq!(question_index("q1_other"), None);
        assert_eq!(question_index("q"), None);
        assert_eq!(question_index("other"), None);
    }
}
