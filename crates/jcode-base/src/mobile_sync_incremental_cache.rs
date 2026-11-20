//! Reuse the exact wire hasher state of an uploaded prefix, without retaining
//! transcript payloads. Source validation hashes borrowed fields directly, so
//! it does not allocate JSON, truncate old tool results, or decode old images.
use super::*;
use std::collections::{VecDeque, hash_map::DefaultHasher};

const MAX_SESSIONS: usize = 32;
static CACHE: Mutex<VecDeque<Entry>> = Mutex::new(VecDeque::new());

#[derive(Clone)]
struct Entry {
    id: String,
    count: usize,
    source: u64,
    wire: std::collections::hash_map::DefaultHasher,
}

#[derive(Clone, Default)]
struct SourceHasher(DefaultHasher);
impl Hasher for SourceHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }
    fn finish(&self) -> u64 {
        self.0.finish()
    }
}
impl SourceHasher {
    fn json(&mut self, value: &serde_json::Value) -> bool {
        let mut budget = MAX_TOOL_INPUT_BYTES * 4;
        self.bounded_json(value, &mut budget)
    }

    fn bounded_json(&mut self, value: &serde_json::Value, budget: &mut usize) -> bool {
        use serde_json::Value;
        let cost = match value {
            Value::String(text) => text.len().saturating_add(16),
            _ => 16,
        };
        let Some(remaining) = budget.checked_sub(cost) else {
            return false;
        };
        *budget = remaining;
        match value {
            Value::Null => 0u8.hash(self),
            Value::Bool(value) => {
                1u8.hash(self);
                value.hash(self);
            }
            Value::Number(value) => {
                2u8.hash(self);
                value.to_string().hash(self);
            }
            Value::String(value) => {
                if value.contains("![") {
                    return false;
                }
                3u8.hash(self);
                value.hash(self);
            }
            Value::Array(values) => {
                4u8.hash(self);
                values.len().hash(self);
                for value in values {
                    if !self.bounded_json(value, budget) {
                        return false;
                    }
                }
            }
            Value::Object(values) => {
                5u8.hash(self);
                values.len().hash(self);
                for (key, value) in values {
                    let Some(remaining) = budget.checked_sub(key.len()) else {
                        return false;
                    };
                    *budget = remaining;
                    if key.contains("![") {
                        return false;
                    }
                    key.hash(self);
                    if !self.bounded_json(value, budget) {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn digest(&self) -> u64 {
        self.0.finish()
    }

    fn extend(&mut self, messages: &[crate::session::StoredMessage], start: usize) -> bool {
        // Wire prose is bounded. Do not hash megabytes of invisible text just
        // to avoid converting a cheap bounded prefix. Admission inspects lengths
        // before any payload work, including external-image marker scans.
        if messages.iter().flat_map(|message| message.content.iter()).any(|block| {
            matches!(block, ContentBlock::Text { text, .. } if text.len() > MAX_TEXT_CHARS * 4)
        }) {
            return false;
        }
        for (offset, message) in messages.iter().enumerate() {
            // Generated images are external mutable files, including missing
            // files that may appear later. Always use the uncached path if an
            // assistant message could reference one. Attachment bytes are inline.
            if matches!(message.role, Role::Assistant)
                && message.content.iter().any(|block| match block {
                    ContentBlock::Text { text, .. } => text.contains("!["),
                    ContentBlock::ToolUse { id, name, .. } => {
                        id.contains("![") || name.contains("![")
                    }
                    _ => false,
                })
            {
                return false;
            }
            (start + offset).hash(self);
            matches!(message.role, Role::Assistant).hash(self);
            message.display_role.is_some().hash(self);
            message.timestamp.hash(self);
            for block in &message.content {
                // Match every variant explicitly so a new wire-visible kind
                // cannot silently bypass invalidation in a future change.
                match block {
                    ContentBlock::Text { text, .. } => {
                        1u8.hash(self);
                        text.hash(self);
                    }
                    ContentBlock::ToolUse {
                        id, name, input, ..
                    } => {
                        2u8.hash(self);
                        id.hash(self);
                        name.hash(self);
                        if !self.json(input) {
                            return false;
                        }
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        3u8.hash(self);
                        tool_use_id.hash(self);
                        // Four bytes per scalar plus one full scalar cover
                        // every byte that can affect the truncated wire result,
                        // including whether the truncation marker is present.
                        let end = content.len().min(MAX_TOOL_RESULT_CHARS * 4 + 4);
                        content.as_bytes()[..end].hash(self);
                        is_error.hash(self);
                    }
                    ContentBlock::Image { media_type, data } => {
                        4u8.hash(self);
                        media_type.hash(self);
                        data.hash(self);
                    }
                    ContentBlock::Reasoning { .. }
                    | ContentBlock::ReasoningTrace { .. }
                    | ContentBlock::AnthropicThinking { .. }
                    | ContentBlock::OpenAIReasoning { .. }
                    | ContentBlock::OpenAICompaction { .. }
                    | ContentBlock::ToolReference { .. }
                    | ContentBlock::ProviderNative { .. } => {}
                }
            }
            0u8.hash(self);
        }
        true
    }
}

#[derive(Default)]
pub(super) struct PrefixCache(VecDeque<Entry>);
impl PrefixCache {
    fn remember(
        &mut self,
        session: &Session,
        source: Option<SourceHasher>,
        wire: std::collections::hash_map::DefaultHasher,
    ) {
        self.0.retain(|entry| entry.id != session.id);
        // Retain only bounded IDs, a digest, count, and opaque hasher state.
        // No source content, thumbnails, or wire messages survive this call.
        if session.id.len() > 128 {
            return;
        }
        if let Some(source) = source {
            while self.0.len() >= MAX_SESSIONS {
                self.0.pop_front();
            }
            self.0.push_back(Entry {
                id: session.id.clone(),
                count: session.messages.len(),
                source: source.digest(),
                wire,
            });
        }
    }

    pub(super) fn messages(
        &mut self,
        session: &Session,
        progress: &SessionProgress,
    ) -> (Vec<WireMessage>, u64) {
        if progress.wire_version == WIRE_VERSION
            && progress.uploaded <= session.messages.len()
            && let Some(entry) = self
                .0
                .iter()
                .find(|entry| {
                    entry.id == session.id
                        && entry.count == progress.uploaded
                        && Some(entry.wire.finish()) == progress.fingerprint
                })
                .cloned()
        {
            let mut source = SourceHasher::default();
            if source.extend(&session.messages[..progress.uploaded], 0)
                && source.digest() == entry.source
            {
                let messages = wire_messages(session, progress.uploaded);
                let mut wire = entry.wire;
                for message in &messages {
                    message.hash(&mut wire);
                }
                let fingerprint = wire.finish();
                let cacheable =
                    source.extend(&session.messages[progress.uploaded..], progress.uploaded);
                self.remember(session, cacheable.then_some(source), wire);
                return (messages, fingerprint);
            }
        }
        let mut messages = wire_messages(session, 0);
        let mut wire = std::collections::hash_map::DefaultHasher::new();
        for message in &messages {
            message.hash(&mut wire);
        }
        let fingerprint = wire.finish();
        let unchanged_prefix = progress.wire_version == WIRE_VERSION
            && progress.uploaded <= session.messages.len()
            && progress.fingerprint == Some(wire_fingerprint(&messages, progress.uploaded));
        let mut source = SourceHasher::default();
        let cacheable = source.extend(&session.messages, 0);
        self.remember(session, cacheable.then_some(source), wire);
        if unchanged_prefix {
            messages.retain(|message| message.seq >= progress.uploaded);
        }
        (messages, fingerprint)
    }
}

pub(super) fn messages(session: &Session, progress: &SessionProgress) -> (Vec<WireMessage>, u64) {
    // The sync pass is already serialized. Take the tiny cache out temporarily
    // so conversion never holds a global mutex during image decoding or I/O.
    let entries = std::mem::take(&mut *CACHE.lock().unwrap_or_else(|e| e.into_inner()));
    let mut cache = PrefixCache(entries);
    let result = cache.messages(session, progress);
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = cache.0;
    result
}

#[cfg(test)]
#[path = "mobile_sync_incremental_cache_tests.rs"]
mod tests;
