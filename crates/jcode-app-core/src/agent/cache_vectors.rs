//! Prompt-cache vector tracking integration for `jcode-cache-vectors`.
//!
//! Enhances the existing `CacheTracker` (message-prefix validation) by
//! snapshotting all 14 prompt-cache vectors before each provider request
//! and detecting which vectors changed between turns.

use jcode_cache_vectors::{DiffableContent, PromptStateSnapshot, snapshot_current_state};
use jcode_message_types::ToolDefinition;

use crate::logging;

/// Tracks prompt-cache vector state across turns.
///
/// Sits alongside the existing `CacheTracker` (which validates message-prefix
/// append-only invariants). This tracker detects *which* cache vector changed
/// when the provider-side prompt cache is invalidated.
#[derive(Debug, Clone, Default)]
pub(crate) struct CacheVectorsTracker {
    /// Previous snapshot; `None` on the first turn.
    previous: Option<PromptStateSnapshot>,
    /// Number of breaks detected across the session lifetime.
    total_breaks: u64,
}

impl CacheVectorsTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take a new snapshot, compare with the previous one, log any cache
    /// breaks, and store the snapshot for the next comparison.
    pub fn record_and_detect(
        &mut self,
        system_prompt: &str,
        tools: &[ToolDefinition],
        model: &str,
        fast_mode: bool,
    ) {
        let snapshot = snapshot_current_state(
            &system_prompt,                                        // system_prompt
            &tools.iter().map(|t| t.input_schema.clone()).collect::<Vec<_>>(), // tool_schemas
            &(),                                                   // cache_control
            per_tool_hashes(tools),                                // per_tool_hashes
            model.to_string(),                                     // model
            fast_mode,                                             // fast_mode
            String::new(),                                         // global_cache_strategy
            Vec::new(),                                            // betas
            false,                                                 // auto_mode_active
            false,                                                 // is_using_overage
            false,                                                 // cached_mc_enabled
            None,                                                  // effort_value
            &(),                                                   // extra_body
            &(),                                                   // diffable_content
        );

        if let Some(ref prev) = self.previous {
            let breaks = jcode_cache_vectors::cache_break::detect_breaks(prev, &snapshot);
            if !breaks.is_empty() {
                self.total_breaks += breaks.len() as u64;
                log_breaks(prev, &snapshot, &breaks, system_prompt);
            }
        }

        self.previous = Some(snapshot);
    }

    /// Reset tracker state (called on compaction, model switch, etc.).
    pub fn reset(&mut self) {
        self.previous = None;
    }

    /// Total cache breaks observed across the session.
    pub fn total_breaks(&self) -> u64 {
        self.total_breaks
    }
}

/// Compute per-tool hashes as `(name, djb2_hash)` pairs for the snapshot.
fn per_tool_hashes(tools: &[ToolDefinition]) -> Vec<(String, u64)> {
    tools
        .iter()
        .map(|t| (t.name.clone(), jcode_cache_vectors::compute_hash(&t.input_schema)))
        .collect()
}

/// Log cache breaks with diffable content for debugging.
fn log_breaks(
    prev: &PromptStateSnapshot,
    current: &PromptStateSnapshot,
    breaks: &[jcode_cache_vectors::CacheBreak],
    system_prompt: &str,
) {
    let kind_names: Vec<&str> = breaks
        .iter()
        .map(|b| match b.kind {
            jcode_cache_vectors::CacheBreakKind::SystemPrompt => "system_prompt",
            jcode_cache_vectors::CacheBreakKind::ToolSchemas => "tool_schemas",
            jcode_cache_vectors::CacheBreakKind::CacheControl => "cache_control",
            jcode_cache_vectors::CacheBreakKind::PerTool { .. } => "per_tool",
            jcode_cache_vectors::CacheBreakKind::Model => "model",
            jcode_cache_vectors::CacheBreakKind::FastMode => "fast_mode",
            jcode_cache_vectors::CacheBreakKind::GlobalCacheStrategy => "cache_strategy",
            jcode_cache_vectors::CacheBreakKind::Betas => "betas",
            jcode_cache_vectors::CacheBreakKind::AutoModeActive => "auto_mode",
            jcode_cache_vectors::CacheBreakKind::IsUsingOverage => "overage",
            jcode_cache_vectors::CacheBreakKind::CachedMcEnabled => "cached_mc",
            jcode_cache_vectors::CacheBreakKind::EffortValue => "effort",
            jcode_cache_vectors::CacheBreakKind::ExtraBody => "extra_body",
            jcode_cache_vectors::CacheBreakKind::DiffableContent => "diffable",
        })
        .collect();

    let tool_params: Vec<(String, usize)> = current
        .per_tool_hashes
        .iter()
        .map(|(name, _)| (name.clone(), 0))
        .collect();
    let diffable = DiffableContent::from_snapshot(current, system_prompt, &tool_params, 200);

    logging::info(&format!(
        "CACHE_VECTORS_CHANGED: {} vectors [{}] structural_old={:016x} structural_new={:016x}",
        breaks.len(),
        kind_names.join(", "),
        prev.structural_hash().0,
        current.structural_hash().0,
    ));
    logging::debug(&format!(
        "CACHE_VECTORS_DIFFABLE: {}",
        serde_json::to_string(&diffable.to_json()).unwrap_or_default(),
    ));
}
