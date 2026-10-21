//! Tests for the `<jcode_tool_call>` envelope parser, kept out of
//! chatgpt_web.rs so that file stays inside its size budget. Most cases are
//! answer shapes observed live from ChatGPT web models.

use super::*;

fn only_call(response: &str) -> ParsedToolCall {
    let mut parsed = parse_response(response).unwrap();
    assert_eq!(parsed.calls.len(), 1, "expected exactly one call");
    parsed.calls.remove(0)
}

#[test]
fn invalid_shell_escapes_in_envelope_json_are_repaired() {
    // Observed live twice: `\(` from a find command, invalid in JSON.
    let parsed = only_call(
        r#"<jcode_tool_call>{"name":"bash","input":{"command":"find / \( -name x \) -o -path \"a b\" | grep \.rs$","intent":"x"}}</jcode_tool_call>"#,
    );
    assert_eq!(
        parsed.input["command"],
        r#"find / \( -name x \) -o -path "a b" | grep \.rs$"#
    );
    // Valid escapes are untouched.
    assert_eq!(
        repair_json_escapes(r#"{"a":"x\ny\\z\u0041"}"#),
        r#"{"a":"x\ny\\z\u0041"}"#
    );
    // A valid envelope is never rewritten, even if repair would change it.
    let valid = only_call(
        r#"<jcode_tool_call>{"name":"bash","input":{"command":"printf 'a\\nb'"}}</jcode_tool_call>"#,
    );
    assert_eq!(valid.input["command"], r#"printf 'a\nb'"#);
}

#[test]
fn stray_angle_bracket_before_closing_tag_is_tolerated() {
    // Observed live from GPT-5.6 Sol: a stray `>` before the closing tag.
    let parsed = only_call(
        r#"<jcode_tool_call>{"name":"bash","input":{"command":"uname -r","intent":"x"}}></jcode_tool_call>"#,
    );
    assert_eq!(parsed.input["command"], "uname -r");
    // Also observed live from GPT-5.6 Sol: a self-closing `/>`.
    let parsed = only_call(
        r#"<jcode_tool_call>{"name":"bash","input":{"command":"nproc","intent":"x"}}/></jcode_tool_call>"#,
    );
    assert_eq!(parsed.input["command"], "nproc");
}

#[test]
fn malformed_envelope_json_becomes_a_correctable_call() {
    // Observed live: an unescaped quote inside a shell command.
    let parsed = parse_response(
        r#"Checking.<jcode_tool_call>{"name":"bash","input":{"command":"echo "hi"","intent":"x"}}</jcode_tool_call>"#,
    )
    .unwrap();
    assert_eq!(parsed.text, "Checking.");
    assert_eq!(parsed.calls.len(), 1);
    assert_eq!(parsed.calls[0].name, "bash");
    let note = parsed.calls[0].input.as_str().expect("string arguments");
    assert!(note.contains("Invalid JSON"));
    assert!(note.contains("echo \"hi\""));
}

#[test]
fn malformed_envelope_without_a_name_still_fails() {
    let err = parse_response("<jcode_tool_call>{not json at all}</jcode_tool_call>")
        .err()
        .expect("no recoverable tool name");
    assert!(err.to_string().contains("invalid JSON"));
    assert_eq!(recover_tool_name(r#"{"name":"rm -rf","input":{}}"#), None);
    assert_eq!(
        recover_tool_name(r#"{ "name" : "mcp__x.y-z", "input": "#).as_deref(),
        Some("mcp__x.y-z")
    );
}

#[test]
fn tool_call_parser_accepts_valid_exact_envelope() {
    let parsed = only_call(
        "  <jcode_tool_call>{\"name\":\"bash\",\"input\":{\"command\":\"pwd\",\"intent\":\"Check cwd\"}}</jcode_tool_call>\n",
    );
    assert_eq!(parsed.name, "bash");
    assert_eq!(parsed.input["command"], "pwd");
}

#[test]
fn a_complete_call_missing_its_closing_tag_is_kept_and_trailing_notes_dropped() {
    // Observed live from GPT-5.6 Sol: leaked notes after the call and no
    // closing tag.
    let call = json!({"name": "bash", "input": {"command": "lscpu", "intent": "x"}});
    let parsed = parse_response(&format!(
        "I'll check the machine directly.{TOOL_CALL_START}{call} kits. But need stop after tool call."
    ))
    .unwrap();
    assert_eq!(parsed.text, "I'll check the machine directly.");
    assert_eq!(parsed.calls.len(), 1);
    assert_eq!(parsed.calls[0].input["command"], "lscpu");
}

#[test]
fn tool_call_parser_rejects_a_truncated_envelope() {
    let call = json!({"name": "bash", "input": {"command": "pwd"}}).to_string();
    let truncated = format!("{TOOL_CALL_START}{}", &call[..call.len() - 10]);
    let err = parse_response(&truncated)
        .err()
        .expect("a truncated call should fail");
    assert!(err.to_string().contains("invalid JSON"), "{err}");
}

#[test]
fn tool_call_parser_keeps_preamble_and_trailing_text_as_visible_text() {
    let parsed = parse_response(
        "Checking the repo first.\n\n<jcode_tool_call>{\"name\":\"bash\",\"input\":{}}</jcode_tool_call> done",
    )
    .unwrap();
    assert_eq!(parsed.calls.len(), 1);
    assert_eq!(parsed.text, "Checking the repo first.\n\n done");
}

#[test]
fn tool_call_parser_strips_chatgpt_escape_wrappers() {
    // Observed live: ChatGPT wraps raw markup in <escape> tags, which its
    // renderer hides completely, and sometimes nests a stray one inside.
    for response in [
        "<escape><jcode_tool_call>{\"name\":\"bash\",\"input\":{\"command\":\"ls\"}}</jcode_tool_call></escape>",
        "<escape><jcode_tool_call>{\"name\":\"bash\",\"input\":{\"command\":\"ls\"}}<escape></jcode_tool_call></escape>",
    ] {
        let parsed = parse_response(response).unwrap();
        assert_eq!(parsed.calls.len(), 1, "{response}");
        assert_eq!(parsed.calls[0].input["command"], "ls");
        assert!(parsed.text.is_empty(), "{response}");
    }
}

#[test]
fn tool_call_parser_accepts_multiple_envelopes() {
    let parsed = parse_response(
        "<jcode_tool_call>{\"name\":\"read\",\"input\":{\"path\":\"a\"}}</jcode_tool_call>\n<jcode_tool_call>\n{\"name\":\"read\",\"input\":{\"path\":\"b\"}}\n</jcode_tool_call>",
    )
    .unwrap();
    let paths: Vec<_> = parsed
        .calls
        .iter()
        .map(|call| call.input["path"].clone())
        .collect();
    assert_eq!(paths, vec![json!("a"), json!("b")]);
}

#[test]
fn tool_call_parser_rejects_prose_inside_an_envelope() {
    let err = parse_response(
        "<jcode_tool_call>{\"name\":\"bash\",\"input\":{}} and then more</jcode_tool_call>",
    )
    .err()
    .expect("text after the JSON inside the envelope should fail");
    assert!(err.to_string().contains("unexpected text"));
}

#[test]
fn tool_call_parser_rejects_non_object_input() {
    let err = parse_response(
        "<jcode_tool_call>{\"name\":\"bash\",\"input\":[\"not\",\"an\",\"object\"]}</jcode_tool_call>",
    )
    .err()
    .expect("array tool input should fail");
    assert!(err.to_string().contains("JSON object"));
}

#[test]
fn tool_call_parser_allows_marker_text_inside_json_string_values() {
    let parsed = only_call(
        "<jcode_tool_call>{\"name\":\"bash\",\"input\":{\"command\":\"printf '</jcode_tool_call>'\"}}</jcode_tool_call>",
    );
    assert_eq!(parsed.input["command"], "printf '</jcode_tool_call>'");
}

#[test]
fn tool_json_preserves_literal_renderer_tags_and_escaped_strings() {
    for literal in [
        "printf '<escape>x</escape>'",
        "write <Cite ref=\"turn0view0\"/> exactly",
        "Unicode λ <escape> and a backslash \\ then \"quoted\" </escape>",
        "<jcode_tool_call></jcode_tool_call><Cite ref=\"literal\"/>",
    ] {
        let payload = json!({"name":"bash", "input":{"command":literal,"intent":literal}});
        let response = format!(
            "<escape>{TOOL_CALL_START}{payload}<escape>{TOOL_CALL_END}</escape> <Cite ref=\"display\"/>"
        );
        let parsed = parse_response(&response).unwrap();
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].input["command"], literal);
        assert_eq!(parsed.calls[0].input["intent"], literal);
        assert!(
            parsed.text.is_empty(),
            "unexpected visible text: {}",
            parsed.text
        );
    }
}

#[test]
fn plain_answers_drop_chatgpt_citation_tokens() {
    let parsed =
        parse_response("The branch is siavz. <Cite ref=\"turn552054view0\"/>\n\nDone.").unwrap();
    assert!(parsed.calls.is_empty());
    assert_eq!(parsed.text, "The branch is siavz.\n\nDone.");
}
