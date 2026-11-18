//! Transcript shard search and polling.

use super::*;

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
                fuzzy_metadata_rank_with_scores(&candidate.search_index, tokens, &mut scores);
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
            let mut rank = literal_metadata_rank(&candidate.search_index, tokens)
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
    for (candidate, priority) in fuzzy_candidates {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let mut fuzzy_missing = candidate.missing_tokens.clone();
        let mut rank =
            fuzzy_metadata_rank_with_scores(&candidate.search_index, tokens, &mut scores)
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
            tokens,
            &candidate.prior,
            cancel,
            false,
            &mut scores,
        ) {
            Some(Some(evidence)) => {
                if sender
                    .send((
                        candidate.id.clone(),
                        SearchRank {
                            literal: true,
                            source_weight: source::weighted_source_weight(&evidence.sources, idf),
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
        let prior = fuzzy_prior(tokens, &candidate.prior, &missing);
        match cached_paths_scan_with_scores(
            &candidate.paths,
            tokens,
            &prior,
            cancel,
            true,
            &mut scores,
        ) {
            Some(Some(evidence)) => {
                if sender
                    .send((
                        candidate.id.clone(),
                        SearchRank {
                            score: rank.score + evidence.score,
                            source_weight: source::weighted_source_weight(&evidence.sources, idf),
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
            tokens,
            &candidate.prior,
            cancel,
            ScanMode::Literal,
            &mut scores,
        ) {
            if sender
                .send((
                    candidate.id.clone(),
                    SearchRank {
                        literal: true,
                        source_weight: source::weighted_source_weight(&evidence.sources, idf),
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
            tokens,
            &candidate.prior,
            cancel,
            ScanMode::Classify,
            &mut scores,
        ) {
            let weight = source::weighted_source_weight(&evidence.sources, idf);
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
        let prior = fuzzy_prior(tokens, &candidate.prior, &missing);
        if let Some(evidence) = scan_paths_with_scores(
            &candidate.paths,
            tokens,
            &prior,
            cancel,
            ScanMode::Fuzzy,
            &mut scores,
        ) {
            let rank = SearchRank {
                score: rank.score + evidence.score,
                source_weight: source::weighted_source_weight(&evidence.sources, idf),
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
            let matched = user_prompt_phrase_rank(&paths, phrase, cancel);
            if matched > priority.opening_prompt
                && sender
                    .send((
                        id,
                        SearchRank {
                            opening_prompt: matched,
                            source_weight: priority.source_weight.max(
                                source::weighted_source_weight(
                                    &vec![Some(Source::User); tokens.len()],
                                    idf,
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

impl TranscriptSearch {
    /// Start pending queries immediately, including cold scans.
    /// IO and fuzzy matching stay on the cancellable background worker.
    pub(in super::super) fn poll(&mut self) -> bool {
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
                    if shared_identities.set(identities).is_err() {
                        crate::logging::debug("Transcript search identities already initialized");
                    }
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
                    if shared_idf.set(idf.clone()).is_err() {
                        crate::logging::debug("Transcript search weights already initialized");
                    }
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
