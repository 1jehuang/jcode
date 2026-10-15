use super::{Tool, ToolContext, ToolOutput};
use crate::companion_client::{CompanionError, CompanionRequest};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

const COMPANION_TIMEOUT: Duration = Duration::from_secs(10);
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

/// Ask `jcode companion` (the user's desktop session) to do it, else fall back
/// to the desktop of the machine this server runs on. The fallback keeps a
/// standalone local `jcode` working without a companion. Returns the backend.
async fn dispatch(
    request: CompanionRequest,
    local: impl std::future::Future<Output = Result<String>>,
) -> Result<String> {
    // Never open real windows from test binaries, and honor
    // NO_BROWSER/JCODE_NO_BROWSER. Without this, agent-loop tests that
    // exercise the open tool pop browsers/viewers on the developer's desktop.
    if crate::auth::browser_suppressed(false) {
        anyhow::bail!(
            "opening files/URLs is suppressed (NO_BROWSER/JCODE_NO_BROWSER or test harness)"
        );
    }
    match crate::companion_client::request_async(request, COMPANION_TIMEOUT).await {
        Ok(response) => Ok(format!("jcode companion ({})", response.message)),
        Err(CompanionError::Unavailable) => local.await,
        Err(error) => Err(anyhow::anyhow!(error.to_string())),
    }
}

fn desktop_target(target: &ResolvedTarget) -> jcode_desktop_open::Target {
    match target {
        ResolvedTarget::Url(url) => jcode_desktop_open::Target::Url(url.clone()),
        ResolvedTarget::Local { path, kind } => jcode_desktop_open::Target::Local {
            path: path.clone(),
            kind: desktop_kind(*kind),
        },
    }
}

fn desktop_kind(kind: LocalTargetKind) -> jcode_desktop_open::LocalTargetKind {
    match kind {
        LocalTargetKind::File => jcode_desktop_open::LocalTargetKind::File,
        LocalTargetKind::Directory => jcode_desktop_open::LocalTargetKind::Directory,
    }
}

async fn perform_open(target: &ResolvedTarget) -> Result<OpenOutcome> {
    let raw = match target {
        ResolvedTarget::Url(url) => url.clone(),
        ResolvedTarget::Local { path, .. } => path.to_string_lossy().into_owned(),
    };
    let local = desktop_target(target);
    let backend = dispatch(CompanionRequest::Open { target: raw }, async move {
        jcode_desktop_open::open_target(&local).await
    })
    .await?;
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

    let local_path = path.clone();
    let local_kind = desktop_kind(*kind);
    let selection = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let selection_out = selection.clone();
    let backend = dispatch(
        CompanionRequest::Reveal { path: path.clone() },
        async move {
            let (backend, selected) =
                jcode_desktop_open::reveal_target(&local_path, local_kind).await?;
            selection_out.store(selected, std::sync::atomic::Ordering::Relaxed);
            Ok(backend)
        },
    )
    .await?;
    let selection_supported = selection.load(std::sync::atomic::Ordering::Relaxed);
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
            "Showed {} in the file manager via {}.",
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

#[cfg(test)]
#[path = "open_tests.rs"]
mod open_tests;
