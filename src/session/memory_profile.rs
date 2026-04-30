use crate::message::ContentBlock;
use serde::Serialize;

fn estimate_json_bytes<T: Serialize>(value: &T) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}

const LARGE_MEMORY_BLOB_THRESHOLD_BYTES: usize = 16 * 1024;

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
            ContentBlock::Reasoning { text } => {
                self.reasoning_blocks += 1;
                self.reasoning_bytes += text.len();
                self.record_bytes(text.len());
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
