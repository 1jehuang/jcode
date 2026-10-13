//! Diffable content – human-readable payloads for debugging cache breaks.
//!
//! When a cache break is detected, operators need to understand *why*.
//! `DiffableContent` serializes every cache vector into a structured
//! [`serde_json::Value`] that can be logged, diffed, or displayed in a
//! debug panel.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cache_hash::VectorHash;
use crate::prompt_state::PromptStateSnapshot;

/// A debug-friendly representation of the cache vectors at a point in time.
///
/// Each field mirrors the corresponding [`PromptStateSnapshot`] vector but
/// stores the raw content (or a truncated summary) rather than just the hash,
/// so that humans can inspect what changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffableContent {
    /// Truncated system prompt (first N chars).
    pub system_prompt_preview: String,
    /// Full system prompt hash.
    pub system_prompt_hash: VectorHash,

    /// Number of tool schemas.
    pub tool_count: usize,
    /// Tool schema hash.
    pub tool_schemas_hash: VectorHash,

    /// Per-tool summary: `(name, hash, param_count)`.
    pub tools: Vec<ToolSummary>,

    /// Model name.
    pub model: String,
    /// Fast mode flag.
    pub fast_mode: bool,
    /// Global cache strategy.
    pub global_cache_strategy: String,
    /// Beta flags.
    pub betas: Vec<String>,

    /// Diffable content hash.
    pub diffable_content_hash: VectorHash,
}

/// Summary of a single tool for diffable output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSummary {
    pub name: String,
    pub hash: VectorHash,
    pub param_count: usize,
}

impl DiffableContent {
    /// Build diffable content from a snapshot and the raw system prompt text.
    ///
    /// `system_prompt_preview_chars` controls truncation length.
    pub fn from_snapshot(
        snapshot: &PromptStateSnapshot,
        system_prompt_text: &str,
        tool_params: &[(String, usize)], // (tool_name, param_count)
        preview_chars: usize,
    ) -> Self {
        let preview = if system_prompt_text.len() <= preview_chars {
            system_prompt_text.to_string()
        } else {
            let mut truncated = system_prompt_text[..preview_chars].to_string();
            truncated.push_str("...");
            truncated
        };

        let tools = snapshot
            .per_tool_hashes
            .iter()
            .map(|(name, hash)| {
                let param_count = tool_params
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, c)| *c)
                    .unwrap_or(0);
                ToolSummary {
                    name: name.clone(),
                    hash: *hash,
                    param_count,
                }
            })
            .collect();

        Self {
            system_prompt_preview: preview,
            system_prompt_hash: snapshot.system_prompt_hash,
            tool_count: snapshot.per_tool_hashes.len(),
            tool_schemas_hash: snapshot.tool_schemas_hash,
            tools,
            model: snapshot.model.clone(),
            fast_mode: snapshot.fast_mode,
            global_cache_strategy: snapshot.global_cache_strategy.clone(),
            betas: snapshot.betas.clone(),
            diffable_content_hash: snapshot.diffable_content_hash,
        }
    }

    /// Serialize to a JSON `Value` suitable for structured logging.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache_hash::hash_str;

    #[test]
    fn round_trip_json() {
        let snapshot = PromptStateSnapshot {
            system_prompt_hash: hash_str("sys"),
            tool_schemas_hash: hash_str("tools"),
            cache_control_hash: hash_str("cc"),
            per_tool_hashes: vec![("read".into(), hash_str("read"))],
            model: "test".into(),
            fast_mode: false,
            global_cache_strategy: "off".into(),
            betas: vec![],
            auto_mode_active: false,
            is_using_overage: false,
            cached_mc_enabled: false,
            effort_value: None,
            extra_body_hash: hash_str("extra"),
            diffable_content_hash: hash_str("d"),
            captured_at_ms: 0,
        };

        let diffable = DiffableContent::from_snapshot(
            &snapshot,
            "You are a helpful assistant.",
            &[("read".into(), 2)],
            200,
        );

        let json = diffable.to_json();
        assert_eq!(json["model"], "test");
        assert_eq!(json["tool_count"], 1);
        assert_eq!(json["tools"][0]["name"], "read");
    }

    #[test]
    fn system_prompt_truncation() {
        let snapshot = PromptStateSnapshot {
            system_prompt_hash: hash_str("x"),
            tool_schemas_hash: hash_str("x"),
            cache_control_hash: hash_str("x"),
            per_tool_hashes: vec![],
            model: "m".into(),
            fast_mode: false,
            global_cache_strategy: "off".into(),
            betas: vec![],
            auto_mode_active: false,
            is_using_overage: false,
            cached_mc_enabled: false,
            effort_value: None,
            extra_body_hash: hash_str("x"),
            diffable_content_hash: hash_str("x"),
            captured_at_ms: 0,
        };

        let long_prompt = "a".repeat(500);
        let diffable = DiffableContent::from_snapshot(&snapshot, &long_prompt, &[], 10);
        assert!(diffable.system_prompt_preview.ends_with("..."));
        assert!(diffable.system_prompt_preview.len() < 500);
    }
}
