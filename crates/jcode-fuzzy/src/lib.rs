//! Small typo-resistant fuzzy matcher shared by jcode's terminal and desktop UIs.
//!
//! The matcher combines subsequence matching with a bounded number of
//! substitutions, adjacent transpositions, and extra typed characters. Exact,
//! consecutive, boundary, and prefix matches receive bonuses, so typo tolerance
//! does not displace stronger literal matches.

fn is_boundary(c: char) -> bool {
    matches!(c, '/' | '-' | '_' | ' ' | '.' | ':')
}

const MATCH: i32 = 16;
const CONSECUTIVE: i32 = 8;
const BOUNDARY: i32 = 9;
const FIRST: i32 = 12;
const GAP: i32 = -1;
const LEADING_GAP: i32 = -3;
const SUBSTITUTION: i32 = -10;
const DELETION: i32 = -12;
const TRANSPOSITION: i32 = 2 * MATCH - 22;
const EXACT: i32 = 32;

/// Result of a successful fuzzy match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuzzyMatch {
    /// Higher scores are better.
    pub score: i32,
    /// Matched haystack character indices, sorted ascending. Substitutions and
    /// deleted pattern characters are intentionally not highlighted.
    pub positions: Vec<usize>,
}

/// Storage strategy for matched haystack positions inside the DP table.
///
/// Score-only callers (`fuzzy_score`, `fuzzy_score_tokens`) run the matcher
/// once per entry per keystroke in picker filters, so they use the allocation
/// free [`PositionSummary`] tracker. Highlighting callers use `Vec<usize>`.
/// Both trackers produce identical scores because tie-breaking only inspects
/// the match count and the anchor check only inspects the first position.
trait PositionTracker: Clone + Default {
    fn push_pos(&mut self, pos: usize);
    fn pos_count(&self) -> usize;
    fn first_pos(&self) -> Option<usize>;
}

impl PositionTracker for Vec<usize> {
    fn push_pos(&mut self, pos: usize) {
        self.push(pos);
    }
    fn pos_count(&self) -> usize {
        self.len()
    }
    fn first_pos(&self) -> Option<usize> {
        self.as_slice().first().copied()
    }
}

/// Allocation-free tracker keeping only what scoring needs: the number of
/// true matches (tie-breaking) and the first matched index (anchoring).
#[derive(Clone, Copy, Default)]
struct PositionSummary {
    count: u32,
    first: Option<usize>,
}

impl PositionTracker for PositionSummary {
    fn push_pos(&mut self, pos: usize) {
        if self.first.is_none() {
            self.first = Some(pos);
        }
        self.count += 1;
    }
    fn pos_count(&self) -> usize {
        self.count as usize
    }
    fn first_pos(&self) -> Option<usize> {
        self.first
    }
}

#[derive(Clone)]
struct Cell<P: PositionTracker> {
    score: i32,
    errors: u8,
    last: i32,
    tail_true: bool,
    positions: P,
}

fn keep_best<P: PositionTracker>(slot: &mut Option<Cell<P>>, candidate: Cell<P>) {
    let replace = match slot {
        None => true,
        Some(existing) => {
            candidate.score > existing.score
                || (candidate.score == existing.score && candidate.errors < existing.errors)
                || (candidate.score == existing.score
                    && candidate.errors == existing.errors
                    && candidate.positions.pos_count() > existing.positions.pos_count())
        }
    };
    if replace {
        *slot = Some(candidate);
    }
}

fn error_budget(meaningful_len: usize) -> u8 {
    match meaningful_len {
        0..=2 => 0,
        3..=8 => 1,
        _ => 2,
    }
}

fn fuzzy_match_impl<P: PositionTracker>(
    needle: &str,
    haystack: &str,
    anchor_first_true_match: bool,
    strip_leading_slash: bool,
    require_true_tail: bool,
) -> Option<(i32, P, usize)> {
    let (hay_offset, hay_src) = if strip_leading_slash {
        match haystack.strip_prefix('/') {
            Some(rest) => (1usize, rest),
            None => (0usize, haystack),
        }
    } else {
        (0usize, haystack)
    };
    let needle_src = if strip_leading_slash {
        needle.strip_prefix('/').unwrap_or(needle)
    } else {
        needle
    };

    let pat: Vec<char> = needle_src.chars().flat_map(char::to_lowercase).collect();
    let hay: Vec<char> = hay_src.chars().flat_map(char::to_lowercase).collect();

    if pat.iter().all(|c| c.is_whitespace()) {
        return Some((0, P::default(), hay_offset));
    }
    if hay.is_empty() {
        return None;
    }

    let m = pat.len();
    let n = hay.len();
    let meaningful = pat.iter().filter(|c| !c.is_whitespace()).count();
    let max_err = error_budget(meaningful);
    let mut dp: Vec<Vec<Option<Cell<P>>>> = vec![vec![None; n + 1]; m + 1];
    dp[0][0] = Some(Cell {
        score: 0,
        errors: 0,
        last: -1,
        tail_true: true,
        positions: P::default(),
    });
    for j in 1..=n {
        if let Some(prev) = dp[0][j - 1].clone() {
            dp[0][j] = Some(Cell {
                score: prev.score + LEADING_GAP,
                errors: prev.errors,
                last: prev.last,
                tail_true: prev.tail_true,
                positions: prev.positions,
            });
        }
    }

    for i in 1..=m {
        for j in 0..=n {
            let mut best = None;

            if j >= 1
                && let Some(prev) = dp[i][j - 1].clone()
            {
                keep_best(
                    &mut best,
                    Cell {
                        score: prev.score + GAP,
                        errors: prev.errors,
                        last: prev.last,
                        tail_true: prev.tail_true,
                        positions: prev.positions,
                    },
                );
            }

            if j >= 1
                && let Some(prev) = dp[i - 1][j - 1].clone()
            {
                let pos = j - 1;
                if pat[i - 1] == hay[pos] {
                    let mut score = prev.score + MATCH;
                    if prev.last == pos as i32 - 1 {
                        score += CONSECUTIVE;
                    }
                    if pos == 0 || is_boundary(hay[pos - 1]) {
                        score += BOUNDARY;
                    }
                    if i == 1 && pos == 0 {
                        score += FIRST;
                    }
                    let mut positions = prev.positions.clone();
                    positions.push_pos(pos);
                    keep_best(
                        &mut best,
                        Cell {
                            score,
                            errors: prev.errors,
                            last: pos as i32,
                            tail_true: true,
                            positions,
                        },
                    );
                } else if prev.errors < max_err
                    && !pat[i - 1].is_whitespace()
                    && !hay[pos].is_whitespace()
                {
                    keep_best(
                        &mut best,
                        Cell {
                            score: prev.score + SUBSTITUTION,
                            errors: prev.errors + 1,
                            last: pos as i32,
                            tail_true: false,
                            positions: prev.positions,
                        },
                    );
                }
            }

            if !pat[i - 1].is_whitespace()
                && let Some(prev) = dp[i - 1][j].clone()
                && prev.errors < max_err
            {
                keep_best(
                    &mut best,
                    Cell {
                        score: prev.score + DELETION,
                        errors: prev.errors + 1,
                        last: prev.last,
                        tail_true: false,
                        positions: prev.positions,
                    },
                );
            }

            if i >= 2
                && j >= 2
                && pat[i - 1] == hay[j - 2]
                && pat[i - 2] == hay[j - 1]
                && pat[i - 1] != pat[i - 2]
                && !pat[i - 1].is_whitespace()
                && !pat[i - 2].is_whitespace()
                && let Some(prev) = dp[i - 2][j - 2].clone()
                && prev.errors < max_err
            {
                let first = j - 2;
                let mut score = prev.score + TRANSPOSITION;
                if first == 0 || is_boundary(hay[first - 1]) {
                    score += BOUNDARY;
                }
                let mut positions = prev.positions.clone();
                positions.push_pos(first);
                positions.push_pos(j - 1);
                keep_best(
                    &mut best,
                    Cell {
                        score,
                        errors: prev.errors + 1,
                        last: (j - 1) as i32,
                        tail_true: true,
                        positions,
                    },
                );
            }

            dp[i][j] = best;
        }
    }

    let mut answer = None;
    for row in &dp[m] {
        if let Some(cell) = row.clone()
            && (!require_true_tail || cell.tail_true)
        {
            keep_best(&mut answer, cell);
        }
    }

    let cell = answer?;
    if anchor_first_true_match && cell.positions.first_pos() != Some(0) {
        return None;
    }

    let exact = pat == hay;
    Some((
        cell.score + if exact { EXACT } else { 0 },
        cell.positions,
        hay_offset,
    ))
}

fn fuzzy_match_full(
    needle: &str,
    haystack: &str,
    anchor_first_true_match: bool,
    strip_leading_slash: bool,
    require_true_tail: bool,
) -> Option<FuzzyMatch> {
    let (score, positions, hay_offset) = fuzzy_match_impl::<Vec<usize>>(
        needle,
        haystack,
        anchor_first_true_match,
        strip_leading_slash,
        require_true_tail,
    )?;
    Some(FuzzyMatch {
        score,
        positions: positions.into_iter().map(|p| p + hay_offset).collect(),
    })
}

fn fuzzy_score_only(
    needle: &str,
    haystack: &str,
    anchor_first_true_match: bool,
    strip_leading_slash: bool,
    require_true_tail: bool,
) -> Option<i32> {
    fuzzy_match_impl::<PositionSummary>(
        needle,
        haystack,
        anchor_first_true_match,
        strip_leading_slash,
        require_true_tail,
    )
    .map(|(score, _, _)| score)
}

/// Match free-form picker/search text. The match may begin at any word boundary
/// or interior position, with earlier and boundary-aligned matches scoring higher.
pub fn fuzzy_match(needle: &str, haystack: &str) -> Option<FuzzyMatch> {
    fuzzy_match_full(needle, haystack, false, false, false)
}

/// Return only the free-form fuzzy score.
pub fn fuzzy_score(needle: &str, haystack: &str) -> Option<i32> {
    fuzzy_score_only(needle, haystack, false, false, false)
}

/// Score search text composed of whitespace-separated metadata fields. A
/// single-token query must match within one field, which prevents a weak match
/// from stitching characters across unrelated model, provider, and detail
/// columns. Multi-word queries may intentionally span the full text.
pub fn fuzzy_score_tokens(needle: &str, haystack: &str) -> Option<i32> {
    let needle = needle.trim();
    if needle.is_empty() {
        return Some(0);
    }
    if needle.contains(char::is_whitespace) {
        return fuzzy_score(needle, haystack);
    }
    haystack
        .split_whitespace()
        .filter_map(|token| fuzzy_score(needle, token))
        .max()
}

/// Return matched positions for free-form picker highlighting.
pub fn fuzzy_match_positions(needle: &str, haystack: &str) -> Vec<usize> {
    fuzzy_match(needle, haystack)
        .map(|matched| matched.positions)
        .unwrap_or_default()
}

/// Match a slash command. A leading slash is ignored for scoring, and the first
/// true character match remains anchored to the command's first letter to keep
/// short slash suggestions precise.
pub fn command_fuzzy_match(needle: &str, haystack: &str) -> Option<FuzzyMatch> {
    fuzzy_match_full(needle, haystack, true, true, true)
}

/// Return only the slash-command fuzzy score.
pub fn command_fuzzy_score(needle: &str, haystack: &str) -> Option<i32> {
    fuzzy_score_only(needle, haystack, true, true, true)
}

/// Return matched positions for slash-command highlighting.
pub fn command_fuzzy_match_positions(needle: &str, haystack: &str) -> Vec<usize> {
    command_fuzzy_match(needle, haystack)
        .map(|matched| matched.positions)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_form_matching_tolerates_common_typos() {
        assert!(fuzzy_match("codxe", "gpt-5-codex").is_some());
        assert!(fuzzy_match("opuz", "claude-opus-4.6").is_some());
        assert!(fuzzy_match("tikcet", "ticket-workspace").is_some());
        assert!(fuzzy_match("coonfig", "config").is_some());
    }

    #[test]
    fn exact_and_prefix_matches_outrank_typo_matches() {
        let exact = fuzzy_score("codex", "codex").unwrap();
        let prefix = fuzzy_score("codex", "codex-mini").unwrap();
        let typo = fuzzy_score("codxe", "codex").unwrap();
        assert!(exact > prefix);
        assert!(prefix > typo);
    }

    #[test]
    fn exact_token_match_outranks_a_longer_prefix_token() {
        let exact = fuzzy_score_tokens("gpt-5", "gpt-5 openai responses").unwrap();
        let longer = fuzzy_score_tokens("gpt-5", "gpt-5.5 openai responses").unwrap();
        assert!(exact > longer);
    }

    #[test]
    fn command_matching_preserves_anchor_and_positions() {
        let matched = command_fuzzy_match("/conifg", "/config").unwrap();
        assert_eq!(matched.positions.first(), Some(&1));
        assert!(command_fuzzy_match("/g", "/config").is_none());
    }

    #[test]
    fn rejects_short_or_distant_noise() {
        assert!(fuzzy_match("xz", "config").is_none());
        assert!(fuzzy_match("configuration", "model").is_none());
    }

    #[test]
    fn token_scoring_does_not_stitch_across_metadata_fields() {
        assert!(fuzzy_score_tokens("codxe", "gpt-5-codex openai coding model").is_some());
        assert!(fuzzy_score_tokens("codxe", "claude-opus anthropic premium").is_none());
    }
}
