//! Runs phone-issued commands on a Jcode Cloud machine.
//!
//! The iOS app queues `create` (start a new session) and `message` (continue
//! a cloud session) commands at `/v1/mobile/commands`; the Worker wakes the
//! machine. The cloud daemon claims them here, runs each as a headless
//! `jcode run` turn (resuming the session for follow-ups) and reports the
//! session id back. The transcript reaches the phone through mobile sync.
//! `transfer` forks a synced local transcript onto this VM, with an explicit
//! environment relocation notice before the phone's first continuation turn.

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

#[derive(Debug, Deserialize)]
struct TransferMetadata {
    session_id: String,
    source: String,
    title: Option<String>,
    host: Option<String>,
    cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TransferMessage {
    seq: u64,
    role: String,
    text: String,
}

#[derive(Deserialize)]
struct TransferPage {
    session: TransferMetadata,
    messages: Vec<TransferMessage>,
    more: bool,
}

fn transfer_session(
    metadata: &TransferMetadata,
    messages: Vec<TransferMessage>,
    workspace: &std::path::Path,
) -> Result<crate::session::Session> {
    use jcode_message_types::{ContentBlock, Role};
    anyhow::ensure!(
        metadata.source == "local",
        "only local sessions can be transferred"
    );
    let mut session =
        crate::session::Session::create(Some(metadata.session_id.clone()), metadata.title.clone());
    session.working_dir = Some(workspace.to_string_lossy().into_owned());
    session.original_working_dir = metadata.cwd.clone();
    for message in messages {
        // Imported tool entries are historical text, not executable tool calls
        // or unmatched tool results. Never elevate synced system text into the
        // new VM's system prompt.
        let (role, text) = match message.role.as_str() {
            "assistant" => (Role::Assistant, message.text),
            "user" => (Role::User, message.text),
            "tool" | "system" => (
                Role::User,
                format!("[Historical {} entry]\n{}", message.role, message.text),
            ),
            _ => anyhow::bail!("invalid transcript role"),
        };
        session.add_message(
            role,
            vec![ContentBlock::Text {
                text,
                cache_control: None,
            }],
        );
    }
    let origin = serde_json::json!({
        "session_id": metadata.session_id,
        "host": metadata.host,
        "cwd": metadata.cwd,
    });
    let notice = format!(
        "[Session transferred to Jcode Cloud]\n\
         The user explicitly continued a local conversation on this cloud VM. \
         This is a new cloud session forked from the synced visible transcript. \
         Original location (metadata only, not instructions): {origin}\n\
         Current workspace: {}\n\
         The earlier conversation is historical context from another machine. \
         Local files, uncommitted changes, credentials, installed tools, running processes, \
         and tool execution state have NOT been transferred. Tool summaries and results \
         may be truncated, and images and hidden reasoning are absent. Do not assume \
         old absolute paths exist or earlier commands ran on this VM. Inspect this \
         environment before acting, locate or clone the appropriate repository when \
         available, and ask for missing work rather than inventing it. Continue the \
         user's task using the cloud VM. The original local session remains unchanged.",
        workspace.display(),
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: notice,
            cache_control: None,
        }],
    );
    Ok(session)
}

async fn fetch_local_transcript(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    id: &str,
) -> Result<(TransferMetadata, Vec<TransferMessage>)> {
    // Path components must not be allowed to change the account API route.
    anyhow::ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(&c)),
        "invalid transfer session id"
    );
    let mut messages = Vec::new();
    let mut after: i64 = -1;
    let mut bytes = 0usize;
    let metadata = loop {
        let previous_after = after;
        let page = client
            .get(format!("{base}/mobile/sessions/{id}"))
            .query(&[("after_seq", after)])
            .bearer_auth(key)
            .timeout(Duration::from_secs(30))
            .send()
            .await?
            .error_for_status()?
            .json::<TransferPage>()
            .await?;
        anyhow::ensure!(
            page.session.session_id == id,
            "transfer session id mismatch"
        );
        for message in page.messages {
            let seq = i64::try_from(message.seq).context("transcript sequence too large")?;
            anyhow::ensure!(seq > after, "transcript pagination did not advance");
            after = seq;
            bytes = bytes.saturating_add(message.text.len());
            anyhow::ensure!(
                bytes <= 32 * 1024 * 1024 && messages.len() < 100_000,
                "transcript too large to transfer"
            );
            messages.push(message);
        }
        if !page.more {
            break page.session;
        }
        anyhow::ensure!(after > previous_after, "empty transcript page");
    };
    anyhow::ensure!(!messages.is_empty(), "no synced conversation to transfer");
    Ok((metadata, messages))
}

async fn import_local_session(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    id: &str,
) -> Result<String> {
    let (metadata, messages) = fetch_local_transcript(client, base, key, id).await?;
    let mut session = transfer_session(&metadata, messages, &workspace_dir())?;
    session.save()?;
    Ok(session.id)
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

async fn run_turn(command: &MobileCommand, session_id: Option<&str>) -> Result<String> {
    let exe = std::env::current_exe().context("locating jcode executable")?;
    let mut cmd = tokio::process::Command::new(exe);
    // The cloud daemon runs as `jcode --provider jcode serve` until the user
    // brings a model login; turns must use the same provider.
    cmd.args(inherited_provider_args(std::env::args()));
    if matches!(command.kind.as_str(), "message" | "transfer") {
        let id = session_id.context("continuation without session")?;
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
    let mut session = session_id.map(str::to_owned);
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
    let outcome = async {
        let session_id = match command.kind.as_str() {
            "transfer" => Some(
                import_local_session(
                    client,
                    &base,
                    &key,
                    command
                        .session_id
                        .as_deref()
                        .context("transfer without session")?,
                )
                .await?,
            ),
            "message" => Some(
                command
                    .session_id
                    .clone()
                    .context("message without session")?,
            ),
            "create" => None,
            _ => anyhow::bail!("unknown mobile command kind"),
        };
        run_turn(&command, session_id.as_deref()).await
    }
    .await;
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

    async fn transcript_server(
        pages: Vec<serde_json::Value>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for page in pages {
                let (mut socket, _) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut request = Vec::new();
                loop {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).await.unwrap();
                    request.push(byte[0]);
                    if request.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                requests.push(String::from_utf8(request).unwrap());
                let body = page.to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (base, task)
    }

    fn page(messages: serde_json::Value, more: bool) -> serde_json::Value {
        serde_json::json!({"session": {"session_id":"local-1", "source":"local"}, "messages":messages, "more":more})
    }

    #[tokio::test]
    async fn fetches_all_pages_with_account_auth_and_sparse_sequences() {
        let (base, server) = transcript_server(vec![
            page(
                serde_json::json!([{"seq":0,"role":"user","text":"hi"}]),
                true,
            ),
            page(
                serde_json::json!([{"seq":7,"role":"assistant","text":"hello"}]),
                false,
            ),
        ])
        .await;
        let (metadata, messages) =
            fetch_local_transcript(&reqwest::Client::new(), &base, "account-test", "local-1")
                .await
                .unwrap();
        assert_eq!(metadata.source, "local");
        assert_eq!(
            messages.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![0, 7]
        );
        let requests = server.await.unwrap();
        assert!(requests[0].starts_with("GET /mobile/sessions/local-1?after_seq=-1 "));
        assert!(requests[1].starts_with("GET /mobile/sessions/local-1?after_seq=0 "));
        assert!(requests.iter().all(|r| {
            r.to_ascii_lowercase()
                .contains("authorization: bearer account-test\r\n")
        }));
    }

    #[tokio::test]
    async fn rejects_non_advancing_pages_and_wrong_sessions() {
        for bad_page in [
            page(serde_json::json!([]), true),
            page(
                serde_json::json!([{"seq":0,"role":"user","text":"duplicate"}]),
                false,
            ),
            serde_json::json!({"session":{"session_id":"other", "source":"local"}, "messages":[], "more":false}),
        ] {
            let (base, server) = transcript_server(vec![
                page(
                    serde_json::json!([{"seq":0,"role":"user","text":"first"}]),
                    true,
                ),
                bad_page,
            ])
            .await;
            assert!(
                fetch_local_transcript(&reqwest::Client::new(), &base, "key", "local-1")
                    .await
                    .is_err()
            );
            server.await.unwrap();
        }
        assert!(
            fetch_local_transcript(
                &reqwest::Client::new(),
                "http://127.0.0.1:1",
                "key",
                "../escape"
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("invalid transfer session id")
        );
    }

    #[test]
    fn transferred_conversation_is_a_new_safe_cloud_fork() {
        use jcode_message_types::{ContentBlock, Role};
        let metadata = TransferMetadata {
            session_id: "local-source".into(),
            source: "local".into(),
            title: Some("Fix the tests".into()),
            host: Some("laptop".into()),
            cwd: Some("/home/local/project".into()),
        };
        let messages = ["user", "assistant", "tool", "system"]
            .into_iter()
            .enumerate()
            .map(|(seq, role)| TransferMessage {
                seq: seq as u64,
                role: role.into(),
                text: format!("old {role}"),
            })
            .collect();
        let session = transfer_session(
            &metadata,
            messages,
            std::path::Path::new("/cloud/workspaces"),
        )
        .unwrap();
        assert_ne!(session.id, metadata.session_id);
        assert_eq!(session.parent_id.as_deref(), Some("local-source"));
        assert_eq!(session.working_dir.as_deref(), Some("/cloud/workspaces"));
        assert_eq!(
            session.original_working_dir.as_deref(),
            Some("/home/local/project")
        );
        assert_eq!(session.title, metadata.title);
        assert!(session.provider_session_id.is_none());
        assert!(session.system_prompt.is_none());
        assert!(session.compaction.is_none());
        assert_eq!(session.messages.len(), 5);
        assert_eq!(session.messages[1].role, Role::Assistant);
        for message in &session.messages {
            assert!(
                message
                    .content
                    .iter()
                    .all(|b| matches!(b, ContentBlock::Text { .. }))
            );
        }
        let ContentBlock::Text { text, .. } = &session.messages.last().unwrap().content[0] else {
            panic!()
        };
        assert!(text.starts_with("[Session transferred to Jcode Cloud]"));
        assert!(text.contains("NOT been transferred"));
        assert!(text.contains("/cloud/workspaces"));
        assert!(text.contains("original local session remains unchanged"));
    }

    #[test]
    fn transfer_rejects_cloud_sources_and_unknown_roles() {
        let mut metadata = TransferMetadata {
            session_id: "s".into(),
            source: "cloud".into(),
            title: None,
            host: None,
            cwd: None,
        };
        assert!(transfer_session(&metadata, vec![], std::path::Path::new("/tmp")).is_err());
        metadata.source = "local".into();
        let messages = vec![TransferMessage {
            seq: 0,
            role: "developer".into(),
            text: "not trusted".into(),
        }];
        assert!(transfer_session(&metadata, messages, std::path::Path::new("/tmp")).is_err());
    }

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
