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
/// A replay needs a few real prompts, not one prompt and a tool storm.
const MIN_USER_TURNS: usize = 2;
const MAX_TURNS: usize = 80;
const MAX_TEXT: usize = 4_000;
const MAX_OUTPUT: usize = 1_500;
/// Recent files considered per harness. Bounded so onboarding stays fast on
/// large histories.
const SCAN_FILES: usize = 40;
/// Only the largest few recent files are parsed. Size is a cheap proxy for
/// length, and parsing decides.
const PARSE_FILES: usize = 10;
const MAX_FILE_BYTES: u64 = 24 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    ClaudeCode,
    Codex,
    Cursor,
    Pi,
    Jcode,
}

impl Format {
    fn label(self) -> &'static str {
        match self {
            Format::ClaudeCode => "Claude Code",
            Format::Codex => "Codex",
            Format::Cursor => "Cursor",
            Format::Pi => "Pi",
            Format::Jcode => "Jcode",
        }
    }
}

/// A parsed session. `total` counts every turn, before the replay cap, so
/// long sessions outrank ones that merely fill the cap.
struct Parsed {
    turns: Vec<SampleTurn>,
    total: usize,
    users: usize,
}

/// The longest recent transcript from any harness on this machine, including
/// Jcode's own sessions. Nothing is written or sent.
pub fn recent_external_transcript() -> Option<TranscriptSample> {
    let home = crate::storage::user_home_path("").ok()?;
    recent_external_transcript_in(&home)
}

pub fn recent_external_transcript_in(home: &Path) -> Option<TranscriptSample> {
    let mut candidates: Vec<(u64, std::time::SystemTime, Format, std::path::PathBuf)> = Vec::new();
    for (format, dir, extension) in [
        (Format::ClaudeCode, home.join(".claude/projects"), "jsonl"),
        (Format::Codex, home.join(".codex/sessions"), "jsonl"),
        (Format::Cursor, home.join(".cursor/projects"), "jsonl"),
        (Format::Pi, home.join(".pi/agent/sessions"), "jsonl"),
        (Format::Jcode, home.join(".jcode/sessions"), "json"),
    ] {
        for path in collect_recent_files_recursive(&dir, extension, SCAN_FILES) {
            if format == Format::Cursor && !is_top_level_cursor_transcript(&path) {
                continue;
            }
            if format == Format::Jcode
                && !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("session_"))
            {
                continue;
            }
            let Ok(meta) = path.metadata() else { continue };
            if meta.len() == 0 || meta.len() > MAX_FILE_BYTES {
                continue;
            }
            let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            candidates.push((meta.len(), modified, format, path));
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    candidates
        .into_iter()
        .take(PARSE_FILES)
        .filter_map(|(_, modified, format, path)| {
            let parsed = match format {
                Format::ClaudeCode => claude_turns(&path),
                Format::Codex => codex_turns(&path),
                Format::Cursor => cursor_turns(&path),
                Format::Pi => pi_turns(&path),
                Format::Jcode => jcode_turns(&path),
            }?;
            (parsed.turns.len() >= MIN_TURNS && parsed.users >= MIN_USER_TURNS)
                .then_some((parsed, modified, format))
        })
        .max_by(|a, b| a.0.total.cmp(&b.0.total).then(a.1.cmp(&b.1)))
        .map(|(parsed, _, format)| TranscriptSample {
            source: format.label(),
            turns: parsed.turns,
        })
}

/// Cursor nests subagent runs under `subagents/`. Only whole sessions replay.
fn is_top_level_cursor_transcript(path: &Path) -> bool {
    path.components()
        .any(|part| part.as_os_str() == "agent-transcripts")
        && !path
            .parent()
            .and_then(|dir| dir.file_name())
            .is_some_and(|name| name == "subagents")
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

impl Parsed {
    fn new() -> Self {
        Self {
            turns: Vec::new(),
            total: 0,
            users: 0,
        }
    }

    /// Record a turn, returning its index when it was kept for the replay.
    fn push(&mut self, turn: SampleTurn) -> Option<usize> {
        self.total += 1;
        if matches!(turn, SampleTurn::User(_)) {
            self.users += 1;
        }
        (self.turns.len() < MAX_TURNS).then(|| {
            self.turns.push(turn);
            self.turns.len() - 1
        })
    }

    fn set_output(&mut self, index: usize, text: &str) {
        if let Some(SampleTurn::Tool { output, .. }) = self.turns.get_mut(index) {
            *output = clip(text, MAX_OUTPUT);
        }
    }
}

/// Tool calls waiting for their results, keyed by call id.
#[derive(Default)]
struct Pending(Vec<(String, usize)>);

impl Pending {
    fn add(&mut self, id: Option<&str>, index: Option<usize>) {
        if let (Some(id), Some(index)) = (id, index) {
            self.0.push((id.to_string(), index));
        }
    }

    fn take(&mut self, id: &str) -> Option<usize> {
        let at = self.0.iter().position(|(pending, _)| pending == id)?;
        Some(self.0.remove(at).1)
    }
}

fn text_turn(user: bool, text: &str) -> SampleTurn {
    if user {
        SampleTurn::User(clip(text, MAX_TEXT))
    } else {
        SampleTurn::Assistant(clip(text, MAX_TEXT))
    }
}

/// Text of a tool result, which harnesses store as a string or text blocks.
fn result_text(value: Option<&serde_json::Value>) -> String {
    codex_text(value)
}

/// Tool arguments as JSON, without the null placeholders some harnesses
/// write for every optional parameter.
fn tool_args(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::Object(map)) => serde_json::Value::Object(
            map.iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
        .to_string(),
        Some(value) => value.to_string(),
        None => "{}".into(),
    }
}

/// Anthropic-style content blocks, shared by Cursor and Jcode sessions.
fn anthropic_blocks(
    parsed: &mut Parsed,
    pending: &mut Pending,
    user: bool,
    content: Option<&serde_json::Value>,
) {
    let blocks = match content {
        Some(serde_json::Value::String(text)) => {
            if !is_synthetic(text) {
                parsed.push(text_turn(user, text));
            }
            return;
        }
        Some(serde_json::Value::Array(blocks)) => blocks,
        _ => return,
    };
    let str_field = |block: &serde_json::Value, key: &str| {
        block
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::to_owned)
    };
    for block in blocks {
        match block
            .get("type")
            .and_then(|kind| kind.as_str())
            .unwrap_or("")
        {
            "text" => {
                if let Some(text) = str_field(block, "text").filter(|text| !is_synthetic(text)) {
                    parsed.push(text_turn(user, &text));
                }
            }
            "thinking" | "reasoning" | "reasoning_trace" => {
                if let Some(text) = str_field(block, "thinking")
                    .or_else(|| str_field(block, "text"))
                    .filter(|text| !text.trim().is_empty())
                {
                    parsed.push(SampleTurn::Reasoning(clip(&text, MAX_TEXT)));
                }
            }
            "tool_use" => {
                let index = parsed.push(SampleTurn::Tool {
                    name: str_field(block, "name").unwrap_or_else(|| "tool".into()),
                    input: tool_args(block.get("input")),
                    output: String::new(),
                });
                pending.add(block.get("id").and_then(|id| id.as_str()), index);
            }
            "tool_result" => {
                let id = str_field(block, "tool_use_id").unwrap_or_default();
                if let Some(index) = pending.take(&id) {
                    parsed.set_output(index, &result_text(block.get("content")));
                }
            }
            _ => {}
        }
    }
}

fn claude_turns(path: &Path) -> Option<Parsed> {
    let file = std::fs::File::open(path).ok()?;
    let entries: Vec<ClaudeCodeEntry> = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str(&line).ok())
        .collect();
    let mut parsed = Parsed::new();
    // Tool results arrive in the following user message, keyed by call id.
    let mut pending = Pending::default();
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
                ClaudeCodeContentBlock::Text { text } if !is_synthetic(&text) => {
                    parsed.push(text_turn(user, &text));
                }
                ClaudeCodeContentBlock::Thinking { thinking, .. }
                    if !thinking.trim().is_empty() =>
                {
                    parsed.push(SampleTurn::Reasoning(clip(&thinking, MAX_TEXT)));
                }
                ClaudeCodeContentBlock::ToolUse { id, name, input } => {
                    let index = parsed.push(SampleTurn::Tool {
                        name,
                        input: input.to_string(),
                        output: String::new(),
                    });
                    pending.add(Some(&id), index);
                }
                ClaudeCodeContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    if let Some(index) = pending.take(&tool_use_id) {
                        parsed.set_output(index, &content);
                    }
                }
                _ => {}
            }
        }
    }
    Some(parsed)
}

/// Cursor agent transcripts: one `{role, message: {content: [...]}}` per line.
fn cursor_turns(path: &Path) -> Option<Parsed> {
    let file = std::fs::File::open(path).ok()?;
    let mut parsed = Parsed::new();
    let mut pending = Pending::default();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let user = match value.get("role").and_then(|role| role.as_str()) {
            Some("user" | "human") => true,
            Some("assistant" | "model") => false,
            _ => continue,
        };
        let content = value
            .get("message")
            .and_then(|message| message.get("content"))
            .or_else(|| value.get("content"));
        anthropic_blocks(&mut parsed, &mut pending, user, content);
    }
    Some(parsed)
}

/// Pi sessions: `{type: "message", message: {role, content: [...]}}` lines,
/// with tool calls as `toolCall` blocks and results as `toolResult` messages.
fn pi_turns(path: &Path) -> Option<Parsed> {
    let file = std::fs::File::open(path).ok()?;
    let mut parsed = Parsed::new();
    let mut pending = Pending::default();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(|kind| kind.as_str()) != Some("message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let role = message
            .get("role")
            .and_then(|role| role.as_str())
            .unwrap_or("");
        let content = message.get("content");
        if role == "toolResult" {
            let id = message
                .get("toolCallId")
                .and_then(|id| id.as_str())
                .unwrap_or("");
            if let Some(index) = pending.take(id) {
                parsed.set_output(index, &result_text(content));
            }
            continue;
        }
        let user = match role {
            "user" => true,
            "assistant" => false,
            _ => continue,
        };
        let Some(serde_json::Value::Array(blocks)) = content else {
            anthropic_blocks(&mut parsed, &mut pending, user, content);
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(|kind| kind.as_str()) == Some("toolCall") {
                let index = parsed.push(SampleTurn::Tool {
                    name: block
                        .get("name")
                        .and_then(|name| name.as_str())
                        .unwrap_or("tool")
                        .to_string(),
                    input: tool_args(block.get("arguments")),
                    output: String::new(),
                });
                pending.add(block.get("id").and_then(|id| id.as_str()), index);
            } else {
                anthropic_blocks(
                    &mut parsed,
                    &mut pending,
                    user,
                    Some(&serde_json::Value::Array(vec![block.clone()])),
                );
            }
        }
    }
    Some(parsed)
}

/// Jcode's own saved sessions. Debug, canary and child (subagent) sessions
/// are skipped, as are system-display messages such as session context.
fn jcode_turns(path: &Path) -> Option<Parsed> {
    let file = std::fs::File::open(path).ok()?;
    let session: serde_json::Value = serde_json::from_reader(BufReader::new(file)).ok()?;
    let flag = |key: &str| {
        session
            .get(key)
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    };
    if flag("is_debug")
        || flag("is_canary")
        || session.get("parent_id").is_some_and(|id| !id.is_null())
    {
        return None;
    }
    let mut parsed = Parsed::new();
    let mut pending = Pending::default();
    for message in session.get("messages")?.as_array()? {
        if message.get("display_role").and_then(|role| role.as_str()) == Some("system") {
            continue;
        }
        let user = match message.get("role").and_then(|role| role.as_str()) {
            Some("user") => true,
            Some("assistant") => false,
            _ => continue,
        };
        anthropic_blocks(&mut parsed, &mut pending, user, message.get("content"));
    }
    Some(parsed)
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

fn codex_turns(path: &Path) -> Option<Parsed> {
    let file = std::fs::File::open(path).ok()?;
    let mut parsed = Parsed::new();
    let mut pending = Pending::default();
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
                    Some("user") => {
                        parsed.push(text_turn(true, &text));
                    }
                    Some("assistant") => {
                        parsed.push(text_turn(false, &text));
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
                    parsed.push(SampleTurn::Reasoning(clip(&summary.join("\n"), MAX_TEXT)));
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
                let index = parsed.push(SampleTurn::Tool {
                    name: if name == "exec" || name == "shell" {
                        "bash".into()
                    } else {
                        name.into()
                    },
                    input,
                    output: String::new(),
                });
                pending.add(item.get("call_id").and_then(|id| id.as_str()), index);
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call = item.get("call_id").and_then(|id| id.as_str()).unwrap_or("");
                let output = codex_text(item.get("output"));
                if let Some(index) = pending.take(call) {
                    parsed.set_output(index, &output);
                }
            }
            _ => {}
        }
    }
    Some(parsed)
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

    fn jcode_session(dir: &Path, name: &str, prompts: usize, extra: serde_json::Value) {
        let mut messages = vec![serde_json::json!({
            "role": "user", "display_role": "system",
            "content": [{"type": "text", "text": "Session Context"}]
        })];
        for n in 0..prompts {
            messages.push(serde_json::json!({"role":"user","content":[{"type":"text","text":format!("jcode task {n}")}]}));
            messages.push(serde_json::json!({"role":"assistant","content":[
                {"type":"reasoning_trace","text":"think"},
                {"type":"tool_use","id":format!("j{n}"),"name":"read","input":{"file_path":"a.rs","limit":null}}
            ]}));
            messages.push(serde_json::json!({"role":"user","content":[{"type":"tool_result","tool_use_id":format!("j{n}"),"content":"ok"}]}));
            messages.push(
                serde_json::json!({"role":"assistant","content":[{"type":"text","text":"done"}]}),
            );
        }
        let mut session = serde_json::json!({"id": name, "parent_id": null, "messages": messages});
        for (key, value) in extra.as_object().unwrap() {
            session[key] = value.clone();
        }
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("{name}.json")), session.to_string()).unwrap();
    }

    #[test]
    fn jcode_sessions_replay_and_skip_debug_children_and_context() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".jcode/sessions");
        jcode_session(&dir, "session_a", 3, serde_json::json!({}));
        // Longer, but debug and subagent sessions never replay.
        jcode_session(
            &dir,
            "session_debug",
            20,
            serde_json::json!({"is_debug": true}),
        );
        jcode_session(
            &dir,
            "session_child",
            20,
            serde_json::json!({"parent_id": "session_a"}),
        );
        std::fs::write(dir.join("notes.json"), "{}").unwrap();
        let sample = recent_external_transcript_in(home.path()).expect("sample");
        assert_eq!(sample.source, "Jcode");
        assert_eq!(sample.turns[0], SampleTurn::User("jcode task 0".into()));
        assert_eq!(sample.turns[1], SampleTurn::Reasoning("think".into()));
        assert_eq!(
            sample.turns[2],
            SampleTurn::Tool {
                name: "read".into(),
                input: r#"{"file_path":"a.rs"}"#.into(),
                output: "ok".into()
            }
        );
        assert_eq!(sample.turns.len(), 12);
    }

    #[test]
    fn the_longest_session_wins_over_the_newest_across_harnesses() {
        let home = tempfile::tempdir().unwrap();
        jcode_session(
            &home.path().join(".jcode/sessions"),
            "session_long",
            30,
            serde_json::json!({}),
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        // Newer, but short.
        let mut lines = Vec::new();
        for n in 0..3 {
            lines.push(codex_line(
                serde_json::json!({"type":"message","role":"user","content":format!("codex {n}")}),
            ));
            lines.push(codex_line(
                serde_json::json!({"type":"message","role":"assistant","content":"ok"}),
            ));
            lines.push(codex_line(
                serde_json::json!({"type":"message","role":"assistant","content":"more"}),
            ));
        }
        write(&home.path().join(".codex/sessions/new.jsonl"), &lines);
        let sample = recent_external_transcript_in(home.path()).expect("sample");
        assert_eq!(sample.source, "Jcode");
        // The replay is capped, the ranking is not.
        assert_eq!(sample.turns.len(), MAX_TURNS);
    }

    #[test]
    fn a_single_prompt_tool_storm_is_not_a_conversation() {
        let home = tempfile::tempdir().unwrap();
        let mut lines = vec![codex_line(
            serde_json::json!({"type":"message","role":"user","content":"go"}),
        )];
        for n in 0..20 {
            lines.push(codex_line(serde_json::json!({"type":"function_call","call_id":format!("c{n}"),"name":"shell","arguments":"{}"})));
        }
        write(&home.path().join(".codex/sessions/storm.jsonl"), &lines);
        assert!(recent_external_transcript_in(home.path()).is_none());
    }

    #[test]
    fn pi_and_cursor_sessions_replay() {
        let home = tempfile::tempdir().unwrap();
        let mut pi = vec![serde_json::json!({"type":"session","id":"p","cwd":"/x"})];
        for n in 0..3 {
            pi.push(serde_json::json!({"type":"message","message":{"role":"user","content":[{"type":"text","text":format!("pi {n}")}]}}));
            pi.push(serde_json::json!({"type":"message","message":{"role":"assistant","content":[
                {"type":"thinking","thinking":"hm"},
                {"type":"toolCall","id":format!("p{n}"),"name":"bash","arguments":{"command":"ls"}}
            ]}}));
            pi.push(serde_json::json!({"type":"message","message":{"role":"toolResult","toolCallId":format!("p{n}"),"content":[{"type":"text","text":"a.rs"}]}}));
            pi.push(serde_json::json!({"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"ok"}]}}));
        }
        write(&home.path().join(".pi/agent/sessions/x/s.jsonl"), &pi);
        let sample = recent_external_transcript_in(home.path()).expect("pi sample");
        assert_eq!(sample.source, "Pi");
        assert_eq!(
            sample.turns[2],
            SampleTurn::Tool {
                name: "bash".into(),
                input: r#"{"command":"ls"}"#.into(),
                output: "a.rs".into()
            }
        );

        let home = tempfile::tempdir().unwrap();
        let mut cursor = Vec::new();
        for n in 0..5 {
            cursor.push(serde_json::json!({"role":"user","message":{"content":[{"type":"text","text":format!("cursor {n}")}]}}));
            cursor.push(serde_json::json!({"role":"assistant","message":{"content":[{"type":"text","text":"sure"}]}}));
        }
        let root = home.path().join(".cursor/projects/p/agent-transcripts/s");
        write(&root.join("s.jsonl"), &cursor);
        // Subagent runs are not whole sessions, even when longer.
        let mut sub = cursor.clone();
        sub.extend(cursor.clone());
        write(&root.join("subagents/c.jsonl"), &sub);
        let sample = recent_external_transcript_in(home.path()).expect("cursor sample");
        assert_eq!(sample.source, "Cursor");
        assert_eq!(sample.turns.len(), 10);
    }
}
