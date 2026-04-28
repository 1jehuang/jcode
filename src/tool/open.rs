use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
#[cfg(unix)]
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
    let backend = open_target(target).await?;
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
        return Ok("open".to_string());
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
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        if kind == LocalTargetKind::Directory {
            cmd.arg(path);
        } else {
            cmd.arg("-R").arg(path);
        }
        spawn_with_grace(cmd, "open").await?;
        return Ok(("open".to_string(), true));
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
        Ok(("explorer".to_string(), true))
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

#[cfg(test)]
#[path = "open_tests.rs"]
mod open_tests;
