//! One-command TUI screenshot and Before/After comparison helpers for `selfdev`.

use crate::tool::debug_socket::execute_debug_command;
use crate::tool::{ToolContext, ToolOutput};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub(super) struct ScreenshotArgs {
    pub(super) size: Option<String>,
    pub(super) label: Option<String>,
    pub(super) crop_rows: Option<String>,
    pub(super) output: Option<String>,
    pub(super) sanitize: Option<bool>,
    pub(super) synthetic: Option<bool>,
}

pub(super) struct BeforeAfterArgs {
    pub(super) before: Option<String>,
    pub(super) after: Option<String>,
    pub(super) size: Option<String>,
    pub(super) label: Option<String>,
    pub(super) before_crop_rows: Option<String>,
    pub(super) after_crop_rows: Option<String>,
    pub(super) before_label: Option<String>,
    pub(super) after_label: Option<String>,
    pub(super) title: Option<String>,
    pub(super) output: Option<String>,
    pub(super) highlight_diff: Option<bool>,
    pub(super) auto_crop: Option<bool>,
    pub(super) sanitize: Option<bool>,
    pub(super) synthetic: Option<bool>,
    pub(super) attach_to_discord: Option<bool>,
}

fn scratch_ui_dir() -> PathBuf {
    if let Ok(scratch) = std::env::var("JCODE_SCRATCH_DIR")
        && !scratch.trim().is_empty()
    {
        return PathBuf::from(scratch).join("selfdev-ui");
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.trim().is_empty()
    {
        return PathBuf::from(home)
            .join(".jcode")
            .join("scratch")
            .join("selfdev-ui");
    }
    std::env::temp_dir().join("jcode-selfdev-ui")
}

fn sanitize_slug(raw: &str) -> String {
    let slug: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "capture".to_string()
    } else {
        trimmed.to_string()
    }
}

fn resolve_composer_script(repo_dir: Option<&Path>) -> Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = repo_dir {
        candidates.push(dir.join("scripts").join("compose_before_after.py"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("scripts").join("compose_before_after.py"));
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| anyhow::anyhow!("scripts/compose_before_after.py not found in repo"))
}

async fn capture_cells_to_file(
    session_id: &str,
    size: &str,
    synthetic: bool,
    dest: &Path,
) -> Result<()> {
    let cmd = if synthetic {
        format!("client:cells:synthetic:{}", size.trim())
    } else {
        format!("client:cells:{}", size.trim())
    };
    let json = execute_debug_command(&cmd, Some(session_id.to_string()), 15).await?;
    let trimmed = json.trim();
    if !trimmed.starts_with('{') {
        anyhow::bail!("unexpected response from `{cmd}`: {trimmed}");
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(dest, trimmed).await?;
    Ok(())
}

pub(super) async fn do_screenshot(
    args: ScreenshotArgs,
    repo_dir: Option<PathBuf>,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    let size = args.size.unwrap_or_else(|| "64x34".to_string());
    let stem = sanitize_slug(args.label.as_deref().unwrap_or("tui-capture"));
    let base_dir = scratch_ui_dir();
    let png_path = args
        .output
        .map(PathBuf::from)
        .unwrap_or_else(|| base_dir.join(format!("{stem}.png")));
    let cells_path = png_path.with_extension("json");

    let synthetic = args.synthetic.unwrap_or(true);
    capture_cells_to_file(&ctx.session_id, &size, synthetic, &cells_path).await?;

    let script = resolve_composer_script(repo_dir.as_deref())?;
    let mut cmd = tokio::process::Command::new("python3");
    cmd.arg(&script)
        .arg("--cells")
        .arg(&cells_path)
        .arg("--output")
        .arg(&png_path);
    if let Some(crop) = args.crop_rows.as_deref().filter(|s| !s.trim().is_empty()) {
        cmd.arg("--crop-rows").arg(crop.trim());
    }
    if args.sanitize.unwrap_or(true) {
        cmd.arg("--sanitize");
    }
    let output = cmd
        .output()
        .await
        .context("failed to run scripts/compose_before_after.py")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("compose_before_after.py failed: {}", stderr.trim());
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(ToolOutput::new(format!(
        "{stdout}\nCells JSON: {}\nPNG: {}\nTip: inspect the PNG with `read` and pass Cells JSON as `before` to `selfdev action=before-after` after your UI change.",
        cells_path.display(),
        png_path.display()
    )))
}

pub(super) async fn do_before_after(
    args: BeforeAfterArgs,
    repo_dir: Option<PathBuf>,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    let before_raw = args
        .before
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "`before` path is required for action=before-after (capture one first with `selfdev action=screenshot`)."
            )
        })?;
    let before_path = PathBuf::from(before_raw.trim());
    if !before_path.is_file() {
        anyhow::bail!("`before` file not found: {}", before_path.display());
    }

    let base_dir = scratch_ui_dir();
    let size = args.size.unwrap_or_else(|| "64x34".to_string());
    let after_path = match args.after.filter(|s| !s.trim().is_empty()) {
        Some(p) => PathBuf::from(p.trim()),
        None => {
            let live_after = base_dir.join("after-live.json");
            let synthetic = args.synthetic.unwrap_or(true);
            capture_cells_to_file(&ctx.session_id, &size, synthetic, &live_after).await?;
            live_after
        }
    };
    if !after_path.is_file() {
        anyhow::bail!("`after` file not found: {}", after_path.display());
    }

    let attach_to_discord = args.attach_to_discord.unwrap_or(false);
    let png_path = if let Some(explicit) = args.output {
        PathBuf::from(explicit)
    } else if attach_to_discord {
        let root = repo_dir
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .ok_or_else(|| {
                anyhow::anyhow!("cannot resolve repo directory for attach_to_discord")
            })?;
        let slug = sanitize_slug(
            args.label
                .as_deref()
                .or(args.title.as_deref())
                .unwrap_or("ui-diff"),
        );
        root.join("changelog")
            .join("discord")
            .join(format!("selfdev-{slug}.png"))
    } else {
        base_dir.join("before-after.png")
    };
    let script = resolve_composer_script(repo_dir.as_deref())?;

    let is_json = before_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        && after_path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));

    let mut cmd = tokio::process::Command::new("python3");
    cmd.arg(&script);
    if is_json {
        cmd.arg("--before-cells")
            .arg(&before_path)
            .arg("--after-cells")
            .arg(&after_path);
        if let Some(crop) = args
            .before_crop_rows
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            cmd.arg("--before-crop-rows").arg(crop.trim());
        }
        if let Some(crop) = args
            .after_crop_rows
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            cmd.arg("--after-crop-rows").arg(crop.trim());
        }
    } else {
        cmd.arg("--before")
            .arg(&before_path)
            .arg("--after")
            .arg(&after_path);
    }
    if let Some(label) = args
        .before_label
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        cmd.arg("--before-label").arg(label.trim());
    }
    if let Some(label) = args.after_label.as_deref().filter(|s| !s.trim().is_empty()) {
        cmd.arg("--after-label").arg(label.trim());
    }
    if let Some(title) = args.title.as_deref().filter(|s| !s.trim().is_empty()) {
        cmd.arg("--title").arg(title.trim());
    }
    if args.highlight_diff == Some(false) {
        cmd.arg("--no-highlight-diff");
    }
    if args.auto_crop == Some(true) {
        cmd.arg("--auto-crop");
    }
    if args.sanitize.unwrap_or(true) {
        cmd.arg("--sanitize");
    }
    cmd.arg("--output").arg(&png_path);

    let output = cmd
        .output()
        .await
        .context("failed to run scripts/compose_before_after.py")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("compose_before_after.py failed: {}", stderr.trim());
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let discord_note = if attach_to_discord {
        "\nDiscord auto-attach: saved under `changelog/discord/`. Commit & push it ONLY after visually verifying with `read` that zero private session content, emails, usernames, or secrets appear."
    } else {
        ""
    };
    Ok(ToolOutput::new(format!(
        "{stdout}\nPNG: {}\nPRIVACY & VISUAL CHECK REQUIRED: call `read` on `{}` right now and verify (1) zero real user prompts/transcripts, emails, local paths, or secrets are visible, and (2) the UI diff renders cleanly before sharing or committing.{discord_note}",
        png_path.display(),
        png_path.display()
    )))
}
