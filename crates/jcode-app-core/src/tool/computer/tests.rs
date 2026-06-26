//! Tests for the computer tool. Pure-logic tests run anywhere on macOS; live
//! tests that synthesize events / capture the screen are `#[ignore]`d.

use super::*;
use jcode_tool_core::{ToolContext, ToolExecutionMode};

fn ctx() -> ToolContext {
    ToolContext {
        session_id: "test".into(),
        message_id: "test".into(),
        tool_call_id: "test".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    }
}

async fn run_action(v: Value) -> Result<ToolOutput> {
    ComputerTool::new().execute(v, ctx()).await
}

// ---- pure logic ----

#[tokio::test]
async fn rejects_bad_action() {
    let err = run_action(json!({ "action": "frobnicate" }))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Unknown computer action"));
}

#[tokio::test]
async fn move_requires_coords() {
    let err = run_action(json!({ "action": "move" })).await.unwrap_err();
    assert!(err.to_string().contains("requires"));
}

#[tokio::test]
async fn discover_all_lists_actions() {
    let out = run_action(json!({ "action": "discover", "category": "all" }))
        .await
        .unwrap();
    // Spot-check that several categories are present.
    for needle in ["press", "set_value", "run_applescript", "list_windows", "screenshot"] {
        assert!(out.output.contains(needle), "missing {needle}");
    }
}

#[tokio::test]
async fn discover_category_scopes() {
    let out = run_action(json!({ "action": "discover", "category": "ax" }))
        .await
        .unwrap();
    assert!(out.output.contains("find_element"));
    assert!(!out.output.contains("set_brightness"));
}

#[tokio::test]
async fn press_requires_element() {
    let err = run_action(json!({ "action": "press" })).await.unwrap_err();
    assert!(err.to_string().contains("element"));
}

#[tokio::test]
async fn dry_run_skips_mutation() {
    let out = run_action(json!({ "action": "click", "x": 10, "y": 10, "dry_run": true }))
        .await
        .unwrap();
    assert!(out.output.contains("dry_run"));
    assert!(out.output.contains("click"));
}

#[tokio::test]
async fn dry_run_ignored_for_readonly() {
    let out = run_action(json!({ "action": "discover", "category": "ax", "dry_run": true }))
        .await
        .unwrap();
    assert!(out.output.contains("find_element"));
}

#[test]
fn cap_output_truncates() {
    let big = "x".repeat(20_000);
    let capped = super::cap_output(ToolOutput::new(big), 16_000);
    assert!(capped.output.len() < 16_200);
    assert!(capped.output.contains("truncated"));
}

#[test]
fn is_mutating_classifies() {
    assert!(super::is_mutating("click"));
    assert!(super::is_mutating("quit_app"));
    assert!(super::is_mutating("set_value"));
    assert!(!super::is_mutating("screenshot"));
    assert!(!super::is_mutating("ui"));
    assert!(!super::is_mutating("discover"));
}

#[test]
fn schema_declares_every_input_field() {
    // Regression guard for the "schema omits half its fields" bug: every field
    // `dispatch` can require must be declared in `parameters_schema()`, or the
    // model can never send it and the action is dead on arrival.
    let tool = ComputerTool::new();
    let schema = tool.parameters_schema();
    let props = schema["properties"].as_object().expect("properties object");
    for field in [
        "action", "category", "x", "y", "to_x", "to_y", "w", "h", "text", "keys", "dx", "dy",
        "depth", "app", "role", "title", "value", "element", "ax_action", "menu_path",
        "window_id", "script", "contains", "timeout_ms", "region", "level", "dry_run",
    ] {
        assert!(props.contains_key(field), "schema is missing field `{field}`");
    }
}

#[test]
fn schema_is_compact() {
    // Guard against context bloat: the always-on schema + description must stay
    // small. Action *specs* live in `discover` (progressive disclosure), but
    // every input *field* must be declared here or the model can't send it, so
    // the field set (not the action set) sets the floor. Keep always-on cost
    // roughly under ~900 tokens; alert if it balloons past that.
    let tool = ComputerTool::new();
    let schema = serde_json::to_string(&tool.parameters_schema()).unwrap();
    let total = tool.description().len() + schema.len();
    assert!(
        total < 3500,
        "computer tool always-on size grew to {total} chars (~{} tokens)",
        total / 4
    );
}

// ---- live (need GUI + permissions); run with --ignored ----

#[tokio::test]
#[ignore = "requires GUI + permissions"]
async fn live_check_permissions() {
    let out = run_action(json!({ "action": "check_permissions" }))
        .await
        .unwrap();
    eprintln!("{}", out.output);
    assert!(out.metadata.is_some());
}

#[tokio::test]
#[ignore = "requires GUI + permissions"]
async fn live_cursor_and_move() {
    run_action(json!({ "action": "move", "x": 400, "y": 300 }))
        .await
        .unwrap();
    let after = run_action(json!({ "action": "cursor" })).await.unwrap();
    let meta = after.metadata.unwrap();
    assert!((meta["x"].as_f64().unwrap() - 400.0).abs() < 5.0);
    assert!((meta["y"].as_f64().unwrap() - 300.0).abs() < 5.0);
}

#[tokio::test]
#[ignore = "requires GUI + permissions"]
async fn live_screenshot() {
    let out = run_action(json!({ "action": "screenshot" })).await.unwrap();
    assert_eq!(out.images.len(), 1);
    assert_eq!(out.images[0].media_type, "image/png");
    eprintln!("{}", out.output);
}

#[tokio::test]
#[ignore = "requires GUI + permissions"]
async fn live_ui_tree() {
    let out = run_action(json!({ "action": "ui", "depth": 3 }))
        .await
        .unwrap();
    eprintln!("{}", out.output);
    assert!(out.output.contains("App:"));
}

#[tokio::test]
#[ignore = "requires GUI + permissions"]
async fn live_list_windows() {
    let out = run_action(json!({ "action": "list_windows" })).await.unwrap();
    eprintln!("{}", out.output);
}

#[tokio::test]
#[ignore = "requires GUI + permissions"]
async fn live_clipboard_roundtrip() {
    run_action(json!({ "action": "set_clipboard", "text": "jcode-clip-test" }))
        .await
        .unwrap();
    let out = run_action(json!({ "action": "get_clipboard" })).await.unwrap();
    assert!(out.output.contains("jcode-clip-test"));
}

#[tokio::test]
#[ignore = "requires GUI + permissions"]
async fn live_applescript() {
    let out = run_action(json!({ "action": "run_applescript", "script": "return 2 + 2" }))
        .await
        .unwrap();
    assert!(out.output.contains("4"));
}

/// Headline capability: set a TextEdit field's value via AX while TextEdit is
/// NOT frontmost, proving background control with no cursor movement.
#[tokio::test]
#[ignore = "requires GUI + permissions"]
async fn live_background_set_value() {
    // Open a fresh TextEdit document.
    run_action(json!({
        "action": "run_applescript",
        "script": "tell application \"TextEdit\" to activate\ndelay 0.4\ntell application \"TextEdit\" to make new document\ndelay 0.4"
    }))
    .await
    .unwrap();

    // Move focus away so TextEdit is in the background.
    run_action(json!({
        "action": "run_applescript",
        "script": "tell application \"System Events\" to set frontmost of (first process whose name is \"System Events\") to true"
    }))
    .await
    .ok();

    let marker = "background-ax-marker-42";
    // Set the text area value by AX path (AXScrollArea[1] -> AXTextArea[1]).
    run_action(json!({
        "action": "set_value",
        "element": { "app": "TextEdit", "path": [1, 1] },
        "value": marker
    }))
    .await
    .unwrap();

    let content = run_action(json!({
        "action": "run_applescript",
        "script": "tell application \"TextEdit\" to get text of document 1"
    }))
    .await
    .unwrap();
    assert!(content.output.contains(marker), "got: {}", content.output);

    // Cleanup.
    run_action(json!({
        "action": "run_applescript",
        "script": "tell application \"TextEdit\" to close every document saving no\ntell application \"TextEdit\" to quit"
    }))
    .await
    .ok();
}
