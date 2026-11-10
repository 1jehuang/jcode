//! Incremental fuzzy and full-transcript search without typing delays.
//! All matching, including literal metadata and identity fields, runs on
//! cancellable workers. The UI only consumes bounded batches of rank evidence.
use super::loading::search_query_tokens;
use super::{ResumeTarget, SessionInfo};
use serde::de::DeserializeSeed;
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
pub(super) mod source;
mod token_cache;
mod warmup;
use source::Source;

#[cfg(test)]
const IMMEDIATE_METADATA_BYTES: usize = 1024 * 1024;

/// A close contiguous opening request outranks incidental transcript evidence.
/// Otherwise literal evidence outranks typo-only evidence, then metadata and
/// whole words break ties instead of unrelated session recency.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct SearchRank {
    opening_prompt: usize,
    /// Sum of per-token origin weights scaled by word rarity: user prompts
    /// beat replies, which beat tool calls, tool output, and reasoning, and a
    /// rare word outweighs ones found in most sessions. Ranked above literal
    /// so a single typo cannot sink a session with strong, distinctive hits.
    source_weight: u32,
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
        source_weight: 0,
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

    pub(super) fn with_source_weight(mut self, weight: u32) -> Self {
        self.source_weight = weight;
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

/// Query-independent search inputs for one session. Shared with the worker
/// so that a keystroke never copies the metadata index on the UI thread.
pub(super) struct Entry {
    id: String,
    paths: Vec<PathBuf>,
    search_index: String,
    title: String,
    short_name: String,
    save_label: Option<String>,
    has_prompt: bool,
    opening_prompt: String,
    intent_text: String,
    working_dir: Option<String>,
    server_name: Option<String>,
    cloud: bool,
}

/// Bytes compared at each end of an index to detect an in-place refresh.
const INDEX_PROBE_BYTES: usize = 64;

impl Entry {
    fn new(session: &SessionInfo) -> Self {
        Self {
            id: session.id.clone(),
            paths: transcript_paths(session),
            search_index: session.search_index.clone(),
            title: session.title.clone(),
            short_name: session.short_name.clone(),
            save_label: session.save_label.clone(),
            working_dir: session.working_dir.clone(),
            server_name: session.server_name.clone(),
            cloud: session.server_icon.as_deref() == Some("☁"),
            has_prompt: session.first_user_prompt.is_some(),
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
        }
    }

    /// Cheap check that this entry still reflects `session`, so an unchanged
    /// history is reused across keystrokes without comparing whole indexes.
    fn describes(&self, session: &SessionInfo) -> bool {
        let (old, new) = (
            self.search_index.as_bytes(),
            session.search_index.as_bytes(),
        );
        let probe = INDEX_PROBE_BYTES.min(old.len());
        self.id == session.id
            && old.len() == new.len()
            && old[..probe] == new[..probe]
            && old[old.len() - probe..] == new[new.len() - probe..]
            && self.title == session.title
            && self.short_name == session.short_name
            && self.save_label == session.save_label
            && self.working_dir == session.working_dir
            && self.server_name == session.server_name
            && self.cloud == (session.server_icon.as_deref() == Some("☁"))
            && self.has_prompt == session.first_user_prompt.is_some()
            && self.opening_prompt == session.first_user_prompt.as_deref().unwrap_or("")
    }

    fn exact_field(&self, query: &str) -> bool {
        [&self.title, &self.short_name, &self.id]
            .iter()
            .any(|field| normalized_equals(field, query))
            || self
                .save_label
                .as_ref()
                .is_some_and(|label| normalized_equals(label, query))
    }

    fn identity_rank(&self, tokens: &[String]) -> (bool, bool, bool) {
        super::identity_search::rank_fields(
            [
                &self.id,
                &self.short_name,
                &self.title,
                self.save_label.as_deref().unwrap_or(""),
                self.working_dir.as_deref().unwrap_or(""),
                self.server_name.as_deref().unwrap_or(""),
            ],
            self.cloud,
            tokens,
        )
    }
}

struct Candidate {
    entry: Arc<Entry>,
    missing_tokens: Vec<String>,
    exact_field: bool,
    /// Per-token origin evidence from metadata, before transcript reads.
    prior: Vec<Option<Source>>,
}

impl std::ops::Deref for Candidate {
    type Target = Entry;
    fn deref(&self) -> &Entry {
        &self.entry
    }
}

/// Decide which sessions need worker evidence for `query`. With a deferred
/// (large) history this runs on the worker, never on the keystroke.
fn build_candidates(
    corpus: &[Arc<Entry>],
    query: &str,
    tokens: &[String],
    defer_metadata: bool,
) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for entry in corpus {
        let missing_tokens: Vec<_> = tokens
            .iter()
            .filter(|token| defer_metadata || !entry.search_index.contains(token.as_str()))
            .cloned()
            .collect();
        // Literal metadata matches still go to the worker, which upgrades
        // their rank once it knows where in the transcript each word came from,
        // unless every word already comes from a user-intent field.
        if !defer_metadata && missing_tokens.is_empty() && (tokens.len() < 4 || !entry.has_prompt) {
            let intent = lowercase_field(&entry.intent_text);
            if tokens.iter().all(|token| intent.contains(token.as_str())) {
                continue;
            }
        }
        candidates.push(Candidate {
            entry: entry.clone(),
            missing_tokens,
            exact_field: entry.exact_field(query),
            prior: Vec::new(),
        });
    }
    candidates
}

/// Per-token metadata origins for the UI-thread ranking of sessions the worker
/// has not ranked yet. Uses the already lowercased intent fields.
pub(super) fn metadata_source_weight(
    intent_fields: &[&str],
    index: &str,
    tokens: &[String],
    idf: &[u32],
) -> u32 {
    source::weighted_source_weight(&source::metadata_sources(intent_fields, index, tokens), idf)
}

/// Transcript evidence: the first-match fuzzy score plus per-token origins.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Evidence {
    score: i64,
    sources: Vec<Option<Source>>,
}

/// Rank in memory before publishing results or opening transcripts. Keep this
/// on the background worker, not the render thread. Stable sorting preserves
/// the incoming order for equally promising candidates.
fn prioritize_candidates(
    candidates: Vec<Candidate>,
    phrase: &Option<PhraseQuery>,
    tokens: &[String],
    idf: &[u32],
    cancel: &AtomicBool,
) -> Vec<(Candidate, SearchRank)> {
    let phrase = phrase.as_ref();
    let mut ranked = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if cancel.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let mut rank = literal_metadata_rank(&candidate.search_index, tokens)
            .with_exact_field(candidate.exact_field);
        rank.opening_prompt = phrase.map_or(0, |phrase| phrase.rank(&candidate.opening_prompt));
        let intent = lowercase_field(&candidate.intent_text);
        rank.intent_terms = tokens
            .iter()
            .filter(|token| intent.contains(token.as_str()))
            .count();
        let prior = source::metadata_sources(&[&intent], &candidate.search_index, tokens);
        drop(intent);
        let mut candidate = candidate;
        candidate.prior = prior;
        rank.source_weight = source::weighted_source_weight(&candidate.prior, idf);
        ranked.push((candidate, rank));
    }
    ranked.sort_by_key(|(_, rank)| std::cmp::Reverse(*rank));
    ranked
}

// A retyped request is stronger evidence than words scattered throughout a
// huge transcript. Compare contiguous word windows, allowing a few misspelled
// words without requiring every short function word to match literally.
#[cfg(test)]
fn opening_prompt_rank(prompt: &str, query: &str) -> usize {
    PhraseQuery::new(query).map_or(0, |phrase| phrase.rank(prompt))
}

fn phrase_words(text: &str, limit: usize) -> Vec<String> {
    text.split_whitespace()
        .take(limit)
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

/// A multiword query prepared once and compared against many user prompts.
struct PhraseQuery {
    words: Vec<String>,
    compact: String,
    prepared: Vec<jcode_fuzzy::PreparedTokenQuery>,
}

impl PhraseQuery {
    fn new(query: &str) -> Option<Self> {
        let words = phrase_words(query, 256);
        if words.len() < 4 || words.len() > 64 {
            return None;
        }
        Some(Self {
            compact: words.concat(),
            prepared: words
                .iter()
                .map(|word| jcode_fuzzy::PreparedTokenQuery::new(word))
                .collect(),
            words,
        })
    }

    fn rank(&self, prompt: &str) -> usize {
        let prompt = phrase_words(prompt, 2048);
        if prompt.len() + 1 < self.words.len() * 4 / 5 {
            return 0;
        }
        // Missing spaces ("ofhow") should not hide an otherwise exact request.
        if self.compact.chars().count() >= 16 && prompt.concat().contains(&self.compact) {
            return self.words.len();
        }
        prompt
            .windows(self.words.len())
            .filter_map(|window| {
                let matched = self
                    .words
                    .iter()
                    .zip(&self.prepared)
                    .zip(window)
                    .filter(|((word, prepared), candidate)| {
                        word == candidate
                            || (word.len() >= 4
                                && candidate.len() <= 128
                                && prepared.matches_search_text(candidate))
                    })
                    .count();
                (matched * 5 >= self.words.len() * 4).then_some(matched)
            })
            .max()
            .unwrap_or(0)
    }
}

/// Best retyped-request match across every user prompt in a transcript, not
/// only the opening one. Later follow-ups ("okay lets do that") are what
/// people remember and retype just as often.
fn user_prompt_phrase_rank(paths: &[PathBuf], phrase: &PhraseQuery, cancel: &AtomicBool) -> usize {
    let mut sink = PhraseSink {
        phrase,
        best: 0,
        cancel,
    };
    for path in paths {
        if cancel.load(Ordering::Relaxed) || sink.best == phrase.words.len() {
            break;
        }
        if let Some(segments) = text_cache::segments(path, cancel) {
            for (text, source) in segments.iter() {
                if source == Source::User {
                    sink.best = sink.best.max(phrase.rank(text));
                }
            }
            continue;
        }
        stream_file(path, &mut sink, cancel);
    }
    sink.best
}

struct PhraseSink<'a> {
    phrase: &'a PhraseQuery,
    best: usize,
    cancel: &'a AtomicBool,
}

impl source::Sink for PhraseSink<'_> {
    fn text(&mut self, text: &str, source: Source) -> Result<(), &'static str> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err("search cancelled");
        }
        if source == Source::User {
            self.best = self.best.max(self.phrase.rank(text));
            if self.best == self.phrase.words.len() {
                return Err("search complete");
            }
        }
        Ok(())
    }
}

/// Stream a transcript too large for the text cache through a sink.
fn stream_file<S: source::Sink>(path: &Path, sink: &mut S, cancel: &AtomicBool) {
    let Ok(file) = std::fs::File::open(path) else {
        return;
    };
    let mut reader = BufReader::with_capacity(
        64 * 1024,
        CancellableReader {
            reader: file,
            cancel,
        },
    );
    if path.extension().is_some_and(|ext| ext == "jsonl") {
        let mut line = Vec::new();
        while !cancel.load(Ordering::Relaxed) {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let _ = source::Walk::new(&mut *sink)
                        .deserialize(&mut serde_json::Deserializer::from_slice(&line));
                }
            }
        }
        return;
    }
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    while source::Walk::new(&mut *sink)
        .deserialize(&mut deserializer)
        .is_ok()
    {}
}

#[derive(Default)]
pub(super) struct TranscriptSearch {
    query: String,
    /// One shared entry per session, reused across queries.
    corpus: Vec<Arc<Entry>>,
    candidates: Vec<Candidate>,
    started_at: Option<Instant>,
    receiver: Option<mpsc::Receiver<(String, SearchRank)>>,
    cancel: Arc<AtomicBool>,
    ranks: HashMap<String, SearchRank>,
    defer_metadata: bool,
    /// Per-token rarity weights, computed by the worker before any rank.
    idf: Arc<std::sync::OnceLock<Vec<u32>>>,
    identities: Arc<std::sync::OnceLock<HashMap<String, (bool, bool, bool)>>>,
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
        // Reuse every unchanged entry. Only a new or refreshed session copies
        // its metadata index, so typing costs no per-keystroke megabytes.
        let mut previous = std::mem::take(&mut self.corpus).into_iter();
        let corpus: Vec<Arc<Entry>> = sessions
            .map(|session| match previous.next() {
                Some(entry) if entry.describes(session) => entry,
                _ => Arc::new(Entry::new(session)),
            })
            .collect();
        *self = Self::default();
        self.corpus = corpus;
        self.query = query.to_string();
        if query.is_empty() {
            return;
        }
        self.defer_metadata = true;
        // Candidate selection touches every index and always belongs to the
        // worker, including tiny histories.
        if !self.corpus.is_empty() {
            self.started_at = Some(Instant::now());
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

    pub(super) fn identity_rank(&self, id: &str) -> (bool, bool, bool) {
        self.identities
            .get()
            .and_then(|identities| identities.get(id).copied())
            .unwrap_or_default()
    }

    /// Rarity weights for the current query, once the worker computed them.
    pub(super) fn idf(&self) -> &[u32] {
        self.idf.get().map_or(&[], Vec::as_slice)
    }

    /// The worker reported at least one match for the current query.
    pub(super) fn has_results(&self) -> bool {
        !self.ranks.is_empty()
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
            let deferred = self.defer_metadata.then(|| self.corpus.clone());
            let cancel = self.cancel.clone();
            let tokens = search_query_tokens(&self.query);
            let query = self.query.clone();
            let shared_idf = self.idf.clone();
            let shared_identities = self.identities.clone();
            let (sender, receiver) = mpsc::channel();
            match std::thread::Builder::new()
                .name("jcode-transcript-search".into())
                .spawn(move || {
                    let candidates = match deferred {
                        Some(corpus) => {
                            let candidates = build_candidates(&corpus, &query, &tokens, true);
                            warmup::schedule(
                                candidates
                                    .iter()
                                    .flat_map(|candidate| candidate.paths.iter()),
                            );
                            candidates
                        }
                        None => candidates,
                    };
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let identities: HashMap<_, _> = candidates
                        .iter()
                        .filter_map(|candidate| {
                            let rank = candidate.identity_rank(&tokens);
                            rank.0.then(|| (candidate.id.clone(), rank))
                        })
                        .collect();
                    let identity_ids: Vec<_> = identities.keys().cloned().collect();
                    let _ = shared_identities.set(identities);
                    for id in identity_ids {
                        if sender.send((id, SearchRank::default())).is_err() {
                            return;
                        }
                    }
                    // Document frequency over every candidate's metadata index.
                    // Off the render thread: this touches every session.
                    let idf = source::idf_weights(
                        &tokens,
                        candidates
                            .iter()
                            .map(|candidate| candidate.search_index.as_str()),
                    );
                    let _ = shared_idf.set(idf.clone());
                    // Transcript reads and fuzzy scans dominate. Shard sessions
                    // across workers round-robin so every shard sees the same
                    // mix of recent and old sessions and ranks arrive evenly.
                    let workers = search_workers(candidates.len());
                    // Rank in memory first so the strongest retyped openings
                    // are published before any shard starts reading disk.
                    let phrase = PhraseQuery::new(&query);
                    let ranked = prioritize_candidates(candidates, &phrase, &tokens, &idf, &cancel);
                    let mut scores = WordScores::default();
                    for (candidate, priority) in &ranked {
                        if priority.opening_prompt > 0 {
                            let mut rank = fuzzy_metadata_rank_with_scores(
                                &candidate.search_index,
                                &tokens,
                                &mut scores,
                            );
                            rank.opening_prompt = priority.opening_prompt;
                            rank.intent_terms = priority.intent_terms;
                            rank.source_weight = priority.source_weight;
                            if sender.send((candidate.id.clone(), rank)).is_err() {
                                return;
                            }
                        }
                    }
                    let mut shards: Vec<Vec<(Candidate, SearchRank)>> =
                        (0..workers).map(|_| Vec::new()).collect();
                    for (index, candidate) in ranked.into_iter().enumerate() {
                        shards[index % workers].push(candidate);
                    }
                    std::thread::scope(|scope| {
                        for shard in shards {
                            let sender = sender.clone();
                            let (query, tokens, idf, cancel) = (&query, &tokens, &idf, &cancel);
                            let spawned = std::thread::Builder::new()
                                .name("jcode-transcript-search-shard".into())
                                .spawn_scoped(scope, move || {
                                    search_shard(shard, query, tokens, idf, cancel, sender)
                                });
                            if let Err(error) = spawned {
                                crate::logging::debug(&format!(
                                    "Cannot start transcript search shard: {error}"
                                ));
                            }
                        }
                    });
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

/// Parallel shards for one query. Bounded so search never starves the UI and
/// small session lists do not pay for idle threads.
fn search_workers(candidates: usize) -> usize {
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    (cores / 2).clamp(1, 8).min(candidates.div_ceil(16).max(1))
}

/// Rank one shard of candidates, cheapest evidence first.
fn search_shard(
    candidates: Vec<(Candidate, SearchRank)>,
    query: &str,
    tokens: &[String],
    idf: &[u32],
    cancel: &AtomicBool,
    sender: mpsc::Sender<(String, SearchRank)>,
) {
    // Finish the in-memory fuzzy pass before opening any large
    // transcripts. Neither DP matching nor disk scans belong
    // on the keystroke/render thread.
    let mut scores = WordScores::default();
    let mut fuzzy_candidates = Vec::new();
    // Metadata matches whose words may have come from a user
    // prompt rather than a sampled tool dump or reasoning.
    let mut classify = Vec::new();
    let phrase = PhraseQuery::new(query);
    // Sessions whose later user prompts may contain the query
    // as a retyped phrase, checked after cheaper evidence.
    let mut phrase_jobs = Vec::new();
    for (mut candidate, priority) in candidates {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if phrase.is_some() && !candidate.paths.is_empty() {
            phrase_jobs.push((candidate.id.clone(), candidate.paths.clone(), priority));
        }
        if priority.opening_prompt > 0 {
            let mut rank =
                fuzzy_metadata_rank_with_scores(&candidate.search_index, &tokens, &mut scores);
            rank.opening_prompt = priority.opening_prompt;
            rank.intent_terms = priority.intent_terms;
            rank.source_weight = priority.source_weight;
            if sender.send((candidate.id.clone(), rank)).is_err() {
                return;
            }
        }
        candidate
            .missing_tokens
            .retain(|token| !candidate.entry.search_index.contains(token.as_str()));
        if candidate.missing_tokens.is_empty() {
            let mut rank = literal_metadata_rank(&candidate.search_index, &tokens)
                .with_exact_field(candidate.exact_field)
                .with_source_weight(priority.source_weight);
            rank.opening_prompt = priority.opening_prompt;
            rank.intent_terms = priority.intent_terms;
            if sender.send((candidate.id.clone(), rank)).is_err() {
                return;
            }
            if !candidate.paths.is_empty()
                && candidate
                    .prior
                    .iter()
                    .any(|source| *source != Some(Source::User))
            {
                classify.push((candidate, rank));
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
        let mut rank =
            fuzzy_metadata_rank_with_scores(&candidate.search_index, &tokens, &mut scores)
                .with_exact_field(candidate.exact_field);
        rank.opening_prompt = priority.opening_prompt;
        rank.intent_terms = priority.intent_terms;
        rank.source_weight = priority.source_weight;
        fuzzy_missing.retain(|token| !scores.matches_text(token, &candidate.search_index));
        if fuzzy_missing.is_empty() && sender.send((candidate.id.clone(), rank)).is_err() {
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
            &tokens,
            &candidate.prior,
            &cancel,
            false,
            &mut scores,
        ) {
            Some(Some(evidence)) => {
                if sender
                    .send((
                        candidate.id.clone(),
                        SearchRank {
                            literal: true,
                            source_weight: source::weighted_source_weight(&evidence.sources, &idf),
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
        let prior = fuzzy_prior(&tokens, &candidate.prior, &missing);
        match cached_paths_scan_with_scores(
            &candidate.paths,
            &tokens,
            &prior,
            &cancel,
            true,
            &mut scores,
        ) {
            Some(Some(evidence)) => {
                if sender
                    .send((
                        candidate.id.clone(),
                        SearchRank {
                            score: rank.score + evidence.score,
                            source_weight: source::weighted_source_weight(&evidence.sources, &idf),
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
        if let Some(evidence) = scan_paths_with_scores(
            &candidate.paths,
            &tokens,
            &candidate.prior,
            &cancel,
            ScanMode::Literal,
            &mut scores,
        ) {
            if sender
                .send((
                    candidate.id.clone(),
                    SearchRank {
                        literal: true,
                        source_weight: source::weighted_source_weight(&evidence.sources, &idf),
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
    // Read where metadata matches came from before slower
    // typo-tolerant scans, so a session whose words the user
    // typed rises above one where they only appear in tool output.
    for (candidate, rank) in classify {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if let Some(evidence) = scan_paths_with_scores(
            &candidate.paths,
            &tokens,
            &candidate.prior,
            &cancel,
            ScanMode::Classify,
            &mut scores,
        ) {
            let weight = source::weighted_source_weight(&evidence.sources, &idf);
            if weight > rank.source_weight
                && sender
                    .send((candidate.id.clone(), rank.with_source_weight(weight)))
                    .is_err()
            {
                return;
            }
        }
    }
    // Exact transcript hits must not wait behind typo-tolerant
    // scans of unrelated, potentially very large sessions.
    for (candidate, missing, rank) in fuzzy_transcripts {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let prior = fuzzy_prior(&tokens, &candidate.prior, &missing);
        if let Some(evidence) = scan_paths_with_scores(
            &candidate.paths,
            &tokens,
            &prior,
            &cancel,
            ScanMode::Fuzzy,
            &mut scores,
        ) {
            let rank = SearchRank {
                score: rank.score + evidence.score,
                source_weight: source::weighted_source_weight(&evidence.sources, &idf),
                ..rank
            };
            if sender.send((candidate.id.clone(), rank)).is_err() {
                return;
            }
        }
    }
    // A follow-up prompt the user retyped outranks scattered
    // word hits, the same as a retyped opening prompt.
    if let Some(phrase) = &phrase {
        for (id, paths, priority) in phrase_jobs {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let matched = user_prompt_phrase_rank(&paths, phrase, &cancel);
            if matched > priority.opening_prompt
                && sender
                    .send((
                        id,
                        SearchRank {
                            opening_prompt: matched,
                            source_weight: priority.source_weight.max(
                                source::weighted_source_weight(
                                    &vec![Some(Source::User); tokens.len()],
                                    &idf,
                                ),
                            ),
                            ..priority
                        },
                    ))
                    .is_err()
            {
                return;
            }
        }
    }
}

/// Sessions decoded ahead of the first query, most recent first. Bounded so
/// prefetch stays well inside the text cache's entry and byte budgets.
const PREFETCH_SESSIONS: usize = 96;

/// Start decoding the most recent transcripts as soon as the picker has its
/// session list, so typing begins against a warm cache.
pub(super) fn prefetch_recent<'a>(sessions: impl Iterator<Item = &'a SessionInfo>) {
    if cfg!(test) {
        return;
    }
    let mut recent: Vec<&SessionInfo> = sessions.collect();
    recent.sort_by(|a, b| b.last_message_time.cmp(&a.last_message_time));
    let paths = recent
        .into_iter()
        .take(PREFETCH_SESSIONS)
        .flat_map(|session| transcript_paths(session).into_iter().take(1))
        .collect();
    warmup::prefetch(paths);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScanMode {
    /// Stop once every token is found.
    Literal,
    /// Typo-tolerant matching. Stop once every token is found.
    Fuzzy,
    /// Literal matching that keeps reading until every token was seen in a
    /// user prompt, so the rank reflects the strongest origin of each word.
    Classify,
}

struct Scan<'a> {
    tokens: &'a [String],
    found: Vec<bool>,
    /// Strongest origin each token was seen in so far.
    sources: Vec<Option<Source>>,
    cancel: &'a AtomicBool,
    fuzzy: bool,
    classify: bool,
    score: i64,
    scores: &'a mut WordScores,
}

impl<'a> Scan<'a> {
    /// `prior` marks tokens already satisfied by metadata and their origin.
    fn new(
        tokens: &'a [String],
        prior: &[Option<Source>],
        cancel: &'a AtomicBool,
        mode: ScanMode,
        scores: &'a mut WordScores,
    ) -> Self {
        let sources: Vec<_> = (0..tokens.len())
            .map(|index| prior.get(index).copied().flatten())
            .collect();
        Self {
            tokens,
            found: sources.iter().map(Option::is_some).collect(),
            sources,
            cancel,
            fuzzy: mode == ScanMode::Fuzzy,
            classify: mode == ScanMode::Classify,
            score: 0,
            scores,
        }
    }

    fn complete(&self) -> bool {
        if self.classify {
            self.sources
                .iter()
                .all(|source| *source == Some(Source::User))
        } else {
            self.found.iter().all(|found| *found)
        }
    }

    fn all_found(&self) -> bool {
        self.found.iter().all(|found| *found)
    }

    fn evidence(&self) -> Option<Evidence> {
        self.all_found().then(|| Evidence {
            score: self.score,
            sources: self.sources.clone(),
        })
    }

    fn normalized_text<E: serde::de::Error>(
        &mut self,
        lower: &str,
        source: Source,
    ) -> Result<(), E> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(E::custom("search cancelled"));
        }
        for ((token, found), best) in self
            .tokens
            .iter()
            .zip(&mut self.found)
            .zip(&mut self.sources)
        {
            if *found {
                // Only a stronger origin can change the rank. Skipping weaker
                // segments keeps tool dumps cheap once a token is found.
                if best.is_none_or(|best| source.weight() > best.weight())
                    && lower.contains(token.as_str())
                {
                    *best = Some(source);
                }
                continue;
            }
            if lower.contains(token.as_str()) {
                *found = true;
                *best = Some(source);
                if self.fuzzy {
                    self.score += i64::from(jcode_fuzzy::fuzzy_score(token, token).unwrap_or(0));
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
                    *best = Some(source);
                    self.score += i64::from(score);
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
impl source::Sink for Scan<'_> {
    fn text(&mut self, text: &str, source: Source) -> Result<(), &'static str> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err("search cancelled");
        }
        self.normalized_text::<serde::de::value::Error>(&text.to_lowercase(), source)
            .map_err(|_| {
                if self.cancel.load(Ordering::Relaxed) {
                    "search cancelled"
                } else {
                    "search complete"
                }
            })
    }
}

/// Tokens outside `missing` were already satisfied by metadata.
fn fuzzy_prior(
    tokens: &[String],
    prior: &[Option<Source>],
    missing: &[String],
) -> Vec<Option<Source>> {
    tokens
        .iter()
        .enumerate()
        .map(|(index, token)| {
            if missing.contains(token) {
                None
            } else {
                prior
                    .get(index)
                    .copied()
                    .flatten()
                    .or(Some(Source::ToolOutput))
            }
        })
        .collect()
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

#[cfg(test)]
fn paths_match_mode(
    paths: &[PathBuf],
    tokens: &[String],
    cancel: &AtomicBool,
    fuzzy: bool,
) -> bool {
    scan_paths(paths, tokens, cancel, fuzzy).is_some()
}

#[cfg(test)]
fn mode(fuzzy: bool) -> ScanMode {
    if fuzzy {
        ScanMode::Fuzzy
    } else {
        ScanMode::Literal
    }
}

#[cfg(test)]
fn scan_paths(
    paths: &[PathBuf],
    tokens: &[String],
    cancel: &AtomicBool,
    fuzzy: bool,
) -> Option<i64> {
    scan_paths_with_scores(
        paths,
        tokens,
        &[],
        cancel,
        mode(fuzzy),
        &mut WordScores::default(),
    )
    .map(|evidence| evidence.score)
}

fn scan_paths_with_scores(
    paths: &[PathBuf],
    tokens: &[String],
    prior: &[Option<Source>],
    cancel: &AtomicBool,
    mode: ScanMode,
    scores: &mut WordScores,
) -> Option<Evidence> {
    let mut scan = Scan::new(tokens, prior, cancel, mode, scores);
    for path in paths {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        if scan.complete() {
            break;
        }
        scan_file(path, &mut scan);
    }
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    scan.evidence()
}

fn apply_token_matches(scan: &mut Scan<'_>, matches: Vec<Option<token_cache::Hit>>) {
    for ((found, best), hit) in scan
        .found
        .iter_mut()
        .zip(scan.sources.iter_mut())
        .zip(matches)
    {
        let Some(hit) = hit else { continue };
        if !*found {
            *found = true;
            scan.score += hit.score;
        }
        *best = source::stronger(*best, Some(hit.source));
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
    cached_paths_scan_with_scores(
        paths,
        tokens,
        &[],
        cancel,
        fuzzy,
        &mut WordScores::default(),
    )
    .map(|evidence| evidence.map(|evidence| evidence.score))
}

fn cached_paths_scan_with_scores(
    paths: &[PathBuf],
    tokens: &[String],
    prior: &[Option<Source>],
    cancel: &AtomicBool,
    fuzzy: bool,
    scores: &mut WordScores,
) -> Option<Option<Evidence>> {
    let mode = if fuzzy {
        ScanMode::Fuzzy
    } else {
        ScanMode::Literal
    };
    let mut scan = Scan::new(tokens, prior, cancel, mode, scores);
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
                return Some(scan.evidence());
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
        Some(scan.evidence())
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
                    if let Err(error) = source::Walk::new(&mut *scan)
                        .deserialize(&mut serde_json::Deserializer::from_slice(&line))
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
        match source::Walk::new(&mut *scan).deserialize(&mut deserializer) {
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
#[path = "queued_submission_tests.rs"]
mod queued_submission_tests;

#[cfg(test)]
#[path = "search_speed_tests.rs"]
mod search_speed_tests;
