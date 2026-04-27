use crate::workspace::SessionCard;
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const DEFAULT_SESSION_LIMIT: usize = 32;
const SESSION_PREVIEW_LINE_LIMIT: usize = 5;
const SESSION_PREVIEW_CHAR_LIMIT: usize = 72;
const SESSION_DETAIL_LINE_LIMIT: usize = 28;
const SESSION_DETAIL_CHAR_LIMIT: usize = 128;

pub fn load_recent_session_cards() -> Result<Vec<SessionCard>> {
    load_recent_session_cards_with_limit(DEFAULT_SESSION_LIMIT)
}

fn load_recent_session_cards_with_limit(limit: usize) -> Result<Vec<SessionCard>> {
    let sessions_dir = jcode_sessions_dir()?;
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut candidates = fs::read_dir(&sessions_dir)
        .with_context(|| format!("failed to read {}", sessions_dir.display()))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| session_file_candidate(entry.path()))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.modified));

    let mut cards = Vec::new();
    for candidate in candidates.into_iter().take(limit.saturating_mul(3)) {
        match load_session_card(&candidate.path) {
            Ok(Some(card)) => cards.push(card),
            Ok(None) => {}
            Err(error) => eprintln!(
                "jcode-desktop: skipped session {}: {error:#}",
                candidate.path.display()
            ),
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

    let modified = path
        .metadata()
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Some(SessionFileCandidate { path, modified })
}

fn load_session_card(path: &Path) -> Result<Option<SessionCard>> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;

    let id = string_field(&value, "id")
        .or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "unknown-session".to_string());
    let short_name = string_field(&value, "short_name").unwrap_or_else(|| short_session_name(&id));
    let message_count = value
        .get("messages")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let title = string_field(&value, "title")
        .or_else(|| latest_user_preview(&value))
        .unwrap_or_else(|| short_name.clone());

    let status = string_field(&value, "status").unwrap_or_else(|| "unknown".to_string());
    let model = string_field(&value, "model").unwrap_or_else(|| "model unknown".to_string());
    let working_dir = string_field(&value, "working_dir").unwrap_or_default();
    let updated = string_field(&value, "last_active_at")
        .or_else(|| string_field(&value, "updated_at"))
        .map(|timestamp| compact_timestamp(&timestamp));
    let cwd = compact_path(&working_dir).unwrap_or_else(|| "no workspace".to_string());

    let subtitle = format!("{status} · {model}");
    let detail = match updated {
        Some(updated) => format!("{message_count} msgs · {updated} · {cwd}"),
        None => format!("{message_count} msgs · {cwd}"),
    };
    let preview_lines = recent_message_preview_lines(
        &value,
        SESSION_PREVIEW_LINE_LIMIT,
        SESSION_PREVIEW_CHAR_LIMIT,
    );
    let detail_lines =
        recent_message_preview_lines(&value, SESSION_DETAIL_LINE_LIMIT, SESSION_DETAIL_CHAR_LIMIT);

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

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

fn latest_user_preview(value: &Value) -> Option<String> {
    value
        .get("messages")
        .and_then(Value::as_array)?
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(message_text_preview)
}

fn message_text_preview(message: &Value) -> Option<String> {
    let mut text = String::new();
    for block in message.get("content")?.as_array()? {
        let Some(block_text) = block.get("text").and_then(Value::as_str) else {
            continue;
        };
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(block_text.trim());
    }

    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(truncate_chars(&normalized, 64))
    }
}

fn recent_message_preview_lines(value: &Value, limit: usize, char_limit: usize) -> Vec<String> {
    let Some(messages) = value.get("messages").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut previews = messages
        .iter()
        .rev()
        .filter_map(|message| message_preview_line(message, char_limit))
        .take(limit)
        .collect::<Vec<_>>();
    previews.reverse();
    previews
}

fn message_preview_line(message: &Value, char_limit: usize) -> Option<String> {
    let role = match message.get("role").and_then(Value::as_str)? {
        "user" => "user",
        "assistant" => "asst",
        "system" => "sys",
        _ => return None,
    };
    let text = message_preview_text(message, char_limit)?;
    Some(format!("{role} {text}"))
}

fn message_preview_text(message: &Value, char_limit: usize) -> Option<String> {
    let mut fragments = Vec::new();
    for block in message.get("content")?.as_array()? {
        match block.get("type").and_then(Value::as_str) {
            Some("text") | None => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    let normalized = normalize_preview_text(text);
                    if !normalized.is_empty() {
                        fragments.push(normalized);
                    }
                }
            }
            Some("tool_use") => {
                if let Some(name) = block.get("name").and_then(Value::as_str) {
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
        Some(truncate_chars(&joined, char_limit))
    }
}

fn normalize_preview_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn short_session_name(id: &str) -> String {
    id.strip_prefix("session_")
        .and_then(|rest| rest.split('_').next())
        .filter(|name| !name.is_empty())
        .unwrap_or(id)
        .to_string()
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

    #[test]
    fn latest_user_preview_uses_recent_user_text() {
        let session = json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "older"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "ignored"}]},
                {"role": "user", "content": [{"type": "text", "text": "newer prompt"}]}
            ]
        });

        assert_eq!(
            latest_user_preview(&session),
            Some("newer prompt".to_string())
        );
    }

    #[test]
    fn recent_message_preview_lines_include_text_and_skip_tool_results() {
        let session = json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hello\nthere"}]},
                {"role": "assistant", "content": [{"type": "tool_use", "name": "bash"}]},
                {"role": "user", "content": [{"type": "tool_result", "content": "noisy payload"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "done now"}]}
            ]
        });

        assert_eq!(
            recent_message_preview_lines(&session, 4, SESSION_PREVIEW_CHAR_LIMIT),
            vec!["user hello there", "asst tool bash", "asst done now"]
        );
    }

    #[test]
    fn short_session_name_extracts_memorable_name() {
        assert_eq!(short_session_name("session_cow_123_abc"), "cow");
        assert_eq!(short_session_name("legacy"), "legacy");
    }
}
