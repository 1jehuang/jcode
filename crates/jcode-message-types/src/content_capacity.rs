//! Diagnostic-only lower-bound accounting of retained transcript allocations.
//!
//! String and Vec capacities are their actual requested backing sizes, not
//! serialized lengths. JSON object keys and descendants are counted, but map
//! nodes/buckets, Arc headers, allocator rounding/metadata and fragmentation are
//! not: `known_heap_bytes` is deliberately not a total allocator-live estimate.

use crate::{ContentBlock, SharedContent};
use std::{collections::HashSet, marker::PhantomData, mem::size_of};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ContentCapacity {
    pub string_len_bytes: usize,
    pub string_capacity_bytes: usize,
    pub vec_capacity_bytes: usize,
    pub shared_backings: usize,
    pub shared_references: usize,
    /// Entries whose map-node/bucket overhead is not included in known bytes.
    pub json_object_entries: usize,
}

impl ContentCapacity {
    pub fn known_heap_bytes(&self) -> usize {
        self.string_capacity_bytes + self.vec_capacity_bytes
    }
}

/// Aggregate all retained roots in one pass to count shared payloads only once.
/// The lifetime keeps observed backing addresses alive until accounting ends.
#[derive(Default)]
pub struct ContentCapacityAccounting<'a> {
    totals: ContentCapacity,
    seen: HashSet<*const Vec<ContentBlock>>,
    retained: PhantomData<&'a SharedContent>,
}

impl<'a> ContentCapacityAccounting<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Count a root Vec's allocation, excluding the inline Vec header and its
    /// elements' child allocations. Call once per distinct owned root Vec.
    pub fn record_vec_allocation<T>(&mut self, values: &Vec<T>) {
        self.totals.vec_capacity_bytes += values.capacity() * size_of::<T>();
    }

    pub fn record_shared(&mut self, content: &'a SharedContent) {
        self.totals.shared_references += 1;
        if !self.seen.insert(&**content as *const Vec<ContentBlock>) {
            return;
        }
        self.totals.shared_backings += 1;
        self.record_vec_allocation(content);
        for block in content.iter() {
            self.record_block(block);
        }
    }

    pub fn finish(self) -> ContentCapacity {
        self.totals
    }

    fn string(&mut self, value: &String) {
        self.totals.string_len_bytes += value.len();
        self.totals.string_capacity_bytes += value.capacity();
    }

    fn optional_string(&mut self, value: &Option<String>) {
        if let Some(value) = value {
            self.string(value);
        }
    }

    fn json(&mut self, value: &serde_json::Value) {
        match value {
            serde_json::Value::String(value) => self.string(value),
            serde_json::Value::Array(values) => {
                self.record_vec_allocation(values);
                for value in values {
                    self.json(value);
                }
            }
            serde_json::Value::Object(values) => {
                self.totals.json_object_entries += values.len();
                for (key, value) in values {
                    self.string(key);
                    self.json(value);
                }
            }
            serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            }
        }
    }

    fn record_block(&mut self, block: &ContentBlock) {
        match block {
            ContentBlock::Text {
                text,
                cache_control,
            } => {
                self.string(text);
                if let Some(cache) = cache_control {
                    self.string(&cache.kind);
                    self.optional_string(&cache.ttl);
                }
            }
            ContentBlock::Reasoning { text } | ContentBlock::ReasoningTrace { text } => {
                self.string(text)
            }
            ContentBlock::AnthropicThinking {
                thinking,
                signature,
            } => {
                self.string(thinking);
                self.string(signature);
            }
            ContentBlock::OpenAIReasoning {
                id,
                summary,
                encrypted_content,
                status,
            } => {
                self.string(id);
                self.record_vec_allocation(summary);
                for text in summary {
                    self.string(text);
                }
                self.optional_string(encrypted_content);
                self.optional_string(status);
            }
            ContentBlock::ToolUse {
                id,
                name,
                input,
                thought_signature,
            } => {
                self.string(id);
                self.string(name);
                self.json(input);
                self.optional_string(thought_signature);
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                self.string(tool_use_id);
                self.string(content);
            }
            ContentBlock::Image { media_type, data } => {
                self.string(media_type);
                self.string(data);
            }
            ContentBlock::OpenAICompaction { encrypted_content } => self.string(encrypted_content),
            ContentBlock::ToolReference {
                tool_use_id,
                tool_name,
            } => {
                self.string(tool_use_id);
                self.string(tool_name);
            }
            ContentBlock::ProviderNative { provider, item } => {
                self.string(provider);
                self.json(item);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reserved(text: &str, capacity: usize) -> String {
        let mut value = String::with_capacity(capacity);
        value.push_str(text);
        value
    }

    #[test]
    fn counts_capacity_not_length_and_shared_backing_once() {
        let text = reserved("x", 4096);
        let text_capacity = text.capacity();
        let mut blocks = Vec::with_capacity(9);
        blocks.push(ContentBlock::Reasoning { text });
        let vector_capacity = blocks.capacity() * size_of::<ContentBlock>();
        let content: SharedContent = blocks.into();
        let clone = content.clone();
        let mut accounting = ContentCapacityAccounting::new();
        accounting.record_shared(&content);
        accounting.record_shared(&clone);
        let totals = accounting.finish();
        assert_eq!(totals.string_len_bytes, 1);
        assert_eq!(totals.string_capacity_bytes, text_capacity);
        assert_eq!(totals.vec_capacity_bytes, vector_capacity);
        assert_eq!(totals.known_heap_bytes(), text_capacity + vector_capacity);
        assert_eq!(totals.shared_backings, 1);
        assert_eq!(totals.shared_references, 2);
    }

    #[test]
    fn nested_json_keys_strings_and_array_spare_capacity_are_counted() {
        let key = reserved("key", 40);
        let value = reserved("value", 200);
        let mut array = Vec::with_capacity(13);
        let expected_strings = key.capacity() + value.capacity();
        let expected_array = array.capacity() * size_of::<serde_json::Value>();
        array.push(serde_json::Value::String(value));
        let mut map = serde_json::Map::new();
        map.insert(key, serde_json::Value::Array(array));
        let content: SharedContent = vec![ContentBlock::ProviderNative {
            provider: String::new(),
            item: serde_json::Value::Object(map),
        }]
        .into();
        let mut accounting = ContentCapacityAccounting::new();
        accounting.record_shared(&content);
        let totals = accounting.finish();
        assert_eq!(totals.string_len_bytes, 8);
        assert_eq!(totals.string_capacity_bytes, expected_strings);
        assert_eq!(
            totals.vec_capacity_bytes,
            content.capacity() * size_of::<ContentBlock>() + expected_array
        );
        assert_eq!(totals.json_object_entries, 1);
    }

    #[test]
    fn distinct_empty_backings_and_detached_snapshots_are_not_deduplicated() {
        let first = SharedContent::default();
        let second = SharedContent::default();
        let original: SharedContent = vec![ContentBlock::Reasoning { text: "x".into() }].into();
        let mut detached = original.clone();
        if let ContentBlock::Reasoning { text } = &mut detached[0] {
            text.reserve(2048);
        }
        let expected_strings = match (&original[0], &detached[0]) {
            (ContentBlock::Reasoning { text: a }, ContentBlock::Reasoning { text: b }) => {
                a.capacity() + b.capacity()
            }
            _ => unreachable!(),
        };
        let mut accounting = ContentCapacityAccounting::new();
        for content in [&first, &second, &original, &detached] {
            accounting.record_shared(content);
        }
        let totals = accounting.finish();
        assert_eq!(totals.shared_backings, 4);
        assert_eq!(totals.string_capacity_bytes, expected_strings);
    }
}
