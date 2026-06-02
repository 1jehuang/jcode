//! Model routing helper (#100 MVP).
//!
//! When the user has cheap + premium model access and wants jcode to
//! pick the cheap one for routine turns and the premium one for hard
//! reasoning, this module provides a thin lookup keyed on the current
//! reasoning effort.
//!
//! ## Configuration (env-var based for MVP)
//!
//! ```bash
//! JCODE_ROUTING_THINKING=claude-opus-4
//! JCODE_ROUTING_ROUTINE=claude-sonnet-4
//! JCODE_ROUTING_THRESHOLD=medium    # "low" | "medium" | "high"
//! ```
//!
//! When all three are set, calls to `model_for_effort(effort)` return:
//!   - `Some(routine_model)` when `effort < threshold`
//!   - `Some(thinking_model)` when `effort >= threshold`
//!   - `None` when routing is disabled (any of the env vars unset)
//!
//! Caller should fall back to its existing model selection on `None`.
//!
//! Threshold semantics:
//!   `low`     = always thinking (routine never wins)
//!   `medium`  = thinking when effort is `medium` or `high`
//!   `high`    = thinking only when effort is `high`
//!
//! ## Why env vars not config.toml for MVP?
//!
//! Adding a new top-level table to [`Config`] touches the global
//! struct, default, serde round-trip, plus several test surfaces.
//! The env-var path lets users + agents experiment with routing
//! immediately without a schema migration. A follow-up PR can graduate
//! this to a `[model_routing]` config block once usage is validated.

/// Resolve which model to use for a given reasoning effort.
///
/// `effort` should be one of `"low" | "medium" | "high" | "minimal"`.
/// Anything else is treated as "low".
///
/// Returns `None` when routing is disabled (env vars unset / blank).
pub fn model_for_effort(effort: &str) -> Option<String> {
    let routine = read_env("JCODE_ROUTING_ROUTINE")?;
    let thinking = read_env("JCODE_ROUTING_THINKING")?;
    let threshold = read_env("JCODE_ROUTING_THRESHOLD").unwrap_or_else(|| "medium".to_string());

    let effort_rank = effort_to_rank(effort);
    let threshold_rank = effort_to_rank(&threshold);

    if effort_rank >= threshold_rank {
        Some(thinking)
    } else {
        Some(routine)
    }
}

fn read_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn effort_to_rank(effort: &str) -> u8 {
    match effort.trim().to_ascii_lowercase().as_str() {
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0, // minimal / unknown
    }
}

/// Whether routing is currently active (both endpoints configured).
/// Useful for surfacing in `jcode doctor`.
pub fn routing_active() -> bool {
    read_env("JCODE_ROUTING_ROUTINE").is_some() && read_env("JCODE_ROUTING_THINKING").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn save(keys: &[&'static str]) -> Vec<(&'static str, Option<std::ffi::OsString>)> {
        keys.iter().map(|k| (*k, std::env::var_os(k))).collect()
    }
    fn restore(saved: Vec<(&'static str, Option<std::ffi::OsString>)>) {
        for (k, v) in saved {
            unsafe {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    #[test]
    fn returns_none_when_routing_unset() {
        let _lock = crate::storage::lock_test_env();
        let saved = save(&[
            "JCODE_ROUTING_THINKING",
            "JCODE_ROUTING_ROUTINE",
            "JCODE_ROUTING_THRESHOLD",
        ]);
        for k in [
            "JCODE_ROUTING_THINKING",
            "JCODE_ROUTING_ROUTINE",
            "JCODE_ROUTING_THRESHOLD",
        ] {
            crate::env::remove_var(k);
        }
        assert_eq!(model_for_effort("low"), None);
        assert_eq!(model_for_effort("high"), None);
        assert!(!routing_active());
        restore(saved);
    }

    #[test]
    fn picks_routine_when_effort_below_threshold() {
        let _lock = crate::storage::lock_test_env();
        let saved = save(&[
            "JCODE_ROUTING_THINKING",
            "JCODE_ROUTING_ROUTINE",
            "JCODE_ROUTING_THRESHOLD",
        ]);
        crate::env::set_var("JCODE_ROUTING_THINKING", "claude-opus-4");
        crate::env::set_var("JCODE_ROUTING_ROUTINE", "claude-sonnet-4");
        crate::env::set_var("JCODE_ROUTING_THRESHOLD", "medium");

        assert_eq!(model_for_effort("low").as_deref(), Some("claude-sonnet-4"));
        assert_eq!(
            model_for_effort("minimal").as_deref(),
            Some("claude-sonnet-4")
        );

        restore(saved);
    }

    #[test]
    fn picks_thinking_when_effort_at_or_above_threshold() {
        let _lock = crate::storage::lock_test_env();
        let saved = save(&[
            "JCODE_ROUTING_THINKING",
            "JCODE_ROUTING_ROUTINE",
            "JCODE_ROUTING_THRESHOLD",
        ]);
        crate::env::set_var("JCODE_ROUTING_THINKING", "claude-opus-4");
        crate::env::set_var("JCODE_ROUTING_ROUTINE", "claude-sonnet-4");
        crate::env::set_var("JCODE_ROUTING_THRESHOLD", "medium");

        assert_eq!(model_for_effort("medium").as_deref(), Some("claude-opus-4"));
        assert_eq!(model_for_effort("high").as_deref(), Some("claude-opus-4"));

        restore(saved);
    }

    #[test]
    fn threshold_high_only_picks_thinking_at_high() {
        let _lock = crate::storage::lock_test_env();
        let saved = save(&[
            "JCODE_ROUTING_THINKING",
            "JCODE_ROUTING_ROUTINE",
            "JCODE_ROUTING_THRESHOLD",
        ]);
        crate::env::set_var("JCODE_ROUTING_THINKING", "opus");
        crate::env::set_var("JCODE_ROUTING_ROUTINE", "sonnet");
        crate::env::set_var("JCODE_ROUTING_THRESHOLD", "high");

        assert_eq!(model_for_effort("low").as_deref(), Some("sonnet"));
        assert_eq!(model_for_effort("medium").as_deref(), Some("sonnet"));
        assert_eq!(model_for_effort("high").as_deref(), Some("opus"));

        restore(saved);
    }

    #[test]
    fn default_threshold_is_medium_when_unset() {
        let _lock = crate::storage::lock_test_env();
        let saved = save(&[
            "JCODE_ROUTING_THINKING",
            "JCODE_ROUTING_ROUTINE",
            "JCODE_ROUTING_THRESHOLD",
        ]);
        crate::env::set_var("JCODE_ROUTING_THINKING", "opus");
        crate::env::set_var("JCODE_ROUTING_ROUTINE", "sonnet");
        crate::env::remove_var("JCODE_ROUTING_THRESHOLD");

        // default threshold = medium
        assert_eq!(model_for_effort("low").as_deref(), Some("sonnet"));
        assert_eq!(model_for_effort("medium").as_deref(), Some("opus"));
        assert_eq!(model_for_effort("high").as_deref(), Some("opus"));

        restore(saved);
    }

    #[test]
    fn routing_active_reflects_both_endpoints_present() {
        let _lock = crate::storage::lock_test_env();
        let saved = save(&["JCODE_ROUTING_THINKING", "JCODE_ROUTING_ROUTINE"]);

        crate::env::remove_var("JCODE_ROUTING_THINKING");
        crate::env::remove_var("JCODE_ROUTING_ROUTINE");
        assert!(!routing_active());

        crate::env::set_var("JCODE_ROUTING_THINKING", "opus");
        assert!(!routing_active(), "thinking only is not active");

        crate::env::set_var("JCODE_ROUTING_ROUTINE", "sonnet");
        assert!(routing_active(), "both endpoints set should be active");

        restore(saved);
    }
}
