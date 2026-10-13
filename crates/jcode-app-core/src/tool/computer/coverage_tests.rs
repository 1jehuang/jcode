//! Exhaustive live coverage of EVERY computer action. These mutate the desktop
//! (open TextEdit, move windows, clipboard, etc.) so they are `#[ignore]`d and
//! run explicitly:
//!   cargo test -p jcode-app-core tool::computer::coverage -- --ignored --nocapture --test-threads=1
//!
//! Each test asserts the action returns Ok and (where checkable) the expected
//! effect. The goal is to prove no action panics or silently misbehaves.

use super::*;
use jcode_tool_core::{ToolContext, ToolExecutionMode};

fn ctx() -> ToolContext {
    ToolContext {
        session_id: "cov".into(),
        message_id: "cov".into(),
        tool_call_id: "cov".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    }
}

async fn act(v: Value) -> Result<ToolOutput> {
    ComputerTool::new().execute(v, ctx()).await
}

async fn ok(v: Value) -> ToolOutput {
    let label = v.to_string();
    match act(v).await {
        Ok(o) => {
            eprintln!("PASS {label} -> {}", o.output.lines().next().unwrap_or(""));
            o
        }
        Err(e) => panic!("FAIL {label} -> {e}"),
    }
}

async fn textedit_new() {
    ok(json!({"action":"run_applescript","script":
        "tell application \"TextEdit\" to activate\ndelay 0.3\ntell application \"TextEdit\" to make new document\ndelay 0.3"})).await;
}
async fn textedit_quit() {
    let _ = act(json!({"action":"run_applescript","script":
        "tell application \"TextEdit\" to close every document saving no\ntell application \"TextEdit\" to quit"})).await;
}

#[tokio::test]
#[ignore = "live"]
async fn coverage_observe() {
    ok(json!({"action":"check_permissions"})).await;
    ok(json!({"action":"setup"})).await; // already granted -> reports ready quickly
    ok(json!({"action":"screenshot"})).await;
    ok(json!({"action":"cursor"})).await;
    ok(json!({"action":"system_state"})).await;
    ok(json!({"action":"discover","category":"all"})).await;
    // OCR may require swift; tolerate absence but it should not panic.
    match act(json!({"action":"ocr"})).await {
        Ok(o) => eprintln!("PASS ocr -> {}", o.output.lines().next().unwrap_or("")),
        Err(e) => eprintln!("SKIP ocr -> {e}"),
    }
}

#[tokio::test]
#[ignore = "live"]
async fn coverage_input() {
    ok(json!({"action":"move","x":300,"y":300})).await;
    ok(json!({"action":"click","x":300,"y":300})).await;
    ok(json!({"action":"double_click","x":300,"y":300})).await;
    ok(json!({"action":"right_click","x":300,"y":300})).await;
    // dismiss any context menu
    ok(json!({"action":"key","keys":"esc"})).await;
    ok(json!({"action":"drag","x":300,"y":300,"to_x":320,"to_y":320})).await;
    ok(json!({"action":"scroll","x":400,"y":400,"dy":-3})).await;
    ok(json!({"action":"key_down","keys":"shift"})).await;
    ok(json!({"action":"key_up","keys":"shift"})).await;
}

#[tokio::test]
#[ignore = "live"]
async fn coverage_keyboard_into_textedit() {
    textedit_new().await;
    // type goes to focused app (TextEdit just activated)
    ok(json!({"action":"type","text":"hello "})).await;
    ok(json!({"action":"key","keys":"cmd+a"})).await; // select all
    ok(json!({"action":"key","keys":"delete"})).await;
    textedit_quit().await;
}

#[tokio::test]
#[ignore = "live"]
async fn coverage_ax() {
    textedit_new().await;
    ok(json!({"action":"ui","app":"TextEdit","depth":3})).await;
    ok(json!({"action":"find_element","app":"TextEdit","role":"AXTextArea"})).await;
    ok(json!({"action":"element_at","app":"TextEdit","x":700,"y":400})).await;
    // background set/get on the text area (path 1.1)
    let el = json!({"app":"TextEdit","path":[1,1]});
    ok(json!({"action":"set_value","element":el,"value":"ax-coverage"})).await;
    let g = ok(json!({"action":"get_value","element":el})).await;
    assert!(
        g.output.contains("ax-coverage"),
        "get_value got: {}",
        g.output
    );
    textedit_quit().await;
}

#[tokio::test]
#[ignore = "live"]
async fn coverage_select_menu() {
    textedit_new().await;
    // Format menu exists in TextEdit; "Make Plain Text" or "Wrap to Page" toggles.
    // Use a stable, reversible item: Edit > Select All.
    let r = act(json!({"action":"select_menu","app":"TextEdit","menu_path":["Edit","Select All"]}))
        .await;
    match r {
        Ok(o) => eprintln!("PASS select_menu -> {}", o.output),
        Err(e) => panic!("FAIL select_menu -> {e}"),
    }
    textedit_quit().await;
}

#[tokio::test]
#[ignore = "live"]
async fn coverage_windows_apps() {
    textedit_new().await;
    ok(json!({"action":"list_apps"})).await;
    ok(json!({"action":"list_windows"})).await;
    ok(json!({"action":"activate_app","app":"TextEdit"})).await;
    ok(json!({"action":"move_window","app":"TextEdit","x":120,"y":120})).await;
    ok(json!({"action":"resize_window","app":"TextEdit","w":700,"h":500})).await;
    ok(json!({"action":"focus_window","app":"TextEdit"})).await;
    // window_screenshot needs an id from list_windows; find TextEdit's.
    let lw = ok(json!({"action":"list_windows"})).await;
    if let Some(id) = first_window_id_for(&lw.output, "TextEdit") {
        ok(json!({"action":"window_screenshot","window_id":id})).await;
    } else {
        eprintln!("SKIP window_screenshot (no TextEdit window id parsed)");
    }
    ok(json!({"action":"minimize_window","app":"TextEdit"})).await;
    // restore + close
    ok(json!({"action":"activate_app","app":"TextEdit"})).await;
    textedit_quit().await;
}

#[tokio::test]
#[ignore = "live"]
async fn coverage_clipboard_scripting_system() {
    ok(json!({"action":"set_clipboard","text":"cov-clip"})).await;
    let c = ok(json!({"action":"get_clipboard"})).await;
    assert!(c.output.contains("cov-clip"));
    ok(json!({"action":"run_applescript","script":"return 7 * 6"})).await;
    ok(json!({"action":"run_jxa","script":"2 + 3"})).await;
    ok(json!({"action":"notify","text":"jcode coverage test","title":"jcode"})).await;
    // wait_for against a known app/text with short timeout (Finder always has a menu)
    textedit_new().await;
    let _ =
        act(json!({"action":"wait_for","app":"TextEdit","contains":"","timeout_ms":1500})).await;
    textedit_quit().await;
    // set_brightness may be unavailable; tolerate.
    match act(json!({"action":"set_brightness","level":0.8})).await {
        Ok(o) => eprintln!("PASS set_brightness -> {}", o.output),
        Err(e) => eprintln!("SKIP set_brightness -> {e}"),
    }
}

#[tokio::test]
#[ignore = "live"]
async fn coverage_destructive_quit_close() {
    textedit_new().await;
    ok(json!({"action":"close_window","app":"TextEdit"})).await;
    // A new empty doc closes without a sheet. Discard anything then quit.
    let _ = act(json!({"action":"run_applescript","script":
        "tell application \"TextEdit\" to close every document saving no"}))
    .await;
    ok(json!({"action":"quit_app","app":"TextEdit"})).await;
}

/// Parse the first CG window id whose owner matches `owner` from list_windows
/// output lines of the form: "<id>\t<owner>\t<title>\t<bounds>".
fn first_window_id_for(text: &str, owner: &str) -> Option<i64> {
    for line in text.lines() {
        let mut parts = line.splitn(4, '\t');
        let id = parts.next()?.trim();
        let own = parts.next().unwrap_or("").trim();
        if own == owner
            && let Ok(n) = id.parse::<i64>()
        {
            return Some(n);
        }
    }
    None
}
