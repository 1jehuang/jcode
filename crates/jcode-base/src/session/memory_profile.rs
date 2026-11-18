use crate::message::ContentBlock;
use crate::process_memory::estimate_json_bytes;
use serde::Serialize;
use std::sync::{Arc, Mutex, Weak};

/// A weak identity keeps allocation reuse and in-place mutation from producing
/// stale counts, without retaining the summary or encrypted payload. With a
/// weak observer, `Arc::get_mut` fails and `Arc::make_mut` changes identity.
#[derive(Debug, Default)]
pub(super) struct CompactionJsonBytesCache(
    Mutex<Option<(Weak<super::StoredCompactionState>, usize)>>,
);

impl Clone for CompactionJsonBytesCache {
    fn clone(&self) -> Self {
        Self(Mutex::new(
            self.0.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        ))
    }
}

impl CompactionJsonBytesCache {
    pub(super) fn bytes(&self, state: Option<&Arc<super::StoredCompactionState>>) -> usize {
        let mut cached = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(state) = state else {
            *cached = None;
            return 0;
        };
        if let Some((identity, bytes)) = cached.as_ref()
            && std::ptr::eq(identity.as_ptr(), Arc::as_ptr(state))
        {
            return *bytes;
        }
        let bytes = estimate_json_bytes(state);
        *cached = Some((Arc::downgrade(state), bytes));
        bytes
    }
}

const LARGE_MEMORY_BLOB_THRESHOLD_BYTES: usize = 16 * 1024;

#[path = "stored_message_metadata.rs"]
mod stored_message_metadata;

pub(super) fn stored_message_json_bytes(message: &super::StoredMessage) -> usize {
    stored_message_metadata::json_bytes(message)
}

#[cfg(test)]
mod cached_size_tests {
    use super::*;

    fn compaction() -> Arc<super::super::StoredCompactionState> {
        Arc::new(super::super::StoredCompactionState {
            summary_text: "escaped\n\"界🦀".repeat(1024),
            openai_encrypted_content: Some("encrypted\\\"\n".repeat(128 * 1024).into()),
            covers_up_to_turn: 3,
            original_turn_count: 7,
            compacted_count: 2,
        })
    }

    fn assert_exact(session: &mut super::super::Session) {
        let expected = session
            .compaction
            .as_ref()
            .map(estimate_json_bytes)
            .unwrap_or(0);
        assert_eq!(session.memory_profile_snapshot().total_json_bytes, expected);
        assert_eq!(
            session.debug_memory_profile()["totals"]["json_bytes"].as_u64(),
            Some(expected as u64)
        );
    }

    #[test]
    fn compaction_accounting_mutation_replacement_clone_and_clear() {
        let mut session = super::super::Session::create_with_id("accounting".into(), None, None);
        session.compaction = Some(compaction());
        for _ in 0..3 {
            assert_exact(&mut session);
        }
        assert!(Arc::get_mut(session.compaction.as_mut().unwrap()).is_none());
        let identity = Arc::as_ptr(session.compaction.as_ref().unwrap());
        Arc::make_mut(session.compaction.as_mut().unwrap())
            .summary_text
            .push_str(" changed");
        assert_ne!(identity, Arc::as_ptr(session.compaction.as_ref().unwrap()));
        assert_exact(&mut session);
        let mut cloned = session.clone();
        let state = Arc::make_mut(cloned.compaction.as_mut().unwrap());
        state.openai_encrypted_content = None;
        state.covers_up_to_turn = usize::MAX;
        assert_exact(&mut cloned);
        assert_exact(&mut session);
        session.compaction = Some(compaction());
        assert_exact(&mut session);
        session.mark_memory_profile_dirty();
        assert_exact(&mut session);
        session.compaction = None;
        assert_exact(&mut session);
        let mut restored = serde_json::from_slice(&serde_json::to_vec(&cloned).unwrap()).unwrap();
        assert_exact(&mut restored);
    }

    #[test]
    fn compaction_accounting_drops_payload_and_bounds_weak_retention() {
        let cache = CompactionJsonBytesCache::default();
        let first = compaction();
        cache.bytes(Some(&first));
        assert_eq!(Arc::weak_count(&first), 1);
        let second = compaction();
        cache.bytes(Some(&second));
        assert_eq!(Arc::weak_count(&first), 0, "only one identity is cached");
        assert_eq!(Arc::weak_count(&second), 1);
        cache.bytes(None);
        assert_eq!(Arc::weak_count(&second), 0);
        for _ in 0..8 {
            let state = compaction();
            let payload = Arc::downgrade(state.openai_encrypted_content.as_ref().unwrap());
            assert_eq!(cache.bytes(Some(&state)), estimate_json_bytes(&state));
            let previous = Arc::downgrade(&state);
            assert_eq!(Arc::strong_count(&state), 1);
            drop(state);
            assert!(previous.upgrade().is_none());
            assert!(payload.upgrade().is_none());
        }
        assert_eq!(cache.bytes(None), 0);
    }

    /// `justrust test --remote -p jcode-base --lib --profile selfdev compaction_accounting_microprobe -- --ignored --nocapture`
    /// Compare the old repeated serialization with the complete warm snapshot
    /// path. This is an accounting microbenchmark, not end-to-end server latency.
    #[test]
    #[ignore]
    fn compaction_accounting_microprobe() {
        use std::{hint::black_box, time::Instant};
        let mut session =
            super::super::Session::create_with_id("accounting_bench".into(), None, None);
        session.compaction = Some(compaction());
        let expected = estimate_json_bytes(session.compaction.as_ref().unwrap());
        assert_exact(&mut session);
        let iterations = 2000;
        let start = Instant::now();
        for _ in 0..iterations {
            assert_eq!(
                black_box(estimate_json_bytes(black_box(
                    session.compaction.as_ref().unwrap()
                ))),
                expected
            );
        }
        let baseline = start.elapsed();
        let start = Instant::now();
        for _ in 0..iterations {
            assert_eq!(
                black_box(session.memory_profile_snapshot()).total_json_bytes,
                expected
            );
        }
        let cached = start.elapsed();
        eprintln!(
            "compaction accounting: json_bytes={expected} iterations={iterations} baseline={baseline:?} cached_snapshot={cached:?} speedup={:.2}x",
            baseline.as_secs_f64() / cached.as_secs_f64()
        );
    }

    #[test]
    fn stored_sizes_match_full_serialization_after_metadata_and_content_changes() {
        let mut message = super::super::StoredMessage {
            id: "escaped\"id".into(),
            role: crate::message::Role::User,
            content: vec![ContentBlock::Text {
                text: "long\n\"界🦀".repeat(1024),
                cache_control: None,
            }]
            .into(),
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        };
        for step in 0..4 {
            assert_eq!(
                stored_message_json_bytes(&message),
                estimate_json_bytes(&message)
            );
            assert_eq!(
                stored_message_json_bytes(&message.clone()),
                estimate_json_bytes(&message)
            );
            match step {
                0 => message.tool_duration_ms = Some(123),
                1 => message.timestamp = Some(chrono::DateTime::from_timestamp(99, 0).unwrap()),
                2 => message.content.clear(),
                _ => {}
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct ContentBlockMemoryStats {
    pub(super) block_count: usize,
    pub(super) text_blocks: usize,
    pub(super) text_bytes: usize,
    pub(super) reasoning_blocks: usize,
    pub(super) reasoning_bytes: usize,
    pub(super) tool_use_blocks: usize,
    pub(super) tool_use_input_json_bytes: usize,
    pub(super) tool_result_blocks: usize,
    pub(super) tool_result_bytes: usize,
    pub(super) image_blocks: usize,
    pub(super) image_data_bytes: usize,
    pub(super) openai_compaction_blocks: usize,
    pub(super) openai_compaction_bytes: usize,
    pub(super) large_block_count: usize,
    pub(super) large_block_bytes: usize,
    pub(super) large_tool_result_count: usize,
    pub(super) large_tool_result_bytes: usize,
    pub(super) max_block_bytes: usize,
    pub(super) max_tool_result_bytes: usize,
}

impl ContentBlockMemoryStats {
    pub(super) fn merge_from(&mut self, other: &Self) {
        self.block_count += other.block_count;
        self.text_blocks += other.text_blocks;
        self.text_bytes += other.text_bytes;
        self.reasoning_blocks += other.reasoning_blocks;
        self.reasoning_bytes += other.reasoning_bytes;
        self.tool_use_blocks += other.tool_use_blocks;
        self.tool_use_input_json_bytes += other.tool_use_input_json_bytes;
        self.tool_result_blocks += other.tool_result_blocks;
        self.tool_result_bytes += other.tool_result_bytes;
        self.image_blocks += other.image_blocks;
        self.image_data_bytes += other.image_data_bytes;
        self.openai_compaction_blocks += other.openai_compaction_blocks;
        self.openai_compaction_bytes += other.openai_compaction_bytes;
        self.large_block_count += other.large_block_count;
        self.large_block_bytes += other.large_block_bytes;
        self.large_tool_result_count += other.large_tool_result_count;
        self.large_tool_result_bytes += other.large_tool_result_bytes;
        self.max_block_bytes = self.max_block_bytes.max(other.max_block_bytes);
        self.max_tool_result_bytes = self.max_tool_result_bytes.max(other.max_tool_result_bytes);
    }

    fn record_bytes(&mut self, bytes: usize) {
        self.max_block_bytes = self.max_block_bytes.max(bytes);
        if bytes >= LARGE_MEMORY_BLOB_THRESHOLD_BYTES {
            self.large_block_count += 1;
            self.large_block_bytes += bytes;
        }
    }

    fn record_block(&mut self, block: &ContentBlock) {
        self.block_count += 1;
        match block {
            ContentBlock::Text { text, .. } => {
                self.text_blocks += 1;
                self.text_bytes += text.len();
                self.record_bytes(text.len());
            }
            ContentBlock::Reasoning { text } | ContentBlock::ReasoningTrace { text } => {
                self.reasoning_blocks += 1;
                self.reasoning_bytes += text.len();
                self.record_bytes(text.len());
            }
            ContentBlock::AnthropicThinking {
                thinking,
                signature,
            } => {
                self.reasoning_blocks += 1;
                let bytes = thinking.len() + signature.len();
                self.reasoning_bytes += bytes;
                self.record_bytes(bytes);
            }
            ContentBlock::OpenAIReasoning {
                id,
                summary,
                encrypted_content,
                status,
            } => {
                self.reasoning_blocks += 1;
                let bytes = id.len()
                    + summary.iter().map(String::len).sum::<usize>()
                    + encrypted_content.as_ref().map(String::len).unwrap_or(0)
                    + status.as_ref().map(String::len).unwrap_or(0);
                self.reasoning_bytes += bytes;
                self.record_bytes(bytes);
            }
            ContentBlock::ToolUse { input, .. } => {
                self.tool_use_blocks += 1;
                let input_bytes = estimate_json_bytes(input);
                self.tool_use_input_json_bytes += input_bytes;
                self.record_bytes(input_bytes);
            }
            ContentBlock::ToolResult { content, .. } => {
                self.tool_result_blocks += 1;
                self.tool_result_bytes += content.len();
                self.max_tool_result_bytes = self.max_tool_result_bytes.max(content.len());
                if content.len() >= LARGE_MEMORY_BLOB_THRESHOLD_BYTES {
                    self.large_tool_result_count += 1;
                    self.large_tool_result_bytes += content.len();
                }
                self.record_bytes(content.len());
            }
            ContentBlock::Image { data, .. } => {
                self.image_blocks += 1;
                self.image_data_bytes += data.len();
                self.record_bytes(data.len());
            }
            ContentBlock::OpenAICompaction { encrypted_content } => {
                self.openai_compaction_blocks += 1;
                self.openai_compaction_bytes += encrypted_content.len();
                self.record_bytes(encrypted_content.len());
            }
            ContentBlock::ToolReference { tool_name, .. } => {
                self.record_bytes(tool_name.len());
            }
            ContentBlock::ProviderNative { item, .. } => {
                self.tool_result_blocks += 1;
                let bytes = estimate_json_bytes(item);
                self.tool_result_bytes += bytes;
                self.max_tool_result_bytes = self.max_tool_result_bytes.max(bytes);
                self.record_bytes(bytes);
            }
        }
    }

    pub(super) fn payload_text_bytes(&self) -> usize {
        self.text_bytes
            + self.reasoning_bytes
            + self.tool_result_bytes
            + self.image_data_bytes
            + self.openai_compaction_bytes
    }

    pub(super) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "content_blocks": self.block_count,
            "text_blocks": self.text_blocks,
            "text_bytes": self.text_bytes,
            "reasoning_blocks": self.reasoning_blocks,
            "reasoning_bytes": self.reasoning_bytes,
            "tool_use_blocks": self.tool_use_blocks,
            "tool_use_input_json_bytes": self.tool_use_input_json_bytes,
            "tool_result_blocks": self.tool_result_blocks,
            "tool_result_bytes": self.tool_result_bytes,
            "image_blocks": self.image_blocks,
            "image_data_bytes": self.image_data_bytes,
            "openai_compaction_blocks": self.openai_compaction_blocks,
            "openai_compaction_bytes": self.openai_compaction_bytes,
            "large_block_count": self.large_block_count,
            "large_block_bytes": self.large_block_bytes,
            "large_tool_result_count": self.large_tool_result_count,
            "large_tool_result_bytes": self.large_tool_result_bytes,
            "max_block_bytes": self.max_block_bytes,
            "max_tool_result_bytes": self.max_tool_result_bytes,
            "payload_text_bytes": self.payload_text_bytes(),
        })
    }
}

pub(super) fn summarize_message_content<'a, I>(messages: I) -> ContentBlockMemoryStats
where
    I: IntoIterator<Item = &'a Vec<ContentBlock>>,
{
    let mut stats = ContentBlockMemoryStats::default();
    for blocks in messages {
        for block in blocks {
            stats.record_block(block);
        }
    }
    stats
}

pub(super) fn summarize_blocks(blocks: &[ContentBlock]) -> ContentBlockMemoryStats {
    let mut stats = ContentBlockMemoryStats::default();
    for block in blocks {
        stats.record_block(block);
    }
    stats
}

#[derive(Debug, Clone, Default)]
pub(super) struct SessionMemoryProfileCache {
    pub(super) compaction_json_bytes: CompactionJsonBytesCache,
    pub(super) messages_count: usize,
    pub(super) messages_json_bytes: usize,
    pub(super) message_stats: ContentBlockMemoryStats,
    pub(super) env_snapshots_count: usize,
    pub(super) env_snapshots_json_bytes: usize,
    pub(super) memory_injections_count: usize,
    pub(super) memory_injections_json_bytes: usize,
    pub(super) replay_events_count: usize,
    pub(super) replay_events_json_bytes: usize,
    pub(super) provider_cache_count: usize,
    pub(super) provider_cache_json_bytes: usize,
    pub(super) provider_cache_stats: ContentBlockMemoryStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionMemoryProfileSnapshot {
    pub message_count: usize,
    pub provider_cache_message_count: usize,
    pub env_snapshot_count: usize,
    pub memory_injection_count: usize,
    pub replay_event_count: usize,
    pub payload_text_bytes: usize,
    pub total_json_bytes: usize,
    pub provider_cache_json_bytes: usize,
    pub canonical_tool_result_bytes: usize,
    pub provider_cache_tool_result_bytes: usize,
    pub canonical_large_blob_bytes: usize,
    pub provider_cache_large_blob_bytes: usize,
}
