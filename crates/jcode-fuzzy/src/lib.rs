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
const GAP: i32 = -3;
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

/// Minimum acceptable score for a match, scaled by pattern length. Weak,
/// scattered matches (long gaps, mostly substitutions/deletions) fall below
/// this floor and are discarded instead of cluttering picker results.
fn score_floor(meaningful_len: usize) -> i32 {
    // A clean boundary-anchored match earns at least MATCH per char plus
    // bonuses. Requiring slightly more than half of the plain per-char MATCH
    // total keeps typo matches while rejecting noise stitched across a token.
    (meaningful_len as i32) * MATCH * 11 / 20
}

/// Cheap rejection test: every pattern char (up to the error budget) must be
/// present in the haystack at all. This avoids running the DP for the vast
/// majority of non-matching entries.
fn prefilter(pat: &[char], hay: &[char], max_err: u8) -> bool {
    // Stage 1: ASCII presence bitmask. This ignores multiplicity (a pattern
    // with repeated chars can pass with a single haystack occurrence), so it
    // is a strictly weaker test than the counting stage below, but it rejects
    // most non-matching entries without touching a 128-slot count table.
    let mut hay_mask = 0u128;
    let mut hay_has_non_ascii = false;
    for &c in hay {
        let idx = c as usize;
        if idx < 128 {
            hay_mask |= 1u128 << idx;
        } else {
            hay_has_non_ascii = true;
        }
    }
    let mut missing = 0u8;
    for &c in pat {
        if c.is_whitespace() {
            continue;
        }
        let idx = c as usize;
        let maybe_present = if idx < 128 {
            hay_mask & (1u128 << idx) != 0
        } else {
            hay_has_non_ascii
        };
        if !maybe_present {
            missing += 1;
            if missing > max_err {
                return false;
            }
        }
    }

    // Stage 2: multiplicity-aware counting for candidates that passed.
    let mut ascii = [0u16; 128];
    let mut other: Vec<char> = Vec::new();
    for &c in hay {
        let idx = c as usize;
        if idx < 128 {
            ascii[idx] += 1;
        } else {
            other.push(c);
        }
    }
    let mut missing = 0u8;
    for &c in pat {
        if c.is_whitespace() {
            continue;
        }
        let present = {
            let idx = c as usize;
            if idx < 128 {
                if ascii[idx] > 0 {
                    ascii[idx] -= 1;
                    true
                } else {
                    false
                }
            } else if let Some(pos) = other.iter().position(|&h| h == c) {
                other.swap_remove(pos);
                true
            } else {
                false
            }
        };
        if !present {
            missing += 1;
            if missing > max_err {
                return false;
            }
        }
    }
    true
}

/// Core DP over pre-lowered pattern/haystack chars. Rows are caller-provided
/// so hot per-keystroke callers can reuse their allocations; they are resized
/// and cleared here. Returns the best final-row cell honoring
/// `require_true_tail`.
fn run_dp<P: PositionTracker>(
    pat: &[char],
    hay: &[char],
    max_err: u8,
    row_prev2: &mut Vec<Option<Cell<P>>>,
    row_prev: &mut Vec<Option<Cell<P>>>,
    row_cur: &mut Vec<Option<Cell<P>>>,
    require_true_tail: bool,
) -> Option<Cell<P>> {
    let m = pat.len();
    let n = hay.len();
    row_prev2.clear();
    row_prev.clear();
    row_cur.clear();
    row_prev2.resize(n + 1, None);
    row_prev.resize(n + 1, None);
    row_cur.resize(n + 1, None);
    row_prev[0] = Some(Cell {
        score: 0,
        errors: 0,
        last: -1,
        tail_true: true,
        positions: P::default(),
    });
    for j in 1..=n {
        if let Some(prev) = row_prev[j - 1].clone() {
            row_prev[j] = Some(Cell {
                score: prev.score + LEADING_GAP,
                errors: prev.errors,
                last: prev.last,
                tail_true: prev.tail_true,
                positions: prev.positions,
            });
        }
    }

    for i in 1..=m {
        for cell in row_cur.iter_mut() {
            *cell = None;
        }
        for j in 0..=n {
            let mut best = None;

            if j >= 1
                && let Some(prev) = row_cur[j - 1].clone()
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
                && let Some(prev) = row_prev[j - 1].clone()
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
                && let Some(prev) = row_prev[j].clone()
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
                && let Some(prev) = row_prev2[j - 2].clone()
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

            row_cur[j] = best;
        }
        std::mem::swap(row_prev2, row_prev);
        std::mem::swap(row_prev, row_cur);
    }

    let mut answer = None;
    for row in row_prev.iter() {
        if let Some(cell) = row.clone()
            && (!require_true_tail || cell.tail_true)
        {
            keep_best(&mut answer, cell);
        }
    }
    answer
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

    if pat.iter().all(|c| c.is_whitespace()) {
        return Some((0, P::default(), hay_offset));
    }

    let meaningful = pat.iter().filter(|c| !c.is_whitespace()).count();
    let max_err = error_budget(meaningful);

    // Cheap length gate before lowercasing the haystack: every meaningful
    // pattern char beyond the error budget must consume a haystack char, and
    // a haystack's char count never exceeds its byte length (lowercasing only
    // expands). Skipping short tokens here avoids most DP work per keystroke.
    if hay_src.len() + (max_err as usize) < meaningful {
        return None;
    }

    let hay: Vec<char> = hay_src.chars().flat_map(char::to_lowercase).collect();
    if hay.is_empty() {
        return None;
    }

    let n = hay.len();
    if n + (max_err as usize) < meaningful {
        return None;
    }
    if !prefilter(&pat, &hay, max_err) {
        return None;
    }
    let mut row_prev2: Vec<Option<Cell<P>>> = Vec::new();
    let mut row_prev: Vec<Option<Cell<P>>> = Vec::new();
    let mut row_cur: Vec<Option<Cell<P>>> = Vec::new();
    let answer = run_dp(
        &pat,
        &hay,
        max_err,
        &mut row_prev2,
        &mut row_prev,
        &mut row_cur,
        require_true_tail,
    );

    let cell = answer?;
    if anchor_first_true_match && cell.positions.first_pos() != Some(0) {
        return None;
    }

    let exact = pat == hay;
    let score = cell.score + if exact { EXACT } else { 0 };
    if !exact && score < score_floor(meaningful) {
        return None;
    }
    Some((score, cell.positions, hay_offset))
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

/// Score search text composed of whitespace-separated metadata fields. Each
/// query word must match within one field, which prevents a weak match from
/// stitching characters across unrelated model, provider, and detail columns.
/// Multi-word query scores are the sum of the best per-word field scores, and
/// every word must match somewhere for the entry to match at all.
pub fn fuzzy_score_tokens(needle: &str, haystack: &str) -> Option<i32> {
    PreparedTokenQuery::new(needle).score(haystack)
}

/// A parsed multi-word query prepared once and scored against many entries.
///
/// Picker filters call the matcher once per entry per keystroke, so the
/// pattern lowercase/error-budget work and the DP scratch rows are prepared
/// here once and reused across every entry instead of being reallocated for
/// each token of each entry.
pub struct PreparedTokenQuery {
    words: Vec<PreparedWord>,
    scratch: std::cell::RefCell<ScoreScratch>,
}

struct PreparedWord {
    chars: Vec<char>,
    meaningful: usize,
    max_err: u8,
    floor: i32,
}

#[derive(Default)]
struct ScoreScratch {
    hay: Vec<char>,
    row_prev2: Vec<Option<Cell<PositionSummary>>>,
    row_prev: Vec<Option<Cell<PositionSummary>>>,
    row_cur: Vec<Option<Cell<PositionSummary>>>,
}

impl PreparedTokenQuery {
    pub fn new(needle: &str) -> Self {
        let words = needle
            .trim()
            .split_whitespace()
            .map(|word| {
                let chars: Vec<char> = word.chars().flat_map(char::to_lowercase).collect();
                let meaningful = chars.iter().filter(|c| !c.is_whitespace()).count();
                PreparedWord {
                    max_err: error_budget(meaningful),
                    floor: score_floor(meaningful),
                    meaningful,
                    chars,
                }
            })
            .collect();
        Self {
            words,
            scratch: std::cell::RefCell::new(ScoreScratch::default()),
        }
    }

    /// Equivalent to [`fuzzy_score_tokens`] for this query.
    pub fn score(&self, haystack: &str) -> Option<i32> {
        if self.words.is_empty() {
            return Some(0);
        }
        let mut scratch = self.scratch.borrow_mut();
        let mut total = 0i32;
        for word in &self.words {
            let best = haystack
                .split_whitespace()
                .filter_map(|token| score_prepared_word(word, token, &mut scratch))
                .max()?;
            total = total.saturating_add(best);
        }
        Some(total)
    }
}

/// Score-only matcher equivalent to `fuzzy_score(word, token)` but with the
/// pattern pre-lowered and DP scratch buffers reused across calls.
fn score_prepared_word(word: &PreparedWord, token: &str, scratch: &mut ScoreScratch) -> Option<i32> {
    if word.meaningful == 0 {
        return Some(0);
    }
    // Byte length is an upper bound on char count; lowercasing only expands.
    if token.len() + (word.max_err as usize) < word.meaningful {
        return None;
    }

    scratch.hay.clear();
    scratch
        .hay
        .extend(token.chars().flat_map(char::to_lowercase));
    if scratch.hay.is_empty() {
        return None;
    }
    let n = scratch.hay.len();
    if n + (word.max_err as usize) < word.meaningful {
        return None;
    }
    if !prefilter(&word.chars, &scratch.hay, word.max_err) {
        return None;
    }

    let cell = run_dp(
        &word.chars,
        &scratch.hay,
        word.max_err,
        &mut scratch.row_prev2,
        &mut scratch.row_prev,
        &mut scratch.row_cur,
        false,
    )?;
    let exact = word.chars == scratch.hay;
    let score = cell.score + if exact { EXACT } else { 0 };
    if !exact && score < word.floor {
        return None;
    }
    Some(score)
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

    #[test]
    fn multi_word_queries_require_every_word_to_match() {
        assert!(fuzzy_score_tokens("gpt openai", "gpt-5-codex openai responses").is_some());
        assert!(fuzzy_score_tokens("gpt anthropic", "gpt-5-codex openai responses").is_none());
        // Words may hit different fields in any order.
        assert!(fuzzy_score_tokens("openai gpt", "gpt-5-codex openai responses").is_some());
    }

    #[test]
    fn weak_scattered_matches_are_discarded() {
        // All chars are present but scattered mid-token with long gaps: reject.
        assert!(fuzzy_score("mrca", "premium-orchestra-cathedral").is_none());
        // Boundary-aligned acronym matches remain valid.
        assert!(fuzzy_score("aeiu", "america-external-input-utility").is_some());
        // Clean subsequence and light typos still pass.
        assert!(fuzzy_score("gpt5", "gpt-5-codex").is_some());
        assert!(fuzzy_score("sonet", "claude-sonnet-4.5").is_some());
    }
}
