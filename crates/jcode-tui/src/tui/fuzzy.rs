//! Typo-resistant fuzzy matching for slash-command suggestions.
//!
//! This is a small dynamic-programming matcher tuned for short command-style
//! haystacks (e.g. `/compact mode semantic`). Compared to a plain subsequence
//! match it adds:
//!
//! * boundary / consecutive / prefix bonuses (fzf-style scoring), so the best
//!   alignment is preferred and matched runs are contiguous where possible;
//! * bounded typo tolerance: a small number of substitutions, adjacent
//!   transpositions, and extra (inserted) needle characters are allowed, scaled
//!   by how much the user has typed. This lets `/conifg`, `/comapct`, and
//!   `/memroy` still resolve to `/config`, `/compact`, and `/memory`.
//!
//! The matcher also reports the haystack character indices that were matched so
//! callers can underline/highlight them in the UI.

/// Characters that start a new "word" inside a command string.
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
/// Net score for an adjacent transposition (two correct chars, swapped).
const TRANSPOSITION: i32 = 2 * MATCH - 22;

/// Result of a successful fuzzy match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FuzzyMatch {
    /// Higher is better.
    pub score: i32,
    /// Matched haystack character indices (into the original haystack), sorted
    /// ascending. Only true character matches are reported; substituted typo
    /// characters are not highlighted.
    pub positions: Vec<usize>,
}

#[derive(Clone)]
struct Cell {
    score: i32,
    errors: u8,
    /// Last matched haystack index (in stripped space), or -1 if none yet.
    last: i32,
    /// Whether the most recently consumed pattern character was a genuine
    /// character match (exact or transposition) rather than a substitution or
    /// deletion. Used to require that the *final* typed character truly lands,
    /// which kills spurious tail matches (e.g. `/goals sh` -> `/goals resume`).
    tail_true: bool,
    positions: Vec<usize>,
}

fn keep_best(slot: &mut Option<Cell>, candidate: Cell) {
    let replace = match slot {
        None => true,
        Some(existing) => {
            candidate.score > existing.score
                || (candidate.score == existing.score && candidate.errors < existing.errors)
                || (candidate.score == existing.score
                    && candidate.errors == existing.errors
                    && candidate.positions.len() > existing.positions.len())
        }
    };
    if replace {
        *slot = Some(candidate);
    }
}

/// Number of typos tolerated, scaled by how many meaningful characters the user
/// has typed. Very short queries get zero tolerance to avoid noise.
fn error_budget(meaningful_len: usize) -> u8 {
    match meaningful_len {
        0..=2 => 0,
        3..=5 => 1,
        _ => 2,
    }
}

/// Fuzzy-match `needle` against `haystack`, returning a score and the matched
/// haystack indices, or `None` if they do not match within the typo budget.
///
/// A leading `/` is treated as a fixed anchor: the first meaningful character of
/// the needle must truly match the first character of the command (this keeps
/// `/g` from matching `/config`). Interior characters tolerate typos.
pub(crate) fn fuzzy_match(needle: &str, haystack: &str) -> Option<FuzzyMatch> {
    // Strip a single leading '/' from both sides; commands and the typed query
    // both start with it, so anchoring on the real first letter is cleaner.
    let (hay_offset, hay_src) = match haystack.strip_prefix('/') {
        Some(rest) => (1usize, rest),
        None => (0usize, haystack),
    };
    let needle_src = needle.strip_prefix('/').unwrap_or(needle);

    // Keep whitespace in the pattern: it is significant. A space in the query
    // must align with a space in the candidate (e.g. `/config ed` -> `/config
    // edit`), and a trailing space after a complete command (`/refresh `) must
    // not fuzzily expand into a longer hyphenated command. Whitespace never
    // participates in typo edits.
    let pat: Vec<char> = needle_src.chars().map(|c| c.to_ascii_lowercase()).collect();
    let hay: Vec<char> = hay_src.chars().map(|c| c.to_ascii_lowercase()).collect();

    if pat.iter().all(|c| c.is_whitespace()) {
        return Some(FuzzyMatch {
            score: 0,
            positions: Vec::new(),
        });
    }

    let m = pat.len();
    let n = hay.len();
    if n == 0 {
        return None;
    }
    // Typo budget scales with the number of meaningful (non-space) chars typed.
    let meaningful = pat.iter().filter(|c| !c.is_whitespace()).count();
    let max_err = error_budget(meaningful);

    // dp[i][j]: best alignment consuming pat[0..i] within hay[0..j], with
    // pat[i-1] placed at or before j-1.
    let mut dp: Vec<Vec<Option<Cell>>> = vec![vec![None; n + 1]; m + 1];
    dp[0][0] = Some(Cell {
        score: 0,
        errors: 0,
        last: -1,
        tail_true: true,
        positions: Vec::new(),
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
            let mut best: Option<Cell> = None;

            // (a) Skip a haystack character (interior gap between matches).
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

            // (b) Align pat[i-1] with hay[j-1] (exact match or substitution).
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
                    positions.push(pos);
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

            // (c) Delete a needle character (user typed an extra char).
            // Never "delete" a typed space; whitespace must align exactly.
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

            // (d) Adjacent transposition of two characters (no spaces).
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
                positions.push(first);
                positions.push(j - 1);
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

    // Best alignment that consumed the whole needle (trailing haystack is free).
    // Require the final consumed pattern character to be a genuine match so that
    // a trailing typo cannot silently expand the query into an unrelated command
    // (e.g. `/goals sh` must not also match `/goals resume`).
    let mut answer: Option<Cell> = None;
    for row in dp[m].iter().take(n + 1) {
        if let Some(cell) = row.clone()
            && cell.tail_true
        {
            keep_best(&mut answer, cell);
        }
    }

    let cell = answer?;
    // Anchor: the first true match must be the first command character.
    if cell.positions.first() != Some(&0) {
        return None;
    }

    Some(FuzzyMatch {
        score: cell.score,
        positions: cell.positions.into_iter().map(|p| p + hay_offset).collect(),
    })
}

/// Convenience wrapper returning just the score.
pub(crate) fn fuzzy_score(needle: &str, haystack: &str) -> Option<i32> {
    fuzzy_match(needle, haystack).map(|m| m.score)
}

/// Convenience wrapper returning just the matched positions (empty on no match).
pub(crate) fn fuzzy_match_positions(needle: &str, haystack: &str) -> Vec<usize> {
    fuzzy_match(needle, haystack)
        .map(|m| m.positions)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(needle: &str, haystack: &str) -> bool {
        fuzzy_match(needle, haystack).is_some()
    }

    #[test]
    fn exact_prefix_matches_with_positions() {
        let m = fuzzy_match("/conf", "/config").expect("match");
        // The leading '/' is an anchor and not itself highlighted; matches are
        // reported as indices into the original haystack ("/config").
        assert_eq!(m.positions, vec![1, 2, 3, 4]);
    }

    #[test]
    fn subsequence_matches() {
        assert!(matches("/mdl", "/model"));
        let m = fuzzy_match("/mdl", "/model").unwrap();
        // '/', then m(1), d(3), l(5)
        assert_eq!(m.positions, vec![1, 3, 5]);
    }

    #[test]
    fn tolerates_transposition() {
        assert!(matches("/conifg", "/config"));
        assert!(matches("/comapct", "/compact"));
        assert!(matches("/memroy", "/memory"));
    }

    #[test]
    fn tolerates_substitution() {
        assert!(matches("/confog", "/config"));
    }

    #[test]
    fn tolerates_extra_char() {
        // Doubled interior character, and a leading stray character.
        assert!(matches("/coonfig", "/config"));
        assert!(matches("/xmodel", "/model"));
    }

    #[test]
    fn trailing_typo_does_not_expand_to_unrelated_command() {
        // The last typed character must genuinely land, so a completed token
        // does not fuzzily expand into a longer, unrelated command.
        assert!(!matches("/goals sh", "/goals resume"));
        assert!(matches("/goals sh", "/goals show"));
    }

    #[test]
    fn rejects_too_many_typos() {
        // Two-char query gets no typo budget.
        assert!(!matches("/xz", "/config"));
        // Wildly different long strings do not match.
        assert!(!matches("/configuration", "/model"));
    }

    #[test]
    fn anchors_on_first_command_char() {
        // 'g' is not the first letter of config, so a single-letter query for it
        // must not match config.
        assert!(!matches("/g", "/config"));
        assert!(matches("/g", "/goals"));
    }

    #[test]
    fn first_char_must_truly_match() {
        // A substituted first char is rejected (anchor requires a real match).
        assert!(!matches("/xonfig", "/config"));
    }

    #[test]
    fn nested_command_matches_across_space() {
        let m = fuzzy_match("/config ed", "/config edit").expect("match");
        assert!(m.positions.contains(&8)); // 'e' in edit
    }

    #[test]
    fn prefix_outscores_scattered_match() {
        let prefix = fuzzy_score("/co", "/config").unwrap();
        let scattered = fuzzy_score("/co", "/compact mode").unwrap();
        // Both match, but the tighter/earlier match should not score lower.
        assert!(prefix >= scattered);
    }

    #[test]
    fn empty_needle_matches_anything() {
        assert!(matches("", "/config"));
        assert!(matches("/", "/config"));
    }
}
