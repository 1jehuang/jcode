//! Prompt state snapshot – captures all cache vectors at a point in time.
//!
//! A `PromptStateSnapshot` is the canonical input for cache-break detection.
//! Build one before sending a request, then diff it against the previous
//! snapshot to see which vectors changed.

use serde::{Deserialize, Serialize};

use crate::cache_hash::{compute_hash, VectorHash};

/// Snapshot of every cache-relevant vector at the moment a request is built.
///
/// All fields are hashes (or opaque identifiers) rather than raw content so
/// that the snapshot is compact and cheap to compare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptStateSnapshot {
    /// Hash of the full system prompt.
    pub system_prompt_hash: VectorHash,

    /// Hash of all tool schemas concatenated.
    pub tool_schemas_hash: VectorHash,

    /// Hash of the cache_control configuration.
    pub cache_control_hash: VectorHash,

    /// Per-tool hashes, keyed by tool name.
    pub per_tool_hashes: Vec<(String, VectorHash)>,

    /// Model identifier string (e.g. `"claude-sonnet-4-20250514"`).
    pub model: String,

    /// Whether fast mode is enabled.
    pub fast_mode: bool,

    /// Global cache strategy (e.g. `"off"`, `"session"`).
    pub global_cache_strategy: String,

    /// Ordered set of beta feature flags.
    pub betas: Vec<String>,

    /// Whether auto mode is active.
    pub auto_mode_active: bool,

    /// Whether the request is using overage tokens.
    pub is_using_overage: bool,

    /// Whether cached MC (message cache) is enabled.
    pub cached_mc_enabled: bool,

    /// Optional effort value (e.g. `"low"`, `"medium"`, `"high"`).
    pub effort_value: Option<String>,

    /// Hash of any extra body parameters not covered by other vectors.
    pub extra_body_hash: VectorHash,

    /// Diffable content hash for debugging cache breaks.
    pub diffable_content_hash: VectorHash,

    /// Timestamp (epoch millis) when this snapshot was taken.
    pub captured_at_ms: u64,
}

impl PromptStateSnapshot {
    /// Compute a composite hash of the "structural" vectors that are most
    /// expensive to re-send (system prompt + tool schemas + per-tool hashes).
    pub fn structural_hash(&self) -> VectorHash {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        self.system_prompt_hash.0.hash(&mut hasher);
        self.tool_schemas_hash.0.hash(&mut hasher);
        for (name, h) in &self.per_tool_hashes {
            name.hash(&mut hasher);
            h.0.hash(&mut hasher);
        }
        VectorHash(hasher.finish())
    }
}

/// Build a [`PromptStateSnapshot`] from the given vector values.
///
/// This is the canonical entry point for capturing cache state before a request.
/// Hashes for complex values (system prompt, tool schemas, etc.) are computed
/// via [`compute_hash`]; callers pass the raw content that should be hashed.
#[allow(clippy::too_many_arguments)]
pub fn snapshot_current_state(
    system_prompt: &impl Serialize,
    tool_schemas: &impl Serialize,
    cache_control: &impl Serialize,
    per_tool_hashes: Vec<(String, u64)>,
    model: String,
    fast_mode: bool,
    global_cache_strategy: String,
    betas: Vec<String>,
    auto_mode_active: bool,
    is_using_overage: bool,
    cached_mc_enabled: bool,
    effort_value: Option<String>,
    extra_body: &impl Serialize,
    diffable_content: &impl Serialize,
) -> PromptStateSnapshot {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    PromptStateSnapshot {
        system_prompt_hash: VectorHash(compute_hash(system_prompt)),
        tool_schemas_hash: VectorHash(compute_hash(tool_schemas)),
        cache_control_hash: VectorHash(compute_hash(cache_control)),
        per_tool_hashes: per_tool_hashes
            .into_iter()
            .map(|(name, h)| (name, VectorHash(h)))
            .collect(),
        model,
        fast_mode,
        global_cache_strategy,
        betas,
        auto_mode_active,
        is_using_overage,
        cached_mc_enabled,
        effort_value,
        extra_body_hash: VectorHash(compute_hash(extra_body)),
        diffable_content_hash: VectorHash(compute_hash(diffable_content)),
        captured_at_ms: now_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_snapshot(system: u64, tools: u64) -> PromptStateSnapshot {
        PromptStateSnapshot {
            system_prompt_hash: VectorHash(system),
            tool_schemas_hash: VectorHash(tools),
            cache_control_hash: VectorHash(0),
            per_tool_hashes: vec![],
            model: "test-model".into(),
            fast_mode: false,
            global_cache_strategy: "off".into(),
            betas: vec![],
            auto_mode_active: false,
            is_using_overage: false,
            cached_mc_enabled: false,
            effort_value: None,
            extra_body_hash: VectorHash(0),
            diffable_content_hash: VectorHash(0),
            captured_at_ms: 0,
        }
    }

    #[test]
    fn structural_hash_changes_with_system_prompt() {
        let s1 = dummy_snapshot(111, 222);
        let s2 = dummy_snapshot(999, 222);
        assert_ne!(s1.structural_hash(), s2.structural_hash());
    }

    #[test]
    fn structural_hash_changes_with_tool_schemas() {
        let s1 = dummy_snapshot(111, 222);
        let s2 = dummy_snapshot(111, 333);
        assert_ne!(s1.structural_hash(), s2.structural_hash());
    }

    #[test]
    fn snapshot_current_state_populates_all_fields() {
        let snap = snapshot_current_state(
            &"system prompt",
            &vec!["tool_a", "tool_b"],
            &"ephemeral",
            vec![("tool_a".into(), 42), ("tool_b".into(), 99)],
            "claude-sonnet-4".into(),
            true,
            "session".into(),
            vec!["beta-1".into()],
            false,
            false,
            true,
            Some("high".into()),
            &"extra body",
            &"diff content",
        );
        assert_ne!(snap.system_prompt_hash, VectorHash::ZERO);
        assert_ne!(snap.tool_schemas_hash, VectorHash::ZERO);
        assert_ne!(snap.cache_control_hash, VectorHash::ZERO);
        assert_ne!(snap.extra_body_hash, VectorHash::ZERO);
        assert_ne!(snap.diffable_content_hash, VectorHash::ZERO);
        assert_eq!(snap.per_tool_hashes.len(), 2);
        assert_eq!(snap.per_tool_hashes[0].0, "tool_a");
        assert_eq!(snap.per_tool_hashes[0].1, VectorHash(42));
        assert_eq!(snap.model, "claude-sonnet-4");
        assert!(snap.fast_mode);
        assert_eq!(snap.global_cache_strategy, "session");
        assert_eq!(snap.betas, vec!["beta-1"]);
        assert!(!snap.auto_mode_active);
        assert!(snap.cached_mc_enabled);
        assert_eq!(snap.effort_value.as_deref(), Some("high"));
        assert!(snap.captured_at_ms > 0);
    }

    #[test]
    fn snapshot_deterministic_for_same_inputs() {
        let make = || {
            snapshot_current_state(
                &"sys", &"tools", &"cc", vec![], "m".into(),
                false, "".into(), vec![], false, false, false, None,
                &"extra", &"diff",
            )
        };
        let s1 = make();
        let s2 = make();
        // Same inputs -> same hashes (captured_at_ms may differ but hashes must match)
        assert_eq!(s1.system_prompt_hash, s2.system_prompt_hash);
        assert_eq!(s1.tool_schemas_hash, s2.tool_schemas_hash);
        assert_eq!(s1.extra_body_hash, s2.extra_body_hash);
    }
}
