use crate::message::{ContentBlock, Message, Role, StreamEvent};
use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

const MAX_PROMPT_CHARS: usize = 120_000;

fn provider_login_hint(provider_name: &str) -> Option<&'static str> {
    match provider_name.to_ascii_lowercase().as_str() {
        "cursor" => Some("Run `jcode login --provider cursor` to re-authenticate."),
        "copilot" => Some(
            "Run `jcode login --provider copilot` to authenticate (or install Copilot CLI via https://gh.io/copilot-cli).",
        ),
        "antigravity" => Some("Run `jcode login --provider antigravity` to re-authenticate."),
        _ => None,
    }
}

fn looks_like_auth_error(stderr_text: &str) -> bool {
    let lower = stderr_text.to_ascii_lowercase();
    [
        "unauthorized",
        "forbidden",
        "auth",
        "token",
        "login",
        "log in",
        "sign in",
        "not installed",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub fn build_cli_prompt(system: &str, messages: &[Message]) -> String {
    let mut out = String::new();

    if !system.trim().is_empty() {
        out.push_str("System:\n");
        out.push_str(system.trim());
        out.push_str("\n\n");
    }

    out.push_str("Conversation:\n");

    for message in messages {
        let role = match message.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
        };
        out.push_str(role);
        out.push_str(":\n");

        for block in &message.content {
            match block {
                ContentBlock::Text { text, .. } => {
                    out.push_str(text);
                    out.push('\n');
                }
                ContentBlock::Reasoning { .. } => {}
                ContentBlock::ToolUse { name, input, .. } => {
                    out.push_str("[tool_use ");
                    out.push_str(name);
                    out.push_str(" input=");
                    out.push_str(&input.to_string());
                    out.push_str("]\n");
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    out.push_str("[tool_result ");
                    out.push_str(tool_use_id);
                    out.push_str(" is_error=");
                    out.push_str(if is_error.unwrap_or(false) {
                        "true"
                    } else {
                        "false"
                    });
                    out.push_str("]\n");
                    out.push_str(content);
                    out.push('\n');
                }
                ContentBlock::Image { .. } => {
                    out.push_str("[image]\n");
                }
            }
        }
        out.push('\n');
    }

    out.push_str("Assistant:\n");

    if out.chars().count() <= MAX_PROMPT_CHARS {
        return out;
    }

    let mut kept = out.chars().rev().take(MAX_PROMPT_CHARS).collect::<Vec<_>>();
    kept.reverse();
    let tail: String = kept.into_iter().collect();
    format!(
        "[Earlier conversation truncated to fit CLI prompt limits]\n\n{}",
        tail
    )
}

pub async fn run_cli_text_command(
    mut cmd: Command,
    tx: mpsc::Sender<Result<StreamEvent>>,
    provider_name: &str,
) -> Result<()> {
    cmd.kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                if let Some(hint) = provider_login_hint(provider_name) {
                    anyhow::bail!("Failed to spawn {} CLI: {}. {}", provider_name, e, hint);
                }
            }
            return Err(e).with_context(|| format!("Failed to spawn {} CLI", provider_name));
        }
    };

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture {} stdout", provider_name))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture {} stderr", provider_name))?;

    let tx_stdout = tx.clone();
    let provider_for_log = provider_name.to_string();
    let stdout_task = tokio::spawn(async move {
        let mut saw_text = false;
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if line.is_empty() {
                continue;
            }
            saw_text = true;
            if tx_stdout
                .send(Ok(StreamEvent::TextDelta(format!("{}\n", line))))
                .await
                .is_err()
            {
                break;
            }
        }
        saw_text
    });

    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        let mut collected = String::new();
        while let Ok(Some(line)) = reader.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            crate::logging::debug(&format!("[{}-cli] {}", provider_for_log, line));
            if !collected.is_empty() {
                collected.push('\n');
            }
            collected.push_str(&line);
        }
        collected
    });

    let status = child.wait().await?;
    let saw_text = stdout_task.await.unwrap_or(false);
    let stderr_text = stderr_task.await.unwrap_or_default();

    if !status.success() {
        let hint = if looks_like_auth_error(&stderr_text) {
            provider_login_hint(provider_name)
        } else {
            None
        };

        if !stderr_text.trim().is_empty() {
            if let Some(hint) = hint {
                anyhow::bail!(
                    "{} CLI exited with status {}: {}\n{}",
                    provider_name,
                    status,
                    stderr_text.trim(),
                    hint
                );
            }
            anyhow::bail!(
                "{} CLI exited with status {}: {}",
                provider_name,
                status,
                stderr_text.trim()
            );
        }

        if let Some(hint) = hint {
            anyhow::bail!(
                "{} CLI exited with status {}. {}",
                provider_name,
                status,
                hint
            );
        }
        anyhow::bail!("{} CLI exited with status {}", provider_name, status);
    }

    if !saw_text {
        if !stderr_text.trim().is_empty() {
            anyhow::bail!(
                "{} CLI produced no output: {}",
                provider_name,
                stderr_text.trim()
            );
        }
        anyhow::bail!("{} CLI produced no output", provider_name);
    }

    let _ = tx
        .send(Ok(StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        }))
        .await;
    Ok(())
}
