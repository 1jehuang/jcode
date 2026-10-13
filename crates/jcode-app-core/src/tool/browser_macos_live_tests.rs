//! Opt-in checks against the actual macOS browser tool, without model credentials.
//! Run one browser at a time with JCODE_BROWSER_LIVE_BROWSER set. The test creates
//! only a disposable loopback tab and never acts on an existing user page.
use super::{BrowserTool, Tool, ToolContext};
use crate::tool::ToolExecutionMode;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Fixture {
    url: String,
    receipt: String,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn start() -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let receipt = format!("macos-browser-{nonce}");
        let root = format!(
            r#"<!doctype html><html><head><title>{receipt}</title></head>
<body><h1>Jcode browser fixture</h1><p id="receipt">{receipt}</p>
<label>Name <input id="name" autocomplete="off"></label>
<label>Choice <select id="choice"><option value="first">First</option><option value="second">Second</option></select></label>
<button id="next" onclick="location.href='/done?name='+encodeURIComponent(document.querySelector('#name').value)+'&amp;choice='+encodeURIComponent(document.querySelector('#choice').value)">Continue</button>
<a id="link" href="/done">Receipt page</a></body></html>"#
        );
        let done = format!(
            "<!doctype html><html><head><title>{receipt}</title></head><body><h1>Browser workflow passed</h1><p id=\"receipt\">{receipt}</p></body></html>"
        );
        let server = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buffer = vec![0u8; 8192];
                let Ok(Ok(length)) =
                    tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer)).await
                else {
                    continue;
                };
                if length == 0 {
                    continue;
                }
                let request = String::from_utf8_lossy(&buffer[..length]);
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                let body = if path.starts_with("/done") {
                    &done
                } else {
                    &root
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        Ok(Self {
            url: format!("http://127.0.0.1:{port}/"),
            receipt,
            server,
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn context(browser: &str) -> ToolContext {
    ToolContext {
        session_id: format!("macos-browser-live-{}-{browser}", std::process::id()),
        message_id: "macos-browser-live".into(),
        tool_call_id: "macos-browser-live".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    }
}

async fn call(browser: &str, ctx: &ToolContext, mut input: Value) -> Result<Value> {
    input["browser"] = json!(browser);
    input["intent"] = json!("Verify the actual browser on a disposable loopback fixture");
    input["timeout_ms"] = json!(15000);
    let action = input["action"].clone();
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        BrowserTool::new().execute(input, ctx.clone()),
    )
    .await
    .context("browser acceptance action timed out")?;
    if let Err(error) = &output {
        let kind = jcode_base::browser_detect::BrowserKind::parse(browser);
        eprintln!(
            "FAILED action={action} error={error:#} endpoint={:?}",
            kind.and_then(jcode_base::browser::native_host_identity_evidence)
        );
    }
    let output = output?;
    eprintln!("{browser}: {}", output.output);
    output.metadata.context("browser action omitted metadata")
}

fn result(value: &Value) -> &Value {
    value.get("result").unwrap_or(value)
}

#[tokio::test]
#[ignore = "requires JCODE_BROWSER_LIVE_BROWSER, an installed enabled browser integration, and macOS browser automation permissions; creates a disposable loopback tab"]
async fn live_macos_browser_core_workflow() -> Result<()> {
    let browser = std::env::var("JCODE_BROWSER_LIVE_BROWSER")
        .context("Set JCODE_BROWSER_LIVE_BROWSER to brave, chrome, opera, safari, or firefox")?;
    ensure!(
        ["brave", "chrome", "opera", "safari", "firefox"].contains(&browser.as_str()),
        "Refusing an unsupported live-test browser"
    );
    let ctx = context(&browser);
    let status = call(&browser, &ctx, json!({"action":"status"})).await?;
    ensure!(
        status["ready"] == true,
        "Requested browser is not ready: {status}"
    );
    ensure!(
        status["connected_browser"] == browser,
        "Refusing to test a different connected browser: {status}"
    );
    let fixture = Fixture::start().await?;
    let opened = call(
        &browser,
        &ctx,
        if browser == "safari" || browser == "firefox" {
            json!({"action":"new_tab", "url":fixture.url, "focus":false})
        } else {
            json!({"action":"provider_command", "provider_action":"newWindow", "params":{"url":fixture.url, "focus":false}})
        },
    )
    .await?;
    let opened = result(&opened);
    let tab_id = opened["tabId"].as_i64().context("new tab omitted its ID")?;
    let window_id = opened["windowId"].as_i64();
    eprintln!(
        "OWNED_FIXTURE_CLEANUP {}",
        json!({"browser":browser,"tabId":tab_id,"windowId":window_id,"title":fixture.receipt,"url":fixture.url,"scope":if browser == "firefox" {"tab"} else {"window"}})
    );
    let scope = json!({"tab_id":tab_id, "window_id":window_id});
    let action = |name: &str| {
        let mut input = scope.clone();
        input["action"] = json!(name);
        input
    };

    let workflow = async {
    let mut input = action("wait");
    input["selector"] = json!("#receipt");
    call(&browser, &ctx, input).await?;
    let mut input = action("get_content");
    input["format"] = json!("text");
    let content = call(&browser, &ctx, input).await?;
    ensure!(
        content.to_string().contains(&fixture.receipt),
        "Page reading did not return the fresh fixture receipt: {content}"
    );

    let mut input = action("type");
    input["selector"] = json!("#name");
    input["text"] = json!("Jcode macOS verified");
    input["clear"] = json!(true);
    call(&browser, &ctx, input).await?;
    let mut input = action("select");
    input["selector"] = json!("#choice");
    input["text"] = json!("second");
    call(&browser, &ctx, input).await?;
    let mut input = action("eval");
    input["script"] = json!("return {name:document.querySelector('#name').value,choice:document.querySelector('#choice').value};");
    let fields = call(&browser, &ctx, input).await?;
    ensure!(result(&fields)["name"] == "Jcode macOS verified", "Typing did not change the actual input: {fields}");
    ensure!(result(&fields)["choice"] == "second", "Selection did not change the actual select: {fields}");

    let mut input = action("click");
    input["selector"] = json!("#next");
    call(&browser, &ctx, input).await?;
    let mut input = action("wait");
    input["contains"] = json!("Browser workflow passed");
    call(&browser, &ctx, input).await?;
    let mut input = action("eval");
    input["script"] = json!("return {url:location.href,title:document.title,receipt:document.querySelector('#receipt')?.textContent,heading:document.querySelector('h1')?.textContent};");
    let outcome = call(&browser, &ctx, input).await?;
    let outcome = result(&outcome);
    ensure!(outcome["receipt"] == fixture.receipt, "Wrong destination receipt: {outcome}");
    ensure!(outcome["heading"] == "Browser workflow passed", "Destination was not rendered: {outcome}");
    let url = reqwest::Url::parse(outcome["url"].as_str().context("Destination URL missing")?)?;
    ensure!(url.path() == "/done", "The click did not navigate to the destination");
    let fields: std::collections::HashMap<_, _> = url.query_pairs().collect();
    ensure!(fields.get("name").is_some_and(|v| v == "Jcode macOS verified"), "Navigation lost the entered field");
    ensure!(fields.get("choice").is_some_and(|v| v == "second"), "Navigation lost the selected value");
    eprintln!("PASS {browser}: requested browser identity, fresh page reading, typing, selection, click, navigation, and rendered receipt verified");
    Ok::<(), anyhow::Error>(())
    }.await;
    if browser == "safari" {
        call(&browser, &ctx, json!({"action":"provider_command", "provider_action":"closeTab", "params":{"tabId":tab_id,"windowId":window_id}}))
            .await.context("Failed to close the disposable owned Safari window")?;
    }
    workflow
}
