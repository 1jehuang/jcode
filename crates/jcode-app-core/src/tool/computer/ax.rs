//! Tier 1: Accessibility (AX) read + action.
//!
//! This is the *background* control path. It drives other apps' UI elements by
//! reference through `System Events`, so it can press buttons and set field
//! values without moving the cursor and (for many actions) without bringing the
//! target app to the front.
//!
//! Elements are addressed by a structural path: an app (by name) plus a chain of
//! 1-based child indices from the front window. `find_element` / `ui` return
//! these paths; the action verbs accept them.

use super::osa;
use anyhow::{Result, bail};
use jcode_tool_types::ToolOutput;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

/// A structural handle to an AX element.
#[derive(Debug, Clone, Deserialize)]
pub struct ElementHandle {
    /// Application process name (e.g. "Safari").
    pub app: String,
    /// 1-based child index chain from the app's front window to the element.
    /// Empty path == the front window itself.
    #[serde(default)]
    pub path: Vec<u32>,
}

impl ElementHandle {
    /// Emit an AppleScript expression that resolves to this element, bound to a
    /// variable name `el`. Assumes a `tell application "System Events"` context
    /// and that `frontApp` is the target process.
    fn resolve_script(&self) -> String {
        let mut expr = String::from("front window of frontApp");
        for idx in &self.path {
            expr = format!("UI element {idx} of ({expr})");
        }
        expr
    }
}

fn tell(app: &str, body: &str) -> String {
    format!(
        "tell application \"System Events\"\n\
         set frontApp to first application process whose name is {app}\n\
         {body}\n\
         end tell",
        app = osa::as_quote(app),
        body = body
    )
}

/// Dump the AX tree of an app (or the frontmost app) to a given depth.
pub fn ui_tree(app: Option<&str>, depth: u32) -> Result<ToolOutput> {
    let target = match app {
        Some(a) => format!("first application process whose name is {}", osa::as_quote(a)),
        None => "first application process whose frontmost is true".to_string(),
    };
    let script = format!(
        r##"
using terms from application "System Events"
    on dumpEl(el, lvl, maxlvl, idxPath)
        set out to ""
        if lvl > maxlvl then return out
        set r to "?"
        try
            set r to (role of el as text)
        end try
        set t to ""
        try
            set t to (title of el as text)
        end try
        if t is "" then
            try
                set t to (value of el as text)
            end try
        end if
        set d to ""
        try
            set d to (description of el as text)
        end try
        set pos to ""
        try
            set p to position of el
            set sz to size of el
            set pos to " @(" & (item 1 of p) & "," & (item 2 of p) & " " & (item 1 of sz) & "x" & (item 2 of sz) & ")"
        end try
        set indent to ""
        repeat lvl times
            set indent to indent & "  "
        end repeat
        set ln to indent & "#" & idxPath & " " & r
        if t is not "" then set ln to ln & " \"" & t & "\""
        if d is not "" then set ln to ln & " [" & d & "]"
        set ln to ln & pos & linefeed
        set out to out & ln
        try
            set i to 0
            repeat with child in (UI elements of el)
                set i to i + 1
                set out to out & (my dumpEl(child, lvl + 1, maxlvl, idxPath & "." & i))
            end repeat
        end try
        return out
    end dumpEl
end using terms from

tell application "System Events"
    set frontApp to {target}
    set appName to name of frontApp
    set out to "App: " & appName & " (element paths shown as #a.b.c == child indices from front window)" & linefeed
    try
        set win to front window of frontApp
        set out to out & (my dumpEl(win, 0, {depth}, ""))
    on error errMsg
        set out to out & "(no window / " & errMsg & ")"
    end try
    return out
end tell
"##,
        target = target,
        depth = depth
    );
    let tree = osa::run_applescript(&script)?;
    let tree = if tree.trim().is_empty() {
        "(empty Accessibility tree)".to_string()
    } else {
        tree
    };
    Ok(ToolOutput::new(tree).with_title("ui tree"))
}

/// Find elements matching role/title/value within an app, returning their paths.
pub fn find_element(
    app: &str,
    role: Option<&str>,
    title: Option<&str>,
    value: Option<&str>,
    max_depth: u32,
) -> Result<ToolOutput> {
    let role_m = role.map(osa::as_quote).unwrap_or_else(|| "\"\"".into());
    let title_m = title.map(osa::as_quote).unwrap_or_else(|| "\"\"".into());
    let value_m = value.map(osa::as_quote).unwrap_or_else(|| "\"\"".into());
    let script = format!(
        r#"
using terms from application "System Events"
    on findEl(el, lvl, maxlvl, idxPath, roleM, titleM, valueM)
        set out to ""
        if lvl > maxlvl then return out
        set r to ""
        try
            set r to (role of el as text)
        end try
        set t to ""
        try
            set t to (title of el as text)
        end try
        set v to ""
        try
            set v to (value of el as text)
        end try
        set ok to true
        if roleM is not "" and r is not roleM then set ok to false
        if titleM is not "" and t does not contain titleM then set ok to false
        if valueM is not "" and v does not contain valueM then set ok to false
        if ok and idxPath is not "" then
            set pos to ""
            try
                set p to position of el
                set sz to size of el
                set pos to " @(" & (item 1 of p) & "," & (item 2 of p) & " " & (item 1 of sz) & "x" & (item 2 of sz) & ")"
            end try
            set out to out & idxPath & "  " & r & " \"" & t & "\"" & pos & linefeed
        end if
        try
            set i to 0
            repeat with child in (UI elements of el)
                set i to i + 1
                set out to out & (my findEl(child, lvl + 1, maxlvl, idxPath & "." & i, roleM, titleM, valueM))
            end repeat
        end try
        return out
    end findEl
end using terms from

tell application "System Events"
    set frontApp to first application process whose name is {app}
    try
        set win to front window of frontApp
        set out to (my findEl(win, 0, {depth}, "", {role_m}, {title_m}, {value_m}))
    on error errMsg
        set out to "(error: " & errMsg & ")"
    end try
    if out is "" then set out to "(no matching elements)"
    return out
end tell
"#,
        app = osa::as_quote(app),
        depth = max_depth,
        role_m = role_m,
        title_m = title_m,
        value_m = value_m,
    );
    let res = osa::run_applescript(&script)?;
    Ok(ToolOutput::new(format!(
        "Matches in {app} (path  role  title  @pos). Use the path with press/set_value/get_value:\n{res}",
    ))
    .with_title("find_element"))
}

/// Perform AXPress on an element (background click).
pub fn press(handle: &ElementHandle) -> Result<ToolOutput> {
    let body = format!("perform action \"AXPress\" of ({})", handle.resolve_script());
    osa::run_applescript_timeout(&tell(&handle.app, &body), Duration::from_secs(10))?;
    Ok(ToolOutput::new(format!(
        "pressed element {:?} in {} (no cursor movement)",
        handle.path, handle.app
    )))
}

/// Perform an arbitrary AX action on an element.
pub fn perform_action(handle: &ElementHandle, ax_action: &str) -> Result<ToolOutput> {
    let body = format!(
        "perform action {} of ({})",
        osa::as_quote(ax_action),
        handle.resolve_script()
    );
    osa::run_applescript_timeout(&tell(&handle.app, &body), Duration::from_secs(10))?;
    Ok(ToolOutput::new(format!(
        "performed {ax_action} on element {:?} in {}",
        handle.path, handle.app
    )))
}

/// Set the value of an element (background typing into a field).
pub fn set_value(handle: &ElementHandle, value: &str) -> Result<ToolOutput> {
    let body = format!(
        "set value of ({}) to {}",
        handle.resolve_script(),
        osa::as_quote(value)
    );
    osa::run_applescript_timeout(&tell(&handle.app, &body), Duration::from_secs(10))?;
    Ok(ToolOutput::new(format!(
        "set value of element {:?} in {} to {} chars",
        handle.path,
        handle.app,
        value.chars().count()
    )))
}

/// Read the value of an element.
pub fn get_value(handle: &ElementHandle) -> Result<ToolOutput> {
    let body = format!("return value of ({}) as text", handle.resolve_script());
    let v = osa::run_applescript_timeout(&tell(&handle.app, &body), Duration::from_secs(10))?;
    Ok(ToolOutput::new(v).with_title("get_value"))
}

/// Select a menu-bar item by path, e.g. ["File", "Export…"].
pub fn select_menu(app: &str, path: &[String]) -> Result<ToolOutput> {
    if path.len() < 2 {
        bail!("select_menu needs at least a top menu and one item, e.g. [\"File\",\"Save\"]");
    }
    // The menu bar belongs to the process; menu access requires the app be
    // frontmost, so activate it first.
    let top = &path[0];
    let mut expr = format!(
        "menu bar item {top} of menu bar 1 of frontApp",
        top = osa::as_quote(top)
    );
    for item in path.iter().skip(1) {
        expr = format!(
            "menu item {item} of menu 1 of ({expr})",
            item = osa::as_quote(item),
            expr = expr
        );
    }
    let body = format!(
        "set frontmost of frontApp to true\n\
         delay 0.2\n\
         click ({expr})"
    );
    osa::run_applescript_timeout(&tell(app, &body), Duration::from_secs(10))?;
    Ok(ToolOutput::new(format!(
        "selected menu {} in {app}",
        path.join(" > ")
    )))
}

/// Return the element at a screen point (role/title), useful to confirm targets.
pub fn element_at(app: &str, x: f64, y: f64) -> Result<ToolOutput> {
    // Hit-test by walking the AX tree, but PRUNE: only descend into a subtree
    // whose own frame contains the point. A child's frame is contained in its
    // parent's, so a parent that doesn't contain the point can't have a matching
    // descendant. This turns a full-tree walk (which times out on huge apps like
    // System Settings) into a path-length walk. Also bound the depth defensively.
    let script = format!(
        r#"
using terms from application "System Events"
    on inFrame(el, px, py)
        try
            set p to position of el
            set sz to size of el
            set x1 to item 1 of p
            set y1 to item 2 of p
            if px >= x1 and px <= (x1 + (item 1 of sz)) and py >= y1 and py <= (y1 + (item 2 of sz)) then
                return true
            end if
        end try
        return false
    end inFrame
    on hit(el, px, py, idxPath, maxlvl, lvl, best)
        set bestHit to best
        if lvl > maxlvl then return bestHit
        -- Record this element if it contains the point (deepest wins).
        if my inFrame(el, px, py) then
            set r to ""
            try
                set r to (role of el as text)
            end try
            set t to ""
            try
                set t to (title of el as text)
            end try
            set p to position of el
            set sz to size of el
            set bestHit to idxPath & "  " & r & " \"" & t & "\" @(" & (item 1 of p) & "," & (item 2 of p) & " " & (item 1 of sz) & "x" & (item 2 of sz) & ")"
        end if
        -- Only descend into children that themselves contain the point.
        try
            set i to 0
            repeat with child in (UI elements of el)
                set i to i + 1
                if my inFrame(child, px, py) then
                    set bestHit to my hit(child, px, py, idxPath & "." & i, maxlvl, lvl + 1, bestHit)
                end if
            end repeat
        end try
        return bestHit
    end hit
end using terms from

tell application "System Events"
    set frontApp to first application process whose name is {app}
    try
        set win to front window of frontApp
        set out to my hit(win, {x}, {y}, "", 40, 0, "(none)")
    on error errMsg
        set out to "(error: " & errMsg & ")"
    end try
    return out
end tell
"#,
        app = osa::as_quote(app),
        x = x,
        y = y
    );
    let res = osa::run_applescript_timeout(&script, Duration::from_secs(12))?;
    Ok(ToolOutput::new(format!("Deepest element at ({x:.0},{y:.0}) in {app}:\n{res}"))
        .with_title("element_at")
        .with_metadata(json!({"app": app, "x": x, "y": y})))
}
