//! Building the stream-json `user` line for one turn from jcode history.
//!
//! Claude Code keeps the transcript itself, so only the newest user input is
//! sent. When a brand new Claude session starts for a conversation that
//! already has history (for example after switching from native Claude), a
//! compact text transcript of the earlier turns is prepended once.

use jcode_message_types::{ContentBlock, Message, Role};
use serde_json::{Value, json};

/// Upper bound for the prepended transcript (characters).
pub const TRANSCRIPT_CHAR_BUDGET: usize = 24_000;

/// Short note appended to Claude Code's own system prompt.
pub const JCODE_APPEND_SYSTEM_PROMPT: &str = "You are running inside jcode, a coding agent harness, through Claude Code. \
Tools named mcp__jcode__* are provided by jcode (memory, session search, swarm coordination and similar); \
use them when they fit the task. Use your built-in tools for files, shell and search.";

fn is_tool_result_message(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
}

fn has_text(message: &Message) -> bool {
    message.content.iter().any(|block| match block {
        ContentBlock::Text { text, .. } => !text.trim().is_empty(),
        ContentBlock::Image { .. } => true,
        _ => false,
    })
}

/// The newest user input split from the earlier history.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnInput {
    /// Text blocks of the new input, in order.
    pub texts: Vec<String>,
    /// `(media_type, base64 data)` images of the new input.
    pub images: Vec<(String, String)>,
    /// Number of messages before the new input.
    pub history_len: usize,
}

/// Split `messages` into the newest user input and the history before it.
///
/// The new input starts at the last user message that carries text or images
/// (not tool results) and includes any following user messages such as the
/// dynamic `<system-reminder>` jcode inserts after it.
pub fn turn_input(messages: &[Message]) -> TurnInput {
    let start = messages
        .iter()
        .rposition(|m| m.role == Role::User && !is_tool_result_message(m) && has_text(m))
        .map(|idx| {
            // Include an immediately preceding run of fresh user messages
            // that were queued together (e.g. several soft interrupts).
            let mut first = idx;
            while first > 0 {
                let prev = &messages[first - 1];
                if prev.role == Role::User && !is_tool_result_message(prev) && has_text(prev) {
                    first -= 1;
                } else {
                    break;
                }
            }
            first
        });
    let Some(start) = start else {
        return TurnInput {
            texts: vec!["Continue.".into()],
            images: Vec::new(),
            history_len: messages.len(),
        };
    };
    let mut texts = Vec::new();
    let mut images = Vec::new();
    for message in &messages[start..] {
        if message.role != Role::User || is_tool_result_message(message) {
            continue;
        }
        for block in &message.content {
            match block {
                ContentBlock::Text { text, .. } if !text.trim().is_empty() => {
                    texts.push(text.clone())
                }
                ContentBlock::Image { media_type, data } => {
                    images.push((media_type.clone(), data.clone()))
                }
                _ => {}
            }
        }
    }
    if texts.is_empty() {
        texts.push("Describe the attached image.".into());
    }
    TurnInput {
        texts,
        images,
        history_len: start,
    }
}

/// Compact text transcript of `history` (user/assistant text only), keeping
/// the most recent turns within [`TRANSCRIPT_CHAR_BUDGET`].
pub fn transcript(history: &[Message]) -> Option<String> {
    let mut entries: Vec<String> = Vec::new();
    for message in history {
        let text: Vec<&str> = message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.trim()),
                _ => None,
            })
            .filter(|t| !t.is_empty() && !t.starts_with("<system-reminder>"))
            .collect();
        if text.is_empty() {
            continue;
        }
        let who = match message.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
        };
        entries.push(format!("{who}: {}", text.join("\n")));
    }
    if entries.is_empty() {
        return None;
    }
    let mut kept: Vec<String> = Vec::new();
    let mut used = 0usize;
    for entry in entries.into_iter().rev() {
        let len = entry.chars().count();
        if used + len > TRANSCRIPT_CHAR_BUDGET {
            if kept.is_empty() {
                let tail: String = entry
                    .chars()
                    .skip(len.saturating_sub(TRANSCRIPT_CHAR_BUDGET))
                    .collect();
                kept.push(tail);
            }
            break;
        }
        used += len;
        kept.push(entry);
    }
    kept.reverse();
    Some(kept.join("\n\n"))
}

/// Build the stream-json `user` line. Images come first, text last.
pub fn user_line(input: &TurnInput, transcript: Option<&str>, uuid: &str) -> Value {
    let mut content: Vec<Value> = input
        .images
        .iter()
        .map(|(media_type, data)| {
            json!({
                "type": "image",
                "source": {"type": "base64", "media_type": media_type, "data": data},
            })
        })
        .collect();
    let mut text = input.texts.join("\n\n");
    if let Some(transcript) = transcript.filter(|t| !t.trim().is_empty()) {
        text = format!(
            "<previous_conversation>\nThis conversation started elsewhere. Earlier turns, for context:\n\n{transcript}\n</previous_conversation>\n\n{text}"
        );
    }
    content.push(json!({"type": "text", "text": text}));
    json!({
        "type": "user",
        "uuid": uuid,
        "session_id": "",
        "parent_tool_use_id": null,
        "message": {"role": "user", "content": content},
    })
}

/// Working directory announced in jcode's system prompt
/// (`Working directory: <path>`), if any.
pub fn working_dir_from_system(system: &str) -> Option<std::path::PathBuf> {
    system.lines().find_map(|line| {
        let path = line.trim().strip_prefix("Working directory: ")?.trim();
        let path = std::path::PathBuf::from(path);
        (path.is_absolute() && path.is_dir()).then_some(path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> Message {
        Message::user(text)
    }

    fn assistant(text: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        }
    }

    fn tool_result() -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t".into(),
                content: "out".into(),
                is_error: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        }
    }

    #[test]
    fn newest_input_includes_trailing_reminder() {
        let messages = vec![
            user("first"),
            assistant("reply"),
            tool_result(),
            user("second"),
            user("<system-reminder>\nnow\n</system-reminder>"),
        ];
        let input = turn_input(&messages);
        assert_eq!(input.history_len, 3);
        assert_eq!(
            input.texts,
            vec!["second", "<system-reminder>\nnow\n</system-reminder>"]
        );
    }

    #[test]
    fn images_go_first_text_last() {
        let mut msg = user("look");
        msg.content.push(ContentBlock::Image {
            media_type: "image/png".into(),
            data: "AAAA".into(),
        });
        let input = turn_input(&[msg]);
        let line = user_line(&input, None, "u1");
        let content = line["message"]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "look");
        assert_eq!(line["uuid"], "u1");
        assert_eq!(line["type"], "user");
    }

    #[test]
    fn transcript_is_prepended_and_bounded() {
        let history = vec![user("hi"), assistant("hello"), tool_result()];
        let t = transcript(&history).unwrap();
        assert_eq!(t, "User: hi\n\nAssistant: hello");
        let input = turn_input(&[user("next")]);
        let line = user_line(&input, Some(&t), "u");
        let text = line["message"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("<previous_conversation>"));
        assert!(text.ends_with("next"));

        let long = "x".repeat(TRANSCRIPT_CHAR_BUDGET * 2);
        let t = transcript(&[user(&long)]).unwrap();
        assert_eq!(t.chars().count(), TRANSCRIPT_CHAR_BUDGET);
        assert!(transcript(&[tool_result()]).is_none());
    }

    #[test]
    fn working_dir_parsing() {
        let dir = std::env::temp_dir();
        let system = format!("OS: x\nWorking directory: {}\n", dir.display());
        assert_eq!(
            working_dir_from_system(&system).as_deref(),
            Some(dir.as_path())
        );
        assert!(working_dir_from_system("Working directory: relative").is_none());
    }
}
