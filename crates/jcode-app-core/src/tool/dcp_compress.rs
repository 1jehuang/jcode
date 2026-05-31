//! DCP compress tool — exposes DCP compress/decompress/recompress to the agent.
//!
//! These tools allow the model to:
//! - `dcp_compress` — compress a range of messages into a block summary
//! - `dcp_decompress` — deactivate a committed compression block
//! - `dcp_recompress` — re-activate a previously deactivated block

#[cfg(feature = "dcp")]
use crate::tool::{ToolContext, ToolOutput};
#[cfg(feature = "dcp")]
use anyhow::Result;
#[cfg(feature = "dcp")]
use async_trait::async_trait;
#[cfg(feature = "dcp")]
use dynamic_context_pruning::{BlockId, CompressArgs, RangeEntry, MessageEntry};
#[cfg(feature = "dcp")]
use jcode_tool_core::Tool;
#[cfg(feature = "dcp")]
use serde::Deserialize;
#[cfg(feature = "dcp")]
use serde_json::Value;

// ─────────────────────────────────────────────────────────────────────────────
// DcpCompressTool
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "dcp")]
#[derive(Default)]
pub struct DcpCompressTool;

#[cfg(feature = "dcp")]
impl DcpCompressTool {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(feature = "dcp")]
#[derive(Deserialize)]
struct CompressInput {
    /// Compress mode: "range" or "message"
    mode: String,
    /// Topic/summary for the compression
    topic: String,
    /// For range mode: list of {start_id, end_id, summary}
    #[serde(default)]
    ranges: Option<Vec<RangeEntry>>,
    /// For message mode: list of {message_id, topic, summary}
    #[serde(default)]
    messages: Option<Vec<MessageEntry>>,
}

#[cfg(feature = "dcp")]
#[async_trait]
impl Tool for DcpCompressTool {
    fn name(&self) -> &str {
        "dcp_compress"
    }

    fn description(&self) -> &str {
        "Compress contiguous ranges or individual messages into block summaries using DCP. \
         Use this when the context is filling up and you want to summarize older content."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["mode", "topic"],
            "properties": {
                "mode": {
                    "type": "string",
                    "description": "Compression mode: 'range' (compress contiguous ranges) or 'message' (compress specific messages).",
                    "enum": ["range", "message"]
                },
                "topic": {
                    "type": "string",
                    "description": "Batch-level topic/summary describing what these compressed messages are about."
                },
                "ranges": {
                    "type": "array",
                    "description": "(Range mode) List of ranges to compress with {start_id, end_id, summary}.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "start_id": { "type": "string" },
                            "end_id": { "type": "string" },
                            "summary": { "type": "string" }
                        }
                    }
                },
                "messages": {
                    "type": "array",
                    "description": "(Message mode) List of messages to compress with {message_id, topic, summary}.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "message_id": { "type": "string" },
                            "topic": { "type": "string" },
                            "summary": { "type": "string" }
                        }
                    }
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: CompressInput = serde_json::from_value(input)?;

        let dcp_plugin = crate::agent::Agent::get_current_dcp()
            .ok_or_else(|| anyhow::anyhow!("DCP is not available in this context"))?;
        let mut dcp = dcp_plugin.lock().map_err(|e| anyhow::anyhow!("DCP lock error: {}", e))?;

        // Build CompressArgs based on mode
        let args = match params.mode.as_str() {
            "message" => {
                let msgs = params.messages.unwrap_or_default();
                CompressArgs::Message {
                    topic: params.topic,
                    content: msgs,
                }
            }
            _ => {
                let ranges = params.ranges.unwrap_or_default();
                CompressArgs::Range {
                    topic: params.topic,
                    content: ranges,
                }
            }
        };

        // Get current session messages for DCP processing
        let session = crate::session::Session::load(&ctx.session_id)
            .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?;
        let messages = session.provider_messages();
        let dcp_messages = crate::dcp_bridge::jcode_to_dcp(&messages);

        let result = dcp.pruner_mut().handle_compress(args, &dcp_messages)
            .map_err(|e| anyhow::anyhow!("DCP compress error: {:?}", e))?;

        let total_tokens_saved: u64 = result.blocks.iter().map(|b| b.compressed_tokens).sum();
        let output = format!(
            "Compressed {} messages into {} block(s). Saved ~{} tokens.",
            result.compressed_messages,
            result.blocks.len(),
            total_tokens_saved
        );

        Ok(ToolOutput {
            output,
            title: Some("DCP Compress".to_string()),
            metadata: Some(serde_json::json!({
                "blocks_created": result.blocks.len(),
                "compressed_messages": result.compressed_messages,
            })),
            images: vec![],
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DcpDecompressTool
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "dcp")]
#[derive(Default)]
pub struct DcpDecompressTool;

#[cfg(feature = "dcp")]
impl DcpDecompressTool {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(feature = "dcp")]
#[derive(Deserialize)]
struct DecompressInput {
    /// Block ID to decompress (restore to anchor verbatim)
    block_id: u32,
}

#[cfg(feature = "dcp")]
#[async_trait]
impl Tool for DcpDecompressTool {
    fn name(&self) -> &str {
        "dcp_decompress"
    }

    fn description(&self) -> &str {
        "Deactivate a committed DCP compression block, restoring its anchor message verbatim. \
         The block stays in history but is no longer used in transforms."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["block_id"],
            "properties": {
                "block_id": {
                    "type": "integer",
                    "description": "The block ID to decompress (restore verbatim)."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: DecompressInput = serde_json::from_value(input)?;

        let dcp_plugin = crate::agent::Agent::get_current_dcp()
            .ok_or_else(|| anyhow::anyhow!("DCP is not available in this context"))?;
        let mut dcp = dcp_plugin.lock().map_err(|e| anyhow::anyhow!("DCP lock error: {}", e))?;

        let block_id = BlockId(params.block_id);
        let result = dcp.pruner_mut().decompress(block_id)
            .map_err(|e| anyhow::anyhow!("DCP decompress error: {:?}", e))?;

        let output = format!(
            "Decompressed block {}. Anchor message {} is now restored verbatim.",
            result.block_id.0, result.anchor_message_id
        );

        Ok(ToolOutput {
            output,
            title: Some("DCP Decompress".to_string()),
            metadata: Some(serde_json::json!({
                "block_id": result.block_id.0,
                "anchor_message_id": result.anchor_message_id,
            })),
            images: vec![],
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DcpRecompressTool
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "dcp")]
#[derive(Default)]
pub struct DcpRecompressTool;

#[cfg(feature = "dcp")]
impl DcpRecompressTool {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(feature = "dcp")]
#[derive(Deserialize)]
struct RecompressInput {
    /// Block ID to recompress (re-activate)
    block_id: u32,
}

#[cfg(feature = "dcp")]
#[async_trait]
impl Tool for DcpRecompressTool {
    fn name(&self) -> &str {
        "dcp_recompress"
    }

    fn description(&self) -> &str {
        "Re-activate a previously deactivated DCP compression block. \
         The block will be used again in future context transforms."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["block_id"],
            "properties": {
                "block_id": {
                    "type": "integer",
                    "description": "The block ID to recompress (re-activate)."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: RecompressInput = serde_json::from_value(input)?;

        let dcp_plugin = crate::agent::Agent::get_current_dcp()
            .ok_or_else(|| anyhow::anyhow!("DCP is not available in this context"))?;
        let mut dcp = dcp_plugin.lock().map_err(|e| anyhow::anyhow!("DCP lock error: {}", e))?;

        let block_id = BlockId(params.block_id);
        let result = dcp.pruner_mut().recompress(block_id)
            .map_err(|e| anyhow::anyhow!("DCP recompress error: {:?}", e))?;

        let output = format!(
            "Recompressed block {}. Block {} is now active again.",
            result.block_id.0, result.block_id.0
        );

        Ok(ToolOutput {
            output,
            title: Some("DCP Recompress".to_string()),
            metadata: Some(serde_json::json!({
                "block_id": result.block_id.0,
                "anchor_message_id": result.anchor_message_id,
            })),
            images: vec![],
        })
    }
}