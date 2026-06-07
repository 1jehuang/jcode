use crate::workspace::SessionCard;
pub use crate::workspace::SessionTranscriptMessage;
use anyhow::{Context, Result};
use jcode_tui_messages::{
    TranscriptPreviewLabels, latest_user_transcript_preview, transcript_preview_lines,
};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_SESSION_LIMIT: usize = 32;
const SESSION_PREVIEW_LINE_LIMIT: usize = 5;
const SESSION_PREVIEW_CHAR_LIMIT: usize = 72;
const SESSION_DETAIL_LINE_LIMIT: usize = 28;
const SESSION_DETAIL_CHAR_LIMIT: usize = 128;
const SESSION_CARD_TRANSCRIPT_MESSAGE_LIMIT: usize = 64;
const SESSION_CARD_TRANSCRIPT_CHAR_LIMIT: usize = 48_000;
const SESSION_CARD_TRANSCRIPT_MESSAGE_CHAR_LIMIT: usize = 4_000;

pub fn load_recent_session_cards() -> Result<Vec<SessionCard>> {
    load_recent_session_cards_with_limit(DEFAULT_SESSION_LIMIT)
}

pub fn load_crashed_session_cards() -> Result<Vec<SessionCard>> {
    Ok(load_recent_session_cards_with_limit(DEFAULT_SESSION_LIMIT)?
        .into_iter()
        .filter(|card| card.subtitle.starts_with("crashed ·"))
        .collect())
}

pub fn load_session_card_by_id(session_id: &str) -> Result<Option<SessionCard>> {
    let sessions_dir = jcode_sessions_dir()?;
    let path = sessions_dir.join(format!("{session_id}.json"));
    if path.exists() {
        return load_session_card(&path, session_file_modified(&path));
    }

    Ok(load_recent_session_cards_with_limit(DEFAULT_SESSION_LIMIT)?
        .into_iter()
        .find(|card| card.session_id == session_id))
}

pub fn load_session_transcript_by_id(
    session_id: &str,
) -> Result<Option<Vec<SessionTranscriptMessage>>> {
    let sessions_dir = jcode_sessions_dir()?;
    let direct_path = sessions_dir.join(format!("{session_id}.json"));
    if direct_path.exists() {
        let session = load_stored_session(&direct_path)?;
        return Ok(Some(session_transcript_messages(&session)));
    }

    if !sessions_dir.exists() {
        return Ok(None);
    }

    for entry in fs::read_dir(&sessions_dir)
        .with_context(|| format!("failed to read {}", sessions_dir.display()))?
    {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if session_file_candidate(path.clone()).is_none() {
            continue;
        }
        let session = match load_stored_session(&path) {
            Ok(session) => session,
            Err(error) => {
                crate::desktop_log::warn(format_args!(
                    "jcode-desktop: skipped transcript {}: {error:#}",
                    path.display()
                ));
                continue;
            }
        };
        let id = stored_string(session.id.as_deref())
            .or_else(|| {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        if id == session_id {
            return Ok(Some(session_transcript_messages(&session)));
        }
    }

    Ok(None)
}

/// A full, uncapped transcript loaded straight from disk, used by the
/// real-transcript scroll benchmark so we profile the production render path
/// against the user's actual session content rather than synthetic fixtures.
#[derive(Debug, Clone)]
pub struct BenchmarkTranscript {
    pub session_id: String,
    pub title: String,
    pub file_bytes: u64,
    pub messages: Vec<SessionTranscriptMessage>,
}

/// Load the largest real session transcripts on disk (by file size), returning
/// the full message list for each (no card-style truncation). Used only by the
/// scroll benchmark. Sessions with fewer than `min_messages` are skipped so the
/// benchmark exercises long, scroll-heavy transcripts.
pub fn load_largest_real_transcripts(
    max_sessions: usize,
    min_messages: usize,
) -> Result<Vec<BenchmarkTranscript>> {
    let sessions_dir = jcode_sessions_dir()?;
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut candidates = fs::read_dir(&sessions_dir)
        .with_context(|| format!("failed to read {}", sessions_dir.display()))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            session_file_candidate(path.clone())?;
            let bytes = path.metadata().ok()?.len();
            Some((path, bytes))
        })
        .collect::<Vec<_>>();
    // Largest files first: they hold the longest transcripts and stress the
    // windowed-scroll path the most.
    candidates.sort_by_key(|(_, bytes)| std::cmp::Reverse(*bytes));

    let mut transcripts = Vec::new();
    for (path, bytes) in candidates {
        if transcripts.len() >= max_sessions {
            break;
        }
        let session = match load_stored_session(&path) {
            Ok(session) => session,
            Err(_) => continue,
        };
        let messages = session_transcript_messages(&session);
        if messages.len() < min_messages {
            continue;
        }
        let id = stored_string(session.id.as_deref())
            .or_else(|| {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "unknown-session".to_string());
        let title = stored_string(session.custom_title.as_deref())
            .or_else(|| stored_string(session.title.as_deref()))
            .or_else(|| latest_user_preview(&messages))
            .unwrap_or_else(|| short_session_name(&id));
        transcripts.push(BenchmarkTranscript {
            session_id: id,
            title,
            file_bytes: bytes,
            messages,
        });
    }

    Ok(transcripts)
}

fn load_recent_session_cards_with_limit(limit: usize) -> Result<Vec<SessionCard>> {
    let sessions_dir = jcode_sessions_dir()?;
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut candidates = fs::read_dir(&sessions_dir)
        .with_context(|| format!("failed to read {}", sessions_dir.display()))?
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(error) => {
                crate::desktop_log::warn(format_args!(
                    "jcode-desktop: failed to read entry in {}: {error}",
                    sessions_dir.display()
                ));
                None
            }
        })
        .filter_map(|entry| session_file_candidate(entry.path()))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.modified));

    let mut cards = Vec::new();
    for candidate in candidates {
        match load_session_card(&candidate.path, candidate.modified) {
            Ok(Some(card)) => cards.push(card),
            Ok(None) => {}
            Err(error) => crate::desktop_log::warn(format_args!(
                "jcode-desktop: skipped session {}: {error:#}",
                candidate.path.display()
            )),
        }
        if cards.len() >= limit {
            break;
        }
    }

    Ok(cards)
}

#[derive(Debug)]
struct SessionFileCandidate {
    path: PathBuf,
    modified: SystemTime,
}

fn session_file_candidate(path: PathBuf) -> Option<SessionFileCandidate> {
    let file_name = path.file_name()?.to_string_lossy();
    if !file_name.ends_with(".json") || file_name.ends_with(".journal.json") {
        return None;
    }

    let modified = session_file_modified(&path);
    Some(SessionFileCandidate { path, modified })
}

fn session_file_modified(path: &Path) -> SystemTime {
    match path.metadata().and_then(|metadata| metadata.modified()) {
        Ok(modified) => modified,
        Err(error) => {
            crate::desktop_log::warn(format_args!(
                "jcode-desktop: failed to read session file timestamp for {}: {error}",
                path.display()
            ));
            SystemTime::UNIX_EPOCH
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct StoredSession {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    short_name: Option<String>,
    #[serde(default)]
    custom_title: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default, deserialize_with = "deserialize_status_string")]
    status: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    last_active_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    messages: Vec<StoredMessage>,
}

#[derive(Debug, Default, Deserialize)]
struct StoredMessage {
    #[serde(default)]
    role: Option<String>,
    #[serde(default, deserialize_with = "deserialize_message_content")]
    content: Vec<StoredContentBlock>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct StoredContentBlock {
    #[serde(
        default,
        rename = "type",
        deserialize_with = "deserialize_optional_string"
    )]
    block_type: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    text: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    name: Option<String>,
}

impl StoredContentBlock {
    fn text(text: String) -> Self {
        Self {
            block_type: Some("text".to_string()),
            text: Some(text),
            name: None,
        }
    }
}

fn deserialize_message_content<'de, D>(deserializer: D) -> Result<Vec<StoredContentBlock>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::Array(blocks) => blocks
            .into_iter()
            .filter_map(|block| serde_json::from_value(block).ok())
            .collect(),
        Value::String(text) => vec![StoredContentBlock::text(text)],
        Value::Object(_) => serde_json::from_value(value)
            .map(|block| vec![block])
            .unwrap_or_default(),
        _ => Vec::new(),
    })
}

fn deserialize_optional_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(Value::String(text)) if !text.trim().is_empty() => Some(text),
        _ => None,
    })
}

fn deserialize_status_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(Value::String(status)) if !status.trim().is_empty() => Some(status),
        Some(Value::Object(map)) => map.keys().next().map(|key| key.to_ascii_lowercase()),
        _ => None,
    })
}

fn load_session_card(path: &Path, modified: SystemTime) -> Result<Option<SessionCard>> {
    let session = load_stored_session(path)?;

    let id = stored_string(session.id.as_deref())
        .or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "unknown-session".to_string());
    let short_name =
        stored_string(session.short_name.as_deref()).unwrap_or_else(|| short_session_name(&id));
    let message_count = session.messages.len();
    let transcript_messages = session_transcript_messages(&session);
    let title = stored_string(session.custom_title.as_deref())
        .or_else(|| stored_string(session.title.as_deref()))
        .or_else(|| latest_user_preview(&transcript_messages))
        .unwrap_or_else(|| short_name.clone());

    let status = stored_string(session.status.as_deref()).unwrap_or_else(|| "unknown".to_string());
    let model =
        stored_string(session.model.as_deref()).unwrap_or_else(|| "model unknown".to_string());
    let working_dir = stored_string(session.working_dir.as_deref()).unwrap_or_default();
    let updated = stored_string(session.last_active_at.as_deref())
        .or_else(|| stored_string(session.updated_at.as_deref()))
        .map(|timestamp| compact_timestamp(&timestamp))
        .or_else(|| compact_file_modified(modified));
    let cwd = compact_path(&working_dir).unwrap_or_else(|| "no workspace".to_string());

    let subtitle = format!("{status} · {model}");
    let detail = match updated {
        Some(updated) => format!("{message_count} msgs · {updated} · {cwd}"),
        None => format!("{message_count} msgs · {cwd}"),
    };
    let preview_lines = recent_message_preview_lines(
        &transcript_messages,
        SESSION_PREVIEW_LINE_LIMIT,
        SESSION_PREVIEW_CHAR_LIMIT,
    );
    let detail_lines = recent_message_preview_lines(
        &transcript_messages,
        SESSION_DETAIL_LINE_LIMIT,
        SESSION_DETAIL_CHAR_LIMIT,
    );
    let card_transcript_messages = session_card_transcript_messages(&transcript_messages);

    Ok(Some(SessionCard {
        session_id: id,
        title,
        subtitle,
        detail,
        preview_lines,
        detail_lines,
        transcript_messages: card_transcript_messages,
    }))
}

fn load_stored_session(path: &Path) -> Result<StoredSession> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn jcode_sessions_dir() -> Result<PathBuf> {
    let jcode_home = match std::env::var_os("JCODE_HOME") {
        Some(path) => PathBuf::from(path),
        None => std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set")?
            .join(".jcode"),
    };
    Ok(jcode_home.join("sessions"))
}

fn stored_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

fn latest_user_preview(messages: &[SessionTranscriptMessage]) -> Option<String> {
    latest_user_transcript_preview(
        messages
            .iter()
            .map(|message| (message.role.as_str(), message.content.as_str())),
        64,
    )
}

fn recent_message_preview_lines(
    messages: &[SessionTranscriptMessage],
    limit: usize,
    char_limit: usize,
) -> Vec<String> {
    transcript_preview_lines(
        messages
            .iter()
            .map(|message| (message.role.as_str(), message.content.as_str())),
        limit,
        char_limit,
        TranscriptPreviewLabels::DESKTOP,
    )
}

fn session_transcript_messages(messages: &StoredSession) -> Vec<SessionTranscriptMessage> {
    messages
        .messages
        .iter()
        .filter_map(|message| {
            let role = transcript_display_role(message.role.as_deref());
            let content = message_transcript_text(message)?;
            if should_skip_desktop_transcript_message(&role, &content) {
                return None;
            }
            Some(SessionTranscriptMessage { role, content })
        })
        .collect()
}

fn session_card_transcript_messages(
    messages: &[SessionTranscriptMessage],
) -> Vec<SessionTranscriptMessage> {
    let mut selected = Vec::new();
    let mut total_chars = 0usize;

    for message in messages.iter().rev() {
        if selected.len() >= SESSION_CARD_TRANSCRIPT_MESSAGE_LIMIT {
            break;
        }

        let content = truncate_chars(&message.content, SESSION_CARD_TRANSCRIPT_MESSAGE_CHAR_LIMIT);
        let content_chars = content.chars().count();
        if !selected.is_empty()
            && total_chars.saturating_add(content_chars) > SESSION_CARD_TRANSCRIPT_CHAR_LIMIT
        {
            break;
        }

        total_chars = total_chars.saturating_add(content_chars);
        selected.push(SessionTranscriptMessage {
            role: message.role.clone(),
            content,
        });
    }

    selected.reverse();
    selected
}

fn transcript_display_role(role: Option<&str>) -> String {
    match role.unwrap_or("meta") {
        role @ ("user" | "assistant" | "system" | "background_task" | "tool") => role.to_string(),
        _ => "meta".to_string(),
    }
}

fn message_transcript_text(message: &StoredMessage) -> Option<String> {
    let mut fragments = Vec::new();
    for block in &message.content {
        match block.block_type.as_deref() {
            Some("text") | None => {
                if let Some(text) = block.text.as_deref() {
                    let text = text.trim();
                    if !text.is_empty() {
                        fragments.push(text.to_string());
                    }
                }
            }
            Some("tool_use") => {
                if let Some(name) = block
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                {
                    fragments.push(format!("tool {name}"));
                }
            }
            Some("tool_result") => {}
            _ => {}
        }
    }

    let joined = fragments.join("\n\n");
    if joined.trim().is_empty() {
        None
    } else {
        Some(joined)
    }
}

fn should_skip_desktop_transcript_message(role: &str, content: &str) -> bool {
    role == "user" && content.trim_start().starts_with("<system-reminder>")
}

fn short_session_name(id: &str) -> String {
    id.strip_prefix("session_")
        .and_then(|rest| rest.split('_').next())
        .filter(|name| !name.is_empty())
        .unwrap_or(id)
        .to_string()
}

fn compact_file_modified(modified: SystemTime) -> Option<String> {
    modified
        .duration_since(UNIX_EPOCH)
        .ok()
        .filter(|duration| !duration.is_zero())
        .map(|duration| format!("mtime {}", duration.as_secs()))
}

fn compact_timestamp(timestamp: &str) -> String {
    timestamp
        .split_once('T')
        .map(|(date, time)| format!("{} {}", date, time.chars().take(5).collect::<String>()))
        .unwrap_or_else(|| truncate_chars(timestamp, 18))
}

fn compact_path(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    let basename = Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_string());
    Some(truncate_chars(&basename, 28))
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stored_session(value: serde_json::Value) -> StoredSession {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn latest_user_preview_uses_recent_user_text() {
        let session = stored_session(json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "older"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "ignored"}]},
                {"role": "user", "content": [{"type": "text", "text": "newer prompt"}]}
            ]
        }));

        let messages = session_transcript_messages(&session);

        assert_eq!(
            latest_user_preview(&messages),
            Some("newer prompt".to_string())
        );
    }

    #[test]
    fn recent_message_preview_lines_include_text_and_skip_tool_results() {
        let session = stored_session(json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hello\nthere"}]},
                {"role": "assistant", "content": [{"type": "tool_use", "name": "bash"}]},
                {"role": "user", "content": [{"type": "tool_result", "content": "noisy payload"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "done now"}]}
            ]
        }));

        let messages = session_transcript_messages(&session);

        assert_eq!(
            recent_message_preview_lines(&messages, 4, SESSION_PREVIEW_CHAR_LIMIT),
            vec!["user hello there", "asst tool bash", "asst done now"]
        );
    }

    #[test]
    fn session_transcript_messages_skip_startup_reminder_and_keep_chat_roles() {
        let session = stored_session(json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "<system-reminder>startup context</system-reminder>"}]},
                {"role": "user", "content": [{"type": "text", "text": "resume prompt"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "resume answer"}]},
                {"role": "assistant", "content": [{"type": "tool_use", "name": "agentgrep"}]},
                {"role": "user", "content": [{"type": "tool_result", "content": "ignored"}]}
            ]
        }));

        let messages = session_transcript_messages(&session);

        assert_eq!(
            messages,
            vec![
                SessionTranscriptMessage {
                    role: "user".to_string(),
                    content: "resume prompt".to_string(),
                },
                SessionTranscriptMessage {
                    role: "assistant".to_string(),
                    content: "resume answer".to_string(),
                },
                SessionTranscriptMessage {
                    role: "assistant".to_string(),
                    content: "tool agentgrep".to_string(),
                },
            ]
        );
    }

    #[test]
    fn load_session_card_filters_startup_reminder_from_preview_and_transcript() -> Result<()> {
        let dir = std::env::temp_dir().join(format!(
            "jcode-desktop-session-card-reminder-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;
        let path = dir.join("session_reminder_123.json");
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "status": "active",
                "model": "test-model",
                "messages": [
                    {"role": "user", "content": [{"type": "text", "text": "<system-reminder>startup context</system-reminder>"}]},
                    {"role": "user", "content": [{"type": "text", "text": "clean prompt"}]},
                    {"role": "assistant", "content": [{"type": "text", "text": "clean answer"}]}
                ]
            }))?,
        )?;

        let card = load_session_card(&path, SystemTime::UNIX_EPOCH)?.unwrap();
        let preview = card.preview_lines.join("\n");
        let detail = card.detail_lines.join("\n");

        assert_eq!(card.title, "clean prompt");
        assert!(!preview.contains("system-reminder"));
        assert!(!detail.contains("system-reminder"));
        assert_eq!(
            card.preview_lines,
            vec!["user clean prompt", "asst clean answer"]
        );
        assert_eq!(
            card.transcript_messages,
            vec![
                SessionTranscriptMessage {
                    role: "user".to_string(),
                    content: "clean prompt".to_string(),
                },
                SessionTranscriptMessage {
                    role: "assistant".to_string(),
                    content: "clean answer".to_string(),
                },
            ]
        );

        let _ = fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn session_card_transcript_messages_are_bounded_to_recent_visible_text() -> Result<()> {
        let dir = std::env::temp_dir().join(format!(
            "jcode-desktop-session-card-bound-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;
        let path = dir.join("session_large_123.json");
        let messages = (0..80)
            .map(|index| {
                json!({
                    "role": "user",
                    "content": [{"type": "text", "text": format!("prompt {index}")}],
                })
            })
            .chain(std::iter::once(json!({
                "role": "assistant",
                "content": [{"type": "text", "text": "x".repeat(SESSION_CARD_TRANSCRIPT_MESSAGE_CHAR_LIMIT + 12)}],
            })))
            .collect::<Vec<_>>();
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "status": "active",
                "model": "test-model",
                "messages": messages,
            }))?,
        )?;

        let card = load_session_card(&path, SystemTime::UNIX_EPOCH)?.unwrap();

        assert_eq!(
            card.transcript_messages.len(),
            SESSION_CARD_TRANSCRIPT_MESSAGE_LIMIT
        );
        assert_eq!(
            card.transcript_messages
                .first()
                .map(|message| message.content.as_str()),
            Some("prompt 17")
        );
        let last = card
            .transcript_messages
            .last()
            .expect("last bounded message");
        assert_eq!(last.role, "assistant");
        assert_eq!(
            last.content.chars().count(),
            SESSION_CARD_TRANSCRIPT_MESSAGE_CHAR_LIMIT + 1,
            "long card transcript message should be truncated with an ellipsis"
        );
        assert!(last.content.ends_with('…'));

        let _ = fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn typed_session_parser_accepts_legacy_string_content() -> Result<()> {
        let dir = std::env::temp_dir().join(format!(
            "jcode-desktop-session-data-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;
        let path = dir.join("session_legacy_123.json");
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "status": "active",
                "model": "claude",
                "working_dir": "/tmp/example",
                "messages": [
                    {"role": "user", "content": "legacy prompt text"},
                    {"role": "assistant", "content": {"type": "text", "text": "legacy reply"}}
                ]
            }))?,
        )?;

        let card = load_session_card(&path, SystemTime::UNIX_EPOCH)?.unwrap();

        assert_eq!(card.session_id, "session_legacy_123");
        assert_eq!(card.title, "legacy prompt text");
        assert_eq!(card.subtitle, "active · claude");
        assert_eq!(
            card.preview_lines,
            vec!["user legacy prompt text", "asst legacy reply"]
        );
        let _ = fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn recent_session_loader_scans_past_many_invalid_recent_files() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_home = std::env::temp_dir().join(format!(
            "jcode-desktop-session-loader-test-{}-{unique}",
            std::process::id()
        ));
        let sessions_dir = temp_home.join("sessions");
        fs::create_dir_all(&sessions_dir).unwrap();

        fs::write(
            sessions_dir.join("valid_old.json"),
            r#"{"id":"valid_old","title":"valid old","status":"Closed","messages":[{"role":"user","content":[{"type":"text","text":"hello from valid session"}]}]}"#,
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        for index in 0..100 {
            fs::write(
                sessions_dir.join(format!("invalid_new_{index:03}.json")),
                "{ not valid json",
            )
            .unwrap();
        }

        let previous_home = std::env::var_os("JCODE_HOME");
        unsafe { std::env::set_var("JCODE_HOME", &temp_home) };
        let cards = load_recent_session_cards().unwrap();
        match previous_home {
            Some(value) => unsafe { std::env::set_var("JCODE_HOME", value) },
            None => unsafe { std::env::remove_var("JCODE_HOME") },
        }
        let _ = fs::remove_dir_all(&temp_home);

        assert!(
            cards.iter().any(|card| card.session_id == "valid_old"),
            "loader should keep scanning after invalid recent files: {cards:?}"
        );
    }

    #[test]
    fn short_session_name_extracts_memorable_name() {
        assert_eq!(short_session_name("session_cow_123_abc"), "cow");
        assert_eq!(short_session_name("legacy"), "legacy");
    }
}
