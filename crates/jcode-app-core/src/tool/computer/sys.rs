//! Tier 3/4: clipboard, scripting bridge, waits, notifications, system state.

use super::osa;
use anyhow::{Result, bail};
use jcode_tool_types::ToolOutput;
use serde_json::json;
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

pub fn get_clipboard() -> Result<ToolOutput> {
    // pbpaste is the most reliable text read.
    let out = Command::new("/usr/bin/pbpaste")
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run pbpaste: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    Ok(ToolOutput::new(text).with_title("clipboard"))
}

pub fn set_clipboard(text: &str) -> Result<ToolOutput> {
    use std::io::Write;
    let mut child = Command::new("/usr/bin/pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to run pbcopy: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| anyhow::anyhow!("failed to write to pbcopy: {e}"))?;
    }
    child
        .wait()
        .map_err(|e| anyhow::anyhow!("pbcopy failed: {e}"))?;
    Ok(ToolOutput::new(format!("copied {} chars to clipboard", text.chars().count())))
}

pub fn run_applescript(script: &str) -> Result<ToolOutput> {
    let out = osa::run_applescript(script)?;
    Ok(ToolOutput::new(if out.is_empty() {
        "(AppleScript ran, no output)".to_string()
    } else {
        out
    })
    .with_title("applescript"))
}

pub fn run_jxa(script: &str) -> Result<ToolOutput> {
    let out = osa::run_jxa(script)?;
    Ok(ToolOutput::new(if out.is_empty() {
        "(JXA ran, no output)".to_string()
    } else {
        out
    })
    .with_title("jxa"))
}

pub fn notify(text: &str, title: Option<&str>) -> Result<ToolOutput> {
    let title = title.unwrap_or("jcode");
    osa::run_applescript(&format!(
        "display notification {} with title {}",
        osa::as_quote(text),
        osa::as_quote(title)
    ))?;
    Ok(ToolOutput::new(format!("posted notification: {text}")))
}

/// Poll an app's AX tree until a substring appears (element_appears) or a
/// timeout elapses. Cheap structural wait instead of fixed sleeps.
pub fn wait_for(app: &str, contains: &str, timeout_ms: u64) -> Result<ToolOutput> {
    let total = Duration::from_millis(timeout_ms.min(60_000));
    let deadline = Instant::now() + total;
    // Keep each poll bounded by the per-poll timeout below so it returns even on
    // big apps. Depth 8 captures most visible labels/values while staying cheap.
    let script = format!(
        r#"
using terms from application "System Events"
    on dumpEl(el, lvl, maxlvl)
        set out to ""
        if lvl > maxlvl then return out
        try
            set out to out & (title of el as text) & " "
        end try
        try
            set out to out & (value of el as text) & " "
        end try
        try
            set out to out & (description of el as text) & " "
        end try
        try
            repeat with child in (UI elements of el)
                set out to out & (my dumpEl(child, lvl + 1, maxlvl))
            end repeat
        end try
        return out
    end dumpEl
end using terms from
tell application "System Events"
    set frontApp to first application process whose name is {app}
    try
        return my dumpEl(front window of frontApp, 0, 8)
    on error
        return ""
    end try
end tell
"#,
        app = osa::as_quote(app)
    );
    loop {
        // Bound each poll by the time left so the whole call honors timeout_ms,
        // and never let a single poll exceed ~3s.
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("wait_for timed out after {timeout_ms}ms (no '{contains}' in {app})");
        }
        let poll_budget = remaining.min(Duration::from_secs(3));
        let tree = osa::run_applescript_timeout(&script, poll_budget).unwrap_or_default();
        if tree.contains(contains) {
            return Ok(ToolOutput::new(format!("matched '{contains}' in {app}")));
        }
        if Instant::now() >= deadline {
            bail!("wait_for timed out after {timeout_ms}ms (no '{contains}' in {app})");
        }
        sleep(Duration::from_millis(200));
    }
}

/// Read common system state (battery, display brightness, focus, etc.).
pub fn system_state() -> Result<ToolOutput> {
    let battery = Command::new("/usr/bin/pmset")
        .args(["-g", "batt"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let date = Command::new("/bin/date")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    Ok(ToolOutput::new(format!("date: {date}\n{battery}"))
        .with_title("system_state")
        .with_metadata(json!({"battery_raw": battery})))
}

/// Set display brightness 0.0..1.0 using the `brightness` cli if present, else
/// fall back to AppleScript key events. Brightness has no stable public API, so
/// this is best-effort.
pub fn set_brightness(level: f64) -> Result<ToolOutput> {
    let level = level.clamp(0.0, 1.0);
    // Try the `brightness` homebrew tool first.
    for path in ["/opt/homebrew/bin/brightness", "/usr/local/bin/brightness"] {
        if std::path::Path::new(path).exists() {
            let ok = Command::new(path)
                .arg(format!("{level}"))
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                return Ok(ToolOutput::new(format!("set brightness to {level:.2}")));
            }
        }
    }
    bail!(
        "no brightness control available. Install with `brew install brightness`, or adjust via the \
         brightness keys."
    )
}
