//! Regression coverage for the curated Anthropic OAuth tool schemas.
//!
//! The OAuth (subscription) endpoint expects Claude-Code builtin tool *names*,
//! so `format_tools` hand-maintains a curated definition for a few of them.
//! Hand-maintained schemas drift from the real tools they stand in for, and the
//! failure is invisible until a model calls the tool and the handler rejects
//! the arguments. These tests pin the drifts that reached users.

use super::*;
use jcode_message_types::ToolDefinition;
use serde_json::json;

fn bash_registry_definition() -> ToolDefinition {
    ToolDefinition {
        name: "bash".to_string(),
        description: "Run a bash command with the registered execution options.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in MILLISECONDS (not seconds), e.g. 600000 = 10min; kills with exit 124. Omit for no timeout."
                },
                "run_in_background": {"type": "boolean"},
                "intent": {"type": "string"},
                "notify": {"type": "boolean"},
                "wake": {"type": "boolean"},
                "stall_wake_seconds": {"type": "integer"},
                "justification": {
                    "type": "string",
                    "description": "Explain why the refused command serves the user request."
                }
            },
            "required": ["command"]
        }),
    }
}

#[test]
fn oauth_bash_forwards_registered_schema_and_timeout_units() {
    assert!(format_tools(&[], true, false).is_empty());
    let mut registry_bash = bash_registry_definition();
    // A new registry property must survive without another provider-side edit.
    registry_bash.input_schema["properties"]["future_execution_option"] =
        json!({"type": "boolean", "description": "A newly registered option."});

    for is_oauth in [false, true] {
        let formatted = format_tools(std::slice::from_ref(&registry_bash), is_oauth, false);
        assert_eq!(formatted.len(), 1, "Bash must be advertised exactly once");
        let bash = &formatted[0];
        assert_eq!(bash.name, if is_oauth { "Bash" } else { "bash" });
        assert_eq!(bash.input_schema, registry_bash.input_schema);
        assert_eq!(bash.description, registry_bash.description);
        assert!(
            bash.input_schema["properties"]["timeout"]["description"]
                .as_str()
                .unwrap()
                .contains("MILLISECONDS (not seconds)")
        );
        assert!(bash.cache_control.is_some());
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
    let formatted = format_tools(&[bash_registry_definition()], true, false);
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
