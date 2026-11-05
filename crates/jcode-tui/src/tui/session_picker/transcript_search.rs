//! Incremental fuzzy and full-transcript search without typing delays.
//! Only cheap literal metadata and resident-cache hints run on the UI thread,
//! never fuzzy DP or transcript file reads.
use super::loading::search_query_tokens;
use super::{ResumeTarget, SessionInfo};
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

mod text_cache;
mod word_scores;
use word_scores::WordScores;
mod token_cache;
mod warmup;

const IMMEDIATE_METADATA_BYTES: usize = 1024 * 1024;

/// A close contiguous opening request outranks incidental transcript evidence.
/// Otherwise literal evidence outranks typo-only evidence, then metadata and
/// whole words break ties instead of unrelated session recency.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct SearchRank {
    opening_prompt: usize,
    literal: bool,
    intent_terms: usize,
    exact_field: bool,
    metadata_terms: usize,
    whole_terms: usize,
    score: i64,
}

pub(super) fn literal_metadata_rank(index: &str, tokens: &[String]) -> SearchRank {
    let mut metadata_terms = 0;
    let mut whole_terms = 0;
    for token in tokens {
        let mut matches = index.match_indices(token.as_str()).peekable();
        if matches.peek().is_none() {
            continue;
        }
        metadata_terms += 1;
        // Search only occurrences, rather than splitting the entire index
        // again for each query term. Boundaries retain Unicode word semantics.
        if token.chars().all(char::is_alphanumeric)
            && matches.any(|(start, matched)| {
                let end = start + matched.len();
                !index[..start]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_alphanumeric)
                    && !index[end..]
                        .chars()
                        .next()
                        .is_some_and(char::is_alphanumeric)
            })
        {
            whole_terms += 1;
        }
    }
    SearchRank {
        opening_prompt: 0,
        literal: metadata_terms == tokens.len(),
        intent_terms: 0,
        exact_field: false,
        metadata_terms,
        whole_terms,
        score: 0,
    }
}

pub(super) fn exact_metadata_field(session: &SessionInfo, query: &str) -> bool {
    [&session.title, &session.short_name, &session.id]
        .iter()
        .any(|field| normalized_equals(field, query))
        || session
            .save_label
            .as_ref()
            .is_some_and(|label| normalized_equals(label, query))
}

fn normalized_equals(field: &str, query: &str) -> bool {
    let field = field.trim();
    if field.is_ascii() && query.is_ascii() {
        field.eq_ignore_ascii_case(query)
    } else {
        field.to_lowercase() == query
    }
}

pub(super) fn lowercase_field(field: &str) -> std::borrow::Cow<'_, str> {
    if field.is_ascii() && !field.bytes().any(|byte| byte.is_ascii_uppercase()) {
        std::borrow::Cow::Borrowed(field)
    } else {
        std::borrow::Cow::Owned(field.to_lowercase())
    }
}

impl SearchRank {
    pub(super) fn with_intent_terms(mut self, terms: usize) -> Self {
        self.intent_terms = terms;
        self
    }

    pub(super) fn with_exact_field(mut self, exact: bool) -> Self {
        self.exact_field = exact;
        self
    }
}

#[cfg(test)]
fn fuzzy_metadata_rank(index: &str, tokens: &[String]) -> SearchRank {
    fuzzy_metadata_rank_with_scores(index, tokens, &mut WordScores::default())
}

fn fuzzy_metadata_rank_with_scores(
    index: &str,
    tokens: &[String],
    scores: &mut WordScores,
) -> SearchRank {
    let mut rank = literal_metadata_rank(index, tokens);
    // Score every term, including terms already matched literally. Scoring
    // only the missing terms perversely rewarded candidates with fewer hits.
    rank.score = tokens
        .iter()
        .filter_map(|token| scores.best_score(token, index, false).map(i64::from))
        .sum();
    rank
}

struct Candidate {
    id: String,
    paths: Vec<PathBuf>,
    search_index: String,
    missing_tokens: Vec<String>,
    exact_field: bool,
    opening_prompt: String,
    intent_text: String,
}

/// Rank in memory before publishing results or opening transcripts. Keep this
/// on the background worker, not the render thread. Stable sorting preserves
/// the incoming order for equally promising candidates.
fn prioritize_candidates(
    candidates: Vec<Candidate>,
    query: &str,
    tokens: &[String],
    cancel: &AtomicBool,
) -> Vec<(Candidate, SearchRank)> {
    let mut ranked = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if cancel.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let mut rank = literal_metadata_rank(&candidate.search_index, tokens)
            .with_exact_field(candidate.exact_field);
        rank.opening_prompt = opening_prompt_rank(&candidate.opening_prompt, query);
        let intent = lowercase_field(&candidate.intent_text);
        rank.intent_terms = tokens
            .iter()
            .filter(|token| intent.contains(token.as_str()))
            .count();
        ranked.push((candidate, rank));
    }
    ranked.sort_by_key(|(_, rank)| std::cmp::Reverse(*rank));
    ranked
}

// A retyped request is stronger evidence than words scattered throughout a
// huge transcript. Compare contiguous word windows, allowing a few misspelled
// words without requiring every short function word to match literally.
fn opening_prompt_rank(prompt: &str, query: &str) -> usize {
    let words = |text: &str| -> Vec<String> {
        text.split_whitespace()
            .take(256)
            .map(|word| {
                word.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .filter(|word| !word.is_empty())
            .collect()
    };
    let query = words(query);
    if query.len() < 4 || query.len() > 64 {
        return 0;
    }
    let prompt = words(prompt);
    // Missing spaces ("ofhow") should not hide an otherwise exact request.
    let compact_query = query.concat();
    if compact_query.chars().count() >= 16 && prompt.concat().contains(&compact_query) {
        return query.len();
    }
    let prepared: Vec<_> = query
        .iter()
        .map(|word| jcode_fuzzy::PreparedTokenQuery::new(word))
        .collect();
    prompt
        .windows(query.len())
        .filter_map(|window| {
            let matched = query
                .iter()
                .zip(&prepared)
                .zip(window)
                .filter(|((word, prepared), candidate)| {
                    word == candidate
                        || (word.len() >= 4
                            && candidate.len() <= 128
                            && prepared.matches_search_text(candidate))
                })
                .count();
            (matched * 5 >= query.len() * 4).then_some(matched)
        })
        .max()
        .unwrap_or(0)
}

#[derive(Default)]
pub(super) struct TranscriptSearch {
    query: String,
    candidates: Vec<Candidate>,
    started_at: Option<Instant>,
    receiver: Option<mpsc::Receiver<(String, SearchRank)>>,
    cancel: Arc<AtomicBool>,
    ranks: HashMap<String, SearchRank>,
    defer_metadata: bool,
}

impl Drop for TranscriptSearch {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl TranscriptSearch {
    pub(super) fn reset<'a>(
        &mut self,
        query: &str,
        sessions: impl Iterator<Item = &'a SessionInfo>,
    ) {
        if self.query == query {
            return;
        }
        *self = Self::default();
        self.query = query.to_string();
        if query.is_empty() {
            return;
        }
        let tokens = search_query_tokens(query);
        let sessions: Vec<_> = sessions.collect();
        self.defer_metadata = sessions
            .iter()
            .map(|session| session.search_index.len())
            .sum::<usize>()
            > IMMEDIATE_METADATA_BYTES;
        for session in sessions {
            let missing_tokens: Vec<_> = tokens
                .iter()
                .filter(|token| {
                    self.defer_metadata || !session.search_index.contains(token.as_str())
                })
                .cloned()
                .collect();
            if missing_tokens.is_empty()
                && (tokens.len() < 4 || session.first_user_prompt.is_none())
            {
                continue;
            }
            let paths = transcript_paths(session);
            self.candidates.push(Candidate {
                id: session.id.clone(),
                paths,
                search_index: session.search_index.clone(),
                missing_tokens,
                exact_field: exact_metadata_field(session, query),
                opening_prompt: session
                    .first_user_prompt
                    .as_deref()
                    .unwrap_or("")
                    .to_owned(),
                intent_text: format!(
                    "{}\n{}\n{}",
                    session.title,
                    session.save_label.as_deref().unwrap_or(""),
                    session.first_user_prompt.as_deref().unwrap_or(""),
                ),
            });
        }
        if !self.candidates.is_empty() {
            self.started_at = Some(Instant::now());
            warmup::schedule(
                self.candidates
                    .iter()
                    .flat_map(|candidate| candidate.paths.iter()),
            );
        }
    }

    pub(super) fn contains(&self, id: &str) -> bool {
        self.ranks.contains_key(id)
    }

    #[cfg(test)]
    pub(super) fn contains_literal(&self, id: &str) -> bool {
        self.ranks.get(id).is_some_and(|rank| rank.literal)
    }

    pub(super) fn rank(&self, id: &str) -> SearchRank {
        self.ranks
            .get(id)
            .map_or_else(SearchRank::default, |rank| *rank)
    }

    pub(super) fn defers_metadata(&self) -> bool {
        self.defer_metadata
    }

    pub(super) fn pending(&self) -> bool {
        self.started_at.is_some() || self.receiver.is_some()
    }

    /// Start pending queries immediately, including cold scans.
    /// IO and fuzzy matching stay on the cancellable background worker.
    pub(super) fn poll(&mut self) -> bool {
        if self.started_at.take().is_some() {
            let candidates = std::mem::take(&mut self.candidates);
            let cancel = self.cancel.clone();
            let tokens = search_query_tokens(&self.query);
            let query = self.query.clone();
            let (sender, receiver) = mpsc::channel();
            match std::thread::Builder::new()
                .name("jcode-transcript-search".into())
                .spawn(move || {
                    // Finish the in-memory fuzzy pass before opening any large
                    // transcripts. Neither DP matching nor disk scans belong
                    // on the keystroke/render thread.
                    let mut scores = WordScores::default();
                    let mut fuzzy_candidates = Vec::new();
                    for (mut candidate, priority) in
                        prioritize_candidates(candidates, &query, &tokens, &cancel)
                    {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        if priority.opening_prompt > 0 {
                            let mut rank = fuzzy_metadata_rank_with_scores(
                                &candidate.search_index,
                                &tokens,
                                &mut scores,
                            );
                            rank.opening_prompt = priority.opening_prompt;
                            rank.intent_terms = priority.intent_terms;
                            if sender.send((candidate.id.clone(), rank)).is_err() {
                                return;
                            }
                        }
                        candidate
                            .missing_tokens
                            .retain(|token| !candidate.search_index.contains(token.as_str()));
                        if candidate.missing_tokens.is_empty() {
                            let rank = literal_metadata_rank(&candidate.search_index, &tokens)
                                .with_exact_field(candidate.exact_field);
                            if sender.send((candidate.id, rank)).is_err() {
                                return;
                            }
                            continue;
                        }
                        fuzzy_candidates.push((candidate, priority));
                    }
                    let mut transcripts = Vec::new();
                    for (mut candidate, priority) in fuzzy_candidates {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let mut fuzzy_missing = candidate.missing_tokens.clone();
                        let mut rank = fuzzy_metadata_rank_with_scores(
                            &candidate.search_index,
                            &tokens,
                            &mut scores,
                        )
                        .with_exact_field(candidate.exact_field);
                        rank.opening_prompt = priority.opening_prompt;
                        rank.intent_terms = priority.intent_terms;
                        fuzzy_missing
                            .retain(|token| !scores.matches_text(token, &candidate.search_index));
                        candidate.search_index = String::new();
                        if fuzzy_missing.is_empty()
                            && sender.send((candidate.id.clone(), rank)).is_err()
                        {
                            return;
                        }
                        if !candidate.paths.is_empty() {
                            transcripts.push((candidate, fuzzy_missing, rank));
                        }
                    }
                    // Fuzzy scoring adds evidence unavailable to the cheap
                    // pass. Use it to schedule disk reads, too.
                    transcripts.sort_by_key(|(_, _, rank)| std::cmp::Reverse(*rank));
                    let mut cold_transcripts = Vec::new();
                    let mut warm_fuzzy = Vec::new();
                    // Reuse resident token evidence before waiting for a typing
                    // pause. Never narrow fuzzy candidates using literal results.
                    for (candidate, fuzzy_missing, rank) in transcripts {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        match cached_paths_scan_with_scores(
                            &candidate.paths,
                            &candidate.missing_tokens,
                            &cancel,
                            false,
                            &mut scores,
                        ) {
                            Some(Some(_)) => {
                                if sender
                                    .send((
                                        candidate.id,
                                        SearchRank {
                                            literal: true,
                                            ..rank
                                        },
                                    ))
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            Some(None) if fuzzy_missing.is_empty() => {}
                            Some(None) => warm_fuzzy.push((candidate, fuzzy_missing, rank)),
                            None => cold_transcripts.push((candidate, fuzzy_missing, rank)),
                        }
                    }
                    for (candidate, missing, rank) in warm_fuzzy {
                        match cached_paths_scan_with_scores(
                            &candidate.paths,
                            &missing,
                            &cancel,
                            true,
                            &mut scores,
                        ) {
                            Some(Some(score)) => {
                                if sender
                                    .send((
                                        candidate.id,
                                        SearchRank {
                                            score: rank.score + score,
                                            ..rank
                                        },
                                    ))
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            Some(None) => {}
                            None => cold_transcripts.push((candidate, missing, rank)),
                        }
                    }
                    let mut fuzzy_transcripts = Vec::new();
                    for (candidate, fuzzy_missing, rank) in cold_transcripts {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        if paths_match_mode(
                            &candidate.paths,
                            &candidate.missing_tokens,
                            &cancel,
                            false,
                        ) {
                            if sender
                                .send((
                                    candidate.id,
                                    SearchRank {
                                        literal: true,
                                        ..rank
                                    },
                                ))
                                .is_err()
                            {
                                return;
                            }
                        } else if !fuzzy_missing.is_empty() {
                            fuzzy_transcripts.push((candidate, fuzzy_missing, rank));
                        }
                    }
                    // Exact transcript hits must not wait behind typo-tolerant
                    // scans of unrelated, potentially very large sessions.
                    for (candidate, missing, rank) in fuzzy_transcripts {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Some(score) = scan_paths_with_scores(
                            &candidate.paths,
                            &missing,
                            &cancel,
                            true,
                            &mut scores,
                        ) {
                            let rank = SearchRank {
                                score: rank.score + score,
                                ..rank
                            };
                            if sender.send((candidate.id, rank)).is_err() {
                                return;
                            }
                        }
                    }
                }) {
                Ok(_) => self.receiver = Some(receiver),
                Err(error) => {
                    crate::logging::debug(&format!("Cannot start transcript search: {error}"))
                }
            }
        }
        let mut changed = false;
        if let Some(receiver) = &self.receiver {
            for _ in 0..64 {
                match receiver.try_recv() {
                    Ok((id, rank)) => {
                        // Progressive evidence may upgrade a match, never
                        // erase a stronger earlier rank when a scan finishes.
                        if self.ranks.get(&id).is_none_or(|previous| rank > *previous) {
                            self.ranks.insert(id, rank);
                            changed = true;
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.receiver = None;
                        break;
                    }
                }
            }
        }
        changed
    }
}

fn transcript_paths(session: &SessionInfo) -> Vec<PathBuf> {
    match &session.resume_target {
        ResumeTarget::JcodeSession { session_id } => {
            // IDs must remain filename stems, not arbitrary paths.
            if session_id.contains(['/', '\\', '\0']) {
                return Vec::new();
            }
            let home = match crate::storage::jcode_dir() {
                Ok(home) => home,
                Err(error) => {
                    crate::logging::debug(&format!(
                        "Cannot locate transcripts for session {session_id}: {error}"
                    ));
                    return Vec::new();
                }
            };
            let dir = home.join("sessions");
            vec![
                // JSONL records use serde's fast slice parser, while snapshots
                // require its streaming reader. Search the journal first and
                // retain the snapshot fallback for compacted/legacy sessions.
                dir.join(format!("{session_id}.journal.jsonl")),
                dir.join(format!("{session_id}.json")),
            ]
        }
        ResumeTarget::ClaudeCodeSession { session_path, .. }
        | ResumeTarget::CodexSession { session_path, .. }
        | ResumeTarget::PiSession { session_path }
        | ResumeTarget::OpenCodeSession { session_path, .. }
        | ResumeTarget::CursorSession { session_path, .. } => vec![PathBuf::from(session_path)],
    }
}

struct Scan<'a> {
    tokens: &'a [String],
    found: Vec<bool>,
    cancel: &'a AtomicBool,
    fuzzy: bool,
    score: i64,
    scores: &'a mut WordScores,
}

impl Scan<'_> {
    fn complete(&self) -> bool {
        self.found.iter().all(|found| *found)
    }

    fn text<E: serde::de::Error>(&mut self, text: &str) -> Result<(), E> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(E::custom("search cancelled"));
        }
        self.normalized_text(&text.to_lowercase())
    }

    fn normalized_text<E: serde::de::Error>(&mut self, lower: &str) -> Result<(), E> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(E::custom("search cancelled"));
        }
        for (token, found) in self.tokens.iter().zip(&mut self.found) {
            if !*found {
                if lower.contains(token) {
                    *found = true;
                    if self.fuzzy {
                        self.score +=
                            i64::from(jcode_fuzzy::fuzzy_score(token, token).unwrap_or(0));
                    }
                } else if self.fuzzy
                    && token.chars().count() >= 4
                    && token.chars().all(char::is_alphabetic)
                {
                    // Score the words that actually matched, not unrelated
                    // metadata. Bound DP work for URLs and minified output.
                    // Transcript evidence prefers the closest word length
                    // over an incidental longer prefix or suffix match.
                    if let Some(score) = self.scores.best_score(token, lower, true) {
                        *found = true;
                        self.score += i64::from(score);
                    }
                }
            }
        }
        if self.complete() {
            return Err(E::custom("search complete"));
        }
        Ok(())
    }
}

// Decode one JSON string at a time, including escaped Unicode and newlines.
// Do not materialize a many-MB session or mistake JSON field names for prose.
impl<'de> DeserializeSeed<'de> for &mut Scan<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(serde::de::Error::custom("search cancelled"));
        }
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for &mut Scan<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("transcript JSON")
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<(), E> {
        self.text(value)
    }
    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<(), E> {
        self.text(&value)
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq.next_element_seed(&mut *self)?.is_some() {}
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while map.next_key::<IgnoredAny>()?.is_some() {
            map.next_value_seed(&mut *self)?;
        }
        Ok(())
    }
}

struct CancellableReader<'a> {
    reader: std::fs::File,
    cancel: &'a AtomicBool,
}
impl Read for CancellableReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("search cancelled"));
        }
        self.reader.read(buf)
    }
}

#[cfg(test)]
fn paths_match(paths: &[PathBuf], tokens: &[String], cancel: &AtomicBool) -> bool {
    paths_match_mode(paths, tokens, cancel, true)
}

#[cfg(test)]
fn paths_match_scored(paths: &[PathBuf], tokens: &[String], cancel: &AtomicBool) -> Option<i64> {
    scan_paths(paths, tokens, cancel, true)
}

fn paths_match_mode(
    paths: &[PathBuf],
    tokens: &[String],
    cancel: &AtomicBool,
    fuzzy: bool,
) -> bool {
    scan_paths(paths, tokens, cancel, fuzzy).is_some()
}

fn scan_paths(
    paths: &[PathBuf],
    tokens: &[String],
    cancel: &AtomicBool,
    fuzzy: bool,
) -> Option<i64> {
    scan_paths_with_scores(paths, tokens, cancel, fuzzy, &mut WordScores::default())
}

fn scan_paths_with_scores(
    paths: &[PathBuf],
    tokens: &[String],
    cancel: &AtomicBool,
    fuzzy: bool,
    scores: &mut WordScores,
) -> Option<i64> {
    let mut scan = Scan {
        tokens,
        found: vec![false; tokens.len()],
        cancel,
        fuzzy,
        score: 0,
        scores,
    };
    for path in paths {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        scan_file(path, &mut scan);
        if scan.complete() {
            return Some(scan.score);
        }
    }
    None
}

fn apply_token_matches(scan: &mut Scan<'_>, matches: Vec<Option<i64>>) {
    for (found, score) in scan.found.iter_mut().zip(matches) {
        if !*found && let Some(score) = score {
            *found = true;
            scan.score += score;
        }
    }
}

/// Outer None means at least one file needs cold work. Inner None is a
/// definitive resident nonmatch. Literal evidence can succeed on a subset,
/// but fuzzy scores require all earlier paths to preserve first-match scoring.
#[cfg(test)]
fn cached_paths_scan(
    paths: &[PathBuf],
    tokens: &[String],
    cancel: &AtomicBool,
    fuzzy: bool,
) -> Option<Option<i64>> {
    cached_paths_scan_with_scores(paths, tokens, cancel, fuzzy, &mut WordScores::default())
}

fn cached_paths_scan_with_scores(
    paths: &[PathBuf],
    tokens: &[String],
    cancel: &AtomicBool,
    fuzzy: bool,
    scores: &mut WordScores,
) -> Option<Option<i64>> {
    let mut scan = Scan {
        tokens,
        found: vec![false; tokens.len()],
        cancel,
        fuzzy,
        score: 0,
        scores,
    };
    let mut all_resident = true;
    for path in paths {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        if let Some(matches) =
            token_cache::matches_cached_with_scores(path, tokens, cancel, fuzzy, scan.scores)
        {
            apply_token_matches(&mut scan, matches);
            if scan.complete() && (!fuzzy || all_resident) {
                return Some(Some(scan.score));
            }
        } else if !std::fs::metadata(path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            // Missing journal/snapshot alternatives need no cold read. Other
            // errors and existing uncached files still require normal fallback.
            all_resident = false;
        }
    }
    if cancel.load(Ordering::Relaxed) || !all_resident {
        None
    } else {
        Some(scan.complete().then_some(scan.score))
    }
}

fn scan_file(path: &Path, scan: &mut Scan<'_>) {
    if let Some(matches) =
        token_cache::matches_with_scores(path, scan.tokens, scan.cancel, scan.fuzzy, scan.scores)
    {
        apply_token_matches(scan, matches);
        return;
    }
    scan_file_streaming(path, scan);
}

fn scan_file_streaming(path: &Path, scan: &mut Scan<'_>) {
    if scan.cancel.load(Ordering::Relaxed) {
        return;
    }
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        // Snapshot and journal paths are alternatives and may not both exist.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            crate::logging::debug(&format!(
                "Cannot open transcript {}: {error}",
                path.display()
            ));
            return;
        }
    };
    let mut reader = BufReader::with_capacity(
        64 * 1024,
        CancellableReader {
            reader: file,
            cancel: scan.cancel,
        },
    );
    if path.extension().is_some_and(|ext| ext == "jsonl") {
        // Parse records independently: a partial or corrupt record must not
        // hide later valid messages. from_slice is also much faster here than
        // serde's byte-at-a-time reader adapter.
        let mut line = Vec::new();
        let mut record = 0;
        while !scan.cancel.load(Ordering::Relaxed) {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Err(error) => {
                    if !scan.cancel.load(Ordering::Relaxed) {
                        crate::logging::debug(&format!(
                            "Cannot read transcript {}: {error}",
                            path.display()
                        ));
                    }
                    break;
                }
                Ok(_) => {
                    record += 1;
                    if let Err(error) =
                        (&mut *scan).deserialize(&mut serde_json::Deserializer::from_slice(&line))
                    {
                        log_scan_error(path, Some(record), scan, &error);
                    }
                    if scan.complete() {
                        break;
                    }
                }
            }
        }
        return;
    }
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    // The stream can be one snapshot or multiple JSONL records.
    loop {
        match (&mut *scan).deserialize(&mut deserializer) {
            Ok(()) => {}
            Err(error) => {
                // EOF is normal between values or in a live, partially written record.
                if !error.is_eof() {
                    log_scan_error(path, None, scan, &error);
                }
                break;
            }
        }
    }
}

fn log_scan_error(path: &Path, record: Option<usize>, scan: &Scan<'_>, error: &serde_json::Error) {
    // The visitor uses errors for successful early exit and cancellation.
    if scan.complete() || scan.cancel.load(Ordering::Relaxed) {
        return;
    }
    crate::logging::debug(&format!(
        "Cannot decode transcript {} (record {record:?}): {error}",
        path.display()
    ));
}

#[cfg(test)]
#[path = "transcript_search_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "relevance_tests.rs"]
mod relevance_tests;

#[cfg(test)]
#[path = "search_speed_tests.rs"]
mod search_speed_tests;
