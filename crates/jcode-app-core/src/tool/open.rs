use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;
#[cfg(all(unix, not(target_os = "macos")))]
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const OPEN_GRACE_PERIOD_MS: u64 = 800;
const URL_SCHEMES: &[&str] = &["http", "https", "mailto", "file"];

pub struct OpenTool;

impl OpenTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Deserialize)]
struct OpenInput {
    #[serde(default)]
    action: Option<String>,
    target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenAction {
    Open,
    Reveal,
}

impl OpenAction {
    fn parse(raw: Option<&str>) -> Result<Self> {
        match raw.unwrap_or("open") {
            "open" => Ok(Self::Open),
            "reveal" => Ok(Self::Reveal),
            other => anyhow::bail!(
                "Unknown open action: {}. Valid actions: open, reveal",
                other
            ),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Reveal => "reveal",
        }
    }
}

#[derive(Debug, Clone)]
enum ParsedTarget {
    Local(PathBuf),
    Url(String),
}

#[derive(Debug, Clone)]
enum ResolvedTarget {
    Local {
        path: PathBuf,
        kind: LocalTargetKind,
    },
    Url(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalTargetKind {
    File,
    Directory,
}

impl LocalTargetKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
        }
    }
}

struct OpenOutcome {
    _backend: String,
    message: String,
    metadata: Value,
}

#[async_trait]
impl Tool for OpenTool {
    fn name(&self) -> &str {
        "open"
    }

    fn description(&self) -> &str {
        "Open or reveal a file, folder, or URL for the user."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["target"],
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "enum": ["open", "reveal"],
                    "description": "Open action. Use 'open' to open the target or 'reveal' to show it in the file manager."
                },
                "target": {
                    "type": "string",
                    "description": "Open target."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        if input.get("mode").is_some() {
            anyhow::bail!("open.mode was removed. Use action='open' or action='reveal'.");
        }
        let params: OpenInput = serde_json::from_value(input)?;
        let requested_target = params.target.clone();
        let action = OpenAction::parse(params.action.as_deref())?;
        let action_name = action.as_str();
        let target = match resolve_target(&params.target, &ctx)
            .with_context(|| format!("Invalid open target: {}", params.target))
        {
            Ok(target) => target,
            Err(err) => {
                crate::logging::warn(&format!(
                    "[tool:open] failed to resolve target action={} session_id={} target={} error={}",
                    action_name, ctx.session_id, requested_target, err
                ));
                return Err(err);
            }
        };

        let outcome = match action {
            OpenAction::Open => perform_open(&target).await,
            OpenAction::Reveal => perform_reveal(&target).await,
        }
        .map_err(|err| {
            crate::logging::warn(&format!(
                "[tool:open] action failed action={} session_id={} target={} error={}",
                action_name, ctx.session_id, requested_target, err
            ));
            err
        })?;

        Ok(ToolOutput::new(outcome.message)
            .with_title(format!("open {}", action_name))
            .with_metadata(outcome.metadata))
    }
}

fn resolve_target(target: &str, ctx: &ToolContext) -> Result<ResolvedTarget> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        anyhow::bail!("target cannot be empty");
    }

    if let Some(parsed_target) = parse_target(trimmed)? {
        return match parsed_target {
            ParsedTarget::Url(url) => Ok(ResolvedTarget::Url(url)),
            ParsedTarget::Local(path) => resolve_local_target(path),
        };
    }

    let expanded = expand_home(trimmed)?;
    let resolved = ctx.resolve_path(Path::new(&expanded));
    resolve_local_target(resolved)
}

fn resolve_local_target(resolved: PathBuf) -> Result<ResolvedTarget> {
    if !resolved.exists() {
        anyhow::bail!("Target path does not exist: {}", resolved.display());
    }

    let kind = if resolved.is_dir() {
        LocalTargetKind::Directory
    } else {
        LocalTargetKind::File
    };

    Ok(ResolvedTarget::Local {
        path: resolved,
        kind,
    })
}

fn parse_target(target: &str) -> Result<Option<ParsedTarget>> {
    let Some(colon_index) = target.find(':') else {
        return Ok(None);
    };

    let scheme = &target[..colon_index];
    if scheme.len() == 1 && cfg!(windows) {
        return Ok(None);
    }
    if !scheme
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return Ok(None);
    }

    let lower = scheme.to_ascii_lowercase();
    if !URL_SCHEMES.iter().any(|allowed| *allowed == lower) {
        anyhow::bail!(
            "Unsupported URL scheme: {}. Allowed schemes: {}",
            scheme,
            URL_SCHEMES.join(", ")
        );
    }

    let parsed =
        url::Url::parse(target).with_context(|| format!("Failed to parse URL: {}", target))?;

    if lower == "file" {
        let path = parsed.to_file_path().map_err(|_| {
            anyhow::anyhow!(
                "Failed to convert file URL to a local path: {}. Use a local path or a valid file:// URL.",
                target
            )
        })?;
        return Ok(Some(ParsedTarget::Local(path)));
    }

    Ok(Some(ParsedTarget::Url(parsed.to_string())))
}

fn expand_home(path: &str) -> Result<PathBuf> {
    if path == "~" {
        return dirs::home_dir().context("Could not determine home directory for '~'");
    }

    let rest = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"));
    if let Some(rest) = rest {
        let home = dirs::home_dir().context("Could not determine home directory for '~'")?;
        return Ok(home.join(rest));
    }

    Ok(PathBuf::from(path))
}

async fn perform_open(target: &ResolvedTarget) -> Result<OpenOutcome> {
    // On Wayland compositors that prevent focus stealing (e.g. niri), launching
    // a URL via the system opener forwards it to the default browser, but the
    // existing browser window is not raised. Snapshot the browser windows before
    // opening so we can detect/raise the right one afterwards.
    let is_url = matches!(target, ResolvedTarget::Url(_));
    let focus_ctx = if is_url {
        capture_browser_windows_before_open()
    } else {
        None
    };
    let backend = open_target(target).await?;
    if is_url {
        focus_browser_window_after_open(focus_ctx).await;
    }
    let (message, metadata) = match target {
        ResolvedTarget::Url(url) => (
            format!("Opened {} in the default browser via {}.", url, backend),
            json!({
                "action": "open",
                "target_kind": "url",
                "target": url,
                "backend": backend,
            }),
        ),
        ResolvedTarget::Local { path, kind } => {
            let noun = match kind {
                LocalTargetKind::File => "file",
                LocalTargetKind::Directory => "folder",
            };
            (
                format!(
                    "Opened {} {} in the default application via {}.",
                    noun,
                    path.display(),
                    backend,
                ),
                json!({
                    "action": "open",
                    "target_kind": kind.as_str(),
                    "target": path.to_string_lossy(),
                    "backend": backend,
                }),
            )
        }
    };

    Ok(OpenOutcome {
        _backend: backend,
        message,
        metadata,
    })
}

async fn perform_reveal(target: &ResolvedTarget) -> Result<OpenOutcome> {
    let ResolvedTarget::Local { path, kind } = target else {
        anyhow::bail!("The reveal action only supports local filesystem paths");
    };

    let (backend, selection_supported) = reveal_target(path, *kind).await?;
    let message = if *kind == LocalTargetKind::Directory {
        format!(
            "Opened folder {} in the file manager via {}.",
            path.display(),
            backend
        )
    } else if selection_supported {
        format!(
            "Revealed {} in the file manager via {}.",
            path.display(),
            backend
        )
    } else {
        format!(
            "Opened the containing folder for {} via {}. File selection is not supported on this platform.",
            path.display(),
            backend,
        )
    };

    Ok(OpenOutcome {
        _backend: backend.clone(),
        message,
        metadata: json!({
            "action": "reveal",
            "target_kind": kind.as_str(),
            "target": path.to_string_lossy(),
            "backend": backend,
            "selection_supported": selection_supported,
        }),
    })
}

async fn open_target(target: &ResolvedTarget) -> Result<String> {
    // Never open real windows from test binaries, and honor
    // NO_BROWSER/JCODE_NO_BROWSER. Without this, agent-loop tests that
    // exercise the open tool pop browsers/viewers on the developer's desktop.
    if crate::auth::browser_suppressed(false) {
        anyhow::bail!(
            "opening files/URLs is suppressed (NO_BROWSER/JCODE_NO_BROWSER or test harness)"
        );
    }

    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        match target {
            ResolvedTarget::Local { path, .. } => {
                cmd.arg(path);
            }
            ResolvedTarget::Url(url) => {
                cmd.arg(url);
            }
        }
        spawn_with_grace(cmd, "open").await?;
        Ok("open".to_string())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let arg = match target {
            ResolvedTarget::Local { path, .. } => OsString::from(path.as_os_str()),
            ResolvedTarget::Url(url) => OsString::from(url),
        };
        try_unix_openers(vec![vec![arg.clone()], vec![OsString::from("open"), arg]]).await
    }

    #[cfg(windows)]
    {
        match target {
            ResolvedTarget::Local { path, .. } => open::that_detached(path),
            ResolvedTarget::Url(url) => open::that_detached(url),
        }
        .context("Failed to open with the system opener")?;
        Ok("system opener".to_string())
    }
}

async fn reveal_target(path: &Path, kind: LocalTargetKind) -> Result<(String, bool)> {
    // Same suppression as open_target: no real windows from tests/NO_BROWSER.
    if crate::auth::browser_suppressed(false) {
        anyhow::bail!(
            "revealing files is suppressed (NO_BROWSER/JCODE_NO_BROWSER or test harness)"
        );
    }

    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        if kind == LocalTargetKind::Directory {
            cmd.arg(path);
        } else {
            cmd.arg("-R").arg(path);
        }
        spawn_with_grace(cmd, "open").await?;
        Ok(("open".to_string(), true))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let to_open = if kind == LocalTargetKind::Directory {
            path.to_path_buf()
        } else {
            path.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| path.to_path_buf())
        };
        let backend = try_unix_openers(vec![
            vec![OsString::from(to_open.as_os_str())],
            vec![OsString::from("open"), OsString::from(to_open.as_os_str())],
        ])
        .await?;
        Ok((backend, false))
    }

    #[cfg(windows)]
    {
        let mut cmd = Command::new("explorer.exe");
        if kind == LocalTargetKind::Directory {
            cmd.arg(path);
        } else {
            cmd.arg(format!("/select,{}", path.display()));
        }
        spawn_with_grace(cmd, "explorer").await?;
        return Ok(("explorer".to_string(), true));
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
async fn try_unix_openers(arg_sets: Vec<Vec<OsString>>) -> Result<String> {
    let candidates = [("xdg-open", 0usize), ("gio", 1usize)];
    let mut not_found = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for (program, arg_index) in candidates {
        let args = arg_sets.get(arg_index).cloned().unwrap_or_else(Vec::new);
        let mut cmd = Command::new(program);
        cmd.args(args);
        match spawn_with_grace(cmd, program).await {
            Ok(()) => return Ok(program.to_string()),
            Err(e) => {
                let is_missing = e
                    .downcast_ref::<std::io::Error>()
                    .map(|io| io.kind() == std::io::ErrorKind::NotFound)
                    .unwrap_or(false);
                if is_missing {
                    not_found += 1;
                } else {
                    failures.push(format!("{}: {}", program, e));
                }
            }
        }
    }

    if not_found == candidates.len() {
        anyhow::bail!("No system opener found. Tried xdg-open and gio.");
    }

    anyhow::bail!(
        "Failed to open with the system opener: {}",
        failures.join("; ")
    )
}

async fn spawn_with_grace(mut cmd: Command, backend: &str) -> Result<()> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut child = crate::platform::spawn_detached(&mut cmd)
        .with_context(|| format!("Failed to open via {}", backend))?;

    tokio::time::sleep(Duration::from_millis(OPEN_GRACE_PERIOD_MS)).await;
    if let Some(status) = child.try_wait()?
        && !status.success()
    {
        match status.code() {
            Some(code) => {
                anyhow::bail!("Opener '{}' exited immediately with code {}", backend, code)
            }
            None => anyhow::bail!("Opener '{}' exited immediately", backend),
        }
    }

    Ok(())
}

/// Snapshot/context used to raise the browser window after opening a URL.
///
/// `stems` are normalized application identifiers for the default browser(s)
/// (e.g. `firefox`), and `pre_ids` are the matching window ids that existed
/// before the open so we can prefer a freshly created window afterwards.
#[allow(dead_code)]
struct BrowserFocusContext {
    stems: Vec<String>,
    pre_ids: HashSet<u64>,
}

#[cfg(all(unix, not(target_os = "macos")))]
fn capture_browser_windows_before_open() -> Option<BrowserFocusContext> {
    // Only the niri compositor is wired up for explicit window raising today.
    std::env::var_os("NIRI_SOCKET")?;
    let stems = browser_app_stems();
    if stems.is_empty() {
        return None;
    }
    let pre_ids = match query_niri_windows() {
        Ok(windows) => windows
            .iter()
            .filter(|w| app_id_matches(w.app_id.as_deref(), &stems))
            .map(|w| w.id)
            .collect(),
        Err(_) => HashSet::new(),
    };
    Some(BrowserFocusContext { stems, pre_ids })
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn capture_browser_windows_before_open() -> Option<BrowserFocusContext> {
    None
}

async fn focus_browser_window_after_open(ctx: Option<BrowserFocusContext>) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let Some(ctx) = ctx else {
            return;
        };
        // niri IPC is synchronous subprocess work; keep it off the async runtime.
        let _ = tokio::task::spawn_blocking(move || focus_browser_window_niri(&ctx)).await;
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = ctx;
    }
}

#[cfg(any(test, all(unix, not(target_os = "macos"))))]
#[derive(Debug, Clone, Deserialize)]
struct NiriWindow {
    id: u64,
    #[serde(rename = "app_id")]
    app_id: Option<String>,
    #[serde(default)]
    focus_timestamp: Option<NiriTimestamp>,
}

#[cfg(any(test, all(unix, not(target_os = "macos"))))]
#[derive(Debug, Clone, Copy, Deserialize)]
struct NiriTimestamp {
    secs: u64,
    nanos: u32,
}

/// Decide which browser window niri should raise after a URL open.
///
/// Prefer a window that appeared after the open (a brand new browser window).
/// Otherwise raise the most recently focused matching window, which is where
/// browsers add a new tab by default.
#[cfg(any(test, all(unix, not(target_os = "macos"))))]
fn select_window_to_focus(
    windows: &[NiriWindow],
    stems: &[String],
    pre_ids: &HashSet<u64>,
) -> Option<u64> {
    let matching: Vec<&NiriWindow> = windows
        .iter()
        .filter(|w| app_id_matches(w.app_id.as_deref(), stems))
        .collect();
    if matching.is_empty() {
        return None;
    }

    if let Some(newest) = matching
        .iter()
        .filter(|w| !pre_ids.contains(&w.id))
        .max_by_key(|w| w.id)
    {
        return Some(newest.id);
    }

    matching
        .iter()
        .max_by_key(|w| {
            w.focus_timestamp
                .map(|t| (t.secs, t.nanos))
                .unwrap_or((0, 0))
        })
        .map(|w| w.id)
}

/// Case-insensitive match between a window `app_id` and known browser stems.
#[cfg(any(test, all(unix, not(target_os = "macos"))))]
fn app_id_matches(app_id: Option<&str>, stems: &[String]) -> bool {
    let Some(app_id) = app_id else {
        return false;
    };
    let app_id = app_id.to_ascii_lowercase();
    if app_id.len() < 3 {
        return false;
    }
    stems.iter().any(|stem| {
        let stem = stem.to_ascii_lowercase();
        stem.len() >= 3 && (app_id == stem || app_id.contains(&stem) || stem.contains(&app_id))
    })
}

/// Normalize a desktop entry (e.g. `org.mozilla.firefox.desktop`) into the
/// application id stems a compositor is likely to report.
#[cfg(any(test, all(unix, not(target_os = "macos"))))]
fn normalize_desktop_entry_to_stems(entry: &str) -> Vec<String> {
    let entry = entry.trim();
    let stem = entry
        .strip_suffix(".desktop")
        .unwrap_or(entry)
        .to_ascii_lowercase();
    if stem.is_empty() {
        return Vec::new();
    }
    let mut stems = vec![stem.clone()];
    if let Some(last) = stem.rsplit('.').next()
        && last != stem
        && !last.is_empty()
    {
        stems.push(last.to_string());
    }
    stems
}

#[cfg(all(unix, not(target_os = "macos")))]
fn browser_app_stems() -> Vec<String> {
    let mut stems: Vec<String> = Vec::new();
    let mut push_unique = |new: Vec<String>| {
        for s in new {
            if !s.is_empty() && !stems.contains(&s) {
                stems.push(s);
            }
        }
    };

    if let Some(entry) = run_capture("xdg-settings", &["get", "default-web-browser"]) {
        push_unique(normalize_desktop_entry_to_stems(&entry));
    }
    if let Some(entry) = run_capture("xdg-mime", &["query", "default", "x-scheme-handler/https"]) {
        push_unique(normalize_desktop_entry_to_stems(&entry));
    }

    // Fall back to well-known browser application ids so raising still works
    // even when the xdg helpers are unavailable.
    push_unique(
        [
            "firefox",
            "librewolf",
            "waterfox",
            "floorp",
            "zen",
            "chromium",
            "google-chrome",
            "brave-browser",
            "vivaldi",
            "opera",
            "epiphany",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );

    stems
}

#[cfg(all(unix, not(target_os = "macos")))]
fn run_capture(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn query_niri_windows() -> Result<Vec<NiriWindow>> {
    let output = Command::new("niri")
        .args(["msg", "-j", "windows"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context("failed to run `niri msg -j windows`")?;
    if !output.status.success() {
        anyhow::bail!("`niri msg -j windows` exited unsuccessfully");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let windows: Vec<NiriWindow> = serde_json::from_str(stdout.trim())
        .context("failed to parse `niri msg -j windows` output")?;
    Ok(windows)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn focus_browser_window_niri(ctx: &BrowserFocusContext) {
    // The browser may need a moment to create/update its window after the open;
    // retry a few times before giving up.
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(300));
        }
        let Ok(windows) = query_niri_windows() else {
            return;
        };
        if let Some(id) = select_window_to_focus(&windows, &ctx.stems, &ctx.pre_ids) {
            let _ = Command::new("niri")
                .args(["msg", "action", "focus-window", "--id"])
                .arg(id.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            return;
        }
    }
}

#[cfg(test)]
#[path = "open_tests.rs"]
mod open_tests;
