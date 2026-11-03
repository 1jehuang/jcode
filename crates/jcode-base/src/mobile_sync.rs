//! Mobile transcript sync.
//!
//! When the user turns mobile sync on (`jcode mobile enable`), jcode uploads
//! session transcripts to the Jcode account API so the iOS app can show them.
//! Local sessions and sessions running on Jcode Cloud machines both upload with
//! the account API key. Uploads are incremental: per-session high-water marks
//! (message count + snapshot mtime) live in `~/.jcode/mobile_sync.json`.
//!
//! Only user-visible text is uploaded: user/assistant text, tool calls as a
//! one-line summary, and tool results truncated. Hidden reasoning, images and
//! provider-native blobs are never sent.

use crate::session::Session;
use crate::storage;
use crate::subscription_api;
use anyhow::{Context, Result};
use jcode_message_types::{ContentBlock, Role};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const SYNC_INTERVAL: Duration = Duration::from_secs(20);
const MAX_MESSAGES_PER_UPLOAD: usize = 200;
const MAX_TOOL_RESULT_CHARS: usize = 4_000;
const MAX_TEXT_CHARS: usize = 100_000;
/// Only sessions touched within this window are synced on enable.
const BACKFILL_WINDOW: Duration = Duration::from_secs(14 * 24 * 3600);
const MAX_SESSIONS_PER_PASS: usize = 200;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MobileSyncState {
    #[serde(default)]
    pub enabled: bool,
    /// session id -> progress
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionProgress>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionProgress {
    /// Number of transcript entries already uploaded.
    pub uploaded: usize,
    /// Snapshot+journal mtime (unix ms) at the last successful pass.
    pub mtime_ms: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WireMessage {
    pub seq: usize,
    pub role: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct UploadBody<'a> {
    title: Option<&'a str>,
    source: &'static str,
    host: Option<String>,
    cwd: Option<&'a str>,
    model: Option<&'a str>,
    created_at: String,
    updated_at: String,
    messages: &'a [WireMessage],
}

fn state_path() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("mobile_sync.json"))
}

pub fn load_state() -> MobileSyncState {
    let state = (|| -> Result<MobileSyncState> {
        let path = state_path()?;
        if !path.exists() {
            return Ok(MobileSyncState::default());
        }
        storage::read_json(&path)
    })();
    match state {
        Ok(state) => state,
        Err(error) => {
            crate::logging::warn(&format!("Could not load mobile sync state: {error:#}"));
            MobileSyncState::default()
        }
    }
}

pub fn save_state(state: &MobileSyncState) -> Result<()> {
    storage::write_json_secret(&state_path()?, state)
}

pub fn is_enabled() -> bool {
    load_state().enabled
}

/// `cloud` when running on a Jcode Cloud machine, `local` otherwise.
pub fn session_source() -> &'static str {
    if std::env::var_os("JCODE_CLOUD_ENV_SESSION").is_some()
        || std::env::var_os("JCODE_CLOUD_MACHINE").is_some()
        || std::env::var("JCODE_SERVER_NAME").as_deref() == Ok("jcode-cloud")
        || Path::new("/opt/jcode-cloud/ready").exists()
    {
        "cloud"
    } else {
        "local"
    }
}

fn host_name() -> Option<String> {
    std::env::var_os("HOSTNAME")
        .map(|value| value.to_string_lossy().into_owned())
        .or_else(|| match std::fs::read_to_string("/etc/hostname") {
            Ok(value) => Some(value),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                crate::logging::debug(&format!("Mobile hostname unavailable: {error}"));
                None
            }
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("\n… (truncated)");
    out
}

fn tool_summary(name: &str, input: &serde_json::Value) -> String {
    let detail = input
        .get("intent")
        .or_else(|| input.get("command"))
        .or_else(|| input.get("file_path"))
        .or_else(|| input.get("query"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if detail.is_empty() {
        format!("🔧 {name}")
    } else {
        format!("🔧 {name}: {}", truncate(detail, 200))
    }
}

/// Convert stored messages into the mobile wire format. One stored message
/// becomes at most one wire message; `seq` is the stored index so uploads are
/// stable and idempotent.
pub fn wire_messages(session: &Session, start: usize) -> Vec<WireMessage> {
    session
        .messages
        .iter()
        .enumerate()
        .skip(start)
        .filter_map(|(seq, message)| {
            let mut parts: Vec<String> = Vec::new();
            let mut is_tool_result = false;
            for block in &message.content {
                match block {
                    ContentBlock::Text { text, .. }
                        if !text.trim().is_empty()
                            && !text.trim_start().starts_with("<system-reminder>") =>
                    {
                        parts.push(text.clone())
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        parts.push(tool_summary(name, input))
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        is_tool_result = true;
                        parts.push(truncate(content, MAX_TOOL_RESULT_CHARS));
                    }
                    _ => {}
                }
            }
            if parts.is_empty() {
                return None;
            }
            let role = if is_tool_result {
                "tool"
            } else if message.display_role.is_some() {
                "system"
            } else {
                match message.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                }
            };
            Some(WireMessage {
                seq,
                role,
                text: truncate(&parts.join("\n\n"), MAX_TEXT_CHARS),
                created_at: message.timestamp.map(|t| t.to_rfc3339()),
            })
        })
        .collect()
}

fn mtime_ms(path: &Path) -> u64 {
    match std::fs::metadata(path).and_then(|metadata| metadata.modified()) {
        Ok(modified) => match modified.duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_millis() as u64,
            Err(error) => {
                crate::logging::debug(&format!("Mobile snapshot predates epoch: {error}"));
                0
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => {
            crate::logging::debug(&format!("Mobile snapshot mtime unavailable: {error}"));
            0
        }
    }
}

fn session_mtime_ms(snapshot: &Path) -> u64 {
    mtime_ms(snapshot).max(mtime_ms(
        &crate::session::session_journal_path_from_snapshot(snapshot),
    ))
}

/// Candidate sessions: real (non-debug) snapshots modified since the window.
fn candidate_sessions(since_ms: u64) -> Result<Vec<(String, PathBuf, u64)>> {
    let dir = storage::jcode_dir()?.join("sessions");
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(out);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        let mtime = session_mtime_ms(&path);
        if mtime >= since_ms {
            out.push((id, path, mtime));
        }
    }
    out.sort_by(|a, b| b.2.cmp(&a.2));
    out.truncate(MAX_SESSIONS_PER_PASS);
    Ok(out)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub sessions_uploaded: usize,
    pub messages_uploaded: usize,
    pub errors: usize,
}

async fn upload(
    client: &reqwest::Client,
    api_base: &str,
    api_key: &str,
    session_id: &str,
    body: &UploadBody<'_>,
) -> Result<()> {
    let url = format!(
        "{}/mobile/sessions/{}",
        api_base.trim_end_matches('/'),
        session_id
    );
    let response = client
        .post(url)
        .bearer_auth(api_key)
        .json(body)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .context("mobile sync upload failed")?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.with_context(|| {
            format!("mobile sync upload returned {status}, response body unreadable")
        })?;
        anyhow::bail!(
            "mobile sync upload returned {status}: {}",
            text.chars().take(300).collect::<String>()
        );
    }
    Ok(())
}

fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
}

/// One incremental sync pass. Safe to call repeatedly.
pub async fn sync_once(client: &reqwest::Client) -> Result<SyncReport> {
    let mut state = load_state();
    if !state.enabled {
        return Ok(SyncReport::default());
    }
    let api_key = crate::subscription_catalog::configured_api_key()
        .context("mobile sync needs a Jcode account: run `jcode account login`")?;
    let api_base = subscription_api::configured_api_base();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let since = now_ms.saturating_sub(BACKFILL_WINDOW.as_millis() as u64);
    let host = host_name();
    let source = session_source();
    let mut report = SyncReport::default();

    for (id, path, mtime) in candidate_sessions(since)? {
        if !valid_session_id(&id) {
            continue;
        }
        // No previous upload means a new session, not an error.
        let progress = state
            .sessions
            .get(&id)
            .cloned()
            .unwrap_or_else(SessionProgress::default);
        if progress.mtime_ms >= mtime {
            continue;
        }
        let session = match Session::load_from_path(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if session.is_debug || session.messages.is_empty() {
            state.sessions.insert(
                id,
                SessionProgress {
                    uploaded: progress.uploaded,
                    mtime_ms: mtime,
                },
            );
            continue;
        }
        let start = progress.uploaded.min(session.messages.len());
        let messages = wire_messages(&session, start);
        let title = session.custom_title.as_deref().or(session.title.as_deref());
        let mut ok = true;
        // Always send at least one request so metadata (title, updated_at) refreshes.
        let chunks: Vec<&[WireMessage]> = if messages.is_empty() {
            vec![&[]]
        } else {
            messages.chunks(MAX_MESSAGES_PER_UPLOAD).collect()
        };
        for chunk in chunks {
            let body = UploadBody {
                title,
                source,
                host: host.clone(),
                cwd: session.working_dir.as_deref(),
                model: session.model.as_deref(),
                created_at: session.created_at.to_rfc3339(),
                updated_at: session.updated_at.to_rfc3339(),
                messages: chunk,
            };
            if let Err(err) = upload(client, &api_base, &api_key, &id, &body).await {
                crate::logging::debug(&format!("mobile sync {id}: {err:#}"));
                report.errors += 1;
                ok = false;
                break;
            }
            report.messages_uploaded += chunk.len();
        }
        if ok {
            report.sessions_uploaded += 1;
            state.sessions.insert(
                id,
                SessionProgress {
                    uploaded: session.messages.len(),
                    mtime_ms: mtime,
                },
            );
        }
    }
    // Reload `enabled` so a concurrent `jcode mobile disable` wins.
    state.enabled = load_state().enabled;
    if state.enabled {
        save_state(&state)?;
    }
    Ok(report)
}

/// Delete everything this account has stored for mobile.
pub async fn wipe_remote(client: &reqwest::Client) -> Result<()> {
    let api_key = crate::subscription_catalog::configured_api_key()
        .context("no Jcode account credential is configured")?;
    let url = format!(
        "{}/mobile/sessions",
        subscription_api::configured_api_base().trim_end_matches('/')
    );
    let response = client
        .delete(url)
        .bearer_auth(api_key)
        .timeout(Duration::from_secs(30))
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("server returned {}", response.status());
    }
    Ok(())
}

/// Background loop used by the jcode server daemon.
pub fn spawn_background_sync() {
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        let mut ticks = tokio::time::interval(SYNC_INTERVAL);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            if !is_enabled() {
                continue;
            }
            if let Err(err) = sync_once(&client).await {
                crate::logging::debug(&format!("mobile sync pass failed: {err:#}"));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_session_types::StoredMessage;

    fn msg(role: Role, content: Vec<ContentBlock>) -> StoredMessage {
        StoredMessage {
            id: "m".into(),
            role,
            content,
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        }
    }

    #[test]
    fn wire_messages_keep_visible_text_and_skip_hidden() {
        let mut session = Session::create_with_id("s1".into(), None, None);
        session.messages = vec![
            msg(
                Role::User,
                vec![ContentBlock::Text {
                    text: "hi".into(),
                    cache_control: None,
                }],
            ),
            msg(
                Role::Assistant,
                vec![ContentBlock::Reasoning {
                    text: "secret".into(),
                }],
            ),
            msg(
                Role::Assistant,
                vec![
                    ContentBlock::Text {
                        text: "running".into(),
                        cache_control: None,
                    },
                    ContentBlock::ToolUse {
                        id: "t".into(),
                        name: "bash".into(),
                        input: serde_json::json!({"command": "ls"}),
                        thought_signature: None,
                    },
                ],
            ),
            msg(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "t".into(),
                    content: "x".repeat(MAX_TOOL_RESULT_CHARS + 10),
                    is_error: None,
                }],
            ),
        ];
        let wire = wire_messages(&session, 0);
        assert_eq!(
            wire.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![0, 2, 3]
        );
        assert_eq!(wire[0].role, "user");
        assert_eq!(wire[1].text, "running\n\n🔧 bash: ls");
        assert_eq!(wire[2].role, "tool");
        assert!(wire[2].text.ends_with("(truncated)"));
        assert!(!wire.iter().any(|m| m.text.contains("secret")));
        assert_eq!(wire_messages(&session, 3).len(), 1);
    }

    #[test]
    fn session_ids_are_validated() {
        assert!(valid_session_id("session_abc-123.x"));
        assert!(!valid_session_id("bad id"));
        assert!(!valid_session_id(""));
    }
}
