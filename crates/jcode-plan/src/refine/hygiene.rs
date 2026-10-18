//! Eval-hygiene guards: keep grading honest.
//!
//! Ported from harness2's `harness2/core/context.py` + `EXPERIMENTS.md`
//! web-access section + `BenchSpec.obs_trunc` floor (Apache-2.0, Google LLC).
//! Three rules, each learned from a real published failure:
//!
//! 1. **No grader verdicts in the improver's evidence.** Rubrics stay outside
//!    the improver's evidence; criteria are stripped from material passed to
//!    the improver. A harness that tells the agent what the grader wants is
//!    not an improvement, it is leakage.
//! 2. **No leak paths.** Public benchmark rubrics are reachable online: in a
//!    validation run a Codex rollout downloaded its own task's `RUBRICS.json`
//!    unprompted. Restrict egress (`huggingface.co`, `github.com`, ...) at the
//!    network level during evaluation and report the restrictions used.
//! 3. **Observation truncation floor.** `obs_trunc < 10_000` is rejected: below
//!    that range the renderer *fabricates* defects that were never there.
//!    Default is 50_000.

/// Words that must never appear in material handed to the improver.
/// Mirrors harness2's `BenchPrompts` validation: prompt slots must describe
/// the WORK, not its measurement.
pub const FORBIDDEN_EVIDENCE_WORDS: &[&str] =
    &["benchmark", "evaluation", "grader", "rubric", "score"];

/// Returns the offending word, if any, found case-insensitively.
pub fn find_grader_verdict(text: &str) -> Option<&'static str> {
    let low = text.to_lowercase();
    FORBIDDEN_EVIDENCE_WORDS
        .iter()
        .find(|w| low.contains(**w))
        .copied()
}

/// Assert no tree file carries a grader verdict. The returned `Err` is fed
/// back verbatim as retry feedback upstream.
pub fn assert_tree_has_no_grader_verdicts(
    tree: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    let mut offenders = Vec::new();
    let mut rels: Vec<&String> = tree.keys().collect();
    rels.sort();
    for rel in rels {
        if let Some(word) = find_grader_verdict(&tree[rel]) {
            offenders.push(format!("{rel} mentions {word:?}"));
        }
    }
    if offenders.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "grader verdicts in harness (describe the WORK, not its measurement): {}",
            offenders.join("; ")
        ))
    }
}

/// Hosts to block at the network level during evaluation runs.
/// From harness2's published web-access guidance.
pub const EVAL_EGRESS_BLOCKLIST: &[&str] = &[
    "huggingface.co",
    "hf.co",
    "*.hf.co",
    "github.com",
    "raw.githubusercontent.com",
];

/// Minimum observation truncation: below this the renderer fabricates
/// defects. harness2 raises for `obs_trunc < 10_000`.
pub const OBS_TRUNC_FLOOR: usize = 10_000;
/// Default observation truncation.
pub const OBS_TRUNC_DEFAULT: usize = 50_000;

pub fn check_obs_trunc(obs_trunc: usize) -> Result<usize, String> {
    if obs_trunc < OBS_TRUNC_FLOOR {
        return Err(format!(
            "obs_trunc={obs_trunc} is in the range that fabricates defects; use >= {OBS_TRUNC_FLOOR} (default {OBS_TRUNC_DEFAULT})"
        ));
    }
    Ok(obs_trunc)
}

/// Returns true when `path` matches one of the benchmark's leak globs
/// (`*` matches any run of chars except `/`; `**` spans separators).
pub fn path_matches_leak_glob(path: &str, glob: &str) -> bool {
    fn go(p: &[u8], g: &[u8]) -> bool {
        if g.is_empty() {
            return p.is_empty();
        }
        if g.len() >= 2 && g[0] == b'*' && g[1] == b'*' {
            let rest = if g.len() > 2 && g[2] == b'/' {
                &g[3..]
            } else {
                &g[2..]
            };
            for i in 0..=p.len() {
                if go(&p[i..], rest) {
                    return true;
                }
            }
            return false;
        }
        if g[0] == b'*' {
            for i in 0..=p.len() {
                if i > 0 && p[i - 1] == b'/' {
                    break;
                }
                if go(&p[i..], &g[1..]) {
                    return true;
                }
            }
            return false;
        }
        if p.is_empty() || p[0] != g[0] {
            return false;
        }
        go(&p[1..], &g[1..])
    }
    go(path.as_bytes(), glob.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_words_detected_case_insensitively() {
        assert_eq!(find_grader_verdict("The GRADER expects X"), Some("grader"));
        assert!(find_grader_verdict("reconcile figures before writing").is_none());
    }

    #[test]
    fn tree_assert_lists_offenders() {
        let mut t = std::collections::HashMap::new();
        t.insert(
            "systemprompt.md".to_string(),
            "maximise the score".to_string(),
        );
        t.insert("guardrails.md".to_string(), "do good work".to_string());
        let err = assert_tree_has_no_grader_verdicts(&t).unwrap_err();
        assert!(err.contains("systemprompt.md"), "{err}");
        assert!(!err.contains("guardrails.md"), "{err}");
    }

    #[test]
    fn trunc_floor_rejects_fabrication_range() {
        assert!(check_obs_trunc(9_999).is_err());
        assert_eq!(check_obs_trunc(50_000).unwrap(), 50_000);
    }

    #[test]
    fn leak_globs_match() {
        assert!(path_matches_leak_glob("RUBRICS.json", "*.json"));
        assert!(path_matches_leak_glob(
            "data/RUBRICS.json",
            "**/RUBRICS.json"
        ));
        assert!(!path_matches_leak_glob("data/report.md", "**/RUBRICS.json"));
        assert!(!path_matches_leak_glob("a/b.json", "*.json"));
    }

    #[test]
    fn blocklist_covers_published_hosts() {
        assert!(EVAL_EGRESS_BLOCKLIST.contains(&"huggingface.co"));
        assert!(EVAL_EGRESS_BLOCKLIST.contains(&"github.com"));
    }
}
