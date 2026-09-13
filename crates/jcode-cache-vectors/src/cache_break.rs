//! Cache-break detection – diff two snapshots to find what changed.
//!
//! When the provider-side cache is invalidated, the first request after the
//! break re-sends the full payload (system prompt, tool schemas, etc.).
//! Detecting *which* vectors changed lets us log the cause, emit telemetry,
//! and potentially avoid unnecessary breaks in the future.

use serde::{Deserialize, Serialize};

use crate::cache_hash::VectorHash;
use crate::prompt_state::PromptStateSnapshot;

/// The kind of vector that changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CacheBreakKind {
    SystemPrompt,
    ToolSchemas,
    CacheControl,
    PerTool { index: usize },
    Model,
    FastMode,
    GlobalCacheStrategy,
    Betas,
    AutoModeActive,
    IsUsingOverage,
    CachedMcEnabled,
    EffortValue,
    ExtraBody,
    DiffableContent,
}

/// A single detected cache break – the kind of vector that changed and its
/// old/new hash values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheBreak {
    pub kind: CacheBreakKind,
    pub old_hash: VectorHash,
    pub new_hash: VectorHash,
}

/// Compare two snapshots and return every vector that changed.
///
/// An empty `Vec` means the cache should still be valid.
pub fn detect_breaks(
    previous: &PromptStateSnapshot,
    current: &PromptStateSnapshot,
) -> Vec<CacheBreak> {
    let mut breaks = Vec::new();

    macro_rules! check {
        ($kind:expr, $old:expr, $new:expr) => {
            if $old != $new {
                breaks.push(CacheBreak {
                    kind: $kind,
                    old_hash: $old,
                    new_hash: $new,
                });
            }
        };
    }

    check!(
        CacheBreakKind::SystemPrompt,
        previous.system_prompt_hash,
        current.system_prompt_hash
    );
    check!(
        CacheBreakKind::ToolSchemas,
        previous.tool_schemas_hash,
        current.tool_schemas_hash
    );
    check!(
        CacheBreakKind::CacheControl,
        previous.cache_control_hash,
        current.cache_control_hash
    );
    check!(
        CacheBreakKind::Model,
        VectorHash(previous.model.len() as u64),
        VectorHash(current.model.len() as u64)
    );
    check!(
        CacheBreakKind::FastMode,
        VectorHash(previous.fast_mode as u64),
        VectorHash(current.fast_mode as u64)
    );
    check!(
        CacheBreakKind::GlobalCacheStrategy,
        VectorHash(previous.global_cache_strategy.len() as u64),
        VectorHash(current.global_cache_strategy.len() as u64)
    );
    check!(
        CacheBreakKind::Betas,
        VectorHash(previous.betas.len() as u64),
        VectorHash(current.betas.len() as u64)
    );
    check!(
        CacheBreakKind::AutoModeActive,
        VectorHash(previous.auto_mode_active as u64),
        VectorHash(current.auto_mode_active as u64)
    );
    check!(
        CacheBreakKind::IsUsingOverage,
        VectorHash(previous.is_using_overage as u64),
        VectorHash(current.is_using_overage as u64)
    );
    check!(
        CacheBreakKind::CachedMcEnabled,
        VectorHash(previous.cached_mc_enabled as u64),
        VectorHash(current.cached_mc_enabled as u64)
    );
    check!(
        CacheBreakKind::EffortValue,
        VectorHash(previous.effort_value.as_ref().map_or(0, |v| v.len() as u64)),
        VectorHash(current.effort_value.as_ref().map_or(0, |v| v.len() as u64))
    );
    check!(
        CacheBreakKind::ExtraBody,
        previous.extra_body_hash,
        current.extra_body_hash
    );
    check!(
        CacheBreakKind::DiffableContent,
        previous.diffable_content_hash,
        current.diffable_content_hash
    );

    // Per-tool hashes: compare pairwise by position.
    let max_len = previous.per_tool_hashes.len().max(current.per_tool_hashes.len());
    for i in 0..max_len {
        let old = previous.per_tool_hashes.get(i).map(|(_, h)| *h);
        let new = current.per_tool_hashes.get(i).map(|(_, h)| *h);
        if old != new {
            breaks.push(CacheBreak {
                kind: CacheBreakKind::PerTool { index: i },
                old_hash: old.unwrap_or(VectorHash::ZERO),
                new_hash: new.unwrap_or(VectorHash::ZERO),
            });
        }
    }

    breaks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache_hash::hash_str;

    fn base_snapshot() -> PromptStateSnapshot {
        PromptStateSnapshot {
            system_prompt_hash: hash_str("sys"),
            tool_schemas_hash: hash_str("tools"),
            cache_control_hash: hash_str("cc"),
            per_tool_hashes: vec![
                ("read".into(), hash_str("read")),
                ("write".into(), hash_str("write")),
            ],
            model: "claude-sonnet-4-20250514".into(),
            fast_mode: false,
            global_cache_strategy: "off".into(),
            betas: vec![],
            auto_mode_active: false,
            is_using_overage: false,
            cached_mc_enabled: false,
            effort_value: None,
            extra_body_hash: hash_str("extra"),
            diffable_content_hash: hash_str("diff"),
            captured_at_ms: 1000,
        }
    }

    #[test]
    fn no_changes() {
        let s1 = base_snapshot();
        let s2 = base_snapshot();
        let breaks = detect_breaks(&s1, &s2);
        assert!(breaks.is_empty(), "expected no breaks, got {breaks:?}");
    }

    #[test]
    fn system_prompt_change() {
        let s1 = base_snapshot();
        let mut s2 = base_snapshot();
        s2.system_prompt_hash = hash_str("new sys");
        let breaks = detect_breaks(&s1, &s2);
        assert_eq!(breaks.len(), 1);
        assert_eq!(breaks[0].kind, CacheBreakKind::SystemPrompt);
    }

    #[test]
    fn model_change() {
        let s1 = base_snapshot();
        let mut s2 = base_snapshot();
        s2.model = "gpt-4o".into();
        let breaks = detect_breaks(&s1, &s2);
        assert!(breaks.iter().any(|b| b.kind == CacheBreakKind::Model));
    }
}
