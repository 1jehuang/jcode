//! Regression coverage for the curated Anthropic OAuth tool schemas.
//!
//! The OAuth (subscription) endpoint expects Claude-Code builtin tool *names*,
//! so `format_tools` hand-maintains a curated definition for a few of them.
//! Hand-maintained schemas drift from the real tools they stand in for, and the
//! failure is invisible until a model calls the tool and the handler rejects
//! the arguments. These tests pin the two drifts that reached users.

use super::*;
use jcode_message_types::ToolDefinition;
use serde_json::json;

fn tool_def(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: format!("{name} description"),
        input_schema: json!({"type":"object","properties":{}}),
    }
}

#[test]
fn oauth_schedule_wakeup_forwards_the_real_schedule_schema() {
    // Regression for #706: the curated ScheduleWakeup definition advertised
    // delaySeconds/reason/prompt while the real `schedule` handler requires
    // `task`, so every call failed with "task is required for action=create".
    let real_schema = json!({
        "type": "object",
        "properties": {
            "action": {"type": "string"},
            "task": {"type": "string"},
            "wake_in_minutes": {"type": "integer"}
        },
        "required": ["intent"]
    });
    let registry = vec![ToolDefinition {
        name: "schedule".to_string(),
        description: "Schedule, list, or cancel future tasks.".to_string(),
        input_schema: real_schema.clone(),
    }];

    let formatted = format_tools(&registry, true, false);
    let scheduled = formatted
        .iter()
        .find(|t| t.name == "ScheduleWakeup")
        .expect("schedule must be advertised under its OAuth name");

    let props = scheduled.input_schema["properties"]
        .as_object()
        .expect("object schema");
    assert!(props.contains_key("task"), "{props:?}");
    assert!(
        !props.contains_key("delaySeconds"),
        "fabricated schema leaked back in: {props:?}"
    );
    assert_eq!(
        formatted
            .iter()
            .filter(|t| t.name == "ScheduleWakeup")
            .count(),
        1,
        "schedule must not be advertised twice"
    );
}

#[test]
fn oauth_bash_schema_advertises_the_justification_escape_hatch() {
    // Regression for #722: the destructive gate consumes `justification`,
    // so it has to be discoverable in the advertised schema.
    let formatted = format_tools(&[tool_def("bash")], true, false);
    let bash = formatted
        .iter()
        .find(|t| t.name == "Bash")
        .expect("Bash must be advertised");
    assert!(
        bash.input_schema["properties"]
            .as_object()
            .is_some_and(|p| p.contains_key("justification")),
        "{:?}",
        bash.input_schema
    );
}
