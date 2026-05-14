use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;

pub struct BrowserTool;

static FIREFOX_PROVIDER: FirefoxBridgeProvider = FirefoxBridgeProvider;
static CLOAK_PROVIDER: CloakBrowserProvider = CloakBrowserProvider;

impl BrowserTool {
    pub fn new() -> Self {
        Self
    }
}

fn browser_tool_description_text() -> &'static str {
    "Control the browser. Use action='status' to check whether the browser bridge is ready. Use action='setup' only for first-time install or repair when status shows the bridge is not already ready. Do not run setup before every browser task."
}

#[derive(Debug, Deserialize)]
struct BrowserInput {
    action: String,
    #[serde(default)]
    browser: Option<String>,
    #[serde(default)]
    provider_action: Option<String>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    tab_id: Option<i64>,
    #[serde(default)]
    frame_id: Option<i64>,
    #[serde(default)]
    all_frames: Option<bool>,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    contains: Option<String>,
    #[serde(default)]
    script: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    wait: Option<bool>,
    #[serde(default)]
    new_tab: Option<bool>,
    #[serde(default)]
    focus: Option<bool>,
    #[serde(default)]
    clear: Option<bool>,
    #[serde(default)]
    submit: Option<bool>,
    #[serde(default)]
    page_world: Option<bool>,
    #[serde(default)]
    position: Option<String>,
    #[serde(default)]
    behavior: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    fields: Option<Vec<BrowserField>>,
    #[serde(default)]
    scroll_to: Option<ScrollTo>,
}

#[derive(Debug, Deserialize)]
struct BrowserField {
    selector: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    checked: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ScrollTo {
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
}

#[async_trait]
trait BrowserProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn supported_browsers(&self) -> &'static [&'static str];

    async fn status(&self, ctx: &ToolContext) -> Result<ToolOutput>;
    async fn setup(&self) -> Result<ToolOutput>;
    async fn ensure_ready(&self) -> Result<Option<String>>;
    async fn execute(
        &self,
        action: &str,
        input: &BrowserInput,
        ctx: &ToolContext,
    ) -> Result<ToolOutput>;
}

struct FirefoxBridgeProvider;

#[async_trait]
impl BrowserProvider for FirefoxBridgeProvider {
    fn id(&self) -> &'static str {
        "firefox_agent_bridge"
    }

    fn supported_browsers(&self) -> &'static [&'static str] {
        &["auto", "firefox"]
    }

    async fn status(&self, ctx: &ToolContext) -> Result<ToolOutput> {
        Ok(attach_browser_metadata(
            firefox_status(self, ctx).await?,
            self.id(),
            "firefox",
        ))
    }

    async fn setup(&self) -> Result<ToolOutput> {
        Ok(attach_browser_metadata(
            firefox_setup(self).await?,
            self.id(),
            "firefox",
        ))
    }

    async fn ensure_ready(&self) -> Result<Option<String>> {
        ensure_firefox_ready().await
    }

    async fn execute(
        &self,
        action: &str,
        input: &BrowserInput,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        Ok(attach_browser_metadata(
            execute_firefox_action(self, action, input, ctx).await?,
            self.id(),
            "firefox",
        ))
    }
}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        browser_tool_description_text()
    }

    fn parameters_schema(&self) -> Value {
        let mut properties = Map::new();
        properties.insert("intent".into(), super::intent_schema_property());
        properties.insert(
            "action".into(),
            json!({
                "type": "string",
                "enum": [
                    "status", "setup", "list_tabs", "new_tab", "select_tab", "get_active_tab",
                    "list_frames", "open", "snapshot", "get_content", "interactables", "click", "type",
                    "fill_form", "select", "wait", "screenshot", "eval", "scroll", "upload",
                    "press", "provider_command"
                ],
                "description": "Action. Use 'status' to check readiness first. Use 'setup' only for first-time install or repair, not before every browser task."
            }),
        );
        properties.insert(
            "browser".into(),
            json!({
                "type": "string",
                "enum": ["auto", "firefox", "chrome", "safari", "edge"],
                "description": "Browser."
            }),
        );
        properties.insert(
            "provider_action".into(),
            json!({
                "type": "string",
                "description": "Provider command name."
            }),
        );
        properties.insert(
            "params".into(),
            json!({
                "type": "object",
                "description": "Raw provider params."
            }),
        );
        for (name, schema) in [
            ("url", json!({"type": "string"})),
            ("tab_id", json!({"type": "integer"})),
            ("frame_id", json!({"type": "integer"})),
            ("all_frames", json!({"type": "boolean"})),
            ("selector", json!({"type": "string"})),
            ("text", json!({"type": "string"})),
            ("contains", json!({"type": "string"})),
            ("script", json!({"type": "string"})),
            ("key", json!({"type": "string"})),
            ("x", json!({"type": "number"})),
            ("y", json!({"type": "number"})),
            ("wait", json!({"type": "boolean"})),
            ("new_tab", json!({"type": "boolean"})),
            ("focus", json!({"type": "boolean"})),
            ("clear", json!({"type": "boolean"})),
            ("submit", json!({"type": "boolean"})),
            ("page_world", json!({"type": "boolean"})),
            ("position", json!({"type": "string"})),
            ("behavior", json!({"type": "string"})),
            ("timeout_ms", json!({"type": "integer"})),
            ("path", json!({"type": "string"})),
        ] {
            properties.insert(name.into(), schema);
        }
        properties.insert(
            "format".into(),
            json!({
                "type": "string",
                "enum": ["annotated", "text", "textFast", "html", "title"],
                "description": "Format."
            }),
        );
        properties.insert(
            "fields".into(),
            json!({
                "type": "array",
                "description": "Form fields.",
                "items": {
                    "type": "object",
                    "required": ["selector"],
                    "properties": {
                        "selector": { "type": "string" },
                        "value": { "type": "string" },
                        "checked": { "type": "boolean" }
                    }
                }
            }),
        );
        properties.insert(
            "scroll_to".into(),
            json!({
                "type": "object",
                "properties": {
                    "x": { "type": "number" },
                    "y": { "type": "number" }
                }
            }),
        );
        Value::Object(Map::from_iter([
            ("type".into(), json!("object")),
            ("required".into(), json!(["action"])),
            ("properties".into(), Value::Object(properties)),
        ]))
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: BrowserInput = serde_json::from_value(input)?;
        let provider = resolve_provider(params.browser.as_deref()).await?;

        match params.action.as_str() {
            "status" => provider.status(&ctx).await,
            "setup" => provider.setup().await,
            other => {
                let setup_message = provider.ensure_ready().await?;
                let output = provider.execute(other, &params, &ctx).await?;
                Ok(match setup_message {
                    Some(message) if !message.is_empty() => prepend_setup_message(output, &message),
                    _ => output,
                })
            }
        }
    }
}

fn prepend_setup_message(mut output: ToolOutput, message: &str) -> ToolOutput {
    output.output = format!("{}\n\n{}", message, output.output);
    if output.title.is_none() {
        output.title = Some("browser".to_string());
    }

    let mut metadata = match output.metadata.take() {
        Some(Value::Object(map)) => map,
        Some(other) => {
            let mut map = Map::new();
            map.insert("result".into(), other);
            map
        }
        None => Map::new(),
    };
    metadata.insert("setup_ran".into(), json!(true));
    output.metadata = Some(Value::Object(metadata));
    output
}

fn attach_browser_metadata(
    mut output: ToolOutput,
    backend: &'static str,
    browser: &'static str,
) -> ToolOutput {
    let mut metadata = match output.metadata.take() {
        Some(Value::Object(map)) => map,
        Some(other) => {
            let mut map = Map::new();
            map.insert("result".into(), other);
            map
        }
        None => Map::new(),
    };
    metadata.insert("backend".into(), json!(backend));
    metadata.insert("browser".into(), json!(browser));
    output.metadata = Some(Value::Object(metadata));
    output
}

async fn resolve_provider(browser: Option<&str>) -> Result<&'static dyn BrowserProvider> {
    let browser = browser.unwrap_or("auto");
    if browser == "auto" {
        if FIREFOX_PROVIDER.ensure_ready().await.is_ok() {
            return Ok(&FIREFOX_PROVIDER);
        }
        if CLOAK_PROVIDER.ensure_ready().await.is_ok() {
            return Ok(&CLOAK_PROVIDER);
        }
        return Ok(&FIREFOX_PROVIDER);
    }
    if FIREFOX_PROVIDER.supported_browsers().contains(&browser) {
        return Ok(&FIREFOX_PROVIDER);
    }
    if CLOAK_PROVIDER.supported_browsers().contains(&browser) {
        return Ok(&CLOAK_PROVIDER);
    }

    anyhow::bail!(
        "Browser backend '{}' is not wired into the built-in browser tool yet. Use auto/firefox/chrome for now.",
        browser
    )
}

struct CloakBrowserProvider;

#[async_trait]
impl BrowserProvider for CloakBrowserProvider {
    fn id(&self) -> &'static str {
        "cloakbrowser_playwright"
    }
    fn supported_browsers(&self) -> &'static [&'static str] {
        &["chrome"]
    }

    async fn status(&self, _ctx: &ToolContext) -> Result<ToolOutput> {
        match cloak_python_check().await {
            Ok(version) => Ok(ToolOutput::new(format!("CloakBrowser fallback is available via Python module cloakbrowser ({}).", version))
                .with_title("browser status")
                .with_metadata(json!({"ready": true, "backend": self.id(), "browser": "chrome", "module_installed": true}))),
            Err(err) => Ok(ToolOutput::new(format!("CloakBrowser fallback is not available yet. Install it with `python3 -m pip install cloakbrowser`, or run browser action='setup' with browser='chrome'.\n\n{}", err))
                .with_title("browser status")
                .with_metadata(json!({"ready": false, "backend": self.id(), "browser": "chrome", "module_installed": false}))),
        }
    }

    async fn setup(&self) -> Result<ToolOutput> {
        let output = tokio::process::Command::new(cloak_python_bin())
            .args(["-m", "pip", "install", "cloakbrowser"])
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .context("failed to run python3 -m pip install cloakbrowser")?;
        let mut log = String::new();
        log.push_str(&String::from_utf8_lossy(&output.stdout));
        log.push_str(&String::from_utf8_lossy(&output.stderr));
        let ready = output.status.success() && cloak_python_check().await.is_ok();
        Ok(ToolOutput::new(log)
            .with_title(if ready {
                "browser setup"
            } else {
                "browser setup (incomplete)"
            })
            .with_metadata(json!({
                "ready": ready, "backend": self.id(), "browser": "chrome", "module_installed": ready
            })))
    }

    async fn ensure_ready(&self) -> Result<Option<String>> {
        cloak_python_check().await.map(|_| None)
    }

    async fn execute(
        &self,
        action: &str,
        input: &BrowserInput,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let result = cloak_run_action(action, input, ctx).await?;
        if action == "screenshot" {
            return cloak_screenshot_output(result, self.id(), "chrome").await;
        }
        Ok(attach_browser_metadata(
            render_browser_output(action, format!("browser {}", action), result),
            self.id(),
            "chrome",
        ))
    }
}

async fn cloak_screenshot_output(
    result: Value,
    backend: &'static str,
    browser: &'static str,
) -> Result<ToolOutput> {
    let saved = result
        .get("saved")
        .and_then(|v| v.as_str())
        .map(PathBuf::from);
    let mut output = ToolOutput::new(match &saved {
        Some(path) => format!("Captured browser screenshot to {}.", path.display()),
        None => "Captured browser screenshot.".to_string(),
    })
    .with_title("browser screenshot")
    .with_metadata(result.clone());

    if let Some(path) = saved
        && let Ok(bytes) = tokio::fs::read(&path).await
    {
        output = output.with_labeled_image(
            "image/png",
            STANDARD.encode(&bytes),
            format!("browser screenshot: {}", path.display()),
        );
        let _ = tokio::fs::remove_file(path).await;
    }

    Ok(attach_browser_metadata(output, backend, browser))
}

fn cloak_python_bin() -> String {
    std::env::var("JCODE_CLOAKBROWSER_PYTHON").unwrap_or_else(|_| "python3".to_string())
}

async fn cloak_python_check() -> Result<String> {
    let output = tokio::process::Command::new(cloak_python_bin())
        .args([
            "-c",
            "import cloakbrowser; print(getattr(cloakbrowser, '__version__', 'installed'))",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .await?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        anyhow::bail!(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

async fn cloak_run_action(action: &str, input: &BrowserInput, ctx: &ToolContext) -> Result<Value> {
    if !matches!(
        action,
        "open" | "snapshot" | "get_content" | "screenshot" | "eval" | "click" | "type" | "wait"
    ) {
        anyhow::bail!(
            "CloakBrowser fallback currently supports open, snapshot, get_content, screenshot, eval, click, type, and wait. Use Firefox bridge for '{}'.",
            action
        );
    }
    let request = json!({
        "action": action,
        "url": input.url,
        "selector": input.selector,
        "text": input.text,
        "script": input.script,
        "format": input.format,
        "wait": input.wait,
        "timeout_ms": input.timeout_ms,
        "screenshot_path": if action == "screenshot" { Some(temp_screenshot_path().to_string_lossy().to_string()) } else { None::<String> },
        "profile_dir": cloak_profile_dir(&ctx.session_id).to_string_lossy().to_string(),
    });
    let mut child = tokio::process::Command::new(cloak_python_bin())
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to start CloakBrowser Python helper")?;
    let script = format!(
        "{}\nREQ = {}\nmain(REQ)\n",
        CLOAK_HELPER_PY,
        serde_json::to_string(&request)?
    );
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .await?;
    drop(child.stdin.take());
    let output = child.wait_with_output().await?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        anyhow::bail!(if stderr.is_empty() { stdout } else { stderr });
    }
    serde_json::from_str(&stdout).or_else(|_| Ok(json!({"raw": stdout})))
}

fn cloak_profile_dir(session_id: &str) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(".jcode")
        .join("cloakbrowser")
        .join(session_id)
}

const CLOAK_HELPER_PY: &str = r#"
import json, pathlib, sys
from cloakbrowser import launch_persistent_context

def main(req):
    pathlib.Path(req['profile_dir']).mkdir(parents=True, exist_ok=True)
    ctx = launch_persistent_context(req['profile_dir'], headless=True, humanize=True)
    page = ctx.pages[0] if ctx.pages else ctx.new_page()
    try:
        action = req['action']
        timeout = req.get('timeout_ms') or 30000
        if req.get('url') and action != 'open':
            page.goto(req['url'], wait_until='domcontentloaded', timeout=timeout)
        if action == 'open':
            page.goto(req['url'], wait_until='domcontentloaded', timeout=timeout)
            result = {'ok': True, 'url': page.url, 'title': page.title()}
        elif action in ('snapshot', 'get_content'):
            fmt = 'annotated' if action == 'snapshot' else (req.get('format') or 'text')
            content = page.content() if fmt == 'html' else page.locator('body').inner_text(timeout=timeout)
            result = {'content': content, 'url': page.url, 'title': page.title(), 'format': fmt}
        elif action == 'screenshot':
            page.screenshot(path=req['screenshot_path'], full_page=True, timeout=timeout)
            result = {'saved': req['screenshot_path'], 'url': page.url, 'title': page.title()}
        elif action == 'eval':
            result = {'result': page.evaluate(req['script']), 'url': page.url}
        elif action == 'click':
            page.click(req['selector'], timeout=timeout)
            result = {'ok': True, 'url': page.url}
        elif action == 'type':
            page.fill(req['selector'], req.get('text') or '', timeout=timeout)
            result = {'ok': True, 'url': page.url}
        elif action == 'wait':
            if req.get('selector'):
                page.wait_for_selector(req['selector'], timeout=timeout)
            elif req.get('text'):
                page.get_by_text(req['text']).wait_for(timeout=timeout)
            result = {'ok': True, 'url': page.url}
        print(json.dumps(result))
    finally:
        ctx.close()
"#;

async fn firefox_status(
    provider: &FirefoxBridgeProvider,
    _ctx: &ToolContext,
) -> Result<ToolOutput> {
    let status = crate::browser::ensure_browser_ready_noninteractive().await?;
    let mut metadata = json!({
        "setup_complete": status.setup_complete,
        "binary_installed": status.binary_installed,
        "responding": status.responding,
        "compatible": status.compatible,
        "missing_actions": status.missing_actions,
        "ready": status.ready,
        "backend": if status.binary_installed || status.setup_complete || status.ready {
            provider.id()
        } else {
            "unconfigured"
        },
        "browser": "firefox",
    });

    if status.ready {
        return Ok(
            ToolOutput::new("Browser bridge is installed and responding.")
                .with_title("browser status")
                .with_metadata(metadata),
        );
    }

    if status.responding && !status.compatible {
        let missing = if status.missing_actions.is_empty() {
            "unknown required actions".to_string()
        } else {
            status.missing_actions.join(", ")
        };
        return Ok(ToolOutput::new(format!(
            "Browser bridge is connected, but the live Firefox extension is out of date and does not support required actions: {}. Use action='setup' only to repair or update the existing install. You do not need to run setup before every browser task.",
            missing
        ))
        .with_title("browser status")
        .with_metadata(metadata));
    }

    if status.binary_installed {
        return Ok(ToolOutput::new(
            "Browser bridge binaries are installed, but the live bridge is not responding. Use action='setup' only if you want to repair the existing install. You do not need to run setup before every browser task.",
        )
        .with_title("browser status")
        .with_metadata(metadata));
    }

    metadata["backend"] = json!("unconfigured");
    Ok(ToolOutput::new(
        "Browser bridge is not installed yet. Use action='setup' only for first-time install or repair. You do not need to run setup before every browser task.",
    )
    .with_title("browser status")
    .with_metadata(metadata))
}

async fn firefox_setup(provider: &FirefoxBridgeProvider) -> Result<ToolOutput> {
    let log = crate::browser::ensure_browser_setup().await?;
    let status = crate::browser::ensure_browser_ready_noninteractive().await?;
    let title = if status.ready {
        "browser setup"
    } else {
        "browser setup (incomplete)"
    };
    Ok(ToolOutput::new(log).with_title(title).with_metadata(json!({
        "setup_complete": status.setup_complete,
        "binary_installed": status.binary_installed,
        "responding": status.responding,
        "compatible": status.compatible,
        "missing_actions": status.missing_actions,
        "ready": status.ready,
        "backend": provider.id(),
        "browser": "firefox"
    })))
}

async fn ensure_firefox_ready() -> Result<Option<String>> {
    if crate::browser::is_setup_complete() {
        return Ok(None);
    }

    let status = crate::browser::ensure_browser_ready_noninteractive().await?;
    if status.ready {
        return Ok(None);
    }

    let mut message = String::from(
        "Browser automation is not ready yet. Use the browser tool with action='status' to confirm current state. Only run action='setup' or `jcode browser setup` for first-time install or repair when the bridge is not already ready.\n",
    );
    if !status.binary_installed {
        message.push_str("Browser bridge binary is not installed yet.\n");
    } else if status.responding && !status.compatible {
        message.push_str("Browser bridge is connected, but the live Firefox extension is missing required actions.");
        if !status.missing_actions.is_empty() {
            message.push_str(&format!(
                " Missing actions: {}.",
                status.missing_actions.join(", ")
            ));
        }
        message.push('\n');
    } else {
        message.push_str("Browser bridge binaries are installed, but the live Firefox bridge is not responding.\n");
    }
    message
        .push_str("Normal browser tool calls will not reopen the installer automatically anymore.");
    anyhow::bail!(message)
}

async fn execute_firefox_action(
    _provider: &FirefoxBridgeProvider,
    action: &str,
    input: &BrowserInput,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    let (bridge_action, bridge_params, title) = bridge_request(action, input)?;

    if bridge_action == "screenshot" {
        return screenshot_via_bridge(&bridge_params, title, ctx).await;
    }

    let result = firefox_run_bridge_command(&bridge_action, bridge_params, ctx).await?;
    Ok(render_browser_output(action, title, result))
}

fn bridge_request(action: &str, input: &BrowserInput) -> Result<(String, Value, String)> {
    let bridge_action = match action {
        "list_tabs" => "listTabs",
        "new_tab" => "newSession",
        "select_tab" => "setActiveTab",
        "get_active_tab" => "getActiveTab",
        "list_frames" => "listFrames",
        "open" => "navigate",
        "snapshot" => "getContent",
        "get_content" => "getContent",
        "interactables" => "getInteractables",
        "click" => "click",
        "type" => "type",
        "fill_form" => "fillForm",
        "select" => "fillForm",
        "wait" => "waitFor",
        "screenshot" => "screenshot",
        "eval" => "evaluate",
        "scroll" => "scroll",
        "upload" => "uploadFile",
        "press" => "evaluate",
        "provider_command" => input.provider_action.as_deref().ok_or_else(|| {
            anyhow::anyhow!("provider_action is required when action='provider_command'")
        })?,
        other => anyhow::bail!("Unsupported browser action: {}", other),
    }
    .to_string();

    let mut params = Map::new();
    apply_common_targeting(&mut params, input);

    match action {
        "new_tab" => {
            if let Some(url) = &input.url {
                params.insert("url".into(), json!(url));
            }
            if let Some(timeout_ms) = input.timeout_ms {
                params.insert("timeoutMs".into(), json!(timeout_ms));
            }
        }
        "select_tab" => {
            let tab_id = input
                .tab_id
                .ok_or_else(|| anyhow::anyhow!("tab_id is required for select_tab"))?;
            params.insert("tabId".into(), json!(tab_id));
            if let Some(focus) = input.focus {
                params.insert("focus".into(), json!(focus));
            }
        }
        "open" => {
            let url = input
                .url
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("url is required for open"))?;
            params.insert("url".into(), json!(url));
            params.insert("wait".into(), json!(input.wait.unwrap_or(true)));
            if let Some(new_tab) = input.new_tab {
                params.insert("newTab".into(), json!(new_tab));
            }
            if let Some(timeout_ms) = input.timeout_ms {
                params.insert("timeoutMs".into(), json!(timeout_ms));
            }
        }
        "snapshot" => {
            params.insert("format".into(), json!("annotated"));
        }
        "get_content" => {
            params.insert(
                "format".into(),
                json!(input.format.as_deref().unwrap_or("text")),
            );
        }
        "interactables" => {}
        "click" => {
            if input.selector.is_none()
                && input.text.is_none()
                && input.x.is_none()
                && input.y.is_none()
            {
                anyhow::bail!("click requires selector, text, or x/y coordinates");
            }
            if let Some(x) = input.x {
                params.insert("x".into(), json!(x));
            }
            if let Some(y) = input.y {
                params.insert("y".into(), json!(y));
            }
        }
        "type" => {
            let text = input
                .text
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("text is required for type"))?;
            params.insert("text".into(), json!(text));
            if let Some(clear) = input.clear {
                params.insert("clear".into(), json!(clear));
            }
            if let Some(submit) = input.submit {
                params.insert("submit".into(), json!(submit));
            }
        }
        "fill_form" => {
            let fields = input
                .fields
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("fields are required for fill_form"))?;
            let mapped: Vec<Value> = fields
                .iter()
                .map(|field| {
                    let mut obj = Map::new();
                    obj.insert("selector".into(), json!(field.selector));
                    if let Some(value) = &field.value {
                        obj.insert("value".into(), json!(value));
                    }
                    if let Some(checked) = field.checked {
                        obj.insert("checked".into(), json!(checked));
                    }
                    Value::Object(obj)
                })
                .collect();
            params.insert("fields".into(), Value::Array(mapped));
        }
        "select" => {
            let selector = input
                .selector
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("selector is required for select"))?;
            let value = input.text.as_deref().ok_or_else(|| {
                anyhow::anyhow!("text is required for select and is used as the option value")
            })?;
            params.insert(
                "fields".into(),
                json!([{ "selector": selector, "value": value }]),
            );
        }
        "wait" => {
            if input.selector.is_none() && input.text.is_none() && input.contains.is_none() {
                anyhow::bail!("wait requires selector, text, or contains");
            }
            if let Some(timeout_ms) = input.timeout_ms {
                params.insert("timeout".into(), json!(timeout_ms));
            }
            if let Some(contains) = &input.contains {
                params.insert("contains".into(), json!(contains));
            }
        }
        "screenshot" => {}
        "eval" => {
            let script = input
                .script
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("script is required for eval"))?;
            params.insert("script".into(), json!(script));
            if let Some(page_world) = input.page_world {
                params.insert("pageWorld".into(), json!(page_world));
            }
        }
        "scroll" => {
            if let Some(x) = input.x {
                params.insert("x".into(), json!(x));
            }
            if let Some(y) = input.y {
                params.insert("y".into(), json!(y));
            }
            if let Some(position) = &input.position {
                params.insert("position".into(), json!(position));
            }
            if let Some(behavior) = &input.behavior {
                params.insert("behavior".into(), json!(behavior));
            }
            if let Some(scroll_to) = &input.scroll_to {
                let mut target = Map::new();
                if let Some(x) = scroll_to.x {
                    target.insert("x".into(), json!(x));
                }
                if let Some(y) = scroll_to.y {
                    target.insert("y".into(), json!(y));
                }
                params.insert("scrollTo".into(), Value::Object(target));
            }
            if !params.contains_key("x")
                && !params.contains_key("y")
                && !params.contains_key("selector")
                && !params.contains_key("position")
                && !params.contains_key("scrollTo")
            {
                anyhow::bail!("scroll requires x/y, selector, position, or scroll_to");
            }
        }
        "upload" => {
            let path = input
                .path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("path is required for upload"))?;
            params.insert("path".into(), json!(path));
        }
        "press" => {
            let script = build_press_script(input.key.as_deref(), input.selector.as_deref())?;
            params.insert("script".into(), json!(script));
            params.insert("pageWorld".into(), json!(true));
        }
        "provider_command" => {
            if let Some(raw) = &input.params {
                return Ok((bridge_action, raw.clone(), format!("browser {}", action)));
            }
        }
        _ => {}
    }

    Ok((
        bridge_action,
        Value::Object(params),
        format!("browser {}", action),
    ))
}

fn apply_common_targeting(params: &mut Map<String, Value>, input: &BrowserInput) {
    if let Some(tab_id) = input.tab_id {
        params.insert("tabId".into(), json!(tab_id));
    }
    if let Some(frame_id) = input.frame_id {
        params.insert("frameId".into(), json!(frame_id));
    }
    if let Some(all_frames) = input.all_frames {
        params.insert("allFrames".into(), json!(all_frames));
    }
    if let Some(selector) = &input.selector {
        params.insert("selector".into(), json!(selector));
    }
    if let Some(text) = &input.text {
        params.insert("text".into(), json!(text));
    }
}

fn build_press_script(key: Option<&str>, selector: Option<&str>) -> Result<String> {
    let key = key.ok_or_else(|| anyhow::anyhow!("key is required for press"))?;
    let selector_literal = selector.map(serde_json::to_string).transpose()?;
    let selector_expr = selector_literal
        .map(|s| format!("document.querySelector({})", s))
        .unwrap_or_else(|| "null".to_string());
    let key_literal = serde_json::to_string(key)?;
    Ok(format!(
        r#"return (() => {{
  const target = {selector_expr} || document.activeElement || document.body;
  if (!target) throw new Error('No target available for key press');
  if (typeof target.focus === 'function') target.focus();
  const key = {key_literal};
  const eventInit = {{ key, bubbles: true, cancelable: true }};
  target.dispatchEvent(new KeyboardEvent('keydown', eventInit));
  target.dispatchEvent(new KeyboardEvent('keypress', eventInit));
  if (key === 'Enter' && target.form && typeof target.form.submit === 'function') {{
    target.form.submit();
  }}
  target.dispatchEvent(new KeyboardEvent('keyup', eventInit));
  return {{ pressed: true, key, tag: target.tagName || null }};
}})();"#
    ))
}

async fn firefox_run_bridge_command(
    action: &str,
    params: Value,
    ctx: &ToolContext,
) -> Result<Value> {
    let bin = crate::browser::browser_binary_path();
    if !bin.exists() {
        anyhow::bail!(
            "Browser bridge binary is not installed yet. Use action='status' to confirm readiness, then run action='setup' only for first-time install or repair."
        );
    }

    let params_json = serde_json::to_string(&params)?;
    let mut command = tokio::process::Command::new(&bin);
    command.arg(action).arg(&params_json);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    #[cfg(not(windows))]
    if std::env::var("BROWSER_SESSION").is_err()
        && let Some(session_name) = crate::browser::ensure_browser_session(&ctx.session_id)
    {
        command.env("BROWSER_SESSION", session_name);
    }

    let output = command
        .output()
        .await
        .with_context(|| format!("Failed to run browser bridge action '{}'.", action))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !output.status.success() {
        let details = if stderr.is_empty() {
            stdout
        } else if stdout.is_empty() {
            stderr
        } else {
            format!("{}\n{}", stderr, stdout)
        };
        if details.contains("Unknown action:") {
            anyhow::bail!(
                "The connected Firefox browser bridge is missing required support for action '{}'. This usually means the installed extension is older than the browser CLI expected by jcode. Use browser action='status' to confirm, then action='setup' to repair or update the extension.\n\nOriginal bridge error: {}",
                action,
                details
            );
        }
        anyhow::bail!(details);
    }

    if stdout.is_empty() {
        return Ok(json!({ "ok": true }));
    }

    serde_json::from_str(&stdout).or_else(|_| Ok(json!({ "raw": stdout })))
}

async fn screenshot_via_bridge(
    params: &Value,
    title: String,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    let filename = temp_screenshot_path();
    let mut screenshot_params = params.clone();
    if let Some(map) = screenshot_params.as_object_mut() {
        map.insert(
            "filename".into(),
            json!(filename.to_string_lossy().to_string()),
        );
    }

    let result = firefox_run_bridge_command("screenshot", screenshot_params, ctx).await?;
    let saved = result
        .get("saved")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or(filename);

    let mut output = ToolOutput::new(format!(
        "Captured browser screenshot to {}.",
        saved.display()
    ))
    .with_title(title)
    .with_metadata(result.clone());

    if let Ok(bytes) = tokio::fs::read(&saved).await {
        output = output.with_labeled_image(
            "image/png",
            STANDARD.encode(&bytes),
            format!("browser screenshot: {}", saved.display()),
        );
        let _ = tokio::fs::remove_file(&saved).await;
    }

    Ok(output)
}

fn temp_screenshot_path() -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("jcode-browser-{}.png", ts))
}

fn render_browser_output(action: &str, title: String, result: Value) -> ToolOutput {
    let body = match action {
        "snapshot" => result
            .get("content")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| serde_json::to_string_pretty(&result).unwrap_or_default()),
        "get_content" => format_content_result(&result),
        "interactables" => format_interactables_result(&result),
        "eval" => format_eval_result(&result),
        _ => serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string()),
    };

    ToolOutput::new(body)
        .with_title(title)
        .with_metadata(result)
}

fn format_content_result(result: &Value) -> String {
    if let Some(content) = result.get("content").and_then(|v| v.as_str()) {
        return content.to_string();
    }
    if let Some(text) = result.get("text").and_then(|v| v.as_str()) {
        return text.to_string();
    }
    if let Some(html) = result.get("html").and_then(|v| v.as_str()) {
        return html.to_string();
    }
    if let Some(title) = result.get("title").and_then(|v| v.as_str()) {
        if let Some(url) = result.get("url").and_then(|v| v.as_str()) {
            return format!("{}\n{}", title, url);
        }
        return title.to_string();
    }
    serde_json::to_string_pretty(result).unwrap_or_default()
}

fn format_eval_result(result: &Value) -> String {
    let value = result.get("result").cloned().unwrap_or(Value::Null);
    let rendered = if let Some(s) = value.as_str() {
        s.to_string()
    } else {
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
    };

    match result.get("type").and_then(|v| v.as_str()) {
        Some(kind) => format!("{}\n\n(type: {})", rendered, kind),
        None => rendered,
    }
}

fn format_interactables_result(result: &Value) -> String {
    let Some(elements) = result.get("elements").and_then(|v| v.as_array()) else {
        return serde_json::to_string_pretty(result).unwrap_or_default();
    };

    if elements.is_empty() {
        return "No interactable elements found.".to_string();
    }

    let mut lines = Vec::new();
    for (idx, element) in elements.iter().enumerate() {
        let kind = element
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("element");
        let tag = element.get("tag").and_then(|v| v.as_str()).unwrap_or("?");
        let text = element
            .get("text")
            .or_else(|| element.get("label"))
            .or_else(|| element.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let selector = element
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        lines.push(format!(
            "{}. [{}] <{}> {} | selector: {}",
            idx + 1,
            kind,
            tag.to_lowercase(),
            text,
            selector
        ));
    }

    lines.join("\n")
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod browser_tests;
