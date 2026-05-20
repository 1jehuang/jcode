use crate::workspace::SessionCard;
use anyhow::{Context, Result};
use jcode_tui_messages::{
    TranscriptPreviewLabels, latest_user_transcript_preview, normalize_transcript_preview_text,
    transcript_preview_lines,
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
    for candidate in candidates.into_iter().take(limit.saturating_mul(3)) {
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
    #[serde(default)]
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
    #[serde(default, rename = "type")]
    block_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
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

fn load_session_card(path: &Path, modified: SystemTime) -> Result<Option<SessionCard>> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let session: StoredSession = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;

    let id = stored_string(session.id.as_deref())
        .or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "unknown-session".to_string());
    let short_name =
        stored_string(session.short_name.as_deref()).unwrap_or_else(|| short_session_name(&id));
    let message_count = session.messages.len();
    let title = stored_string(session.custom_title.as_deref())
        .or_else(|| stored_string(session.title.as_deref()))
        .or_else(|| latest_user_preview(&session))
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
        &session.messages,
        SESSION_PREVIEW_LINE_LIMIT,
        SESSION_PREVIEW_CHAR_LIMIT,
    );
    let detail_lines = recent_message_preview_lines(
        &session.messages,
        SESSION_DETAIL_LINE_LIMIT,
        SESSION_DETAIL_CHAR_LIMIT,
    );

    Ok(Some(SessionCard {
        session_id: id,
        title,
        subtitle,
        detail,
        preview_lines,
        detail_lines,
    }))
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

fn latest_user_preview(session: &StoredSession) -> Option<String> {
    let messages = transcript_preview_messages(&session.messages);
    latest_user_transcript_preview(
        messages
            .iter()
            .map(|(role, text)| (role.as_str(), text.as_str())),
        64,
    )
}

fn recent_message_preview_lines(
    messages: &[StoredMessage],
    limit: usize,
    char_limit: usize,
) -> Vec<String> {
    let messages = transcript_preview_messages(messages);
    transcript_preview_lines(
        messages
            .iter()
            .map(|(role, text)| (role.as_str(), text.as_str())),
        limit,
        char_limit,
        TranscriptPreviewLabels::DESKTOP,
    )
}

fn transcript_preview_messages(messages: &[StoredMessage]) -> Vec<(String, String)> {
    messages
        .iter()
        .filter_map(|message| {
            let role = match message.role.as_deref()? {
                role @ ("user" | "assistant" | "system") => role.to_string(),
                _ => return None,
            };
            let text = message_preview_text(message)?;
            Some((role, text))
        })
        .collect()
}

fn message_preview_text(message: &StoredMessage) -> Option<String> {
    let mut fragments = Vec::new();
    for block in &message.content {
        match block.block_type.as_deref() {
            Some("text") | None => {
                if let Some(text) = block.text.as_deref() {
                    let normalized = normalize_transcript_preview_text(text);
                    if !normalized.is_empty() {
                        fragments.push(normalized);
                    }
                }
            }
            Some("tool_use") => {
                if let Some(name) = block.name.as_deref() {
                    fragments.push(format!("tool {name}"));
                }
            }
            Some("tool_result") => {}
            _ => {}
        }
    }

    let joined = fragments.join(" ");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
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

        assert_eq!(
            latest_user_preview(&session),
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

        assert_eq!(
            recent_message_preview_lines(&session.messages, 4, SESSION_PREVIEW_CHAR_LIMIT),
            vec!["user hello there", "asst tool bash", "asst done now"]
        );
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
    fn short_session_name_extracts_memorable_name() {
        assert_eq!(short_session_name("session_cow_123_abc"), "cow");
        assert_eq!(short_session_name("legacy"), "legacy");
    }
}
