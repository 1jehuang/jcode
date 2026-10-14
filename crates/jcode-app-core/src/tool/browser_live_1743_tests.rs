//! Live acceptance checks for #1743 through the public `browser` tool against
//! a real bridge and a real Chromium. Each check prints `REQ <id> PASS|FAIL`
//! so the same file runs on the pre-fix tree for a before/after comparison.
//!
//! Needs an isolated browser: its own XDG_RUNTIME_DIR, profile, and JCODE_HOME
//! with the bridge installed. Env:
//!   JCODE_LIVE_CHROME=1, JCODE_LIVE_DEVTOOLS_PORT, JCODE_LIVE_FIXTURE (http
//!   base serving /ok, /slow, /login, /long, /upload and a hanging asset).
//!   Run the bridge host on its own FAB_WS_PORT so it never reaches the
//!   user's real browser.
use super::*;
use crate::tool::{Tool, ToolContext, ToolExecutionMode};
use serde_json::{Value, json};

fn ctx() -> ToolContext {
    ToolContext {
        session_id: format!("live1743-{}", std::process::id()),
        message_id: "m".into(),
        tool_call_id: "t".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    }
}

async fn call(input: Value) -> std::result::Result<crate::tool::ToolOutput, String> {
    BrowserTool::new()
        .execute(input, ctx())
        .await
        .map_err(|e| format!("{e:#}"))
}

async fn devtools(path: &str, method: reqwest::Method) -> Value {
    let port = std::env::var("JCODE_LIVE_DEVTOOLS_PORT").unwrap();
    reqwest::Client::new()
        .request(method, format!("http://127.0.0.1:{port}{path}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap_or(Value::Null)
}

fn report(id: &str, ok: bool, detail: impl std::fmt::Display) -> bool {
    let detail = detail.to_string();
    let detail: String = detail
        .chars()
        .take(240)
        .collect::<String>()
        .replace('\n', " ");
    eprintln!("REQ {id} {} :: {detail}", if ok { "PASS" } else { "FAIL" });
    ok
}

#[tokio::test]
#[ignore = "needs an isolated live Chromium with the bridge extension"]
async fn live_issue_1743_acceptance() {
    assert!(std::env::var("JCODE_LIVE_CHROME").is_ok());
    let base = std::env::var("JCODE_LIVE_FIXTURE").unwrap();
    let mut all = true;

    // A "user" tab opened before the agent starts, scrolled down.
    devtools(&format!("/json/new?{base}/long"), reqwest::Method::PUT).await;
    let user_url = format!("{base}/long");
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let user_tab = {
        let listing = call(json!({"action":"list_tabs","browser":"chrome"})).await;
        listing.ok().and_then(|o| o.metadata).and_then(|m| {
            m["windows"]
                .as_array()?
                .iter()
                .flat_map(|w| w["tabs"].as_array().cloned().unwrap_or_default())
                .find(|t| t["url"] == user_url.as_str())
                .and_then(|t| t["tabId"].as_i64())
        })
    };
    if let Some(tab) = user_tab {
        let _ = call(json!({"action":"eval","browser":"chrome","tab_id":tab,"script":"window.scrollTo(0,1500); return scrollY"})).await;
    }

    // R1 open on a page that never finishes loading
    let started = std::time::Instant::now();
    let slow = call(json!({"action":"open","browser":"chrome","url":format!("{base}/slow")})).await;
    all &= report(
        "R1-slow-page-open",
        slow.as_ref().is_ok_and(|o| o.output.contains("Dashboard")),
        format!(
            "{:?} after {:?}",
            slow.as_ref().map(|o| &o.output).map_err(|e| e),
            started.elapsed()
        ),
    );

    // R2 user's own tabs visible
    let listing = call(json!({"action":"list_tabs","browser":"chrome"})).await;
    let sees_user = listing
        .as_ref()
        .ok()
        .and_then(|o| o.metadata.clone())
        .is_some_and(|m| {
            m["windows"].as_array().is_some_and(|ws| {
                ws.iter().any(|w| {
                    w["tabs"]
                        .as_array()
                        .is_some_and(|ts| ts.iter().any(|t| t["url"] == user_url.as_str()))
                })
            })
        });
    all &= report(
        "R2-user-tabs-visible",
        sees_user && user_tab.is_some(),
        format!("user_tab={user_tab:?} listing_ok={}", listing.is_ok()),
    );

    // R3 acting on the user's tab does not need a new login (same tab, same cookies)
    if let Some(tab) = user_tab {
        let read =
            call(json!({"action":"get_content","browser":"chrome","tab_id":tab,"format":"title"}))
                .await;
        all &= report(
            "R3-act-in-user-tab",
            read.as_ref().is_ok_and(|o| o.output.contains("Long")),
            format!("{:?}", read.map(|o| o.output)),
        );
    }

    // R4 user's scroll position untouched by agent actions elsewhere
    let _ = call(json!({"action":"open","browser":"chrome","url":format!("{base}/ok")})).await;
    let _ = call(json!({"action":"get_content","browser":"chrome"})).await;
    if let Some(tab) = user_tab {
        let y = call(
            json!({"action":"eval","browser":"chrome","tab_id":tab,"script":"return scrollY"}),
        )
        .await;
        all &= report(
            "R4-user-scroll-untouched",
            y.as_ref().is_ok_and(|o| o.output.starts_with("1500")),
            format!("{:?}", y.map(|o| o.output)),
        );
    }

    // R5 upload a document
    let file = std::env::temp_dir().join("jcode-1743-upload.txt");
    std::fs::write(&file, "hello").unwrap();
    let _ = call(json!({"action":"open","browser":"chrome","url":format!("{base}/upload")})).await;
    let up = call(json!({"action":"upload","browser":"chrome","selector":"#f","path":file})).await;
    let name = call(json!({"action":"eval","browser":"chrome","script":"return document.getElementById('f').files[0]?.name || 'none'"})).await;
    all &= report(
        "R5-upload",
        name.as_ref()
            .is_ok_and(|o| o.output.contains("jcode-1743-upload.txt")),
        format!(
            "upload={:?} file={:?}",
            up.map(|o| o.output),
            name.map(|o| o.output)
        ),
    );

    // R6 login page: hand back to the user with resume guidance
    let opened =
        call(json!({"action":"open","browser":"chrome","url":format!("{base}/login")})).await;
    let tab = opened
        .ok()
        .and_then(|o| o.metadata)
        .and_then(|m| m["tabId"].as_i64());
    let handoff = call(
        json!({"action":"handoff","browser":"chrome","tab_id":tab,"goal":"Open the settings page"}),
    )
    .await;
    let meta = handoff
        .as_ref()
        .ok()
        .and_then(|o| o.metadata.clone())
        .unwrap_or(Value::Null);
    all &= report(
        "R6-login-handback",
        matches!(
            meta["requested_help"].as_str(),
            Some("user_login") | Some("direct_actions")
        ),
        format!("help={} reason={}", meta["requested_help"], meta["reason"]),
    );

    // R7 user closes the agent's window: next call still works
    let before = call(json!({"action":"get_active_tab","browser":"chrome"})).await;
    let agent_url = before
        .as_ref()
        .ok()
        .and_then(|o| o.metadata.clone())
        .and_then(|m| m["url"].as_str().map(str::to_owned));
    if let Some(url) = &agent_url {
        let targets = devtools("/json", reqwest::Method::GET).await;
        for t in targets.as_array().into_iter().flatten() {
            if t["url"] == url.as_str() && t["type"] == "page" {
                devtools(
                    &format!("/json/close/{}", t["id"].as_str().unwrap()),
                    reqwest::Method::PUT,
                )
                .await;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    }
    let after = call(json!({"action":"open","browser":"chrome","url":format!("{base}/ok")})).await;
    all &= report(
        "R7-closed-agent-window",
        after.as_ref().is_ok_and(|o| o.output.contains("OK page")),
        format!("{:?}", after.map(|o| o.output)),
    );
    let again = call(json!({"action":"get_content","browser":"chrome","format":"title"})).await;
    all &= report(
        "R7b-later-calls-ok",
        again.is_ok(),
        format!("{:?}", again.map(|o| o.output)),
    );

    // R8 internal pages: actionable error
    let internal = call(json!({"action":"open","browser":"chrome","url":"chrome://version"})).await;
    let snap = call(json!({"action":"scroll","browser":"chrome","position":"top"})).await;
    all &= report(
        "R8-internal-page-error",
        snap.as_ref().is_err_and(|e| e.contains("internal pages")),
        format!("open={:?} scroll={:?}", internal.is_ok(), snap.err()),
    );

    assert!(all, "see REQ lines");
}
