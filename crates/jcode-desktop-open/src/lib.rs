//! Open URLs and files, and reveal paths, with the desktop's own tools.
//!
//! Shared by `jcode companion` (which runs in the user's session) and the
//! CLI. The server never calls this directly: it asks the companion, so a
//! headless or remote server never pops windows on the wrong machine.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashSet;
#[cfg(all(unix, not(target_os = "macos")))]
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const OPEN_GRACE_PERIOD_MS: u64 = 800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalTargetKind {
    File,
    Directory,
}

impl LocalTargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
        }
    }

    pub fn of(path: &Path) -> Self {
        if path.is_dir() {
            Self::Directory
        } else {
            Self::File
        }
    }
}

#[derive(Debug, Clone)]
pub enum Target {
    Local {
        path: PathBuf,
        kind: LocalTargetKind,
    },
    Url(String),
}

impl Target {
    /// A URL (`scheme:...`) or an existing local path.
    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        if let Some((scheme, _)) = raw.split_once(':')
            && scheme.len() > 1
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            if scheme.eq_ignore_ascii_case("file") {
                let url = url::Url::parse(raw).context("invalid file URL")?;
                let path = url
                    .to_file_path()
                    .map_err(|_| anyhow::anyhow!("invalid file URL: {raw}"))?;
                let kind = LocalTargetKind::of(&path);
                return Ok(Self::Local { path, kind });
            }
            return Ok(Self::Url(raw.to_string()));
        }
        let path = PathBuf::from(raw);
        if !path.exists() {
            anyhow::bail!("Target path does not exist: {}", path.display());
        }
        let kind = LocalTargetKind::of(&path);
        Ok(Self::Local { path, kind })
    }
}

fn spawn_detached(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and only detaches the child.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }
    cmd.spawn()
}

/// Open with the system opener. Returns the backend that ran.
pub async fn open_target(target: &Target) -> Result<String> {
    let is_url = matches!(target, Target::Url(_));
    let focus_ctx = if is_url {
        capture_browser_windows_before_open()
    } else {
        None
    };
    let backend = open_with_system(target).await?;
    if is_url {
        focus_browser_window_after_open(focus_ctx).await;
    }
    Ok(backend)
}

async fn open_with_system(target: &Target) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        match target {
            Target::Local { path, .. } => {
                cmd.arg(path);
            }
            Target::Url(url) => {
                cmd.arg(url);
            }
        }
        spawn_with_grace(cmd, "open").await?;
        Ok("open".to_string())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let arg = match target {
            Target::Local { path, .. } => OsString::from(path.as_os_str()),
            Target::Url(url) => OsString::from(url),
        };
        try_unix_openers(vec![vec![arg.clone()], vec![OsString::from("open"), arg]]).await
    }

    #[cfg(windows)]
    {
        match target {
            Target::Local { path, .. } => open::that_detached(path),
            Target::Url(url) => open::that_detached(url),
        }
        .context("Failed to open with the system opener")?;
        Ok("system opener".to_string())
    }
}

/// Show `path` in the file manager. Returns the backend and whether the file
/// itself is selected (vs. just its folder opened).
pub async fn reveal_target(path: &Path, kind: LocalTargetKind) -> Result<(String, bool)> {
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

    let mut child =
        spawn_detached(&mut cmd).with_context(|| format!("Failed to open via {}", backend))?;

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
        if let Err(error) =
            tokio::task::spawn_blocking(move || focus_browser_window_niri(&ctx)).await
        {
            eprintln!("Browser focus worker failed: {error}");
        }
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        drop(ctx);
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
    let output = match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            eprintln!("Cannot query browser focus using {program}: {error}");
            return None;
        }
    };
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
            let result = Command::new("niri")
                .args(["msg", "action", "focus-window", "--id"])
                .arg(id.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            match result {
                Ok(status) if status.success() => {}
                Ok(status) => eprintln!("Browser focus command exited with {status}"),
                Err(error) => eprintln!("Browser focus command failed: {error}"),
            }
            return;
        }
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
