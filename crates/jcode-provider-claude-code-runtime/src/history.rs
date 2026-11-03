//! Bounded checkpoints for the history owned by a retained Claude child.
//!
//! Request prefixes and native output are fingerprinted, never retained as a
//! second transcript. Presentation metadata (timestamps/durations) is excluded.

use std::collections::{HashSet, hash_map::DefaultHasher};
use std::hash::Hasher;
use std::io::Write;

use jcode_message_types::{ContentBlock, Message, Role, StreamEvent};
use serde::Serialize;

const MAX_NATIVE_TOOLS: usize = 256;
const MAX_TOOL_INPUT_BYTES: usize = 64 * 1024;

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;

#[derive(Clone, Default)]
struct Fingerprint {
    hash: DefaultHasher,
    bytes: usize,
    chunk: [u8; 32],
    used: usize,
}

impl Write for Fingerprint {
    fn write(&mut self, mut bytes: &[u8]) -> std::io::Result<usize> {
        let len = bytes.len();
        self.bytes = self.bytes.saturating_add(len);
        // Hasher::write need not be chunk-independent. Feed fixed-size chunks
        // so streamed text and the persisted assistant text compare identically.
        while !bytes.is_empty() {
            let take = (self.chunk.len() - self.used).min(bytes.len());
            self.chunk[self.used..self.used + take].copy_from_slice(&bytes[..take]);
            self.used += take;
            bytes = &bytes[take..];
            if self.used == self.chunk.len() {
                self.hash.write(&self.chunk);
                self.used = 0;
            }
        }
        Ok(len)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Fingerprint {
    fn digest(&self) -> u64 {
        let mut hash = self.hash.clone();
        hash.write(&self.chunk[..self.used]);
        hash.finish()
    }

    fn matches(&self, other: &Self) -> bool {
        self.bytes == other.bytes && self.digest() == other.digest()
    }
}

fn fingerprint(value: &impl Serialize) -> Fingerprint {
    let mut out = Fingerprint::default();
    // All callers use JSON-serializable message fields or tuples of them.
    serde_json::to_writer(&mut out, value).expect("serializable history fields");
    out
}

/// These standalone harness messages are re-created by `complete_split` and
/// memory injection, not persisted conversation turns. They still travel with
/// new input, but must not make the next stable prefix appear edited.
fn is_reminder(message: &Message) -> bool {
    message.role == Role::User
        && !message.content.is_empty()
        && message.content.iter().all(|block| matches!(
            block,
            ContentBlock::Text { text, .. } if text.trim_start().starts_with("<system-reminder>")
        ))
}

fn prefix(messages: &[Message]) -> Fingerprint {
    let mut out = Fingerprint::default();
    for message in messages.iter().filter(|message| !is_reminder(message)) {
        serde_json::to_writer(&mut out, &message.role).expect("serializable role");
        for block in &message.content {
            match block {
                ContentBlock::Text { text, .. } => {
                    // Cache breakpoints move between turns without changing text.
                    serde_json::to_writer(&mut out, &("text", text)).expect("serializable text");
                }
                ContentBlock::ReasoningTrace { .. } => {}
                block => serde_json::to_writer(&mut out, block).expect("serializable block"),
            }
        }
    }
    out
}

/// Native output can be represented in jcode history as one assistant message
/// plus tool-result messages. Compare text independent of stream chunking, and
/// recognize tool blocks without replaying already-executed tool results.
#[derive(Default)]
pub(crate) struct NativeHistory {
    text: Fingerprint,
    tools: HashSet<u64>,
    results: HashSet<u64>,
    pending_tool: Option<(String, String, String)>,
    overflow: bool,
}

impl NativeHistory {
    pub(crate) fn observe(&mut self, event: &StreamEvent) {
        match event {
            StreamEvent::TextDelta(text) => {
                let _ = self.text.write_all(text.as_bytes());
            }
            StreamEvent::ToolUseStart { id, name } => {
                if id.len() + name.len() > MAX_TOOL_INPUT_BYTES {
                    self.overflow = true;
                } else {
                    self.pending_tool = Some((id.clone(), name.clone(), String::new()));
                }
            }
            StreamEvent::ToolInputDelta(delta) => {
                if let Some((_, _, input)) = self.pending_tool.as_mut() {
                    if input.len().saturating_add(delta.len()) > MAX_TOOL_INPUT_BYTES {
                        self.overflow = true;
                        self.pending_tool = None;
                    } else {
                        input.push_str(delta);
                    }
                }
            }
            StreamEvent::ToolUseEnd => {
                if let Some((id, name, input)) = self.pending_tool.take() {
                    match serde_json::from_str::<serde_json::Value>(&input) {
                        Ok(input) if self.tools.len() < MAX_NATIVE_TOOLS => {
                            self.tools.insert(fingerprint(&(id, name, input)).digest());
                        }
                        _ => self.overflow = true,
                    }
                }
            }
            StreamEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                if self.results.len() < MAX_NATIVE_TOOLS {
                    self.results
                        .insert(fingerprint(&(tool_use_id, content, is_error)).digest());
                } else {
                    self.overflow = true;
                }
            }
            _ => {}
        }
    }

    fn recognizes(&self, messages: &[Message]) -> bool {
        if self.overflow {
            return false;
        }
        let mut text = Fingerprint::default();
        for message in messages.iter().filter(|message| !is_reminder(message)) {
            for block in &message.content {
                match block {
                    ContentBlock::Text { text: value, .. } if message.role == Role::Assistant => {
                        let _ = text.write_all(value.as_bytes());
                    }
                    ContentBlock::ToolUse {
                        id, name, input, ..
                    } => {
                        if !self
                            .tools
                            .contains(&fingerprint(&(id, name, input)).digest())
                        {
                            return false;
                        }
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        if !self.results.contains(
                            &fingerprint(&(tool_use_id, content, is_error.unwrap_or(false)))
                                .digest(),
                        ) {
                            return false;
                        }
                    }
                    ContentBlock::Reasoning { .. }
                    | ContentBlock::ReasoningTrace { .. }
                    | ContentBlock::AnthropicThinking { .. } => {}
                    _ => return false,
                }
            }
        }
        self.text.matches(&text)
    }
}

pub(crate) struct SeenHistory {
    len: usize,
    prefix: Fingerprint,
    native: NativeHistory,
}

impl SeenHistory {
    pub(crate) fn new(messages: &[Message], native: NativeHistory) -> Self {
        Self {
            len: messages
                .iter()
                .filter(|message| !is_reminder(message))
                .count(),
            prefix: prefix(messages),
            native,
        }
    }

    /// First turn the child has not seen. A changed/truncated prefix or changed
    /// native response cannot safely be appended to the retained CLI session.
    pub(crate) fn missing_start(&self, messages: &[Message]) -> Result<usize, ()> {
        let prefix_end = if self.len == 0 {
            0
        } else {
            messages
                .iter()
                .enumerate()
                .filter(|(_, message)| !is_reminder(message))
                .nth(self.len - 1)
                .map(|(index, _)| index + 1)
                .ok_or(())?
        };
        if !self.prefix.matches(&prefix(&messages[..prefix_end])) {
            return Err(());
        }
        let end = messages[prefix_end..]
            .iter()
            .position(|message| {
                message.role == Role::User
                    && !is_reminder(message)
                    && !message
                        .content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
                    && message.content.iter().any(|b| {
                        matches!(b, ContentBlock::Text { .. } | ContentBlock::Image { .. })
                    })
            })
            .map_or(messages.len(), |offset| prefix_end + offset);
        self.native
            .recognizes(&messages[prefix_end..end])
            .then_some(end)
            .ok_or(())
    }
}
