//! Scoped-model allowlist for `Ctrl+P` / `/scoped-models` cycling (issue #26).
//!
//! A "scoped model" set is a user-defined ordered list of model id patterns
//! resolved at session start. When non-empty, model cycling is restricted to
//! entries from `available_models_for_switching()` whose ids match at least one
//! pattern. Order in the allowlist is preserved across cycles so users can
//! flip between, e.g., `sonnet:high` and `gpt-5-codex` with two key presses.
//!
//! Resolution order (highest priority first):
//!
//! 1. `JCODE_SCOPED_MODELS` env var (set by `--models` CLI flag in
//!    `cli::startup::parse_and_prepare_args`).
//! 2. `provider.scoped_models` config value (`~/.jcode/config.toml`).
//! 3. Empty — cycling falls back to the full
//!    `available_models_for_switching()` list (existing behavior).
//!
//! Patterns support either case-insensitive substring matching or shell-style
//! globs with `*` and `?`. The first non-empty match anywhere in the model id
//! counts.

/// Resolve the active allowlist, in priority order.
pub fn resolve_allowlist() -> Vec<String> {
    if let Ok(value) = std::env::var("JCODE_SCOPED_MODELS") {
        let parsed = parse_pattern_list(&value);
        if !parsed.is_empty() {
            return parsed;
        }
    }

    // Config-value fallback. `provider.scoped_models` is `Vec<String>` once
    // PR #200 (this PR's config schema patch) lands; before that, this branch
    // is a no-op.
    let cfg = crate::config::config();
    if !cfg.provider.scoped_models.is_empty() {
        return cfg
            .provider
            .scoped_models
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }

    Vec::new()
}

fn parse_pattern_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Filter `available` against `patterns`, preserving the **patterns** order so
/// `cycle_model` advances along the user's intent rather than the provider's
/// default order. If `patterns` is empty, returns `available` unchanged so
/// pre-#26 behavior is bit-for-bit preserved.
pub fn filter_by_allowlist(available: &[String], patterns: &[String]) -> Vec<String> {
    if patterns.is_empty() {
        return available.to_vec();
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for pat in patterns {
        for model in available {
            if seen.contains(model) {
                continue;
            }
            if matches_pattern(pat, model) {
                seen.insert(model.clone());
                out.push(model.clone());
            }
        }
    }
    out
}

/// Match `pattern` against `model`. Lower-cased on both sides.
///
/// - `*` matches any (possibly empty) span of characters.
/// - `?` matches a single character.
/// - Anything else is matched as a case-insensitive substring (so a bare
///   `sonnet` matches `claude-sonnet-4-6@1m` etc.).
fn matches_pattern(pattern: &str, model: &str) -> bool {
    let pat = pattern.to_lowercase();
    let m = model.to_lowercase();
    if pat.contains('*') || pat.contains('?') {
        glob_match(&pat, &m)
    } else {
        m.contains(&pat)
    }
}

/// Tiny glob matcher (`*` = many, `?` = single) — sufficient for model ids.
/// Avoids pulling a glob crate just for this. Iterative DP over `pat` vs `s`.
fn glob_match(pat: &str, s: &str) -> bool {
    let pb = pat.as_bytes();
    let sb = s.as_bytes();
    // dp[i][j] = pat[..i] matches s[..j]
    let mut dp = vec![vec![false; sb.len() + 1]; pb.len() + 1];
    dp[0][0] = true;
    for i in 1..=pb.len() {
        if pb[i - 1] == b'*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=pb.len() {
        for j in 1..=sb.len() {
            dp[i][j] = match pb[i - 1] {
                b'*' => dp[i - 1][j] || dp[i][j - 1],
                b'?' => dp[i - 1][j - 1],
                c => dp[i - 1][j - 1] && c.eq_ignore_ascii_case(&sb[j - 1]),
            };
        }
    }
    dp[pb.len()][sb.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn empty_allowlist_returns_input_unchanged() {
        let got = filter_by_allowlist(&s(&["a", "b", "c"]), &[]);
        assert_eq!(got, s(&["a", "b", "c"]));
    }

    #[test]
    fn substring_pattern_matches_case_insensitively() {
        let got = filter_by_allowlist(
            &s(&["claude-sonnet-4-6", "gpt-5.4-codex", "GEMINI-2.5-pro"]),
            &s(&["sonnet", "GEMINI"]),
        );
        // Output preserves pattern order, then per-pattern provider order.
        assert_eq!(got, s(&["claude-sonnet-4-6", "GEMINI-2.5-pro"]));
    }

    #[test]
    fn glob_pattern_matches() {
        let got = filter_by_allowlist(
            &s(&["claude-opus-4-6", "claude-sonnet-4-6", "gpt-5.4"]),
            &s(&["claude-*-4-6"]),
        );
        assert_eq!(got, s(&["claude-opus-4-6", "claude-sonnet-4-6"]));
    }

    #[test]
    fn dedup_preserves_first_pattern_match_order() {
        // Pattern A and B both match the same model — model only appears once,
        // in the position dictated by the first pattern.
        let got = filter_by_allowlist(
            &s(&["claude-sonnet-4-6", "claude-opus-4-6"]),
            &s(&["sonnet", "claude"]),
        );
        assert_eq!(got, s(&["claude-sonnet-4-6", "claude-opus-4-6"]));
    }

    #[test]
    fn unmatched_patterns_are_silently_dropped() {
        let got = filter_by_allowlist(
            &s(&["gpt-5.4", "gpt-4o"]),
            &s(&["does-not-exist", "gpt-5.4"]),
        );
        assert_eq!(got, s(&["gpt-5.4"]));
    }

    #[test]
    fn parse_pattern_list_trims_and_drops_empty() {
        let got = parse_pattern_list("sonnet , , gpt-* ,, claude  ");
        assert_eq!(got, s(&["sonnet", "gpt-*", "claude"]));
    }

    #[test]
    fn glob_match_question_mark_is_single_char() {
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "abbc"));
    }
}
