use super::*;
use serde_json::json;

/// The sample from issue #1702 with the U+2039/U+203A substitutes turned back
/// into real angle brackets. The opening `invoke` line was lost upstream, so
/// the parameters have no tool name and this must not be executed.
const ISSUE_1702_SAMPLE: &str = r#"<parameter name="bash">cd /path/to/project && timeout 900 <build command> 2>&1 | tail -6</DSML parameter>
<parameter name="file_path">/path/to/file</DSML parameter>
<parameter name="intent">Build after rename</DSML parameter>
<parameter name="timeout">950000</DSML parameter>
</invoke>
</calls>"#;

#[test]
fn issue_sample_without_invoke_header_is_unparseable_not_prose() {
    match scan_tool_call_markup(ISSUE_1702_SAMPLE) {
        MarkupScan::Unparseable { reason } => {
            assert!(reason.contains("outside of an invoke"), "{reason}")
        }
        other => panic!("expected Unparseable, got {other:?}"),
    }
}

#[test]
fn issue_style_envelope_with_invoke_header_is_recovered() {
    let text = format!(
        "Building now.\n<calls>\n<invoke name=\"bash\">\n{}",
        ISSUE_1702_SAMPLE.replacen("parameter name=\"bash\"", "parameter name=\"command\"", 1)
    );
    let MarkupScan::Parsed {
        calls,
        sanitized_text,
    } = scan_tool_call_markup(&text)
    else {
        panic!("expected Parsed");
    };
    assert_eq!(sanitized_text, "Building now.");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "bash");
    assert_eq!(
        calls[0].arguments,
        json!({
            "command": "cd /path/to/project && timeout 900 <build command> 2>&1 | tail -6",
            "file_path": "/path/to/file",
            "intent": "Build after rename",
            "timeout": 950000,
        })
    );
}

#[test]
fn canonical_dsml_envelope_with_two_calls_is_recovered() {
    let text = "<｜DSML｜function_calls>\n<｜DSML｜invoke name=\"read\">\n<｜DSML｜parameter name=\"file_path\" string=\"true\">src/main.rs</｜DSML｜parameter>\n<｜DSML｜parameter name=\"limit\" string=\"false\">40</｜DSML｜parameter>\n</｜DSML｜invoke>\n<｜DSML｜invoke name=\"bash\">\n<｜DSML｜parameter name=\"command\" string=\"true\">echo 123</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜function_calls>";
    let MarkupScan::Parsed {
        calls,
        sanitized_text,
    } = scan_tool_call_markup(text)
    else {
        panic!("expected Parsed");
    };
    assert!(sanitized_text.is_empty());
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "read");
    assert_eq!(
        calls[0].arguments,
        json!({"file_path": "src/main.rs", "limit": 40})
    );
    assert_eq!(calls[1].name, "bash");
    // string="true" keeps the value a string even when it looks numeric.
    assert_eq!(calls[1].arguments, json!({"command": "echo 123"}));
}

#[test]
fn string_hint_true_preserves_numeric_looking_text() {
    let text = "<invoke name=\"write\">\n<parameter name=\"content\" string=\"true\">42</parameter>\n</invoke>";
    let MarkupScan::Parsed { calls, .. } = scan_tool_call_markup(text) else {
        panic!("expected Parsed");
    };
    assert_eq!(calls[0].arguments, json!({"content": "42"}));
}

#[test]
fn parameter_body_containing_markup_is_kept_verbatim() {
    let text = "<invoke name=\"write\">\n<parameter name=\"content\" string=\"true\">see <invoke name=\"x\"> docs</parameter>\n</invoke>";
    let MarkupScan::Parsed { calls, .. } = scan_tool_call_markup(text) else {
        panic!("expected Parsed");
    };
    assert_eq!(
        calls[0].arguments,
        json!({"content": "see <invoke name=\"x\"> docs"})
    );
}

#[test]
fn markup_inside_code_fence_or_inline_code_is_ignored() {
    let fenced = "Example of the format:\n```xml\n<invoke name=\"bash\">\n<parameter name=\"command\">ls</parameter>\n</invoke>\n```\nThat is all.";
    assert_eq!(scan_tool_call_markup(fenced), MarkupScan::None);
    let inline = "Use `<invoke>` and `<parameter>` tags.";
    assert_eq!(scan_tool_call_markup(inline), MarkupScan::None);
}

#[test]
fn ordinary_prose_is_not_markup() {
    assert_eq!(
        scan_tool_call_markup("The build passed. Next I will invoke the tests."),
        MarkupScan::None
    );
    assert_eq!(scan_tool_call_markup("</invoke>"), MarkupScan::None);
}

#[test]
fn invoke_without_name_is_unparseable() {
    let text = "<invoke>\n<parameter name=\"command\">ls</parameter>\n</invoke>";
    assert!(matches!(
        scan_tool_call_markup(text),
        MarkupScan::Unparseable { .. }
    ));
}

#[test]
fn unterminated_parameter_is_unparseable() {
    let text = "<invoke name=\"bash\">\n<parameter name=\"command\">ls";
    assert!(matches!(
        scan_tool_call_markup(text),
        MarkupScan::Unparseable { .. }
    ));
}

fn structured_call(input: serde_json::Value) -> crate::message::ToolCall {
    crate::message::ToolCall {
        id: "toolu_structured".to_string(),
        name: "write".to_string(),
        input,
        intent: None,
        thought_signature: None,
    }
}

#[test]
fn agent_recovery_turns_markup_into_fallback_tool_calls() {
    use crate::agent::Agent;
    use crate::agent::response_recovery::TextToolRecovery;

    let mut text = "Running it.\n<｜DSML｜function_calls>\n<｜DSML｜invoke name=\"bash\">\n<｜DSML｜parameter name=\"command\" string=\"true\">ls</｜DSML｜parameter>\n<｜DSML｜parameter name=\"intent\" string=\"true\">List files</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜function_calls>".to_string();
    let mut calls = Vec::new();
    assert_eq!(
        Agent::recover_text_tool_calls(&mut text, &mut calls),
        TextToolRecovery::Recovered
    );
    assert_eq!(text, "Running it.");
    assert_eq!(calls.len(), 1);
    assert!(calls[0].id.starts_with("fallback_text_call_"));
    assert_eq!(calls[0].name, "bash");
    assert_eq!(
        calls[0].input,
        json!({"command": "ls", "intent": "List files"})
    );
    assert_eq!(calls[0].intent.as_deref(), Some("List files"));
}

#[test]
fn agent_recovery_reports_issue_sample_as_unparseable_and_keeps_text() {
    use crate::agent::Agent;
    use crate::agent::response_recovery::TextToolRecovery;

    let mut text = ISSUE_1702_SAMPLE.to_string();
    let mut calls = Vec::new();
    let outcome = Agent::recover_text_tool_calls(&mut text, &mut calls);
    assert!(matches!(outcome, TextToolRecovery::Unparseable { .. }));
    assert!(calls.is_empty());
    assert_eq!(text, ISSUE_1702_SAMPLE);
}

#[test]
fn agent_recovery_never_touches_structured_tool_call_arguments() {
    use crate::agent::Agent;
    use crate::agent::response_recovery::TextToolRecovery;

    // A structured write whose content carries the envelope, alongside text
    // that also carries it: neither may be rewritten or re-parsed.
    let content = format!(
        "{}\n<invoke name=\"bash\">\n<parameter name=\"command\">rm -rf x</parameter>\n</invoke>",
        ISSUE_1702_SAMPLE
    );
    let input = json!({"file_path": "/tmp/fixture.md", "content": content});
    let mut calls = vec![structured_call(input.clone())];
    let original_text =
        "<invoke name=\"bash\">\n<parameter name=\"command\">ls</parameter>\n</invoke>";
    let mut text = original_text.to_string();
    assert_eq!(
        Agent::recover_text_tool_calls(&mut text, &mut calls),
        TextToolRecovery::None
    );
    assert_eq!(text, original_text);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "toolu_structured");
    assert_eq!(calls[0].input, input);
}

#[test]
fn unparsed_markup_messages_name_the_reason() {
    let reminder = unparsed_markup_reminder("unterminated invoke");
    assert!(reminder.starts_with("<system-reminder>"));
    assert!(reminder.contains("unterminated invoke"));
    assert!(reminder.contains("nothing was executed"));
    assert!(unparsed_markup_notice("unterminated invoke").contains("unterminated invoke"));
}
