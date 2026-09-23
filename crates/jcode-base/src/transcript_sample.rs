//! Read-only samples of recent transcripts from other coding harnesses.
//!
//! Desktop onboarding replays one behind its Continue button so the first chat
//! panel a user sees is their own work. Unlike `import`, nothing is saved,
//! copied, or sent anywhere. Files are only read.

use jcode_import_core::{
    ClaudeCodeContent, ClaudeCodeContentBlock, ClaudeCodeEntry, collect_recent_files_recursive,
    ordered_claude_code_message_entries,
};
use std::io::{BufRead, BufReader};
use std::path::Path;

/// One renderable step of a sampled conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SampleTurn {
    User(String),
    Assistant(String),
    Reasoning(String),
    Tool {
        name: String,
        /// JSON arguments, as the Desktop tool row expects.
        input: String,
        output: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptSample {
    /// Harness the transcript came from, e.g. "Claude Code".
    pub source: &'static str,
    pub turns: Vec<SampleTurn>,
}

/// Enough back-and-forth to feel like a real session.
const MIN_TURNS: usize = 8;
const MAX_TURNS: usize = 80;
const MAX_TEXT: usize = 4_000;
const MAX_OUTPUT: usize = 1_500;
/// Bounded scan so onboarding stays fast on large histories.
const SCAN_FILES: usize = 12;
const MAX_FILE_BYTES: u64 = 24 * 1024 * 1024;

/// The most recent sufficiently long transcript from Claude Code or Codex.
pub fn recent_external_transcript() -> Option<TranscriptSample> {
    let home = crate::storage::user_home_path("").ok()?;
    recent_external_transcript_in(&home)
}

pub fn recent_external_transcript_in(home: &Path) -> Option<TranscriptSample> {
    let mut candidates: Vec<(std::time::SystemTime, &'static str, std::path::PathBuf)> = Vec::new();
    for (source, dir) in [
        ("Claude Code", home.join(".claude/projects")),
        ("Codex", home.join(".codex/sessions")),
    ] {
        for path in collect_recent_files_recursive(&dir, "jsonl", SCAN_FILES) {
            let Ok(meta) = path.metadata() else { continue };
            if meta.len() > MAX_FILE_BYTES {
                continue;
            }
            candidates.push((meta.modified().ok()?, source, path));
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    candidates.into_iter().find_map(|(_, source, path)| {
        let turns = match source {
            "Claude Code" => claude_turns(&path),
            _ => codex_turns(&path),
        }?;
        (turns.len() >= MIN_TURNS).then_some(TranscriptSample { source, turns })
    })
}

fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Harness wrappers such as `<command-name>` or injected context are noise.
fn is_synthetic(text: &str) -> bool {
    let text = text.trim_start();
    text.is_empty()
        || text.starts_with('<')
        || text.starts_with("Caveat:")
        || text.starts_with("# AGENTS.md")
        || text.starts_with("[Request interrupted")
}

fn push(turns: &mut Vec<SampleTurn>, turn: SampleTurn) {
    if turns.len() < MAX_TURNS {
        turns.push(turn);
    }
}

fn claude_turns(path: &Path) -> Option<Vec<SampleTurn>> {
    let file = std::fs::File::open(path).ok()?;
    let entries: Vec<ClaudeCodeEntry> = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str(&line).ok())
        .collect();
    let mut turns = Vec::new();
    // Tool results arrive in the following user message, keyed by call id.
    let mut pending: Vec<(String, usize)> = Vec::new();
    for entry in ordered_claude_code_message_entries(&entries) {
        let Some(message) = &entry.message else {
            continue;
        };
        let user = message.role == "user";
        let blocks = match &message.content {
            ClaudeCodeContent::Empty => continue,
            ClaudeCodeContent::Text(text) => {
                vec![ClaudeCodeContentBlock::Text { text: text.clone() }]
            }
            ClaudeCodeContent::Blocks(blocks) => blocks.clone(),
        };
        for block in blocks {
            match block {
                ClaudeCodeContentBlock::Text { text } if !is_synthetic(&text) => push(
                    &mut turns,
                    if user {
                        SampleTurn::User(clip(&text, MAX_TEXT))
                    } else {
                        SampleTurn::Assistant(clip(&text, MAX_TEXT))
                    },
                ),
                ClaudeCodeContentBlock::Thinking { thinking, .. }
                    if !thinking.trim().is_empty() =>
                {
                    push(&mut turns, SampleTurn::Reasoning(clip(&thinking, MAX_TEXT)))
                }
                ClaudeCodeContentBlock::ToolUse { id, name, input } => {
                    pending.push((id, turns.len()));
                    push(
                        &mut turns,
                        SampleTurn::Tool {
                            name,
                            input: input.to_string(),
                            output: String::new(),
                        },
                    );
                }
                ClaudeCodeContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    if let Some(index) = pending
                        .iter()
                        .position(|(id, _)| *id == tool_use_id)
                        .map(|at| pending.remove(at).1)
                        && let Some(SampleTurn::Tool { output, .. }) = turns.get_mut(index)
                    {
                        *output = clip(&content, MAX_OUTPUT);
                    }
                }
                _ => {}
            }
        }
    }
    Some(turns)
}

fn codex_text(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(|text| text.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Codex Desktop wraps shell calls as `tools.exec_command({"cmd": ...})`
/// scripts. Unwrap those, and turn other free-form input into a command so
/// the Desktop tool row has a readable summary.
fn codex_tool_input(raw: &str) -> String {
    if serde_json::from_str::<serde_json::Value>(raw).is_ok_and(|value| value.is_object()) {
        return raw.to_string();
    }
    let wrapped = raw
        .split_once("exec_command(")
        .and_then(|(_, rest)| rest.rfind('}').map(|end| &rest[..=end]))
        .and_then(|args| serde_json::from_str::<serde_json::Value>(args).ok())
        .and_then(|args| {
            args.get("cmd")
                .and_then(|cmd| cmd.as_str())
                .map(str::to_owned)
        });
    serde_json::json!({ "command": clip(&wrapped.unwrap_or_else(|| raw.to_string()), MAX_OUTPUT) })
        .to_string()
}

fn codex_turns(path: &Path) -> Option<Vec<SampleTurn>> {
    let file = std::fs::File::open(path).ok()?;
    let mut turns = Vec::new();
    let mut pending: Vec<(String, usize)> = Vec::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(|kind| kind.as_str()) != Some("response_item") {
            continue;
        }
        let Some(item) = value.get("payload") else {
            continue;
        };
        match item
            .get("type")
            .and_then(|kind| kind.as_str())
            .unwrap_or("")
        {
            "message" => {
                let text = codex_text(item.get("content"));
                if is_synthetic(&text) {
                    continue;
                }
                match item.get("role").and_then(|role| role.as_str()) {
                    Some("user") => push(&mut turns, SampleTurn::User(clip(&text, MAX_TEXT))),
                    Some("assistant") => {
                        push(&mut turns, SampleTurn::Assistant(clip(&text, MAX_TEXT)))
                    }
                    _ => {}
                }
            }
            "reasoning" => {
                let summary: Vec<&str> = item
                    .get("summary")
                    .and_then(|summary| summary.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|part| part.get("text").and_then(|text| text.as_str()))
                    .collect();
                if !summary.is_empty() {
                    push(
                        &mut turns,
                        SampleTurn::Reasoning(clip(&summary.join("\n"), MAX_TEXT)),
                    );
                }
            }
            kind @ ("function_call" | "custom_tool_call") => {
                let name = item
                    .get("name")
                    .and_then(|name| name.as_str())
                    .unwrap_or("tool");
                let raw = item
                    .get(if kind == "function_call" {
                        "arguments"
                    } else {
                        "input"
                    })
                    .and_then(|input| input.as_str())
                    .unwrap_or("");
                let input = codex_tool_input(raw);
                if let Some(call) = item.get("call_id").and_then(|id| id.as_str()) {
                    pending.push((call.to_string(), turns.len()));
                }
                push(
                    &mut turns,
                    SampleTurn::Tool {
                        name: if name == "exec" || name == "shell" {
                            "bash".into()
                        } else {
                            name.into()
                        },
                        input,
                        output: String::new(),
                    },
                );
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call = item.get("call_id").and_then(|id| id.as_str()).unwrap_or("");
                let output = codex_text(item.get("output"));
                if let Some(index) = pending
                    .iter()
                    .position(|(id, _)| id == call)
                    .map(|at| pending.remove(at).1)
                    && let Some(SampleTurn::Tool { output: slot, .. }) = turns.get_mut(index)
                {
                    *slot = clip(&output, MAX_OUTPUT);
                }
            }
            _ => {}
        }
    }
    Some(turns)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, lines: &[serde_json::Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let body: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
        std::fs::write(path, body.join("\n")).unwrap();
    }

    fn codex_line(payload: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "type": "response_item", "payload": payload })
    }

    #[test]
    fn codex_rollout_pairs_tool_output_and_skips_injected_context() {
        let home = tempfile::tempdir().unwrap();
        let mut lines = vec![serde_json::json!({"type":"session_meta","payload":{"id":"x"}})];
        lines.push(codex_line(serde_json::json!({"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>cwd</environment_context>"}]})));
        for n in 0..4 {
            lines.push(codex_line(serde_json::json!({"type":"message","role":"user","content":[{"type":"input_text","text":format!("fix bug {n}")}]})));
            lines.push(codex_line(serde_json::json!({"type":"custom_tool_call","call_id":format!("c{n}"),"name":"exec","input":"ls -la"})));
            lines.push(codex_line(serde_json::json!({"type":"custom_tool_call_output","call_id":format!("c{n}"),"output":[{"type":"input_text","text":"README.md"}]})));
            lines.push(codex_line(serde_json::json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done."}]})));
        }
        write(
            &home.path().join(".codex/sessions/2026/09/20/rollout.jsonl"),
            &lines,
        );
        let sample = recent_external_transcript_in(home.path()).expect("sample");
        assert_eq!(sample.source, "Codex");
        assert_eq!(sample.turns[0], SampleTurn::User("fix bug 0".into()));
        assert_eq!(
            sample.turns[1],
            SampleTurn::Tool {
                name: "bash".into(),
                input: r#"{"command":"ls -la"}"#.into(),
                output: "README.md".into()
            }
        );
        assert_eq!(sample.turns.len(), 12);
    }

    #[test]
    fn codex_exec_scripts_unwrap_to_their_command() {
        let raw = r#"const r = await tools.exec_command({"cmd":"sed -n '1,9p' a.md","yield_time_ms":10}); text(r.output);"#;
        assert_eq!(codex_tool_input(raw), r#"{"command":"sed -n '1,9p' a.md"}"#);
        assert_eq!(codex_tool_input(r#"{"path":"a"}"#), r#"{"path":"a"}"#);
    }

    #[test]
    fn short_or_missing_histories_yield_nothing() {
        let home = tempfile::tempdir().unwrap();
        assert!(recent_external_transcript_in(home.path()).is_none());
        write(
            &home.path().join(".codex/sessions/a.jsonl"),
            &[codex_line(
                serde_json::json!({"type":"message","role":"user","content":"hi"}),
            )],
        );
        assert!(recent_external_transcript_in(home.path()).is_none());
    }

    #[test]
    fn claude_transcript_attaches_tool_results_to_their_calls() {
        let home = tempfile::tempdir().unwrap();
        let mut lines = Vec::new();
        let mut parent: Option<String> = None;
        let mut entry = |kind: &str, content: serde_json::Value| {
            let uuid = format!("u{}", lines.len());
            lines.push(serde_json::json!({
                "type": kind, "uuid": uuid, "parentUuid": parent, "sessionId": "s",
                "message": {"role": kind, "content": content}
            }));
            parent = Some(uuid);
        };
        for n in 0..3 {
            entry("user", serde_json::json!(format!("task {n}")));
            entry(
                "assistant",
                serde_json::json!([
                    {"type":"thinking","thinking":"plan it","signature":"x"},
                    {"type":"tool_use","id":format!("t{n}"),"name":"Read","input":{"file_path":"a.rs"}}
                ]),
            );
            entry(
                "user",
                serde_json::json!([{"type":"tool_result","tool_use_id":format!("t{n}"),"content":"fn main() {}"}]),
            );
            entry(
                "assistant",
                serde_json::json!([{"type":"text","text":"Looks good."}]),
            );
        }
        write(&home.path().join(".claude/projects/p/s.jsonl"), &lines);
        let sample = recent_external_transcript_in(home.path()).expect("sample");
        assert_eq!(sample.source, "Claude Code");
        assert_eq!(sample.turns[1], SampleTurn::Reasoning("plan it".into()));
        assert!(
            matches!(&sample.turns[2], SampleTurn::Tool { output, .. } if output == "fn main() {}")
        );
        assert_eq!(sample.turns.len(), 12);
    }
}
