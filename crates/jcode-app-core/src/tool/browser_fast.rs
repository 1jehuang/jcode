//! Task-level browser agent. Each decision sees the task, current page and accumulated
//! action results. Executable actions come from trusted browser code or the parent,
//! never from page instructions or model-generated JavaScript.
use super::*;
use serde::Serialize;
use std::future::Future;
use std::time::Duration;

#[path = "browser_jev.rs"]
mod browser_jev;

const MAX_OPTIONS: usize = 240; // Includes four terminal/help options, below Jev's 255 limit.
const MAX_OBSERVATION: usize = 48_000;
// Internal state guard. The transport separately compacts history against its
// stricter final serialized wire budget (including JSON string escaping).
const MAX_REQUEST: usize = 160_000;

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

#[path = "browser_fast_actions.rs"]
mod browser_fast_actions;
use browser_fast_actions::{OBSERVE_SCRIPT, candidates};

pub(super) async fn handoff(
    provider: &dyn BrowserProvider,
    input: &BrowserInput,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    match browser_jev::JevTransport::new() {
        Ok(transport) => {
            let mut output = run(provider, &transport, input, ctx).await?;
            if let Some(metadata) = output.metadata.as_mut() {
                metadata["decision_provider"] = json!(transport.provider_name());
                output.output = serde_json::to_string(metadata)?;
            }
            Ok(output)
        }
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
        Value::String(text) => {
            if TOKENS.is_match(text) {
                *text = "[REDACTED: credential material]".into();
                true
            } else if (text.trim_start().starts_with('{') || text.trim_start().starts_with('['))
                && let Ok(mut structured) = serde_json::from_str::<Value>(text)
                && redact_credentials(&mut structured)
            {
                // Bridge output commonly duplicates metadata as encoded JSON.
                *text = structured.to_string();
                true
            } else {
                false
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |found, item| redact_credentials(item) || found),
        Value::Object(items) => items.iter_mut().fold(false, |found, (key, item)| {
            let key = key.to_ascii_lowercase().replace('-', "_");
            if matches!(
                key.as_str(),
                "password"
                    | "passwd"
                    | "secret"
                    | "access_token"
                    | "refresh_token"
                    | "id_token"
                    | "api_key"
                    | "apikey"
                    | "authorization"
                    | "cookie"
                    | "set_cookie"
                    | "otp"
                    | "cvv"
                    | "cvc"
                    | "token"
            ) && !item.is_null()
            {
                *item = json!("[REDACTED: credential material]");
                true
            } else {
                redact_credentials(item) || found
            }
        }),
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

fn retain_result(result: Value, trace: &mut [Value]) -> Value {
    let bytes = result.to_string().len();
    let omitted = || json!({"omitted":"Result exceeds the per-action or rolling 32000-byte retention budget. Inspect current state with a read-only direct browser action. Do not repeat side effects."});
    if bytes > 16_000 {
        return omitted();
    }
    let mut retained: usize = trace
        .iter()
        .filter_map(|entry| entry.get("result"))
        .map(|value| value.to_string().len())
        .sum();
    for entry in trace {
        if retained + bytes <= 32_000 {
            break;
        }
        if let Some(old) = entry.get_mut("result") {
            let old_size = old.to_string().len();
            let marker = json!({"omitted":"Older result evicted. Inspect read-only state. Do not repeat side effects."});
            let marker_size = marker.to_string().len();
            if old_size > marker_size {
                retained -= old_size - marker_size;
                *old = marker;
            }
        }
    }
    result
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

fn transition_disconnect(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}").to_lowercase();
    text.contains("receiving end does not exist")
        || text.contains("message port closed")
        || text.contains("frame was removed")
        || text.contains("execution context was destroyed")
}

fn parse_observation(output: ToolOutput) -> Result<Value> {
    let metadata = output.metadata.context("Observation missing metadata")?;
    let mut page = metadata.get("result").cloned().unwrap_or(metadata);
    if let Some(text) = page.as_str() {
        page = serde_json::from_str(text).context("Invalid observation JSON")?;
    }
    anyhow::ensure!(
        page.is_object() && page["elements"].is_array(),
        "Malformed DOM observation"
    );
    anyhow::ensure!(
        page.to_string().len() <= MAX_OBSERVATION,
        "DOM observation exceeds safe size limit"
    );
    if redact_credentials(&mut page) {
        page["sensitive"] = json!(true);
    }
    Ok(page)
}

async fn observe_page(
    provider: &dyn BrowserProvider,
    observe: &BrowserInput,
    ctx: &ToolContext,
    timeout: Duration,
    deadline: tokio::time::Instant,
) -> Result<Value> {
    let deadline = deadline.min(tokio::time::Instant::now() + Duration::from_secs(8));
    loop {
        let remaining =
            timeout.min(deadline.saturating_duration_since(tokio::time::Instant::now()));
        match bounded(ctx, remaining, provider.execute("eval", observe, ctx)).await {
            Ok(output) => return parse_observation(output),
            Err(error)
                if transition_disconnect(&error) && tokio::time::Instant::now() < deadline =>
            {
                bounded(
                    ctx,
                    deadline.saturating_duration_since(tokio::time::Instant::now()),
                    async {
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        Ok(())
                    },
                )
                .await?;
            }
            Err(error) => return Err(error),
        }
    }
}

// Keep evidence from previous pages as well as action results. A research task must
// not forget everything it read when it follows its next link. Old entries retain
// action/status summaries even when their larger evidence has aged out of context.
fn task_history(trace: &[Value]) -> Vec<Value> {
    let mut remaining = 20_000usize;
    let mut history = Vec::new();
    for entry in trace.iter().rev() {
        let mut item = json!({"step":entry["step"],"action":entry["action"],"label":entry["label"],"status":entry["status"]});
        for key in ["result", "after", "before"] {
            if let Some(value) = entry.get(key) {
                let size = value.to_string().len();
                if size <= remaining {
                    item[key] = value.clone();
                    remaining -= size;
                } else {
                    item[key] = json!({"omitted":"Older or oversized task evidence omitted from model context. Do not repeat side effects to recover it."});
                }
            }
        }
        history.push(item);
    }
    history.reverse();
    history
}

fn page_evidence(page: &Value) -> Value {
    json!({"url":page["url"],"title":page["title"],"text":page["text"].as_str().unwrap_or("").chars().take(2000).collect::<String>()})
}

// Irrelevant timers/ads may change while Jev decides. Auto-generated actions need
// the same page and identical target, not an identical whole-document snapshot.
// Exact scripts have opaque effects, so retain the conservative full-page check.
fn action_still_valid(chosen: &Candidate, before: &Value, fresh: &Value) -> bool {
    if fresh["sensitive"] == true
        || before["url"] != fresh["url"]
        || before["document_id"] != fresh["document_id"]
    {
        return false;
    }
    if chosen.exact_index.is_some() {
        return before == fresh;
    }
    if chosen.input.action == "wait" {
        return true;
    }
    if let Some(selector) = chosen.input.selector.as_deref() {
        for key in ["elements", "scroll_containers"] {
            let find = |page: &Value| {
                page[key]
                    .as_array()
                    .and_then(|items| items.iter().find(|e| e["selector"] == selector))
                    .cloned()
            };
            if let Some(target) = find(before) {
                return find(fresh).as_ref() == Some(&target);
            }
        }
        return false;
    }
    matches!(chosen.input.action.as_str(), "scroll" | "wait")
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
    let deadline = deadline.min(tokio::time::Instant::now() + Duration::from_secs(8));
    let remaining = || timeout.min(deadline.saturating_duration_since(tokio::time::Instant::now()));
    bounded(ctx, remaining(), async {
        tokio::time::sleep(Duration::from_millis(if require_progress {
            100
        } else {
            500
        }))
        .await;
        Ok(())
    })
    .await?;
    let mut previous: Option<Value> = None;
    loop {
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "Browser transition did not settle within eight seconds"
        );
        let output = match bounded(ctx, remaining(), provider.execute("eval", observe, ctx)).await {
            Ok(output) => output,
            Err(error) if transition_disconnect(&error) => {
                // Navigation tears down the old content script. Only retry the
                // read-only observation, never the click/submission itself.
                bounded(ctx, remaining(), async {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    Ok(())
                })
                .await?;
                continue;
            }
            Err(error) => return Err(error),
        };
        let fresh = parse_observation(output)?;
        if fresh["sensitive"] == true {
            return Ok(fresh);
        }
        let loaded = fresh["ready_state"]
            .as_str()
            .is_none_or(|state| state == "complete");
        let progressed =
            fresh["url"] != before["url"] || fresh["document_id"] != before["document_id"];
        // Dynamic text does not mean navigation is still loading. Two loaded
        // observations at the same URL suffice; the next action target is checked
        // separately immediately before execution.
        let same_page = previous.as_ref().is_some_and(|old| {
            old["url"] == fresh["url"] && old["ready_state"] == fresh["ready_state"]
        });
        if loaded && same_page && (!require_progress || progressed) {
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
    let mut used_exact = std::collections::HashSet::new();
    let mut requested_help = Some("uncertain");
    let result: Result<(&str,String)> = async {
        anyhow::ensure!(input.tab_id.is_some(),"handoff requires explicit tab_id");
        anyhow::ensure!(input.all_frames!=Some(true),"handoff must target a single frame");
        let mut caller_material=json!({"goal":input.goal,"context":input.context,"text_values":input.text_values});
        anyhow::ensure!(!redact_credentials(&mut caller_material),"Credential material must stay with the main agent/user, not the fast browser model");
        let goal=input.goal.as_deref().filter(|g|!g.trim().is_empty()).context("handoff requires goal")?;
        anyhow::ensure!(goal.len()<=8000,"Goal too large");
        anyhow::ensure!(input.context.as_ref().is_none_or(|context|context.len()<=12_000),"Task context too large");
        let budget=input.max_steps.unwrap_or(40);
        anyhow::ensure!((1..=100).contains(&budget),"max_steps must be 1..100");
        let threshold=input.confidence_threshold.unwrap_or(0.8);
        anyhow::ensure!(threshold.is_finite() && (0.0..=1.0).contains(&threshold),"confidence_threshold must be 0..1");
        anyhow::ensure!(input.candidates.len()<=64 && input.text_values.len()<=16 && input.text_values.iter().all(|s|s.len()<=2000),"Too many or oversized caller candidates/text values");
        // Validate parent actions even if the page would cause an immediate handback.
        candidates(input,&Value::Null)?;
        let timeout=Duration::from_millis(input.timeout_ms.unwrap_or(20_000).clamp(1,60_000));
        let deadline=tokio::time::Instant::now()+Duration::from_secs(600);
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
        let mut initial_document=Value::Null;
        let mut stale=0;
        let mut previous_action=None;
        let mut settled_observation=None;
        let mut replans=0;
        let mut initial_observed=false;
        let mut task_note=String::new();
        let mut explore_only=false;
        loop {
            anyhow::ensure!(tokio::time::Instant::now()<deadline,"Handoff time budget exhausted");
            let observe=scoped(BrowserInput{action:"eval".into(),script:Some(OBSERVE_SCRIPT.into()),..Default::default()},input)?;
            let fresh=if let Some(settled)=settled_observation.take() {settled} else {
                observe_page(provider,&observe,ctx,timeout,deadline).await?
            };
            observation=fresh;
            let url=observation["url"].as_str().map(str::to_owned);
            if !initial_observed { initial_url=url.clone(); initial_document=observation["document_id"].clone(); initial_observed=true; }
            if url!=initial_url || observation["document_id"]!=initial_document {
                // Caller scripts are page-bound. Retire them on navigation rather
                // than aborting the whole task or reusing them on another page.
                used_exact.extend(0..input.candidates.len());
            }
            if observation["sensitive"]==true {return Ok(("hand_back","Credential material, password, OTP, CAPTCHA, or account recovery requires the main agent/user. Never reset passwords.".into()));}
            if stale>=3 {return Ok(("hand_back","Browser stalled: repeated unchanged observations".into()));}
            if trace.len()==budget {return Ok(("hand_back","Action step budget exhausted; final action has been observed".into()));}
            let choices:Vec<_>=candidates(input,&observation)?.into_iter().filter(|c| c.exact_index.is_none_or(|i|!used_exact.contains(&i))).filter(|c|!explore_only || (c.exact_index.is_none() && matches!(c.input.action.as_str(),"scroll"|"wait"))).collect();
            let mut options:Vec<_>=choices.iter().enumerate().map(|(index,c)|DecisionOption{id:format!("a{index}"),label:c.label.clone()}).collect();
            options.push(DecisionOption{id:"done".into(),label:"Finish: the goal is already achieved.".into()});
            options.push(DecisionOption{id:"hand_back".into(),label:"Stop: uncertain, blocked, or needs user authorization.".into()});
            options.push(DecisionOption{id:"script_needed".into(),label:"Ask main agent for new code because no already available action can perform the next step.".into()});
            options.push(DecisionOption{id:"text_needed".into(),label:"Ask main agent for missing text to type.".into()});
            let capabilities:Vec<_>=choices.iter().enumerate().filter(|(_,choice)|choice.exact_index.is_some()).map(|(index,choice)|json!({"action_id":format!("a{index}"),"operation":choice.input.action,"source":"trusted_caller","executable_now":true,"requires_code_generation":false})).collect();
            let request=DecisionRequest{goal:goal.into(),observation:json!({"task_context":input.context,"caller_capabilities":capabilities,"page":observation,"supplied_text_values":input.text_values,"action_history":task_history(&trace),"controller_note":task_note,"remaining_actions":budget-trace.len()}),options};
            anyhow::ensure!(serde_json::to_vec(&request)?.len()<=MAX_REQUEST,"Decision state exceeds safe size limit");
            let decision=bounded(ctx,timeout.min(deadline.saturating_duration_since(tokio::time::Instant::now())),transport.decide(&request)).await?;
            anyhow::ensure!(decision.confidence.is_finite() && (0.0..=1.0).contains(&decision.confidence),"Invalid decision probability/confidence");
            anyhow::ensure!(decision.reason.len()<=2000,"Decision reason exceeds limit");
            let index=request.options.iter().position(|o|o.id==decision.choice).context("Decision selected an unknown action ID")?;
            // Asking for help never executes an action and should not be masked
            // by a low confidence score.
            if matches!(decision.choice.as_str(),"script_needed"|"text_needed") {
                requested_help=Some(if decision.choice=="script_needed" {"script"}else{"text"});
                return Ok(("hand_back",if decision.choice=="script_needed" {"Main agent must supply an exact script/browser action candidate".into()}else{"Main agent must supply the required text_values".into()}));
            }
            if decision.choice=="hand_back" {return Ok(("hand_back",decision.reason));}
            // Scrolling/waiting only obtains more evidence. Low confidence about
            // which viewport to inspect is not a reason to abandon the task.
            // Keep the threshold for interactions, exact caller actions and done.
            let observation_only=choices.get(index).is_some_and(|choice|choice.exact_index.is_none() && matches!(choice.input.action.as_str(),"scroll"|"wait"));
            if decision.confidence<threshold && !observation_only {
                if !explore_only && decision.choice!="done" && choices.iter().any(|choice|choice.exact_index.is_none() && matches!(choice.input.action.as_str(),"scroll"|"wait")) {
                    // Do not guess a click or ask the parent to micromanage a
                    // viewport. Let Jev choose only a safe evidence-gathering
                    // action, then reconsider interactions on the updated page.
                    explore_only=true;
                    task_note="No interaction was executed because confidence was insufficient. Gather more page evidence with the offered scrolling/waiting actions before reconsidering interactions.".into();
                    continue;
                }
                let label=&request.options[index].label;
                return Ok(("hand_back",format!("Low confidence: {:.3} below {threshold:.3}; tentative {}: {label}",decision.confidence,decision.choice)));
            }
            // A DOM can change during decision latency. Refuse a stale selector instead of
            // applying an enumerated ID to a different fresh page/control.
            let fresh=observe_page(provider,&observe,ctx,timeout,deadline).await?;
            let valid=if decision.choice=="done" {fresh["sensitive"]!=true && fresh["document_id"]==observation["document_id"] && fresh["url"]==observation["url"] && fresh["title"]==observation["title"] && fresh["text"]==observation["text"]} else {choices.get(index).is_some_and(|chosen|action_still_valid(chosen,&observation,&fresh))};
            if !valid {
                replans+=1;
                if replans>=3 {
                    observation=fresh;
                    return Ok(("hand_back","DOM changed repeatedly while deciding; no stale action was executed".into()));
                }
                task_note="The page or selected target changed while deciding. No action was executed. Replan from the fresh observation.".into();
                settled_observation=Some(fresh);
                continue;
            }
            observation=fresh;
            if decision.choice=="done" {requested_help=None;return Ok(("done",decision.reason));}
            replans=0;
            explore_only=false;
            task_note.clear();
            let chosen=choices.get(index).context("Decision selected invalid action")?;
            if let Some(index)=chosen.exact_index {used_exact.insert(index);}
            trace.push(json!({"step":trace.len()+1,"id":decision.choice,"action":chosen.input.action,"label":chosen.label,"confidence":decision.confidence,"status":"started","before":page_evidence(&observation)}));
            let navigation=chosen.input.action=="open" || (chosen.input.action=="click" && chosen.input.url.is_some()) || chosen.input.submit==Some(true) || chosen.input.provider_action.as_deref()==Some("navigate");
            let executed=bounded(ctx,timeout.min(deadline.saturating_duration_since(tokio::time::Instant::now())),provider.execute(&chosen.input.action,&chosen.input,ctx)).await;
            let executed=match executed {
                Ok(output) => Ok(output),
                // Only ordinary navigation/search can be verified this way. Never
                // replay a click, and never infer success for an opaque script or
                // a caller-authorized consequential action from a changed page.
                Err(error) if transition_disconnect(&error) && navigation && (chosen.exact_index.is_none() || chosen.input.action=="open") => {
                    let after=settle_after_action(provider,&observe,ctx,timeout,deadline,&observation,true).await?;
                    anyhow::ensure!(after["url"]!=observation["url"],"Navigation response was lost and destination could not be verified; do not repeat the action");
                    let recovered=ToolOutput::new("Navigation response lost, but the new page was observed. The action was not retried.").with_metadata(json!({"navigation_observed":true,"url":after["url"]}));
                    settled_observation=Some(after);
                    Ok(recovered)
                }
                Err(error) => Err(error),
            }?;
            let failed=executed.metadata.as_ref().is_some_and(action_failed);
            trace.last_mut().unwrap()["status"]=json!("executed");
            // Results are part of the next decision's context, not merely returned
            // to the parent at the end. Never forward credential-bearing results.
            let mut retained=json!({"output":executed.output,"metadata":executed.metadata});
            let sensitive_result=redact_credentials(&mut retained);
            let retained=retain_result(retained,&mut trace);
            trace.last_mut().unwrap()["result"]=retained;
            anyhow::ensure!(!sensitive_result,"Browser action returned credential material; requires the main agent/user");
            let image_bytes:usize=images.iter().map(|i: &jcode_tool_types::ToolImage|i.data.len()).sum();
            if image_bytes+executed.images.iter().map(|i|i.data.len()).sum::<usize>()<=16_000_000 && images.len()+executed.images.len()<=4 {
                images.extend(executed.images);
            } else {anyhow::bail!("Image result exceeds handoff limits; retrieve using direct browser action");}
            if failed {
                trace.last_mut().unwrap()["status"]=json!("partial_failure");
                anyhow::bail!("Browser action reported failure or partial completion; do not repeat without checking the retained result");
            }
            let after=if let Some(after)=settled_observation.take() {after} else {settle_after_action(provider,&observe,ctx,timeout,deadline,&observation,navigation).await?};
            let action_key=json!({"action":chosen.input.action,"selector":chosen.input.selector,"text":chosen.input.text,"script":chosen.input.script,"params":chosen.input.params,"x":chosen.input.x,"y":chosen.input.y,"result":trace.last().unwrap()["result"]});
            if after==observation {
                stale=if previous_action.as_ref()==Some(&action_key) {stale+1} else {1};
            } else {stale=0;}
            previous_action=Some(action_key);
            trace.last_mut().unwrap()["after"]=page_evidence(&after);
            settled_observation=Some(after);
            // Always re-observe before consulting the transport again, including done.
        }
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
