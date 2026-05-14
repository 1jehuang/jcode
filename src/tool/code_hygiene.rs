use crate::tool::ToolContext;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

const POST_EDIT_HOOK_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) async fn run_post_edit_hygiene_for_paths(
    ctx: &ToolContext,
    paths: &[PathBuf],
) -> String {
    if std::env::var("JCODE_POST_EDIT_HOOKS")
        .ok()
        .is_some_and(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off"
            )
        })
    {
        return String::new();
    }

    let mut reports = Vec::new();
    for path in paths {
        if !path.is_file() || !looks_like_code_file(path) {
            continue;
        }
        if let Some(report) = run_post_edit_hygiene_for_path(ctx, path).await {
            reports.push(report);
        }
    }

    if reports.is_empty() {
        String::new()
    } else {
        format!("\n\nPost-edit hygiene:\n{}", reports.join("\n"))
    }
}

fn looks_like_code_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some(
            "rs" | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "mjs"
                | "cjs"
                | "py"
                | "go"
                | "json"
                | "css"
                | "scss"
                | "html"
                | "md"
                | "yaml"
                | "yml"
        )
    )
}

async fn run_post_edit_hygiene_for_path(ctx: &ToolContext, path: &Path) -> Option<String> {
    let cwd = ctx
        .working_dir
        .clone()
        .or_else(|| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let display = path
        .strip_prefix(&cwd)
        .unwrap_or(path)
        .display()
        .to_string();
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();

    let mut steps: Vec<(&str, Vec<String>)> = Vec::new();
    match ext {
        "rs" => {
            steps.push(("format", vec!["rustfmt".into(), path.display().to_string()]));
            if nearest_named_file(path, "Cargo.toml").is_some() {
                steps.push((
                    "typecheck",
                    vec!["cargo".into(), "check".into(), "-q".into()],
                ));
            }
        }
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "json" | "css" | "scss" | "html" | "md"
        | "yaml" | "yml" => {
            if nearest_named_file(path, "package.json").is_some() {
                steps.push((
                    "format",
                    vec![
                        "npx".into(),
                        "--yes".into(),
                        "prettier".into(),
                        "--write".into(),
                        path.display().to_string(),
                    ],
                ));
                steps.push((
                    "lint",
                    vec![
                        "npx".into(),
                        "--yes".into(),
                        "eslint".into(),
                        path.display().to_string(),
                    ],
                ));
            }
        }
        "py" => {
            steps.push((
                "format",
                vec![
                    "python3".into(),
                    "-m".into(),
                    "black".into(),
                    path.display().to_string(),
                ],
            ));
            steps.push((
                "lint",
                vec![
                    "python3".into(),
                    "-m".into(),
                    "ruff".into(),
                    "check".into(),
                    path.display().to_string(),
                ],
            ));
        }
        "go" => {
            steps.push((
                "format",
                vec!["gofmt".into(), "-w".into(), path.display().to_string()],
            ));
            if nearest_named_file(path, "go.mod").is_some() {
                steps.push((
                    "typecheck",
                    vec!["go".into(), "test".into(), "./...".into()],
                ));
            }
        }
        _ => {}
    }

    if steps.is_empty() {
        return None;
    }

    let mut outcomes = Vec::new();
    for (label, command) in steps {
        let outcome = run_command(&cwd, command).await;
        outcomes.push(format!("{} {}", label, outcome));
    }

    Some(format!("- `{}`: {}", display, outcomes.join("; ")))
}

async fn run_command(cwd: &Path, command: Vec<String>) -> String {
    let rendered = command.join(" ");
    let Some((program, args)) = command.split_first() else {
        return "skipped: empty command".to_string();
    };

    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match timeout(POST_EDIT_HOOK_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) if output.status.success() => format!("✓ `{}`", rendered),
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let details = first_nonempty_line(&stderr)
                .or_else(|| first_nonempty_line(&stdout))
                .unwrap_or("no output")
                .to_string();
            format!("✗ `{}` ({})", rendered, details)
        }
        Ok(Err(err)) => format!("skipped `{}` ({})", rendered, err),
        Err(_) => format!(
            "timed out `{}` after {}s",
            rendered,
            POST_EDIT_HOOK_TIMEOUT.as_secs()
        ),
    }
}

fn first_nonempty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

fn nearest_named_file(path: &Path, name: &str) -> Option<PathBuf> {
    for ancestor in path.ancestors() {
        let candidate = ancestor.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
