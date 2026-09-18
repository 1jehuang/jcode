//! Enumerated-action browser controller. Only the parent supplies executable payloads.
//! The decision transport sees untrusted observations and labels, never an API for code generation.
use super::*;
use serde::Serialize;
use std::future::Future;
use std::time::Duration;

#[path = "browser_jev.rs"]
mod browser_jev;

const MAX_OPTIONS: usize = 240; // Includes four terminal/help options, below Jev's 255 limit.
const MAX_OBSERVATION: usize = 48_000;
const MAX_REQUEST: usize = 96_000;

pub(super) fn null_vec<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExactCandidate {
    pub label: String,
    pub input: Value,
}

#[derive(Debug, Serialize)]
pub(super) struct DecisionOption {
    pub id: String,
    pub label: String,
}
#[derive(Debug, Serialize)]
pub(super) struct DecisionRequest {
    pub goal: String,
    pub observation: Value,
    pub options: Vec<DecisionOption>,
}
#[derive(Debug)]
pub(super) struct Decision {
    pub choice: String,
    pub confidence: f64,
    pub reason: String,
}
#[async_trait]
pub(super) trait DecisionTransport: Send + Sync {
    fn model(&self) -> &str;
    async fn decide(&self, request: &DecisionRequest) -> Result<Decision>;
}

// Static trusted code. Never interpolate page text, goal, selectors, or model output.
// Values are deliberately NOT read, including arbitrary text inputs and passwords.
const OBSERVE_SCRIPT: &str = r#"return (() => {
 const clip=(s,n=180)=>String(s||'').slice(0,n);
 const selector=(e)=>{const p=[];while(e && e.nodeType===1){let i=1;for(let s=e.previousElementSibling;s;s=s.previousElementSibling)if(s.localName===e.localName)i++;p.unshift(e.localName+':nth-of-type('+i+')');e=e.parentElement;}return p.join(' > ');};
 const intersects=r=>r.width>0&&r.height>0&&r.bottom>0&&r.right>0&&r.top<innerHeight&&r.left<innerWidth;
 const visible=e=>getComputedStyle(e).visibility!=='hidden'&&Array.from(e.getClientRects()).some(intersects);
 const textVisible=node=>{const range=document.createRange();range.selectNodeContents(node);const rects=Array.from(range.getClientRects());if(rects.length)return rects.some(intersects);let p=node.parentElement;while(p){const boxes=Array.from(p.getClientRects());if(boxes.some(r=>r.width>0&&r.height>0))return visible(p);p=p.parentElement;}return false;};
 const identity=globalThis.__jcodeFastBrowserIdentity||(globalThis.__jcodeFastBrowserIdentity={nodes:new WeakMap(),next:1});
 const elements=[];
 let sensitive=false, scanned=0;
 for(const e of document.querySelectorAll('a,button,input,textarea,select,[role="button"],[role="link"],[contenteditable="true"],iframe')){
  if(++scanned>2048)break;
  if(!visible(e))continue;
  const type=clip(e.getAttribute('type')||'',40).toLowerCase();
  const aria=clip(e.getAttribute('aria-label'));
  const name=clip(e.getAttribute('name'));
  const autocomplete=clip(e.getAttribute('autocomplete'),80);
  const text=e.matches('input,textarea,[contenteditable]')||e.querySelector('input,textarea,[contenteditable]')?'':clip(e.innerText||e.textContent);
  if(type==='password'||/one-time-code/i.test(autocomplete)||/captcha|\botp\b|verification code|security code|reset password/i.test(aria+' '+name+' '+text+' '+e.getAttribute('src')))sensitive=true;
  if(elements.length>=64)continue;
  const css=selector(e);if(css.length>1500||document.querySelectorAll(css).length!==1)continue;
  if(!identity.nodes.has(e))identity.nodes.set(e,identity.next++);
  elements.push({identity:identity.nodes.get(e),selector:css,tag:e.localName,type,role:clip(e.getAttribute('role'),40),text,aria,name,autocomplete,href:e.localName==='a'?clip(e.href,1000):'',target:clip(e.getAttribute('target'),40),disabled:!!e.disabled,form:!!e.form,options:e.localName==='select'?Array.from(e.options).filter(o=>o.value.length<=200).slice(0,16).map(o=>({text:clip(o.text),value:o.value,disabled:o.disabled})):[]});
 }
 // TreeWalker excludes form values, scripts, hidden nodes and contenteditable contents.
 const walker=document.createTreeWalker(document.body||document.documentElement,NodeFilter.SHOW_TEXT);
 let body='',node,visited=0;while(body.length<6000 && ++visited<=8000 && (node=walker.nextNode())){const p=node.parentElement;if(p&&!p.closest('input,textarea,select,script,style,[contenteditable]')&&getComputedStyle(p).visibility!=='hidden'&&textVisible(node))body+=clip(node.textContent,400)+' ';}
 if(/captcha|one.time (password|code)|verification code|reset (your )?password/i.test(body))sensitive=true;
 const root=document.scrollingElement||document.documentElement;
 return {ready_state:document.readyState,scroll:{x:scrollX,y:scrollY,can_up:scrollY>0,can_down:scrollY+innerHeight<root.scrollHeight-1,can_left:scrollX>0,can_right:scrollX+innerWidth<root.scrollWidth-1},url:clip(location.href,1500),title:clip(document.title,300),text:body.slice(0,6000),sensitive,elements};
})()"#;

pub(super) async fn handoff(
    provider: &dyn BrowserProvider,
    input: &BrowserInput,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    match browser_jev::JevTransport::new() {
        Ok(transport) => run(provider, &transport, input, ctx).await,
        Err(error) => Ok(outcome(
            "hand_back",
            &format!("Decision transport unavailable: {error}"),
            &[],
            &Value::Null,
            "typesafe/jev-1.13",
            Some("uncertain"),
        )),
    }
}

// Defense in depth for credential material rendered outside form controls. Redact the
// whole containing string, not a clipped substring that might leave a token suffix.
fn redact_credentials(value: &mut Value) -> bool {
    static TOKENS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)(?:\bsk-[a-z0-9_-]{8,}|\b(?:ghp|github_pat|gho|ghu|ghs|ghr)_[a-z0-9_]{8,}|\bbearer\s+[a-z0-9._~+/-]{8,}|\beyJ[a-z0-9_-]{8,}\.[a-z0-9_-]{8,}(?:\.[a-z0-9_-]+)?|[?&#](?:password|access_token|refresh_token|id_token|code|api_key|apikey|token|secret)=)").expect("static credential regex")
    });
    match value {
        Value::String(text) if TOKENS.is_match(text) => {
            *text = "[REDACTED: credential material]".into();
            true
        }
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |found, item| redact_credentials(item) || found),
        Value::Object(items) => items
            .values_mut()
            .fold(false, |found, item| redact_credentials(item) || found),
        _ => false,
    }
}

fn outcome(
    status: &str,
    reason: &str,
    trace: &[Value],
    observation: &Value,
    model: &str,
    requested_help: Option<&str>,
) -> ToolOutput {
    let mut result = json!({"status":status,"reason":reason,"action_trace":trace,"final_observation":observation,"model":model,"requested_help":requested_help});
    redact_credentials(&mut result);
    ToolOutput::new(result.to_string())
        .with_title(format!("browser handoff: {status}"))
        .with_metadata(result)
}

async fn bounded<T>(
    ctx: &ToolContext,
    timeout: Duration,
    future: impl Future<Output = Result<T>>,
) -> Result<T> {
    let cancel = async {
        match &ctx.graceful_shutdown_signal {
            Some(signal) => signal.notified().await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        biased;
        _ = cancel => anyhow::bail!("Handoff cancelled. An in-flight browser action may already have taken effect."),
        result = tokio::time::timeout(timeout, future) => result.context("Handoff operation timed out; an in-flight action may already have taken effect")?,
    }
}

fn scoped(mut input: BrowserInput, parent: &BrowserInput) -> Result<BrowserInput> {
    for (name, actual, expected) in [
        ("tab", input.tab_id, parent.tab_id),
        ("window", input.window_id, parent.window_id),
        ("frame", input.frame_id, Some(parent.frame_id.unwrap_or(0))),
    ] {
        anyhow::ensure!(
            actual.is_none() || actual == expected,
            "Candidate escapes {name} scope"
        );
    }
    anyhow::ensure!(
        input.browser.is_none() || input.browser == parent.browser,
        "Candidate changes browser"
    );
    anyhow::ensure!(
        input.all_frames != Some(true) && input.new_tab != Some(true),
        "Candidate escapes tab/frame scope"
    );
    anyhow::ensure!(
        !matches!(
            input.action.as_str(),
            "handoff" | "setup" | "new_tab" | "list_tabs" | "get_active_tab" | "status"
        ),
        "Action cannot run inside a scoped handoff"
    );
    if parent.frame_id.unwrap_or(0) != 0 {
        anyhow::ensure!(
            !matches!(
                input.action.as_str(),
                "open" | "screenshot" | "list_frames" | "select_tab"
            ) && !matches!(
                input.provider_action.as_deref(),
                Some("navigate" | "screenshot" | "listFrames" | "setActiveTab")
            ),
            "Whole-tab action cannot honor a nonzero frame scope"
        );
    }
    input.handoff_single_click = true;
    input.tab_id = parent.tab_id;
    input.window_id = parent.window_id;
    input.frame_id = Some(parent.frame_id.unwrap_or(0));
    input.all_frames = Some(false);
    // Raw provider params replace common targeting in bridge_request. Restrict commands and
    // keys, reject alternate/nested scope knobs, then inject our authoritative scope.
    if input.action == "provider_command" {
        let command = input.provider_action.as_deref().unwrap_or("");
        let allowed: &[&str] = match command {
            "navigate" => &["url", "wait", "timeoutMs"],
            "getContent" => &["format"],
            "getInteractables" | "listFrames" => &[],
            "click" => &["selector", "text", "x", "y", "dispatchEvents"],
            "type" => &["selector", "text", "clear", "submit"],
            "fillForm" => &["fields"],
            "waitFor" => &["selector", "contains", "timeout"],
            "screenshot" => &["selector", "path", "format"],
            "evaluate" => &["script", "pageWorld"],
            "scroll" => &["selector", "x", "y", "position", "behavior", "scrollTo"],
            "uploadFile" => &["selector", "filePath", "fileName"],
            "setActiveTab" => &["focus"],
            _ => anyhow::bail!("Raw provider command cannot be safely scoped"),
        };
        let mut raw = input
            .params
            .take()
            .unwrap_or(json!({}))
            .as_object()
            .cloned()
            .context("Raw params must be an object")?;
        for (key, value) in &raw {
            let expected = match key.as_str() {
                "tabId" => input.tab_id,
                "windowId" => input.window_id,
                "frameId" => input.frame_id,
                _ => None,
            };
            if matches!(key.as_str(), "tabId" | "windowId" | "frameId") {
                anyhow::ensure!(
                    expected.is_some() && value.as_i64() == expected,
                    "Raw params escape scope"
                );
            } else if key == "allFrames" {
                anyhow::ensure!(value == &json!(false), "Raw params escape frame scope");
            } else {
                anyhow::ensure!(
                    allowed.contains(&key.as_str()),
                    "Unsupported raw parameter {key}"
                );
                // Only known structured payloads are allowed; their member keys are validated.
                if key == "fields" {
                    let fields = value.as_array().context("fields must be an array")?;
                    for field in fields {
                        let obj = field.as_object().context("field must be an object")?;
                        anyhow::ensure!(
                            obj.keys()
                                .all(|k| matches!(k.as_str(), "selector" | "value" | "checked")),
                            "Unsupported field parameter"
                        );
                        anyhow::ensure!(
                            obj.values().all(|v| v.is_string() || v.is_boolean()),
                            "Invalid field parameter"
                        );
                    }
                } else if key == "scrollTo" {
                    let obj = value.as_object().context("scrollTo must be an object")?;
                    anyhow::ensure!(
                        obj.iter()
                            .all(|(k, v)| matches!(k.as_str(), "x" | "y") && v.is_number()),
                        "Invalid scroll target"
                    );
                } else {
                    anyhow::ensure!(
                        !value.is_object() && !value.is_array(),
                        "Nested raw parameters are not allowed"
                    );
                }
            }
        }
        if command == "click" {
            anyhow::ensure!(
                raw.get("dispatchEvents").is_none_or(|v| v == &json!(false)),
                "Handoff click must disable duplicate synthetic dispatch"
            );
            raw.insert("dispatchEvents".into(), json!(false));
        }
        raw.insert("tabId".into(), json!(input.tab_id));
        if let Some(window) = input.window_id {
            raw.insert("windowId".into(), json!(window));
        }
        raw.insert("frameId".into(), json!(input.frame_id));
        raw.insert("allFrames".into(), json!(false));
        input.params = Some(Value::Object(raw));
    } else {
        anyhow::ensure!(
            input.params.is_none() && input.provider_action.is_none(),
            "Raw parameters require provider_command"
        );
    }
    // Validate high-level action and its required fields before exposing an option.
    bridge_request(&input.action, &input)?;
    Ok(input)
}

struct Candidate {
    label: String,
    input: BrowserInput,
    exact_index: Option<usize>,
}

// A conservative convenience filter, not a sandbox for arbitrary website handlers.
// Sensitive workflows should be completed by the parent with exact authorized actions.
fn risky(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "send",
        "buy",
        "purchase",
        "pay",
        "delete",
        "remove",
        "submit",
        "confirm",
        "order",
        "checkout",
        "subscribe",
        "unsubscribe",
        "transfer",
        "donate",
        "publish",
        "post",
        "invite",
        "accept",
        "agree",
        "approve",
        "authorize",
        "sign",
        "log out",
        "logout",
        "reset",
        "password",
        "otp",
        "captcha",
        "verification",
        "security code",
        "credit",
        "card",
        "bank",
        "billing",
        "cc-",
        "cvv",
        "cvc",
        "iban",
        "routing",
        "social security",
        "ssn",
        "transaction-",
        "save",
        "cancel",
        "disable",
        "enable",
        "install",
        "download",
        "execute",
        "run",
    ]
    .iter()
    .any(|word| text.contains(word))
}

fn candidates(input: &BrowserInput, observation: &Value) -> Result<Vec<Candidate>> {
    let mut result = Vec::new();
    for (exact_index, exact) in input.candidates.iter().enumerate() {
        anyhow::ensure!(
            exact.label.len() <= 500 && exact.input.to_string().len() <= 16000,
            "Exact candidate too large"
        );
        result.push(Candidate {
            exact_index: Some(exact_index),
            label: exact.label.clone(),
            input: scoped(serde_json::from_value(exact.input.clone())?, input)?,
        });
    }
    let mut add = |label: String, action: Value| -> Result<()> {
        if result.len() < MAX_OPTIONS - 4 {
            result.push(Candidate {
                exact_index: None,
                label,
                input: scoped(serde_json::from_value(action)?, input)?,
            });
        }
        Ok(())
    };
    if let Some(elements) = observation["elements"].as_array() {
        for element in elements.iter().take(64) {
            let Some(selector) = element["selector"].as_str() else {
                continue;
            };
            let tag = element["tag"].as_str().unwrap_or("");
            let kind = element["type"].as_str().unwrap_or("");
            let description = format!(
                "{} {} {} {} {}",
                element["text"].as_str().unwrap_or(""),
                element["aria"].as_str().unwrap_or(""),
                element["name"].as_str().unwrap_or(""),
                element["href"].as_str().unwrap_or(""),
                element["autocomplete"].as_str().unwrap_or("")
            );
            let label = ["aria", "text", "name"]
                .iter()
                .filter_map(|key| element[*key].as_str())
                .find(|text| !text.trim().is_empty())
                .unwrap_or(tag)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let label: String = label.chars().take(160).collect();
            if risky(&description) || element["disabled"] == true {
                continue;
            }
            if tag == "a"
                && element["target"].as_str().unwrap_or("").is_empty()
                && element["href"]
                    .as_str()
                    .is_some_and(|href| href.starts_with("https://") || href.starts_with("http://"))
            {
                add(
                    format!("Click link {label}"),
                    json!({"action":"click","selector":selector,"url":element["href"]}),
                )?;
            }
            let button_text = element["text"].as_str().unwrap_or("").trim().to_lowercase();
            if (tag == "button" || element["role"] == "button")
                && element["form"] == false
                && kind != "submit"
                && matches!(
                    button_text.as_str(),
                    "next" | "previous" | "back" | "menu" | "show more" | "expand"
                )
            {
                add(
                    format!("Click {label}"),
                    json!({"action":"click","selector":selector}),
                )?;
            }
            if matches!(tag, "input" | "textarea")
                && matches!(kind, "" | "text" | "search" | "email" | "url")
            {
                for (index, text) in input.text_values.iter().enumerate() {
                    add(
                        format!("Type supplied text {index} in {label}"),
                        json!({"action":"type","selector":selector,"text":text,"clear":true,"submit":false}),
                    )?;
                }
            }
            if tag == "select"
                && let Some(options) = element["options"].as_array()
            {
                for option in options.iter().take(16) {
                    if option["disabled"] == true || risky(option["text"].as_str().unwrap_or("")) {
                        continue;
                    }
                    if let Some(value) = option["value"].as_str() {
                        add(
                            format!(
                                "Select {} in {label}",
                                option["text"].as_str().unwrap_or(value)
                            ),
                            json!({"action":"select","selector":selector,"text":value}),
                        )?;
                    }
                }
            }
        }
    }
    if observation["scroll"]["can_down"].as_bool().unwrap_or(true) {
        add("Scroll down".into(), json!({"action":"scroll","y":600}))?;
    }
    if observation["scroll"]["can_up"].as_bool().unwrap_or(true) {
        add("Scroll up".into(), json!({"action":"scroll","y":-600}))?;
    }
    if observation["scroll"]["can_right"] == true {
        add("Scroll right".into(), json!({"action":"scroll","x":600}))?;
    }
    if observation["scroll"]["can_left"] == true {
        add("Scroll left".into(), json!({"action":"scroll","x":-600}))?;
    }
    if observation["ready_state"] == "loading"
        || observation["elements"].as_array().is_none_or(Vec::is_empty)
    {
        add(
            "Wait for page content".into(),
            json!({"action":"wait","selector":"body","timeout_ms":1000}),
        )?;
    }
    Ok(result)
}

fn retain_result(result: Value, retained_bytes: &mut usize) -> Value {
    let bytes = result.to_string().len();
    if bytes <= 16_000 && *retained_bytes + bytes <= 32_000 {
        *retained_bytes += bytes;
        result
    } else {
        json!({"omitted":"Result exceeds the per-action or cumulative 32000-byte retention budget. Inspect current state with a read-only direct browser action. Do not repeat side effects."})
    }
}

fn action_failed(value: &Value) -> bool {
    value["ok"] == false
        || value["success"] == false
        || value.get("error").is_some_and(|e| !e.is_null())
        || value["success"]
            .as_u64()
            .zip(value["total"].as_u64())
            .is_some_and(|(success, total)| success < total)
        || value["results"]
            .as_array()
            .is_some_and(|results| results.iter().any(action_failed))
}

// Browser clicks return before navigation and async handlers necessarily finish. Do not
// spend a model decision on a transient old page merely because dispatch completed.
async fn settle_after_action(
    provider: &dyn BrowserProvider,
    observe: &BrowserInput,
    ctx: &ToolContext,
    timeout: Duration,
    deadline: tokio::time::Instant,
    before: &Value,
    require_progress: bool,
) -> Result<Value> {
    let deadline = deadline.min(tokio::time::Instant::now() + Duration::from_secs(2));
    let remaining = || timeout.min(deadline.saturating_duration_since(tokio::time::Instant::now()));
    bounded(ctx, remaining(), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    })
    .await?;
    let mut previous = None;
    loop {
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "Browser transition did not settle within two seconds"
        );
        let output = bounded(ctx, remaining(), provider.execute("eval", observe, ctx)).await?;
        let metadata = output
            .metadata
            .context("Settling observation missing metadata")?;
        let mut fresh = metadata.get("result").cloned().unwrap_or(metadata);
        if let Some(text) = fresh.as_str() {
            fresh = serde_json::from_str(text)?;
        }
        anyhow::ensure!(
            fresh.is_object()
                && fresh["elements"].is_array()
                && fresh.to_string().len() <= MAX_OBSERVATION,
            "Malformed or oversized settling observation"
        );
        let loaded = fresh["ready_state"]
            .as_str()
            .is_none_or(|state| state == "complete");
        let progressed = fresh["url"] != before["url"]
            || fresh["text"] != before["text"]
            || fresh["title"] != before["title"];
        if loaded && previous.as_ref() == Some(&fresh) && (!require_progress || progressed) {
            return Ok(fresh);
        }
        previous = Some(fresh);
        bounded(ctx, remaining(), async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(())
        })
        .await?;
    }
}

pub(super) async fn run(
    provider: &dyn BrowserProvider,
    transport: &dyn DecisionTransport,
    input: &BrowserInput,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    let mut trace: Vec<Value> = Vec::new();
    let mut observation = Value::Null;
    let mut images = Vec::new();
    let mut retained_result_bytes = 0;
    let mut used_exact = std::collections::HashSet::new();
    let mut requested_help = Some("uncertain");
    let result: Result<(&str,String)> = async {
        anyhow::ensure!(input.tab_id.is_some(),"handoff requires explicit tab_id");
        anyhow::ensure!(input.all_frames!=Some(true),"handoff must target a single frame");
        let mut caller_material=json!({"goal":input.goal,"text_values":input.text_values});
        anyhow::ensure!(!redact_credentials(&mut caller_material),"Credential material must stay with the main agent/user, not the fast browser model");
        let goal=input.goal.as_deref().filter(|g|!g.trim().is_empty()).context("handoff requires goal")?;
        anyhow::ensure!(goal.len()<=8000,"Goal too large");
        let budget=input.max_steps.unwrap_or(12);
        anyhow::ensure!((1..=30).contains(&budget),"max_steps must be 1..30");
        let threshold=input.confidence_threshold.unwrap_or(0.8);
        anyhow::ensure!(threshold.is_finite() && (0.0..=1.0).contains(&threshold),"confidence_threshold must be 0..1");
        anyhow::ensure!(input.candidates.len()<=64 && input.text_values.len()<=16 && input.text_values.iter().all(|s|s.len()<=2000),"Too many or oversized caller candidates/text values");
        // Validate parent actions even if the page would cause an immediate handback.
        candidates(input,&Value::Null)?;
        let timeout=Duration::from_millis(input.timeout_ms.unwrap_or(20_000).clamp(1,60_000));
        let deadline=tokio::time::Instant::now()+Duration::from_secs(180);
        let status=bounded(ctx,timeout.min(deadline.saturating_duration_since(tokio::time::Instant::now())),provider.status(ctx)).await?;
        anyhow::ensure!(status.metadata.as_ref().is_some_and(|m|m["ready"]==true),"Browser not ready; check status and use setup only if needed");
        if let Some(window)=input.window_id {
            let listing=BrowserInput{action:"list_tabs".into(),tab_id:input.tab_id,frame_id:Some(0),all_frames:Some(false),..Default::default()};
            let tabs=bounded(ctx,timeout.min(deadline.saturating_duration_since(tokio::time::Instant::now())),provider.execute("list_tabs",&listing,ctx)).await?;
            let metadata=tabs.metadata.context("Cannot verify tab/window membership")?;
            let matched=metadata["windows"].as_array().is_some_and(|windows| windows.iter().any(|w|w["windowId"].as_i64()==Some(window)
                && w["tabs"].as_array().is_some_and(|tabs|tabs.iter().any(|t|t["tabId"].as_i64()==input.tab_id))));
            anyhow::ensure!(matched,"Requested tab does not belong to requested window");
        }
        let mut initial_url=None;
        let mut stale=0;
        let mut settled_observation=None;
        for step in 0..=budget {
            anyhow::ensure!(tokio::time::Instant::now()<deadline,"Handoff time budget exhausted");
            let observe=scoped(BrowserInput{action:"eval".into(),script:Some(OBSERVE_SCRIPT.into()),..Default::default()},input)?;
            let mut fresh=if let Some(settled)=settled_observation.take() {settled} else {
                let output=bounded(ctx,timeout.min(deadline.saturating_duration_since(tokio::time::Instant::now())),provider.execute("eval",&observe,ctx)).await?;
                let metadata=output.metadata.context("Observation missing metadata")?;
                metadata.get("result").cloned().unwrap_or(metadata)
            };
            if let Some(text)=fresh.as_str() {fresh=serde_json::from_str(text).context("Invalid observation JSON")?;}
            anyhow::ensure!(fresh.is_object() && fresh["elements"].is_array(),"Malformed DOM observation");
            anyhow::ensure!(fresh.to_string().len()<=MAX_OBSERVATION,"DOM observation exceeds safe size limit");
            if redact_credentials(&mut fresh) {fresh["sensitive"]=json!(true);}
            if fresh==observation {stale+=1;} else {stale=0;}
            observation=fresh;
            let url=observation["url"].as_str().map(str::to_owned);
            if step==0 { initial_url=url.clone(); }
            if input.candidates.iter().enumerate().any(|(index,_)|!used_exact.contains(&index)) && url!=initial_url {
                return Ok(("hand_back","Page URL changed; exact caller actions need renewed authorization for this page".into()));
            }
            if observation["sensitive"]==true {return Ok(("hand_back","Credential material, password, OTP, CAPTCHA, or account recovery requires the main agent/user. Never reset passwords.".into()));}
            if stale>=3 {return Ok(("hand_back","Browser stalled: repeated unchanged observations".into()));}
            if step==budget {return Ok(("hand_back","Action step budget exhausted; final action has been observed".into()));}
            let choices:Vec<_>=candidates(input,&observation)?.into_iter().filter(|c| c.exact_index.is_none_or(|i|!used_exact.contains(&i))).collect();
            let mut options:Vec<_>=choices.iter().enumerate().map(|(index,c)|DecisionOption{id:format!("a{index}"),label:c.label.clone()}).collect();
            options.push(DecisionOption{id:"done".into(),label:"Finish: the goal is already achieved.".into()});
            options.push(DecisionOption{id:"hand_back".into(),label:"Stop: uncertain, blocked, or needs user authorization.".into()});
            options.push(DecisionOption{id:"script_needed".into(),label:"Ask main agent for new code because no already available action can perform the next step.".into()});
            options.push(DecisionOption{id:"text_needed".into(),label:"Ask main agent for missing text to type.".into()});
            let request=DecisionRequest{goal:goal.into(),observation:json!({"page":observation,"supplied_text_values":input.text_values,"action_history":trace.iter().map(|entry|json!({"step":entry["step"],"action":entry["action"],"label":entry["label"],"status":entry["status"]})).collect::<Vec<_>>()}),options};
            anyhow::ensure!(serde_json::to_vec(&request)?.len()<=MAX_REQUEST,"Decision state exceeds safe size limit");
            let decision=bounded(ctx,timeout.min(deadline.saturating_duration_since(tokio::time::Instant::now())),transport.decide(&request)).await?;
            anyhow::ensure!(decision.confidence.is_finite() && (0.0..=1.0).contains(&decision.confidence),"Invalid decision probability/confidence");
            anyhow::ensure!(decision.reason.len()<=2000,"Decision reason exceeds limit");
            if decision.confidence<threshold {
                let label=request.options.iter().find(|option|option.id==decision.choice).map(|option|option.label.as_str()).unwrap_or("unknown choice");
                return Ok(("hand_back",format!("Low confidence: {:.3} below {threshold:.3}; tentative {}: {label}",decision.confidence,decision.choice)));
            }
            if matches!(decision.choice.as_str(),"script_needed"|"text_needed") {
                requested_help=Some(if decision.choice=="script_needed" {"script"}else{"text"});
                return Ok(("hand_back",if decision.choice=="script_needed" {"Main agent must supply an exact script/browser action candidate".into()}else{"Main agent must supply the required text_values".into()}));
            }
            if decision.choice=="hand_back" {return Ok(("hand_back",decision.reason));}
            let index=request.options.iter().position(|o|o.id==decision.choice).context("Decision selected an unknown action ID")?;
            // A DOM can change during decision latency. Refuse a stale selector instead of
            // applying an enumerated ID to a different fresh page/control.
            let check=bounded(ctx,timeout.min(deadline.saturating_duration_since(tokio::time::Instant::now())),provider.execute("eval",&observe,ctx)).await?;
            let metadata=check.metadata.context("Pre-action observation missing metadata")?;
            let mut fresh=metadata.get("result").cloned().unwrap_or(metadata);
            if let Some(text)=fresh.as_str() {fresh=serde_json::from_str(text)?;}
            anyhow::ensure!(fresh.is_object() && fresh["elements"].is_array() && fresh.to_string().len()<=MAX_OBSERVATION,"Malformed or oversized pre-action observation");
            if redact_credentials(&mut fresh) {fresh["sensitive"]=json!(true);}
            if fresh!=observation {
                observation=fresh;
                return Ok(("hand_back","DOM changed while deciding; return control rather than execute a stale action".into()));
            }
            if decision.choice=="done" {requested_help=None;return Ok(("done",decision.reason));}
            let chosen=choices.get(index).context("Decision selected invalid action")?;
            if let Some(index)=chosen.exact_index {used_exact.insert(index);}
            trace.push(json!({"step":step+1,"id":decision.choice,"action":chosen.input.action,"label":chosen.label,"confidence":decision.confidence,"status":"started"}));
            let executed=bounded(ctx,timeout.min(deadline.saturating_duration_since(tokio::time::Instant::now())),provider.execute(&chosen.input.action,&chosen.input,ctx)).await?;
            let failed=executed.metadata.as_ref().is_some_and(action_failed);
            trace.last_mut().unwrap()["status"]=json!("executed");
            // Retain exact capability results for the parent, not for Jev. Never truncate JSON.
            let retained=json!({"output":executed.output,"metadata":executed.metadata});
            trace.last_mut().unwrap()["result"]=retain_result(retained,&mut retained_result_bytes);
            let image_bytes:usize=images.iter().map(|i: &jcode_tool_types::ToolImage|i.data.len()).sum();
            if image_bytes+executed.images.iter().map(|i|i.data.len()).sum::<usize>()<=16_000_000 && images.len()+executed.images.len()<=4 {
                images.extend(executed.images);
            } else {anyhow::bail!("Image result exceeds handoff limits; retrieve using direct browser action");}
            if failed {
                trace.last_mut().unwrap()["status"]=json!("partial_failure");
                anyhow::bail!("Browser action reported failure or partial completion; do not repeat without checking the retained result");
            }
            let navigation=chosen.input.action=="open" || (chosen.input.action=="click" && chosen.input.url.is_some()) || chosen.input.provider_action.as_deref()==Some("navigate");
            settled_observation=Some(settle_after_action(provider,&observe,ctx,timeout,deadline,&observation,navigation).await?);
            // Always re-observe before consulting the transport again, including done.
        }
        unreachable!()
    }.await;
    let (status, reason) = match result {
        Ok(pair) => pair,
        Err(error) => ("hand_back", format!("{error:#}")),
    };
    if status == "hand_back"
        && let Some(last) = trace.last_mut()
        && last["status"] == "started"
    {
        last["status"] = json!("uncertain");
    }
    let mut output = outcome(
        status,
        &reason,
        &trace,
        &observation,
        transport.model(),
        requested_help,
    );
    output.images = images;
    Ok(output)
}

#[cfg(test)]
#[path = "browser_fast_tests.rs"]
mod tests;
