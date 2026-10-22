//! Eval-hygiene guards: keep grading honest.
//!
//! Ported from harness2's `harness2/core/context.py` +
//! `harness2/core/spec.py` (`BenchPrompts` validation) + `EXPERIMENTS.md`
//! web-access section + `BenchSpec.obs_trunc` floor (Apache-2.0, Google LLC).
//! Four rules, each learned from a real published failure:
//!
//! 1. **Prompt slots describe the WORK, not its measurement** (`spec.py`).
//!    Benchmark-shaped prompt text must never contain benchmark, evaluation,
//!    grader, rubric, or score: say what the practitioner needs.
//! 2. **Grader-verdict tripwire on evidence trees** (`context.py`). The
//!    grader's own output markers (`deterministic_eval_summary`,
//!    `"pass_rate"`, judge findings, ...) must never enter model context, by
//!    file NAME or by content. If the tripwire fires, find the read path
//!    that produced the match: it is a leak, not a false positive to widen
//!    around.
//! 3. **No leak paths.** Public benchmark rubrics are reachable online: in a
//!    validation run a Codex rollout downloaded its own task's `RUBRICS.json`
//!    unprompted. Answer-key paths are rejected before a context reaches a
//!    model (fnmatch semantics, `./`-prefix tolerant); egress
//!    (`huggingface.co`, `github.com`, ...) is restricted at the network
//!    level during evaluation and the restrictions are reported.
//! 4. **Observation truncation floor.** `obs_trunc < 10_000` is rejected: below
//!    that range the renderer *fabricates* defects that were never there.
//!    Default is 50_000.

/// Words that must never appear in benchmark-shaped prompt slots.
/// Mirrors harness2's `BenchPrompts.__post_init__`: prompt text must describe
/// the WORK, not its measurement.
pub const FORBIDDEN_PROMPT_WORDS: &[&str] =
    &["benchmark", "evaluation", "grader", "rubric", "score"];

/// Returns the offending word, if any, found case-insensitively.
pub fn find_prompt_measurement_word(text: &str) -> Option<&'static str> {
    let low = text.to_lowercase();
    FORBIDDEN_PROMPT_WORDS
        .iter()
        .find(|w| low.contains(**w))
        .copied()
}

/// The grader's own output markers. If any appears in a file NAME or in the
/// body of a scanned file, the context is refused. Mirrors harness2's
/// `GRADER_VERDICT_MARKERS`: a tripwire on the read boundary, not the
/// defence itself.
pub const GRADER_VERDICT_MARKERS: &[&str] = &[
    "deterministic_eval_summary",
    "deterministic_eval_result",
    "judge_scope_and_deterministic_status",
    "Judge findings on the rendered result",
    "\"passed_count\"",
    "\"pass_rate\"",
    "\"test_pass_rate\"",
];

/// Suffixes scanned for marker content. Other files are name-checked only.
/// Mirrors harness2's `_MARKER_SCAN_SUFFIXES`.
pub const MARKER_SCAN_SUFFIXES: &[&str] = &[
    ".md", ".txt", ".json", ".csv", ".tsv", ".yaml", ".yml", ".html", ".htm", ".log", ".py", ".js",
    ".sh", ".xml", ".ini", ".toml", ".patch", ".diff", "",
];

fn suffix_scanned(rel: &str) -> bool {
    let lower = rel.to_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    match file.rfind('.') {
        Some(dot) => MARKER_SCAN_SUFFIXES.contains(&&file[dot..]),
        None => MARKER_SCAN_SUFFIXES.contains(&""),
    }
}

/// Check one tree file against the tripwire. Returns the refusal message.
fn check_tree_file(rel: &str, body: &str) -> Option<String> {
    for marker in GRADER_VERDICT_MARKERS {
        if rel.contains(marker) {
            return Some(format!(
                "harness file {rel:?} has grader-verdict marker {marker:?} in its NAME; rename it: files carrying these strings cannot be served as evidence to later tasks"
            ));
        }
    }
    if !suffix_scanned(rel) {
        return None;
    }
    for marker in GRADER_VERDICT_MARKERS {
        if let Some(at) = body.find(marker) {
            let start = at.saturating_sub(60);
            let end = (at + 120).min(body.len());
            let window = body
                .get(start..end)
                .unwrap_or(&body[at..body.len().min(at + 1)]);
            return Some(format!(
                "harness file {rel:?} contains grader-verdict marker {marker:?} at offset {at} ({window:?}); remove or rename that string: files carrying it cannot be served as evidence to later tasks"
            ));
        }
    }
    None
}

/// Assert no tree file carries the grader's own output. The returned `Err`
/// is fed back verbatim as retry feedback upstream.
pub fn assert_tree_has_no_grader_verdicts(
    tree: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    let mut rels: Vec<&String> = tree.keys().collect();
    rels.sort();
    for rel in rels {
        if let Some(msg) = check_tree_file(rel, &tree[rel]) {
            return Err(msg);
        }
    }
    Ok(())
}

/// Back-compat alias: the prompt-slot word check.
pub fn find_grader_verdict(text: &str) -> Option<&'static str> {
    find_prompt_measurement_word(text)
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

/// Returns true when `path` matches a benchmark answer-key glob.
/// Mirrors harness2's `assert_no_leak_paths`: the pattern is tried as-is and
/// as `*/<pattern>` (after stripping a leading `./`), with Python `fnmatch`
/// semantics: `*` spans any run INCLUDING separators, `?` any single char
/// including the separator, `[...]` a class including the separator, and
/// `**` any run including separators (so `**/` also matches zero dirs).
pub fn path_matches_leak_glob(path: &str, glob: &str) -> bool {
    let pat = glob.replace('\\', "/").trim_start_matches("./").to_string();
    fnmatch_match(&pat, path) || fnmatch_match(&format!("*/{pat}"), path)
}

fn fnmatch_match(pattern: &str, name: &str) -> bool {
    // Operates on Unicode scalar values, not bytes: `?` consumes one
    // character (Greptile P2), and memoization bounds `*` backtracking
    // to O(P*N) states instead of exponential retry (Greptile P2).
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let mut memo: std::collections::HashMap<(usize, usize), bool> =
        std::collections::HashMap::new();
    fn go(
        p: &[char],
        n: &[char],
        pi: usize,
        ni: usize,
        memo: &mut std::collections::HashMap<(usize, usize), bool>,
    ) -> bool {
        if let Some(&cached) = memo.get(&(pi, ni)) {
            return cached;
        }
        let out = go_inner(p, n, pi, ni, memo);
        memo.insert((pi, ni), out);
        out
    }
    fn go_inner(
        p: &[char],
        n: &[char],
        pi: usize,
        ni: usize,
        memo: &mut std::collections::HashMap<(usize, usize), bool>,
    ) -> bool {
        if pi == p.len() {
            return ni == n.len();
        }
        // `**/` spans separators (incl. zero dirs); lone `**` spans all.
        if p[pi] == '*' && pi + 1 < p.len() && p[pi + 1] == '*' {
            if pi + 2 < p.len() && p[pi + 2] == '/' {
                for i in ni..=n.len() {
                    if go(p, n, pi + 3, i, memo) {
                        return true;
                    }
                }
                return false;
            }
            for i in ni..=n.len() {
                if go(p, n, pi + 2, i, memo) {
                    return true;
                }
            }
            return false;
        }
        // fnmatch `*`: any run, separators included.
        if p[pi] == '*' {
            for i in ni..=n.len() {
                if go(p, n, pi + 1, i, memo) {
                    return true;
                }
            }
            return false;
        }
        // fnmatch `?`: any single character.
        if p[pi] == '?' {
            if ni == n.len() {
                return false;
            }
            return go(p, n, pi + 1, ni + 1, memo);
        }
        if p[pi] == '[' {
            let Some(rel_close) = p[pi..].iter().position(|&c| c == ']') else {
                // No closing bracket: literal `[`.
                if ni == n.len() || n[ni] != '[' {
                    return false;
                }
                return go(p, n, pi + 1, ni + 1, memo);
            };
            if ni == n.len() {
                return false;
            }
            let close = pi + rel_close;
            let mut class = &p[pi + 1..close];
            let rest = close + 1;
            let mut negate = false;
            if !class.is_empty() && (class[0] == '!' || class[0] == '^') {
                negate = true;
                class = &class[1..];
            }
            let mut hit = false;
            let mut ci = 0;
            while ci < class.len() {
                if ci + 2 < class.len() && class[ci + 1] == '-' {
                    if class[ci] <= n[ni] && n[ni] <= class[ci + 2] {
                        hit = true;
                    }
                    ci += 3;
                } else {
                    if class[ci] == n[ni] {
                        hit = true;
                    }
                    ci += 1;
                }
            }
            if hit == negate {
                return false;
            }
            return go(p, n, rest, ni + 1, memo);
        }
        if ni == n.len() || p[pi] != n[ni] {
            return false;
        }
        go(p, n, pi + 1, ni + 1, memo)
    }
    go(&p, &n, 0, 0, &mut memo)
}

// Reject answer-key paths before a context reaches a model.
/// Mirrors harness2's `assert_no_leak_paths`.
pub fn assert_no_leak_paths(paths: &[&str], patterns: &[&str]) -> Result<(), String> {
    if patterns.is_empty() {
        return Ok(());
    }
    let mut hits = Vec::new();
    for path in paths {
        for pattern in patterns {
            if path_matches_leak_glob(path, pattern) {
                hits.push((*path).to_string());
                break;
            }
        }
    }
    if hits.is_empty() {
        Ok(())
    } else {
        hits.sort();
        let shown: Vec<String> = hits.into_iter().take(8).collect();
        Err(format!(
            "answer-key path(s) would enter model context: {}",
            shown.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_slots_reject_measurement_words() {
        assert_eq!(
            find_prompt_measurement_word("The GRADER expects X"),
            Some("grader")
        );
        assert!(find_prompt_measurement_word("reconcile figures before writing").is_none());
        // Alias keeps working.
        assert_eq!(find_grader_verdict("maximise the score"), Some("score"));
    }

    #[test]
    fn tripwire_fires_on_marker_content_not_ordinary_words() {
        let mut t = std::collections::HashMap::new();
        // Ordinary measurement words are NOT the tripwire: they describe
        // prompts, not grader output. Content markers are.
        t.insert(
            "systemprompt.md".to_string(),
            "maximise the score".to_string(),
        );
        assert_tree_has_no_grader_verdicts(&t).expect("ordinary words pass the tripwire");
        t.insert(
            "guardrails.md".to_string(),
            "result: \"pass_rate\": 0.9".to_string(),
        );
        let err = assert_tree_has_no_grader_verdicts(&t).unwrap_err();
        assert!(err.contains("guardrails.md"), "{err}");
        // Marker is debug-formatted in the message, so match the bare name.
        assert!(err.contains("pass_rate"), "{err}");
    }

    #[test]
    fn tripwire_fires_on_marker_in_name() {
        let mut t = std::collections::HashMap::new();
        t.insert(
            "deterministic_eval_summary.md".to_string(),
            "innocent body".to_string(),
        );
        let err = assert_tree_has_no_grader_verdicts(&t).unwrap_err();
        assert!(err.contains("in its NAME"), "{err}");
    }

    #[test]
    fn tripwire_suffix_scan_is_case_insensitive() {
        let mut t = std::collections::HashMap::new();
        t.insert("A.MD".to_string(), "\"pass_rate\":1".to_string());
        assert!(assert_tree_has_no_grader_verdicts(&t).is_err());
        // Unlisted suffix: content passes, name still trips.
        let mut u = std::collections::HashMap::new();
        u.insert("artifact.png".to_string(), "\"pass_rate\":1".to_string());
        assert!(assert_tree_has_no_grader_verdicts(&u).is_ok());
        u.insert(
            "deterministic_eval_summary.png".to_string(),
            "innocent".to_string(),
        );
        assert!(assert_tree_has_no_grader_verdicts(&u).is_err());
    }

    #[test]
    fn tripwire_skips_unscanned_suffixes_for_content() {
        let mut t = std::collections::HashMap::new();
        // A binary-ish suffix is name-checked only.
        t.insert("artifact.png".to_string(), "\"pass_rate\": 0.9".to_string());
        assert_tree_has_no_grader_verdicts(&t).expect("unscanned suffix skips content check");
    }

    #[test]
    fn trunc_floor_rejects_fabrication_range() {
        assert!(check_obs_trunc(9_999).is_err());
        assert_eq!(check_obs_trunc(50_000).unwrap(), 50_000);
    }

    #[test]
    fn leak_globs_match_fnmatch_semantics() {
        assert!(path_matches_leak_glob("RUBRICS.json", "*.json"));
        assert!(path_matches_leak_glob(
            "data/RUBRICS.json",
            "**/RUBRICS.json"
        ));
        assert!(!path_matches_leak_glob("data/report.md", "**/RUBRICS.json"));
        // Python fnmatch `*` spans separators (unlike glob): verified with
        // `fnmatch.fnmatch('a/b.json', '*.json') is True` on 2026-10-18.
        assert!(path_matches_leak_glob("a/b.json", "*.json"));
        // `./`-prefix tolerant, bare name matches at any depth.
        assert!(path_matches_leak_glob(
            "data/RUBRICS.json",
            "./RUBRICS.json"
        ));
        assert!(path_matches_leak_glob("RUBRICS.json", "RUBRICS.json"));
        // `?` and classes behave (also separator-crossing, per fnmatch).
        assert!(path_matches_leak_glob("a1.json", "a?.json"));
        assert!(path_matches_leak_glob("a1.json", "a[0-9].json"));
        // `?` is exactly one char: `ab` (two) does not fit `a?`.
        assert!(!path_matches_leak_glob("abc.json", "a?.json"));
        assert!(path_matches_leak_glob("a/b", "a?b"));
        // Greptile P2: one ? per CHARACTER, not per byte.
        assert!(path_matches_leak_glob("秘果.json", "??.json"));
        assert!(!path_matches_leak_glob("秘.json", "??.json"));
        // Classes, negation, `**`, and empties under true fnmatch.
        assert!(path_matches_leak_glob("a", "[a-z]"));
        assert!(!path_matches_leak_glob("A", "[a-z]"));
        assert!(path_matches_leak_glob("A", "[!a-z]"));
        assert!(path_matches_leak_glob("[", "[[]"));
        assert!(!path_matches_leak_glob("x", "["));
        assert!(path_matches_leak_glob("anything", "**"));
        assert!(path_matches_leak_glob("", "**"));
        assert!(path_matches_leak_glob("", "*"));
        assert!(path_matches_leak_glob("a/b", "a*b"));
        assert!(!path_matches_leak_glob("AXB", "a*b"));
    }

    #[test]
    fn wildcard_matching_is_bounded() {
        // Greptile P2: the adversarial '*a*x40*b' case must finish fast.
        let pat = "*a".repeat(20) + "b";
        let name = "a".repeat(48);
        let t = std::time::Instant::now();
        let matched = path_matches_leak_glob(&name, &pat);
        assert!(t.elapsed().as_secs() < 5, "took {:?}", t.elapsed());
        let _ = matched;
        // And correctness is preserved on the classic blowup shape.
        assert!(path_matches_leak_glob(
            &("a".repeat(40) + "b"),
            &("*a".repeat(8) + "b")
        ));
        assert!(!path_matches_leak_glob(
            &"a".repeat(40),
            &("*a".repeat(8) + "b")
        ));
    }

    #[test]
    fn leak_assert_lists_hits() {
        let err = assert_no_leak_paths(&["src/main.rs", "data/RUBRICS.json"], &["**/RUBRICS.json"])
            .unwrap_err();
        assert!(err.contains("data/RUBRICS.json"), "{err}");
        assert!(assert_no_leak_paths(&["src/main.rs"], &["**/RUBRICS.json"]).is_ok());
        assert!(assert_no_leak_paths(&["anything"], &[]).is_ok());
    }

    #[test]
    fn blocklist_covers_published_hosts() {
        assert!(EVAL_EGRESS_BLOCKLIST.contains(&"huggingface.co"));
        assert!(EVAL_EGRESS_BLOCKLIST.contains(&"github.com"));
    }
}
