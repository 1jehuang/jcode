//! Runs phone-issued commands on a Jcode Cloud machine.
//!
//! The iOS app queues `create` (start a new session) and `message` (continue
//! a cloud session) commands at `/v1/mobile/commands`; the Worker wakes the
//! machine. The cloud daemon claims them here, runs each as a headless
//! `jcode run` turn (resuming the session for follow-ups) and reports the
//! session id back. The transcript reaches the phone through mobile sync.

use crate::mobile_sync;
use crate::subscription_api;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const TURN_TIMEOUT: Duration = Duration::from_secs(40 * 60);
const BUSY_TOUCH_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize)]
pub struct MobileCommand {
    pub command_id: String,
    pub kind: String,
    pub session_id: Option<String>,
    pub text: String,
}

#[derive(Deserialize)]
struct ClaimResponse {
    command: Option<MobileCommand>,
}

/// True on a managed Jcode Cloud machine.
pub fn is_cloud_machine() -> bool {
    mobile_sync::session_source() == "cloud"
}

fn busy_marker() -> Option<PathBuf> {
    match crate::storage::jcode_dir() {
        Ok(directory) => Some(directory.join("mobile-runner-busy")),
        Err(error) => {
            crate::logging::warn(&format!("mobile runner busy marker unavailable: {error:#}"));
            None
        }
    }
}

fn workspace_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|h| h.join("workspaces"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")))
}

/// `--provider`/`--model` flags from this process's command line.
pub fn inherited_provider_args(args: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut iter = args.into_iter().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--provider" | "-p" | "--model" | "-m" => {
                if let Some(value) = iter.next() {
                    out.push(arg);
                    out.push(value);
                }
            }
            _ if arg.starts_with("--provider=") || arg.starts_with("--model=") => out.push(arg),
            _ => {}
        }
    }
    out
}

/// Extract the session id from `jcode run --ndjson` output.
pub fn session_id_from_ndjson_line(line: &str) -> Option<String> {
    // Non-JSON stdout is not a session-start event.
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return None;
    };
    value
        .get("session_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

async fn claim(client: &reqwest::Client, base: &str, key: &str) -> Result<Option<MobileCommand>> {
    let response = client
        .post(format!("{base}/mobile/commands/claim"))
        .bearer_auth(key)
        .json(&serde_json::json!({ "machine": std::env::var_os("HOSTNAME").map_or_else(String::new, |name| name.to_string_lossy().into_owned()) }))
        .timeout(Duration::from_secs(30))
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("claim returned {}", response.status());
    }
    Ok(response.json::<ClaimResponse>().await?.command)
}

async fn complete(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    id: &str,
    outcome: &Result<String>,
) -> Result<()> {
    let body = match outcome {
        Ok(session) => serde_json::json!({ "status": "done", "session_id": session }),
        Err(err) => serde_json::json!({ "status": "failed", "error": format!("{err:#}") }),
    };
    client
        .post(format!("{base}/mobile/commands/{id}/complete"))
        .bearer_auth(key)
        .json(&body)
        .timeout(Duration::from_secs(30))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

async fn run_turn(command: &MobileCommand) -> Result<String> {
    let exe = std::env::current_exe().context("locating jcode executable")?;
    let mut cmd = tokio::process::Command::new(exe);
    // The cloud daemon runs as `jcode --provider jcode serve` until the user
    // brings a model login; turns must use the same provider.
    cmd.args(inherited_provider_args(std::env::args()));
    if command.kind == "message" {
        let id = command
            .session_id
            .as_deref()
            .context("message without session")?;
        cmd.arg("--resume").arg(id);
    }
    cmd.arg("run").arg("--ndjson").arg(&command.text);
    cmd.current_dir(workspace_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().context("starting jcode run")?;
    let stdout = child.stdout.take().context("no stdout")?;
    let mut lines = BufReader::new(stdout).lines();
    let mut session = command.session_id.clone();
    let marker = busy_marker();
    let mut touch = tokio::time::interval(BUSY_TOUCH_INTERVAL);
    let deadline = tokio::time::sleep(TURN_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            line = lines.next_line() => match line? {
                Some(line) => {
                    if session.is_none() {
                        session = session_id_from_ndjson_line(&line);
                    }
                }
                None => break,
            },
            _ = touch.tick() => {
                if let Some(m) = &marker
                    && let Err(error) = std::fs::write(m, b"")
                {
                    crate::logging::warn(&format!("mobile runner busy marker update failed: {error}"));
                }
            }
            _ = &mut deadline => {
                if let Err(error) = child.kill().await {
                    crate::logging::warn(&format!("mobile runner timed-out child cleanup failed: {error}"));
                }
                anyhow::bail!("turn timed out");
            }
        }
    }
    let output = child.wait_with_output().await?;
    if let Some(m) = &marker
        && let Err(error) = std::fs::remove_file(m)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        crate::logging::warn(&format!(
            "mobile runner busy marker removal failed: {error}"
        ));
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .chars()
            .rev()
            .take(600)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        anyhow::bail!("jcode run failed ({}): {}", output.status, tail.trim());
    }
    session.context("jcode run did not report a session id")
}

/// Claim and run at most one queued command. Returns whether one ran.
pub async fn run_one(client: &reqwest::Client) -> Result<bool> {
    let key = crate::subscription_catalog::configured_api_key()
        .context("no Jcode account credential is configured")?;
    let base = subscription_api::configured_api_base()
        .trim_end_matches('/')
        .to_string();
    let Some(command) = claim(client, &base, &key).await? else {
        return Ok(false);
    };
    crate::logging::info(&format!(
        "mobile runner: {} command {}",
        command.kind, command.command_id
    ));
    let outcome = run_turn(&command).await;
    if let Err(error) = mobile_sync::sync_once(client).await {
        crate::logging::warn(&format!("mobile runner transcript sync failed: {error:#}"));
    }
    complete(client, &base, &key, &command.command_id, &outcome).await?;
    outcome.map(|_| true)
}

/// Turn mobile sync on so cloud sessions reach the phone.
pub fn ensure_sync_enabled() {
    let mut state = mobile_sync::load_state();
    if !state.enabled {
        state.enabled = true;
        if let Err(error) = mobile_sync::save_state(&state) {
            crate::logging::warn(&format!("mobile runner could not enable sync: {error:#}"));
        }
    }
}

/// Background loop for the daemon on a Jcode Cloud machine.
pub fn spawn_cloud_runner() {
    if !is_cloud_machine() {
        return;
    }
    ensure_sync_enabled();
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            if let Err(err) = run_one(&client).await {
                crate::logging::warn(&format!("mobile runner: {err:#}"));
                tokio::time::sleep(Duration::from_secs(20)).await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_session_from_start_event() {
        assert_eq!(
            session_id_from_ndjson_line(r#"{"type":"start","session_id":"session_abc"}"#)
                .as_deref(),
            Some("session_abc")
        );
        assert_eq!(session_id_from_ndjson_line("not json"), None);
        assert_eq!(session_id_from_ndjson_line(r#"{"type":"text"}"#), None);
    }

    #[test]
    fn forwards_provider_and_model_only() {
        let args = ["jcode", "--provider", "jcode", "--model=x", "serve", "-q"].map(String::from);
        assert_eq!(
            inherited_provider_args(args),
            vec!["--provider", "jcode", "--model=x"]
        );
    }
}
