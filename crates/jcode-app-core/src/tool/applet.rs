//! `applet` tool: mount declarative native UI (agent applets) into the session.
use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use jcode_applet_types::{
    Anchor, AssetDecl, Document, Instance, Lifetime, PatchOp, Placement, Scope, View,
    agent::{self, AgentApplets},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::oneshot;

/// Cap on agent-facing action message size.
const MAX_ACTION_MESSAGE: usize = 4000;

type WaiterMap = HashMap<(String, String), oneshot::Sender<String>>;

fn waiters() -> &'static Mutex<WaiterMap> {
    static WAITERS: OnceLock<Mutex<WaiterMap>> = OnceLock::new();
    WAITERS.get_or_init(Default::default)
}

/// Hand an action message to an `applet` tool call waiting on this instance.
/// Returns false when nothing was waiting.
pub fn deliver_to_waiter(session_id: &str, instance: &str, message: String) -> bool {
    let sender = waiters()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&(session_id.to_string(), instance.to_string()));
    sender.is_some_and(|tx| tx.send(message).is_ok())
}

fn cap(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s.push('…');
    }
    s
}

/// Agent-facing text for a user action.
pub fn format_action_message(
    instance: &str,
    title: &str,
    action: &jcode_applet_types::Action,
    state: &Value,
    source_key: Option<&str>,
) -> String {
    let args = if action.args.is_null() {
        "{}".to_string()
    } else {
        cap(action.args.to_string(), 1500)
    };
    let mut out = format!(
        "[applet action] instance `{instance}` ({}): `{}` args {args}",
        serde_json::to_string(title).unwrap_or_default(),
        action.action,
    );
    if let Some(key) = source_key {
        out.push_str(&format!(" from `{key}`"));
    }
    out.push_str(&format!("\nstate: {}", cap(state.to_string(), 2000)));
    cap(out, MAX_ACTION_MESSAGE)
}

/// Publish a session's applets snapshot to its clients.
pub fn publish(session_id: &str, snapshot: AgentApplets) {
    crate::applets::publish(session_id, snapshot);
}

/// Short, session-local id: slug of `title` plus 4 hex digits.
pub fn generate_instance_id(title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 24 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "applet" } else { slug };
    format!("{slug}-{:04x}", rand::random::<u16>())
}

#[derive(Default)]
pub struct AppletTool;

impl AppletTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct AppletInput {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    instance: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    view: Option<Value>,
    #[serde(default)]
    state: Option<Value>,
    #[serde(default)]
    assets: Option<Vec<AssetDecl>>,
    #[serde(default)]
    placement: Option<Value>,
    #[serde(default)]
    lifetime: Option<Lifetime>,
    #[serde(default)]
    ops: Option<Vec<PatchOp>>,
    #[serde(default)]
    base_revision: Option<u64>,
    #[serde(default)]
    wait: Option<bool>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

fn resolve_placement(raw: Option<&Value>, ctx: &ToolContext) -> Result<Placement> {
    let inline = |anchor: Anchor| Placement::Inline {
        session_id: ctx.session_id.clone(),
        anchor,
    };
    let tool_call = || Anchor::ToolCall {
        call_id: ctx.tool_call_id.clone(),
    };
    let Some(raw) = raw.filter(|v| !v.is_null()) else {
        return Ok(inline(tool_call()));
    };
    if let Some(s) = raw.as_str() {
        return Ok(match s {
            "inline" => inline(tool_call()),
            "end" => inline(Anchor::End),
            "panel" => Placement::Panel {
                open: Default::default(),
            },
            "sidebar" => Placement::Sidebar,
            "overlay" => Placement::Overlay {
                corner: Default::default(),
            },
            "composer" => Placement::Composer {
                session_id: ctx.session_id.clone(),
            },
            other => bail!(
                "unknown placement {other:?}; use inline, end, panel, sidebar, overlay, composer, or an object"
            ),
        });
    }
    let mut obj = raw
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow!("placement must be a string or object"))?;
    let kind = obj
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("inline")
        .to_string();
    let kind = kind.as_str();
    obj.insert("kind".into(), json!(kind));
    if matches!(kind, "inline" | "composer") {
        obj.insert("session_id".into(), json!(ctx.session_id));
    }
    if kind == "inline" && !obj.contains_key("anchor") {
        obj.insert(
            "anchor".into(),
            json!({"kind":"tool_call","call_id":ctx.tool_call_id}),
        );
    }
    serde_json::from_value(Value::Object(obj)).context("invalid placement")
}

fn parse_view(view: Value) -> Result<View> {
    serde_json::from_value(view).context("invalid view (see the node list in the tool description)")
}

#[async_trait]
impl Tool for AppletTool {
    fn name(&self) -> &str {
        "applet"
    }

    fn description(&self) -> &str {
        "Show interactive native UI (an applet) in Jcode Desktop. By default the card replaces this tool call in the transcript. \
View is a tree of nodes, each {\"type\":...}. Layout: stack{direction:vertical|horizontal,gap,padding,align,children}, grid{min_column_width,children}, scroll{max_height,children}, card{title,children} (don't nest), tabs{bind,tabs:[{id,label,children}]}, spacer, divider. \
Content: text{text,style:body|title|heading|caption|mono,tone,max_lines}, markdown{text}, code{text,language}, image{source:{url|data|asset},alt,aspect_ratio}, icon{name}, key_value{rows:[{key,value}]}, table{columns,rows}, progress{value 0-1,label}, empty{title,detail}, error{message,retry}. \
Controls (always pills): button{label,variant:primary|secondary|compact|danger,on_press}, chip{label,on_press}, toggle{label,bind}, input{bind,placeholder,multiline,on_submit}, select{bind,options:[{id,label}]}, list{children:[list_item{title,subtitle,meta,badges,on_press}]}. Escape hatch: html{source,height}. \
Spacing tokens none|xs|sm|md|lg|xl; tones default|dim|accent|success|warning|danger. Inputs/toggles/selects/tabs bind to keys in `state`. \
Actions are {action,args}. Custom names come back to you as an `[applet action]` message with the current state (or as this tool's output with wait=true). \
Host actions run locally: host.open_url{url}, host.copy{text}, host.send_prompt{text}, host.close, host.set_state{...}. \
Use update to replace the document, patch for small changes (ops on /view, /state, /title), move to change placement, close to remove."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {"type":"string","enum":["mount","update","patch","move","close","list"],"default":"mount"},
                "instance": {"type":"string","description":"Instance id. Optional for mount (generated). Required otherwise."},
                "title": {"type":"string"},
                "view": {"type":"object","description":"Root view node, e.g. {\"type\":\"stack\",\"gap\":\"sm\",\"children\":[...]}."},
                "state": {"type":"object","description":"Initial values for bound controls."},
                "assets": {"type":"array","items":{"type":"object"},"description":"Images: [{id,mime,data(base64)}], referenced as {\"asset\":id}."},
                "placement": {"description":"Default: inline in place of this tool call. Shorthand: inline, end, panel, sidebar, overlay, composer. Or an object like {\"kind\":\"panel\"}.","anyOf":[{"type":"string"},{"type":"object"}]},
                "lifetime": {"type":"string","enum":["ephemeral","session","persistent"]},
                "ops": {"type":"array","items":{"type":"object"},"description":"patch: [{op:add|replace|remove,path:\"/state/q\",value}]"},
                "base_revision": {"type":"integer","description":"patch: revision the ops apply to. Default current."},
                "wait": {"type":"boolean","description":"Block until the user acts on this instance, then return the action and state."},
                "timeout_seconds": {"type":"integer","description":"wait timeout, default 600."}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: AppletInput = serde_json::from_value(input).context("invalid applet input")?;
        let action = params.action.as_deref().unwrap_or("mount");
        let sid = ctx.session_id.clone();
        let require_id = || {
            params
                .instance
                .clone()
                .ok_or_else(|| anyhow!("instance is required for {action}"))
        };

        let (snapshot, id) = match action {
            "list" => {
                let snapshot = crate::applets::snapshot_for_session(&sid)?;
                let lines: Vec<String> = snapshot
                    .instances
                    .iter()
                    .map(|i| {
                        format!(
                            "- `{}` rev {} \"{}\" state {}",
                            i.id,
                            i.document.revision,
                            i.document.title,
                            cap(i.document.state.to_string(), 300)
                        )
                    })
                    .collect();
                let text = if lines.is_empty() {
                    "No applets mounted in this session.".to_string()
                } else {
                    lines.join("\n")
                };
                return Ok(ToolOutput::new(text).with_title("applet list"));
            }
            "mount" | "update" => {
                let existing = crate::applets::snapshot_for_session(&sid)?;
                let id = match (&params.instance, action) {
                    (Some(id), _) => id.clone(),
                    (None, "mount") => {
                        generate_instance_id(params.title.as_deref().unwrap_or("applet"))
                    }
                    _ => bail!("instance is required for update"),
                };
                let prior = existing.get(&id);
                if action == "update" && prior.is_none() {
                    bail!("no applet instance `{id}` in this session; use mount");
                }
                let view = match params.view.clone() {
                    Some(v) => parse_view(v)?,
                    None => prior
                        .map(|p| p.document.view.clone())
                        .ok_or_else(|| anyhow!("view is required for mount"))?,
                };
                let title = params
                    .title
                    .clone()
                    .or_else(|| prior.map(|p| p.document.title.clone()))
                    .unwrap_or_else(|| "Applet".to_string());
                let state = params
                    .state
                    .clone()
                    .or_else(|| prior.map(|p| p.document.state.clone()))
                    .unwrap_or_else(|| json!({}));
                let assets = params
                    .assets
                    .clone()
                    .or_else(|| prior.map(|p| p.document.assets.clone()))
                    .unwrap_or_default();
                let placement = match (&params.placement, prior) {
                    (None, Some(p)) => p.placement.clone(),
                    (raw, _) => resolve_placement(raw.as_ref(), &ctx)?,
                };
                let lifetime = params
                    .lifetime
                    .or_else(|| prior.map(|p| p.lifetime))
                    .unwrap_or_default();
                let revision = prior.map(|p| p.document.revision + 1).unwrap_or(1);
                let snapshot = crate::applets::mount(
                    &sid,
                    Instance {
                        id: id.clone(),
                        applet: agent::APPLET_ID.to_string(),
                        placement,
                        scope: Scope::Session {
                            session_id: sid.clone(),
                        },
                        lifetime,
                        document: Document {
                            revision,
                            title,
                            view,
                            state,
                            assets,
                        },
                    },
                )?;
                (snapshot, id)
            }
            "patch" => {
                let id = require_id()?;
                let ops = params
                    .ops
                    .clone()
                    .ok_or_else(|| anyhow!("ops is required for patch"))?;
                let (snapshot, _) = crate::applets::patch(&sid, &id, params.base_revision, &ops)?;
                (snapshot, id)
            }
            "move" => {
                let id = require_id()?;
                let placement = resolve_placement(
                    Some(
                        params
                            .placement
                            .as_ref()
                            .ok_or_else(|| anyhow!("placement is required for move"))?,
                    ),
                    &ctx,
                )?;
                (crate::applets::move_instance(&sid, &id, placement)?, id)
            }
            "close" => {
                let id = require_id()?;
                let (snapshot, existed) = crate::applets::close(&sid, &id)?;
                if !existed {
                    bail!("no applet instance `{id}` in this session");
                }
                publish(&sid, snapshot);
                return Ok(ToolOutput::new(format!("Closed applet `{id}`.")).with_title("applet"));
            }
            other => bail!("unknown applet action {other:?}"),
        };

        let revision = snapshot
            .get(&id)
            .map(|i| i.document.revision)
            .unwrap_or_default();

        // Register the waiter before publishing so no action can slip past.
        let wait_rx = if params.wait.unwrap_or(false) {
            let (tx, rx) = oneshot::channel();
            waiters()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert((sid.clone(), id.clone()), tx);
            Some(rx)
        } else {
            None
        };
        publish(&sid, snapshot);

        let summary = format!("Applet `{id}` revision {revision}.");
        let Some(rx) = wait_rx else {
            return Ok(ToolOutput::new(format!(
                "{summary} User actions with custom names arrive as `[applet action]` messages; pass wait=true to block for one."
            ))
            .with_title("applet"));
        };

        let timeout = Duration::from_secs(params.timeout_seconds.unwrap_or(600).clamp(1, 3600));
        let interrupted = async {
            match &ctx.graceful_shutdown_signal {
                Some(signal) => {
                    if !signal.is_set() {
                        signal.notified().await;
                    }
                }
                None => std::future::pending::<()>().await,
            }
        };
        let outcome = tokio::select! {
            result = rx => result.ok(),
            _ = tokio::time::sleep(timeout) => None,
            _ = interrupted => None,
        };
        // Drop a stale waiter on timeout or interrupt.
        waiters()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&(sid, id.clone()));
        Ok(match outcome {
            Some(message) => ToolOutput::new(format!("{summary}\n{message}")),
            None => ToolOutput::new(format!(
                "{summary} No user action before the wait ended. Later actions arrive as `[applet action]` messages."
            )),
        }
        .with_title("applet"))
    }
}

#[cfg(test)]
#[path = "applet_tests.rs"]
mod tests;
