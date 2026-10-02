//! Native macOS "computer use" tool.
//!
//! The desktop analog of the `browser` tool: a single `action`-dispatched tool
//! that lets the agent see the screen and control the macOS GUI.
//!
//! ## Mechanisms and visibility
//!
//! - **Coordinate input** (`click`/`type`/`key`/`scroll`/`drag`) uses Core
//!   Graphics CGEvents on the shared HID stream, so it is *visible*: it moves the
//!   real cursor and types into the focused app.
//! - **Accessibility actions** (`press`/`set_value`/`select_menu`/...) and
//!   **scripting** (`run_applescript`) act on apps *by reference*, so they can
//!   work in the **background** without moving the cursor.
//!
//! By default the tool prefers non-disruptive mechanisms and restraint: because
//! this runs on the user's own live machine, the agent should act only on the
//! requested task, prefer background AX/scripting over moving the cursor or
//! stealing focus, and never take proactive control of the desktop. This policy
//! is conveyed to the model via the tool description and the `discover` output.
//!
//! ## Progressive disclosure
//!
//! Only a small set of common actions is described in the always-on schema to
//! keep prompt cost low. The full action set is fetched on demand via
//! `action="discover"` with a `category`.
//!
//! Everything is gated behind `cfg(target_os = "macos")`; other platforms return
//! a clear "unsupported" error.

use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

#[cfg(target_os = "macos")]
mod ax;
#[cfg(target_os = "macos")]
mod discover;
#[cfg(target_os = "macos")]
mod input;
#[cfg(target_os = "macos")]
mod keys;
#[cfg(target_os = "macos")]
mod osa;
#[cfg(target_os = "macos")]
mod screen;
#[cfg(target_os = "macos")]
mod setup;
#[cfg(target_os = "macos")]
mod sys;
#[cfg(target_os = "macos")]
mod win;

pub struct ComputerTool;

impl ComputerTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Deserialize)]
struct ComputerInput {
    action: String,
    // discovery
    #[serde(default)]
    category: Option<String>,
    // coordinates
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
    #[serde(default)]
    to_x: Option<f64>,
    #[serde(default)]
    to_y: Option<f64>,
    #[serde(default)]
    w: Option<f64>,
    #[serde(default)]
    h: Option<f64>,
    // text / keys
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    keys: Option<String>,
    #[serde(default)]
    dx: Option<i32>,
    #[serde(default)]
    dy: Option<i32>,
    #[serde(default)]
    depth: Option<u32>,
    // AX / scoping
    #[serde(default)]
    app: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    element: Option<Value>,
    #[serde(default)]
    ax_action: Option<String>,
    #[serde(default)]
    menu_path: Option<Vec<String>>,
    // windows
    #[serde(default)]
    window_id: Option<i64>,
    // scripting / wait / system
    #[serde(default)]
    script: Option<String>,
    #[serde(default)]
    contains: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    region: Option<[f64; 4]>,
    #[serde(default)]
    level: Option<f64>,
    /// For mutating actions: resolve and report the target without acting.
    #[serde(default)]
    dry_run: Option<bool>,
}

/// Cap a tool output's text so a huge AX tree / clipboard / OCR dump cannot
/// blow up the context. Keeps the head and notes how much was dropped.
#[cfg(target_os = "macos")]
fn cap_output(mut out: ToolOutput, max_chars: usize) -> ToolOutput {
    if out.output.len() > max_chars {
        let mut cut = max_chars;
        while cut > 0 && !out.output.is_char_boundary(cut) {
            cut -= 1;
        }
        let dropped = out.output.len() - cut;
        let head = out.output[..cut].to_string();
        out.output = format!("{head}\n… [truncated {dropped} chars]");
    }
    out
}

/// Actions that change desktop/app state. Used for dry_run gating.
#[cfg(target_os = "macos")]
fn is_mutating(action: &str) -> bool {
    matches!(
        action,
        "move"
            | "click"
            | "double_click"
            | "right_click"
            | "drag"
            | "scroll"
            | "type"
            | "key"
            | "key_down"
            | "key_up"
            | "press"
            | "set_value"
            | "perform_action"
            | "select_menu"
            | "activate_app"
            | "hide_app"
            | "quit_app"
            | "focus_window"
            | "move_window"
            | "resize_window"
            | "minimize_window"
            | "close_window"
            | "set_clipboard"
            | "run_applescript"
            | "run_jxa"
            | "notify"
            | "set_brightness"
    )
}

#[async_trait]
impl Tool for ComputerTool {
    fn name(&self) -> &str {
        "macos_computer_use"
    }

    fn description(&self) -> &str {
        "Control the macOS desktop: see the screen (screenshot/ocr/ui tree), click and type \
         (visible coordinate input), act on UI elements in the BACKGROUND via Accessibility \
         (press/set_value, no cursor movement), manage apps and windows, use the clipboard, and \
         run AppleScript. Coordinates are in points (top-left origin). This is the user's live \
         machine: act only on the requested task (not proactively) and prefer BACKGROUND \
         AX/scripting over moving the cursor or stealing focus; click/type only when AX can't \
         reach the target. Call action='discover' with a category for the full action set. Run \
         action='setup' first if permissions are missing."
    }

    fn parameters_schema(&self) -> Value {
        // Progressive disclosure: only the common actions + discover are spelled
        // out here to keep always-on prompt cost low (~370 tokens). Advanced
        // actions and their params are returned by action="discover".
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "description": "Common: screenshot, ocr, ui (see); click, type, key (visible input); \
                        press, set_value (BACKGROUND AX action on an `element` handle); find_element; \
                        run_applescript; setup, check_permissions; discover (load full action set). \
                        Many more actions (move, drag, scroll, window/app management, clipboard, \
                        select_menu, notify, ...) take the same fields; call discover for their params."
                },
                "category": {
                    "type": "string",
                    "enum": ["mouse","keyboard","observe","ax","windows","apps","clipboard","scripting","system","setup","all"],
                    "description": "For action='discover': which group to return full action specs for."
                },
                "x": { "type": "number", "description": "Screen X in points (top-left origin)." },
                "y": { "type": "number", "description": "Screen Y in points." },
                "text": { "type": "string", "description": "Text for type / set_clipboard / notify." },
                "keys": { "type": "string", "description": "Key chord, e.g. cmd+space, return, esc, ctrl+shift+t." },
                "app": { "type": "string", "description": "Target app/process name (AX, windows, scripting scope)." },
                "role": { "type": "string", "description": "AX role filter for find_element, e.g. AXButton." },
                "title": { "type": "string", "description": "AX title/label substring for find_element." },
                "value": { "type": "string", "description": "Value to match (find_element) or set (set_value)." },
                "element": {
                    "type": "object",
                    "description": "Element handle from find_element/ui: {app, path:[child indices]}. Used by press/set_value/get_value/perform_action.",
                    "properties": {
                        "app": { "type": "string" },
                        "path": { "type": "array", "items": { "type": "integer" } }
                    }
                },
                "script": { "type": "string", "description": "AppleScript (run_applescript) or JS (run_jxa) source." },
                "depth": { "type": "integer", "description": "Max AX tree depth for ui/find_element (default 12)." },
                "to_x": { "type": "number" },
                "to_y": { "type": "number" },
                "dx": { "type": "integer" },
                "dy": { "type": "integer" },
                "w": { "type": "number" },
                "h": { "type": "number" },
                "window_id": { "type": "integer", "description": "window_screenshot id (from list_windows)." },
                "menu_path": { "type": "array", "items": { "type": "string" }, "description": "select_menu path, e.g. [\"File\",\"Save\"]." },
                "ax_action": { "type": "string", "description": "perform_action AX action, e.g. AXShowMenu." },
                "contains": { "type": "string", "description": "wait_for: substring to await." },
                "timeout_ms": { "type": "integer" },
                "region": { "type": "array", "items": { "type": "number" }, "description": "ocr region [x,y,w,h]; omit for full screen." },
                "level": { "type": "number", "description": "set_brightness 0..1." },
                "dry_run": { "type": "boolean", "description": "Mutating actions: report intended action without doing it." }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let parsed: ComputerInput =
            serde_json::from_value(input).context("invalid `macos_computer_use` tool input")?;
        tokio::task::spawn_blocking(move || run(parsed))
            .await
            .context("macos_computer_use tool task panicked")?
    }
}

#[cfg(not(target_os = "macos"))]
fn run(_input: ComputerInput) -> Result<ToolOutput> {
    bail!("The `macos_computer_use` tool is only supported on macOS.")
}

#[cfg(target_os = "macos")]
fn run(input: ComputerInput) -> Result<ToolOutput> {
    let action = input.action.as_str();

    // dry_run: for mutating actions, report the intended target and stop.
    if input.dry_run == Some(true) && is_mutating(action) {
        return Ok(ToolOutput::new(format!(
            "[dry_run] would perform '{action}' (no action taken). \
             Re-issue without dry_run to execute."
        )));
    }

    let result = dispatch(action, &input);
    // Cap large textual outputs to protect context (images are unaffected).
    result.map(|o| cap_output(o, 16_000))
}

#[cfg(target_os = "macos")]
fn dispatch(action: &str, input: &ComputerInput) -> Result<ToolOutput> {
    match action {
        // ---- discovery & setup ----
        "discover" => discover::discover(input.category.as_deref()),
        "setup" => setup::setup(),
        "check_permissions" => setup::check_permissions(),

        // ---- observe ----
        "screenshot" => screen::screenshot(),
        "ocr" => screen::ocr(input.region),
        "window_screenshot" => {
            let id = input
                .window_id
                .context("window_screenshot requires `window_id`")?;
            screen::window_screenshot(id)
        }
        "ui" => ax::ui_tree(input.app.as_deref(), input.depth.unwrap_or(12)),
        "cursor" => {
            let p = input::current_cursor()?;
            Ok(
                ToolOutput::new(format!("cursor at ({:.0}, {:.0})", p.x, p.y))
                    .with_metadata(json!({ "x": p.x, "y": p.y })),
            )
        }

        // ---- coordinate input (visible) ----
        "move" => {
            let (x, y) = require_xy(input)?;
            input::move_to(x, y)?;
            Ok(ToolOutput::new(format!("moved cursor to ({x:.0}, {y:.0})")))
        }
        "click" => {
            let p = input::click(input.x, input.y, input::Button::Left, 1)?;
            Ok(ToolOutput::new(format!(
                "clicked at ({:.0}, {:.0})",
                p.x, p.y
            )))
        }
        "double_click" => {
            let p = input::click(input.x, input.y, input::Button::Left, 2)?;
            Ok(ToolOutput::new(format!(
                "double-clicked at ({:.0}, {:.0})",
                p.x, p.y
            )))
        }
        "right_click" => {
            let p = input::click(input.x, input.y, input::Button::Right, 1)?;
            Ok(ToolOutput::new(format!(
                "right-clicked at ({:.0}, {:.0})",
                p.x, p.y
            )))
        }
        "drag" => {
            let (x, y) = require_xy(input)?;
            match (input.to_x, input.to_y) {
                (Some(tx), Some(ty)) => {
                    input::drag(x, y, tx, ty)?;
                    Ok(ToolOutput::new(format!(
                        "dragged from ({x:.0},{y:.0}) to ({tx:.0},{ty:.0})"
                    )))
                }
                _ => bail!("action='drag' requires `to_x` and `to_y`"),
            }
        }
        "scroll" => {
            let dx = input.dx.unwrap_or(0);
            let dy = input.dy.unwrap_or(0);
            if dx == 0 && dy == 0 {
                bail!("action='scroll' requires non-zero `dx` and/or `dy`");
            }
            input::scroll(input.x, input.y, dx, dy)?;
            Ok(ToolOutput::new(format!("scrolled dx={dx} dy={dy}")))
        }
        "type" => {
            let text = input
                .text
                .as_deref()
                .filter(|s| !s.is_empty())
                .context("action='type' requires non-empty `text`")?;
            input::type_text(text)?;
            Ok(ToolOutput::new(format!(
                "typed {} characters",
                text.chars().count()
            )))
        }
        "key" => {
            let keys = input
                .keys
                .as_deref()
                .filter(|s| !s.is_empty())
                .context("action='key' requires a `keys` chord, e.g. 'cmd+space'")?;
            input::key_chord(keys)?;
            Ok(ToolOutput::new(format!("pressed {keys}")))
        }
        "key_down" | "key_up" => {
            let keys = input
                .keys
                .as_deref()
                .filter(|s| !s.is_empty())
                .context("requires a `keys` value")?;
            input::key_hold(keys, action == "key_down")?;
            Ok(ToolOutput::new(format!("{action} {keys}")))
        }

        // ---- AX background actions (Tier 1) ----
        "find_element" => {
            let app = input
                .app
                .as_deref()
                .context("find_element requires `app`")?;
            ax::find_element(
                app,
                input.role.as_deref(),
                input.title.as_deref(),
                input.value.as_deref(),
                input.depth.unwrap_or(20),
            )
        }
        "element_at" => {
            let app = input.app.as_deref().context("element_at requires `app`")?;
            let (x, y) = require_xy(input)?;
            ax::element_at(app, x, y)
        }
        "press" => ax::press(&parse_element(input)?),
        "get_value" => ax::get_value(&parse_element(input)?),
        "set_value" => {
            let v = input
                .value
                .as_deref()
                .context("set_value requires `value`")?;
            ax::set_value(&parse_element(input)?, v)
        }
        "perform_action" => {
            let a = input
                .ax_action
                .as_deref()
                .context("perform_action requires `ax_action`")?;
            ax::perform_action(&parse_element(input)?, a)
        }
        "select_menu" => {
            let app = input.app.as_deref().context("select_menu requires `app`")?;
            let path = input
                .menu_path
                .as_ref()
                .context("select_menu requires `menu_path`")?;
            ax::select_menu(app, path)
        }

        // ---- windows / apps (Tier 2) ----
        "list_apps" => win::list_apps(),
        "list_windows" => win::list_windows(),
        "activate_app" => win::activate_app(req_app(input)?),
        "hide_app" => win::hide_app(req_app(input)?),
        "quit_app" => win::quit_app(req_app(input)?),
        "focus_window" => win::focus_window(req_app(input)?),
        "move_window" => {
            let (x, y) = require_xy(input)?;
            win::move_window(req_app(input)?, x, y)
        }
        "resize_window" => {
            let w = input.w.context("resize_window requires `w`")?;
            let h = input.h.context("resize_window requires `h`")?;
            win::resize_window(req_app(input)?, w, h)
        }
        "minimize_window" => win::minimize_window(req_app(input)?),
        "close_window" => win::close_window(req_app(input)?),

        // ---- clipboard / scripting / system (Tier 3/4) ----
        "get_clipboard" => sys::get_clipboard(),
        "set_clipboard" => {
            let t = input
                .text
                .as_deref()
                .context("set_clipboard requires `text`")?;
            sys::set_clipboard(t)
        }
        "run_applescript" => {
            let s = input
                .script
                .as_deref()
                .context("run_applescript requires `script`")?;
            sys::run_applescript(s)
        }
        "run_jxa" => {
            let s = input
                .script
                .as_deref()
                .context("run_jxa requires `script`")?;
            sys::run_jxa(s)
        }
        "wait_for" => {
            let app = input.app.as_deref().context("wait_for requires `app`")?;
            let c = input
                .contains
                .as_deref()
                .context("wait_for requires `contains`")?;
            sys::wait_for(app, c, input.timeout_ms.unwrap_or(10_000))
        }
        "notify" => {
            let t = input.text.as_deref().context("notify requires `text`")?;
            sys::notify(t, input.title.as_deref())
        }
        "system_state" => sys::system_state(),
        "set_brightness" => {
            let l = input
                .level
                .context("set_brightness requires `level` (0..1)")?;
            sys::set_brightness(l)
        }

        other => bail!(
            "Unknown macos_computer_use action: {other}. Call action='discover' (category='all') to list every action."
        ),
    }
}

#[cfg(target_os = "macos")]
fn require_xy(input: &ComputerInput) -> Result<(f64, f64)> {
    match (input.x, input.y) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => bail!("action='{}' requires both `x` and `y`", input.action),
    }
}

#[cfg(target_os = "macos")]
fn req_app(input: &ComputerInput) -> Result<&str> {
    input
        .app
        .as_deref()
        .with_context(|| format!("action='{}' requires `app`", input.action))
}

#[cfg(target_os = "macos")]
fn parse_element(input: &ComputerInput) -> Result<ax::ElementHandle> {
    let raw = input.element.clone().context(
        "this action requires an `element` handle {app, path:[...]} from find_element/ui",
    )?;
    serde_json::from_value(raw).context("invalid `element` handle")
}

#[cfg(all(test, target_os = "macos"))]
mod coverage_tests;
#[cfg(all(test, target_os = "macos"))]
mod tests;
