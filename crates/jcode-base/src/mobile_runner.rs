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
#[path = "mobile_runner_events.rs"]
mod events;
#[cfg(test)]
#[path = "mobile_runner_poll_tests.rs"]
mod poll_tests;
#[path = "mobile_runner_process.rs"]
mod process;
#[cfg(test)]
#[path = "mobile_runner_process_tests.rs"]
mod process_tests;

struct StageTimer {
    stage: &'static str,
    outcome: &'static str,
    start: std::time::Instant,
}
impl StageTimer {
    fn new(stage: &'static str) -> Self {
        Self {
            stage,
            outcome: "error_or_cancelled",
            start: std::time::Instant::now(),
        }
    }

    fn record(&self, duration_ms: u128) -> String {
        format!(
            "[CLOUD_TIMING] operation=mobile_runner stage={} outcome={} duration_ms={}",
            self.stage, self.outcome, duration_ms
        )
    }
}
impl Drop for StageTimer {
    fn drop(&mut self) {
        crate::logging::info(&self.record(self.start.elapsed().as_millis()));
    }
}

async fn timed_stage<T>(
    stage: &'static str,
    future: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    let mut timing = StageTimer::new(stage);
    let result = future.await;
    timing.outcome = if result.is_ok() { "ok" } else { "error" };
    result
}

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const TURN_TIMEOUT: Duration = Duration::from_secs(40 * 60);
const BUSY_TOUCH_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize)]
pub struct MobileCommand {
    pub command_id: String,
    pub kind: String,
    pub session_id: Option<String>,
    pub text: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub claimed_at: Option<String>,
}

impl MobileCommand {
    // Both timestamps originate at the queue API, avoiding client clock skew.
    // This includes wake/backlog/poll delay, not claim response transport time.
    fn queue_wait_ms(&self) -> Option<u64> {
        let (Ok(created), Ok(claimed)) = (
            chrono::DateTime::parse_from_rfc3339(self.created_at.as_deref()?),
            chrono::DateTime::parse_from_rfc3339(self.claimed_at.as_deref()?),
        ) else {
            return None;
        };
        if claimed < created {
            return None;
        }
        Some(claimed.signed_duration_since(created).num_milliseconds() as u64)
    }
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
    timed_stage("transfer", async {
        let (metadata, messages) = fetch_local_transcript(client, base, key, id).await?;
        let mut session = transfer_session(&metadata, messages, &workspace_dir())?;
        session.save()?;
        Ok(session.id)
    })
    .await
}

/// True on a managed Jcode Cloud machine.
pub fn is_cloud_machine() -> bool {
    mobile_sync::session_source() == "cloud"
}

/// Keep the cloud host's idle-poweroff guard from stopping the VM while a
/// daemon session is mid-turn without an SSH client attached. The host's
/// `idle.sh` treats a marker touched within 5 minutes as busy. Only touches,
/// never removes: staleness releases it, and the mobile runner owns removal.
pub fn touch_cloud_activity_marker() {
    if !is_cloud_machine() {
        return;
    }
    if let Some(path) = busy_marker()
        && let Err(error) = std::fs::write(&path, b"")
    {
        crate::logging::warn(&format!("cloud activity marker update failed: {error}"));
    }
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
    timed_stage(
        "turn",
        process::run(
            cmd,
            session_id,
            busy_marker(),
            TURN_TIMEOUT,
            BUSY_TOUCH_INTERVAL,
        ),
    )
    .await
}

/// Claim and run at most one queued command. Returns whether one ran.
pub async fn run_one(client: &reqwest::Client) -> Result<bool> {
    let key = crate::subscription_catalog::configured_api_key()
        .context("no Jcode account credential is configured")?;
    let base = subscription_api::configured_api_base()
        .trim_end_matches('/')
        .to_string();
    let claimed = timed_stage("claim", claim(client, &base, &key)).await?;
    let Some(command) = claimed else {
        return Ok(false);
    };
    if let Some(duration_ms) = command.queue_wait_ms() {
        crate::logging::info(&format!(
            "[CLOUD_TIMING] operation=mobile_runner stage=queue_wait outcome=ok duration_ms={duration_ms}"
        ));
    }
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
    let sync_result = timed_stage("sync", mobile_sync::sync_once(client)).await;
    if let Err(error) = sync_result {
        crate::logging::warn(&format!("mobile runner transcript sync failed: {error:#}"));
    }
    timed_stage(
        "complete",
        complete(client, &base, &key, &command.command_id, &outcome),
    )
    .await?;
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

trait PollWait {
    fn wait(&mut self, delay: Duration) -> impl std::future::Future<Output = ()> + Send;
}

impl PollWait for events::Wakeups {
    async fn wait(&mut self, delay: Duration) {
        events::Wakeups::wait(self, delay).await;
    }
}

#[cfg(test)]
struct SleepWait;

#[cfg(test)]
impl PollWait for SleepWait {
    async fn wait(&mut self, delay: Duration) {
        tokio::time::sleep(delay).await;
    }
}

#[cfg(test)]
async fn poll_commands<F, Fut>(run: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    poll_commands_with_wait(run, SleepWait).await;
}

async fn poll_commands_with_wait<F, Fut, W>(mut run: F, mut wait: W)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
    W: PollWait,
{
    loop {
        let delay = match run().await {
            // A completed command is evidence of activity. Drain existing work
            // without another five-second gap, then return to normal idle load.
            Ok(true) => continue,
            Ok(false) => POLL_INTERVAL,
            Err(error) => {
                crate::logging::warn(&format!("mobile runner: {error:#}"));
                Duration::from_secs(20)
            }
        };
        let mut timing = StageTimer::new("pollwait");
        wait.wait(delay).await;
        timing.outcome = "ok";
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
        poll_commands_with_wait(|| run_one(&client), events::spawn()).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_wait_uses_server_timestamps_and_accepts_legacy_commands() {
        let legacy = serde_json::json!({
            "command_id": "fixture", "kind": "create", "text": "test"
        });
        let command: MobileCommand = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(command.queue_wait_ms(), None);
        for (created, claimed, expected) in [
            (
                "2026-11-08T09:00:00Z",
                "2026-11-08T09:00:04.750Z",
                Some(4750),
            ),
            ("2026-11-08T01:00:00-08:00", "2026-11-08T09:00:00Z", Some(0)),
            ("2026-11-08T09:00:00.0005Z", "2026-11-08T09:00:00Z", None),
            ("invalid", "2026-11-08T09:00:00Z", None),
            ("2026-11-08T09:00:00Z", "invalid", None),
        ] {
            let mut wire = legacy.clone();
            wire["created_at"] = created.into();
            wire["claimed_at"] = claimed.into();
            let command: MobileCommand = serde_json::from_value(wire).unwrap();
            assert_eq!(command.queue_wait_ms(), expected);
        }
    }

    #[test]
    fn stage_timing_uses_content_free_shared_report_fields() {
        for stage in ["claim", "transfer", "turn", "sync", "complete", "pollwait"] {
            let mut timing = StageTimer::new(stage);
            assert_eq!(
                timing.record(17),
                format!(
                    "[CLOUD_TIMING] operation=mobile_runner stage={stage} outcome=error_or_cancelled duration_ms=17"
                )
            );
            for outcome in ["ok", "error"] {
                timing.outcome = outcome;
                let record = timing.record(23);
                assert_eq!(
                    record,
                    format!(
                        "[CLOUD_TIMING] operation=mobile_runner stage={stage} outcome={outcome} duration_ms=23"
                    )
                );
                // These are exactly the fixed-label fields consumed by cloud_latency_report.
                assert_eq!(record.split_whitespace().count(), 5);
                assert!(!record.contains("elapsed_ms="));
            }
        }
    }

    #[tokio::test]
    async fn timed_stage_preserves_success_and_error_results() {
        assert_eq!(timed_stage("claim", async { Ok(7) }).await.unwrap(), 7);
        let error = timed_stage::<()>("claim", async { anyhow::bail!("synthetic failure") })
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "synthetic failure");
    }

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
    async fn claim_preserves_server_queue_age_without_logging_command_content() {
        let (base, server) = transcript_server(vec![serde_json::json!({
            "command": {"command_id":"fixture", "kind":"create", "text":"private fixture",
                "created_at":"2026-11-08T09:00:00Z", "claimed_at":"2026-11-08T09:00:04.750Z"}
        })])
        .await;
        let command = claim(&reqwest::Client::new(), &base, "fixture-key")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(command.queue_wait_ms(), Some(4750));
        assert_eq!(command.text, "private fixture");
        let requests = server.await.unwrap();
        assert!(requests[0].starts_with("POST /mobile/commands/claim "));
        assert!(
            requests[0]
                .to_ascii_lowercase()
                .contains("authorization: bearer fixture-key\r\n")
        );
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
