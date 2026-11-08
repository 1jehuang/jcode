//! Bounded token evidence tied to validated decoded-text generations.
use super::source::Source;
use super::text_cache::Segments;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{
    Arc, Mutex, OnceLock, Weak,
    atomic::{AtomicBool, Ordering},
};

const MAX_ENTRIES: usize = 4096;
const MAX_BYTES: usize = 1024 * 1024;
const MAX_TOKEN_BYTES: usize = 4096;
const MAX_PREFIX_PROBES: usize = 64;
/// Larger match lists are not worth their memory: such tokens are common, and
/// the full scan for them stops early on the first user-prompt hit anyway.
const MAX_NARROW_SEGMENTS: usize = 2048;

/// A token found in a transcript: its first-match fuzzy score and the
/// strongest origin it appears in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Hit {
    pub(crate) score: i64,
    pub(crate) source: Source,
}
#[cfg(test)]
pub(crate) fn hit(score: i64) -> Hit {
    Hit {
        score,
        source: Source::Other,
    }
}

struct Evidence {
    outcome: Option<Hit>,
    /// Literal mode only: every segment containing the token, when bounded.
    /// A longer token can only occur in these, so typing `alph` -> `alpha`
    /// rescans a few segments instead of the whole transcript.
    segments: Option<Arc<[u32]>>,
    used: u64,
    bytes: usize,
}
struct Generation {
    text: Weak<Segments>,
    modes: [HashMap<String, Evidence>; 2],
}
// Include owned keys, values, and conservative hash-table bookkeeping. Text is
// weakly referenced and is never charged here or kept alive by this cache.
const GENERATION_BYTES: usize = std::mem::size_of::<(usize, Generation)>() + 64;
const EVIDENCE_BYTES: usize = std::mem::size_of::<(String, Evidence)>() + 32;
const PRUNE_EVERY: usize = 64;

#[derive(Default)]
struct Cache {
    generations: HashMap<usize, Generation>,
    entries: usize,
    bytes: usize,
    clock: u64,
    insertions: usize,
}
impl Cache {
    fn prune(&mut self) {
        self.generations.retain(|_, generation| {
            generation.text.strong_count() != 0
                && generation.modes.iter().any(|mode| !mode.is_empty())
        });
        self.recount();
    }
    fn recount(&mut self) {
        self.entries = 0;
        self.bytes = self.generations.len() * GENERATION_BYTES;
        for generation in self.generations.values() {
            for mode in &generation.modes {
                self.entries += mode.len();
                self.bytes += mode.values().map(|entry| entry.bytes).sum::<usize>();
            }
        }
    }
    fn get(&mut self, text: &Arc<Segments>, token: &str, fuzzy: bool) -> Option<Option<Hit>> {
        if token.len() > MAX_TOKEN_BYTES {
            return None;
        }
        let generation = self.generations.get_mut(&(Arc::as_ptr(text) as usize))?;
        if !generation.text.ptr_eq(&Arc::downgrade(text)) {
            return None;
        }
        self.clock = self.clock.wrapping_add(1);
        let mode = &mut generation.modes[usize::from(fuzzy)];
        // Borrowed lookup allocates no token key and never scans other files.
        if let Some(entry) = mode.get_mut(token) {
            entry.used = self.clock;
            return Some(entry.outcome);
        }
        if !fuzzy && !mode.is_empty() {
            // Only shorter literal prefixes imply absence. Check UTF-8
            // boundaries, including the empty prefix, with borrowed map keys.
            // Bound probes for pasted/minified queries. Missing a longer cached
            // prefix only causes a fresh scan, never an incorrect exclusion.
            for (end, _) in token.char_indices().take(MAX_PREFIX_PROBES) {
                if let Some(entry) = mode.get_mut(&token[..end])
                    && entry.outcome.is_none()
                {
                    entry.used = self.clock;
                    return Some(None);
                }
            }
        }
        None
    }
    /// The segment list of the longest cached literal prefix of `token` that
    /// matched. None means no narrowing is available.
    fn narrowing(&mut self, text: &Arc<Segments>, token: &str) -> Option<Arc<[u32]>> {
        let generation = self.generations.get_mut(&(Arc::as_ptr(text) as usize))?;
        if !generation.text.ptr_eq(&Arc::downgrade(text)) {
            return None;
        }
        let mode = &mut generation.modes[0];
        let ends: Vec<usize> = token
            .char_indices()
            .skip(1)
            .take(MAX_PREFIX_PROBES)
            .map(|(end, _)| end)
            .collect();
        for end in ends.into_iter().rev() {
            if let Some(entry) = mode.get_mut(&token[..end])
                && entry.outcome.is_some()
                && let Some(segments) = &entry.segments
            {
                self.clock = self.clock.wrapping_add(1);
                entry.used = self.clock;
                return Some(segments.clone());
            }
        }
        None
    }
    fn evict(&mut self, incoming_bytes: usize) {
        // Batch eviction amortizes the global LRU pass over many insertions.
        // Hits only touch timestamps, without allocating or pruning anything.
        let mut oldest: Vec<_> = self
            .generations
            .values()
            .flat_map(|generation| generation.modes.iter())
            .flat_map(|mode| mode.values())
            .map(|entry| (entry.used, entry.bytes))
            .collect();
        oldest.sort_unstable_by_key(|entry| entry.0);
        let mut entries = self.entries;
        let mut bytes = self.bytes;
        let mut cutoff = None;
        for (used, size) in oldest {
            if entries < MAX_ENTRIES * 7 / 8 && bytes + incoming_bytes <= MAX_BYTES * 7 / 8 {
                break;
            }
            entries -= 1;
            bytes -= size;
            cutoff = Some(used);
        }
        if let Some(cutoff) = cutoff {
            for generation in self.generations.values_mut() {
                for mode in &mut generation.modes {
                    mode.retain(|_, entry| entry.used > cutoff);
                    mode.shrink_to_fit();
                }
            }
            self.prune();
        }
    }
    fn insert(&mut self, text: &Arc<Segments>, token: &str, fuzzy: bool, outcome: Option<Hit>) {
        self.insert_narrowed(text, token, fuzzy, outcome, None);
    }
    fn insert_narrowed(
        &mut self,
        text: &Arc<Segments>,
        token: &str,
        fuzzy: bool,
        outcome: Option<Hit>,
        segments: Option<Arc<[u32]>>,
    ) {
        if token.len() > MAX_TOKEN_BYTES {
            return;
        }
        // Concurrent scans may have already published the same complete evidence.
        if self.get(text, token, fuzzy).is_some() {
            return;
        }
        self.insertions += 1;
        let token = token.to_owned();
        let bytes = EVIDENCE_BYTES
            + token.capacity()
            + segments.as_ref().map_or(0, |list| list.len() * 4 + 16);
        // Reserve generation overhead even for an existing generation: eviction
        // could remove its final entry before this insertion recreates it.
        let incoming_bytes = bytes + GENERATION_BYTES;
        if self.insertions >= PRUNE_EVERY
            || self.entries >= MAX_ENTRIES
            || self.bytes + incoming_bytes > MAX_BYTES
        {
            self.prune();
            self.insertions = 0;
        }
        if self.entries >= MAX_ENTRIES || self.bytes + incoming_bytes > MAX_BYTES {
            self.evict(incoming_bytes);
        }
        let generation = self
            .generations
            .entry(Arc::as_ptr(text) as usize)
            .or_insert_with(|| {
                self.bytes += GENERATION_BYTES;
                Generation {
                    text: Arc::downgrade(text),
                    modes: Default::default(),
                }
            });
        self.clock = self.clock.wrapping_add(1);
        generation.modes[usize::from(fuzzy)].insert(
            token,
            Evidence {
                outcome,
                segments,
                used: self.clock,
                bytes,
            },
        );
        self.entries += 1;
        self.bytes += bytes;
    }
}
fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

/// Validate the text generation before consulting evidence. None requests the
/// streaming fallback, or signals cancellation. No lock spans IO or matching.
#[cfg(test)]
pub(super) fn matches(
    path: &Path,
    tokens: &[String],
    cancel: &AtomicBool,
    fuzzy: bool,
) -> Option<Vec<Option<Hit>>> {
    matches_with_scores(
        path,
        tokens,
        cancel,
        fuzzy,
        &mut super::WordScores::default(),
    )
}

pub(super) fn matches_with_scores(
    path: &Path,
    tokens: &[String],
    cancel: &AtomicBool,
    fuzzy: bool,
    scores: &mut super::WordScores,
) -> Option<Vec<Option<Hit>>> {
    let text = super::text_cache::segments(path, cancel)?;
    resolve(
        cache(),
        &text,
        tokens,
        cancel,
        fuzzy,
        |text, token, cancel, fuzzy| scan_token_with_scores(text, token, cancel, fuzzy, scores),
    )
}

/// Resolve evidence only for validated resident text, without hydrating files.
#[cfg(test)]
pub(super) fn matches_cached(
    path: &Path,
    tokens: &[String],
    cancel: &AtomicBool,
    fuzzy: bool,
) -> Option<Vec<Option<Hit>>> {
    matches_cached_with_scores(
        path,
        tokens,
        cancel,
        fuzzy,
        &mut super::WordScores::default(),
    )
}

/// Consult only fingerprint-validated resident text. Missing text is a cache
/// miss, not negative evidence, and must be handled by the caller.
pub(super) fn matches_cached_with_scores(
    path: &Path,
    tokens: &[String],
    cancel: &AtomicBool,
    fuzzy: bool,
    scores: &mut super::WordScores,
) -> Option<Vec<Option<Hit>>> {
    let text = super::text_cache::cached_segments(path, cancel)?;
    resolve(
        cache(),
        &text,
        tokens,
        cancel,
        fuzzy,
        |text, token, cancel, fuzzy| scan_token_with_scores(text, token, cancel, fuzzy, scores),
    )
}

fn resolve(
    cache: &Mutex<Cache>,
    text: &Arc<Segments>,
    tokens: &[String],
    cancel: &AtomicBool,
    fuzzy: bool,
    mut scan: impl FnMut(&Segments, &str, &AtomicBool, bool) -> Option<Option<Hit>>,
) -> Option<Vec<Option<Hit>>> {
    let mut outcomes = Vec::with_capacity(tokens.len());
    for token in tokens {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let cached =
            super::super::preview_result(cache.lock(), "token cache lock")?.get(text, token, fuzzy);
        let narrowing = if cached.is_none() && !fuzzy {
            super::super::preview_result(cache.lock(), "token cache lock")?.narrowing(text, token)
        } else {
            None
        };
        let outcome = match (cached, narrowing) {
            (Some(outcome), _) => outcome,
            (None, Some(candidates)) => {
                let (outcome, segments) =
                    literal_in_segments(text, token, candidates.iter().copied(), cancel)?;
                super::super::preview_result(cache.lock(), "token cache lock")?
                    .insert_narrowed(text, token, false, outcome, segments);
                outcome
            }
            (None, None) if !fuzzy && token.chars().count() <= 2 => {
                // Short literal tokens are the prefixes of whatever is typed
                // next. Record where they match so extensions scan only there.
                let (outcome, segments) =
                    literal_in_segments(text, token, 0..text.text.len() as u32, cancel)?;
                super::super::preview_result(cache.lock(), "token cache lock")?
                    .insert_narrowed(text, token, false, outcome, segments);
                outcome
            }
            (None, None) => {
                let outcome = scan(text, token, cancel, fuzzy)?;
                // Publish each completed token independently, even if a later
                // token cancels. Incomplete negatives never reach this point.
                super::super::preview_result(cache.lock(), "token cache lock")?
                    .insert(text, token, fuzzy, outcome);
                outcome
            }
        };
        outcomes.push(outcome);
    }
    if cancel.load(Ordering::Relaxed) {
        None
    } else {
        Some(outcomes)
    }
}
/// Literal match over the given segments, with the same semantics as the
/// literal scanner (substring match, score 0, strongest origin), plus the
/// list of matching segments when it is small enough to keep.
fn literal_in_segments(
    text: &Segments,
    token: &str,
    candidates: impl Iterator<Item = u32>,
    cancel: &AtomicBool,
) -> Option<(Option<Hit>, Option<Arc<[u32]>>)> {
    let mut best: Option<Source> = None;
    let mut found = Vec::new();
    let mut bounded = true;
    for (step, index) in candidates.enumerate() {
        if step % 256 == 0 && cancel.load(Ordering::Relaxed) {
            return None;
        }
        let (Some(segment), Some(source)) = (
            text.text.get(index as usize),
            text.sources.get(index as usize),
        ) else {
            continue;
        };
        if segment.contains(token) {
            best = super::source::stronger(best, Some(*source));
            if bounded {
                if found.len() < MAX_NARROW_SEGMENTS {
                    found.push(index);
                } else {
                    bounded = false;
                    found = Vec::new();
                }
            }
        }
    }
    let outcome = best.map(|source| Hit { score: 0, source });
    let segments = (bounded && outcome.is_some()).then(|| Arc::from(found));
    Some((outcome, segments))
}

#[cfg(test)]
fn scan_token(
    text: &Segments,
    token: &str,
    cancel: &AtomicBool,
    fuzzy: bool,
) -> Option<Option<Hit>> {
    scan_token_with_scores(
        text,
        token,
        cancel,
        fuzzy,
        &mut super::WordScores::default(),
    )
}

/// Matches the streaming scanner: the first matching segment sets the score,
/// later literal occurrences can only raise the origin. Stops early once the
/// token is seen in a user prompt, the strongest origin.
fn scan_token_with_scores(
    text: &Segments,
    token: &str,
    cancel: &AtomicBool,
    fuzzy: bool,
    scores: &mut super::WordScores,
) -> Option<Option<Hit>> {
    let tokens = [token.to_owned()];
    let mode = if fuzzy {
        super::ScanMode::Fuzzy
    } else {
        super::ScanMode::Literal
    };
    let mut scan = super::Scan::new(&tokens, &[], cancel, mode, scores);
    for (segment, source) in text.iter() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        if scan.sources[0] == Some(Source::User) {
            break;
        }
        let scanned = scan.normalized_text::<serde_json::Error>(segment, source);
        // The scanner signals completion with an error sentinel. Keep reading
        // for a stronger origin, but stop on a genuine failure.
        if let Err(error) = scanned
            && !scan.all_found()
        {
            if !cancel.load(Ordering::Relaxed) {
                crate::logging::debug(&format!("Transcript token scan stopped: {error}"));
            }
            return None;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        None
    } else {
        Some(scan.sources[0].map(|source| Hit {
            score: scan.score,
            source,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tokens(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn long_queries_bound_prefix_probes_and_fall_back_safely() {
        let mut cache = Cache::default();
        let text = Arc::new(Segments::default());
        cache.insert(&text, "x", false, None);
        assert_eq!(
            cache.get(&text, &"x".repeat(MAX_TOKEN_BYTES + 1), false),
            None
        );
        let prefix = "界".repeat(MAX_PREFIX_PROBES + 1);
        cache.insert(&text, &prefix, false, None);
        assert_eq!(cache.get(&text, &format!("{prefix}面"), false), None);
        assert_eq!(cache.get(&text, &prefix, false), Some(None));
    }

    #[test]
    fn completed_tokens_and_backspacing_reuse_positive_and_negative_evidence() {
        let cache = Mutex::new(Cache::default());
        let text = Arc::new(Segments::from(tokens(&["alpha beta"])));
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        for query in [
            tokens(&["alpha", "absent"]),
            tokens(&["alpha", "absent", "beta"]),
            tokens(&["alpha", "absent"]),
        ] {
            let result = resolve(
                &cache,
                &text,
                &query,
                &cancel,
                false,
                |text, token, cancel, fuzzy| {
                    calls += 1;
                    scan_token(text, token, cancel, fuzzy)
                },
            )
            .unwrap();
            assert_eq!(result[0], Some(hit(0)));
            assert_eq!(result[1], None);
        }
        assert_eq!(calls, 3);
    }

    #[test]
    fn validated_generation_changes_invalidate_results() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.json");
        let cancel = AtomicBool::new(false);
        let query = tokens(&["alpha"]);
        std::fs::write(&path, "\"alpha\"").unwrap();
        assert_eq!(
            matches(&path, &query, &cancel, false),
            Some(vec![Some(hit(0))])
        );
        std::fs::write(&path, "\"replacement\"").unwrap();
        assert_eq!(matches(&path, &query, &cancel, false), Some(vec![None]));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(matches(&path, &query, &cancel, false), None);
    }

    #[test]
    fn fuzzy_matches_single_scan_first_segment_scores_and_separate_mode() {
        let text = Arc::new(Segments::from(tokens(&["alphabet betta", "alpha beta"])));
        let query = tokens(&["alpha", "beta", "unmatched", "alp"]);
        let cancel = AtomicBool::new(false);
        let mut scores = super::super::WordScores::default();
        let mut scan = super::super::Scan::new(
            &query,
            &[],
            &cancel,
            super::super::ScanMode::Fuzzy,
            &mut scores,
        );
        for (segment, source) in text.iter() {
            let _ = scan.normalized_text::<serde_json::Error>(segment, source);
        }
        let cache = Mutex::new(Cache::default());
        let result = resolve(&cache, &text, &query, &cancel, true, scan_token).unwrap();
        assert_eq!(
            resolve(&cache, &text, &query, &cancel, true, |_, _, _, _| {
                panic!("completed fuzzy tokens must be reused")
            }),
            Some(result.clone())
        );
        assert_eq!(
            result.iter().flatten().map(|hit| hit.score).sum::<i64>(),
            scan.score
        );
        assert_eq!(
            result.iter().map(Option::is_some).collect::<Vec<_>>(),
            scan.found
        );
        let literal = resolve(&cache, &text, &query, &cancel, false, scan_token).unwrap();
        assert!(literal.iter().flatten().all(|hit| hit.score == 0));
        assert!(result[0].unwrap().score > 0);
    }

    #[test]
    fn literal_extensions_scan_only_segments_matching_their_prefix() {
        let cache = Mutex::new(Cache::default());
        let text = Arc::new(Segments {
            text: tokens(&["alpine", "nothing", "alpha tool", "beta", "alpha prompt"]),
            sources: vec![
                Source::Other,
                Source::User,
                Source::ToolOutput,
                Source::User,
                Source::User,
            ],
        });
        let cancel = AtomicBool::new(false);
        let never = |_: &Segments, _: &str, _: &AtomicBool, _: bool| -> Option<Option<Hit>> {
            panic!("extension must reuse its prefix's segments")
        };
        let resolve_one =
            |token: &str,
             scan: &dyn Fn(&Segments, &str, &AtomicBool, bool) -> Option<Option<Hit>>| {
                resolve(&cache, &text, &tokens(&[token]), &cancel, false, scan).unwrap()[0]
            };
        let user = Some(Hit {
            score: 0,
            source: Source::User,
        });
        assert_eq!(resolve_one("al", &never), user);
        assert_eq!(resolve_one("alp", &never), user);
        assert_eq!(resolve_one("alpha", &never), user);
        assert_eq!(
            resolve_one("alpine", &never),
            Some(Hit {
                score: 0,
                source: Source::Other
            })
        );
        assert_eq!(resolve_one("alphax", &never), None);
        // Narrowed evidence agrees with a full scan.
        for token in ["alp", "alpha", "alpine", "alphax", "alpha tool"] {
            assert_eq!(
                scan_token(&text, token, &cancel, false).unwrap(),
                resolve_one(token, &never),
                "{token}"
            );
        }
        let narrowed = cache.lock().unwrap().narrowing(&text, "alphaz").unwrap();
        assert_eq!(&*narrowed, &[2, 4]);
    }

    #[test]
    fn cancellation_keeps_completed_work_not_incomplete_negatives() {
        let cache = Mutex::new(Cache::default());
        let text = Arc::new(Segments::from(tokens(&["alpha"])));
        let cancel = AtomicBool::new(false);
        let query = tokens(&["alpha", "absent", "missing"]);
        assert!(
            resolve(
                &cache,
                &text,
                &query,
                &cancel,
                false,
                |text, token, cancel, fuzzy| {
                    if token == "missing" {
                        cancel.store(true, Ordering::Relaxed);
                        return None;
                    }
                    scan_token(text, token, cancel, fuzzy)
                }
            )
            .is_none()
        );
        let mut guard = cache.lock().unwrap();
        assert_eq!(guard.get(&text, "alpha", false), Some(Some(hit(0))));
        assert_eq!(guard.get(&text, "absent", false), Some(None));
        assert_eq!(guard.get(&text, "missing", false), None);
        drop(guard);
        assert!(scan_token(&text, "missing", &cancel, false).is_none());
        cancel.store(false, Ordering::Relaxed);
        assert_eq!(
            resolve(&cache, &text, &query, &cancel, false, scan_token),
            Some(vec![Some(hit(0)), None, None])
        );
    }

    #[test]
    fn cache_only_lookup_never_hydrates_missing_or_invalidated_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("warm.json");
        let query = tokens(&["alpha", "missing"]);
        let cancel = AtomicBool::new(false);
        let mut scores = super::super::WordScores::default();
        std::fs::write(&path, "\"alpha\"").unwrap();
        assert_eq!(
            matches_cached_with_scores(&path, &query, &cancel, false, &mut scores),
            None
        );
        assert!(super::super::text_cache::cached_segments(&path, &cancel).is_none());
        let expected = Some(vec![Some(hit(0)), None]);
        assert_eq!(
            matches_with_scores(&path, &query, &cancel, false, &mut scores),
            expected
        );
        assert_eq!(matches_cached(&path, &query, &cancel, false), expected);
        std::fs::write(&path, "\"replacement\"").unwrap();
        assert_eq!(
            matches_cached_with_scores(&path, &query, &cancel, false, &mut scores),
            None
        );
        assert!(super::super::text_cache::cached_segments(&path, &cancel).is_none());
        assert_eq!(
            matches_with_scores(&path, &query, &cancel, false, &mut scores),
            Some(vec![None, None])
        );
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            matches_cached_with_scores(&path, &query, &cancel, false, &mut scores),
            None
        );
    }

    #[test]
    fn shared_word_scores_preserve_results_across_transcript_generations() {
        let dir = tempfile::tempdir().unwrap();
        let query = tokens(&["conifg", "alpha", "missing", "café"]);
        let cancel = AtomicBool::new(false);
        let mut scores = super::super::WordScores::default();
        for (index, text) in [
            tokens(&["config café alphabet", "alpha config"]),
            tokens(&["config alphabet café", "alpha"]),
        ]
        .into_iter()
        .enumerate()
        {
            let segments = Segments::from(text.clone());
            let expected = query
                .iter()
                .map(|token| scan_token(&segments, token, &cancel, true).unwrap())
                .collect::<Vec<_>>();
            let path = dir.path().join(format!("{index}.json"));
            std::fs::write(&path, serde_json::to_vec(&text).unwrap()).unwrap();
            assert_eq!(
                matches_with_scores(&path, &query, &cancel, true, &mut scores),
                Some(expected.clone())
            );
            assert_eq!(
                matches_cached_with_scores(&path, &query, &cancel, true, &mut scores),
                Some(expected)
            );
        }
    }

    #[test]
    fn literal_negative_extensions_skip_scans_but_not_backspacing_or_fuzzy() {
        let cache = Mutex::new(Cache::default());
        let text = Arc::new(Segments::from(tokens(&["alphabet"])));
        let cancel = AtomicBool::new(false);
        assert_eq!(
            resolve(&cache, &text, &tokens(&["zz"]), &cancel, false, scan_token),
            Some(vec![None])
        );
        assert_eq!(
            resolve(
                &cache,
                &text,
                &tokens(&["zzmore"]),
                &cancel,
                false,
                |_, _, _, _| panic!("absent literal prefix proves extension absent")
            ),
            Some(vec![None])
        );
        let mut calls = 0;
        for (token, fuzzy) in [("z", false), ("zzmore", true)] {
            assert_eq!(
                resolve(
                    &cache,
                    &text,
                    &tokens(&[token]),
                    &cancel,
                    fuzzy,
                    |text, token, cancel, fuzzy| {
                        calls += 1;
                        scan_token(text, token, cancel, fuzzy)
                    }
                ),
                Some(vec![None])
            );
        }
        // Short literal tokens resolve through the segment-recording scan, so
        // only the fuzzy lookup reaches the injected scanner.
        assert_eq!(calls, 1);
        let mut guard = cache.lock().unwrap();
        guard.insert(&text, "alpha", false, Some(hit(0)));
        assert_eq!(guard.get(&text, "alphabeta", false), None);
        guard.insert(&text, "missing", true, None);
        assert_eq!(guard.get(&text, "missinglonger", true), None);
        assert_eq!(guard.get(&text, "missinglonger", false), None);
        let other = Arc::new(Segments::from(tokens(&["zzmore"])));
        assert_eq!(guard.get(&other, "zzmore", false), None);
        guard.insert(&text, "界", false, None);
        assert_eq!(guard.get(&text, "界面", false), Some(None));
        assert_eq!(guard.get(&text, "", false), None);
    }

    #[test]
    fn warm_lookups_do_not_prune_unrelated_generations() {
        let mut cache = Cache::default();
        let stale = Arc::new(Segments::from(tokens(&["stale"])));
        let stale_key = Arc::as_ptr(&stale) as usize;
        cache.insert(&stale, "old", false, Some(hit(0)));
        drop(stale);
        let live = Arc::new(Segments::from(tokens(&["live"])));
        cache.insert(&live, "hit", false, Some(hit(0)));
        let insertions = cache.insertions;
        for _ in 0..PRUNE_EVERY * 2 {
            assert_eq!(cache.get(&live, "hit", false), Some(Some(hit(0))));
            assert_eq!(cache.get(&live, "unknown", true), None);
        }
        assert_eq!(cache.insertions, insertions);
        assert!(cache.generations.contains_key(&stale_key));
        for i in 0..PRUNE_EVERY {
            cache.insert(&live, &format!("new-{i}"), false, Some(hit(0)));
        }
        assert!(!cache.generations.contains_key(&stale_key));
        assert_eq!(cache.entries, PRUNE_EVERY + 1);
    }

    #[test]
    fn generation_maps_and_batch_lru_keep_recent_hits_at_full_capacity() {
        let mut cache = Cache::default();
        let texts: Vec<_> = (0..256).map(|_| Arc::new(Segments::default())).collect();
        for text in &texts {
            for token in 0..16 {
                cache.insert(text, &format!("token-{token}!"), false, Some(hit(0)));
            }
        }
        assert_eq!(cache.entries, MAX_ENTRIES);
        assert_eq!(cache.generations.len(), 256);
        assert_eq!(cache.get(&texts[0], "token-0!", false), Some(Some(hit(0))));
        assert_eq!(cache.get(&texts[0], "token-0!", true), None);
        cache.insert(&texts[255], "new", false, Some(hit(0)));
        assert!(cache.entries <= MAX_ENTRIES * 7 / 8);
        assert_eq!(cache.get(&texts[0], "token-0!", false), Some(Some(hit(0))));
        assert_eq!(cache.get(&texts[0], "token-1!", false), None);
        assert_eq!(cache.get(&texts[255], "new", false), Some(Some(hit(0))));
        assert!(cache.bytes <= MAX_BYTES);
    }

    #[test]
    fn weak_generations_lru_entry_and_key_byte_limits() {
        let mut cache = Cache::default();
        let text = Arc::new(Segments::default());
        cache.insert(&text, "old", false, None);
        let distinct_generation = Arc::new(Segments::default());
        assert_eq!(cache.get(&distinct_generation, "old", false), None);
        let weak = Arc::downgrade(&text);
        drop(text);
        assert!(weak.upgrade().is_none());
        cache.prune();
        assert_eq!(cache.entries, 0);
        assert!(cache.generations.is_empty());
        let text = Arc::new(Segments::default());
        for i in 0..MAX_ENTRIES {
            cache.insert(&text, &format!("{i}!"), false, None);
        }
        assert_eq!(cache.get(&text, "0!", false), Some(None));
        cache.insert(&text, "new", false, Some(hit(0)));
        assert_eq!(cache.get(&text, "1!", false), None);
        assert_eq!(cache.get(&text, "0!", false), Some(None));
        for i in 0..MAX_ENTRIES {
            cache.insert(&text, &format!("{i}{}", "x".repeat(1000)), false, None);
        }
        assert!(cache.bytes <= MAX_BYTES);
        assert!(cache.entries <= MAX_ENTRIES);
        let oversized = "x".repeat(MAX_TOKEN_BYTES + 1);
        cache.insert(&text, &oversized, false, None);
        assert_eq!(cache.get(&text, &oversized, false), None);
        assert_eq!(
            cache.bytes,
            cache.generations.len() * GENERATION_BYTES
                + cache
                    .generations
                    .values()
                    .flat_map(|generation| generation.modes.iter())
                    .flat_map(|mode| mode.values())
                    .map(|entry| entry.bytes)
                    .sum::<usize>()
        );
    }
}
