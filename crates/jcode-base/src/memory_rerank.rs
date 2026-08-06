//! Listwise LLM reranking of memory retrieval candidates (recall-5, Mode-2).
//!
//! Hybrid retrieval (`MemoryManager::find_similar_hybrid`) reliably pulls the
//! relevant memories into a top-N candidate pool, but ranks them poorly: on the
//! recall benchmark the pool holds ~99% of relevant memories yet only ~53% reach
//! the top 5. A reranker that reorders the existing pool closes most of that gap.
//!
//! A local cross-encoder (MS-MARCO) was tried and *hurt* recall (out-of-domain
//! for memory statements, and it chokes on noisy multi-message context). A
//! listwise LLM reranker, fed the *focused* query (latest user intent, with
//! system-reminder/tool noise stripped) and all candidates in one call, lifts
//! benchmark recall@5 0.53 -> 0.75 and precision@5 0.23 -> 0.35.
//!
//! This module is the single source of truth for that reranking, shared by the
//! offline benchmark (`memory_recall_bench`) and the live memory agent so the
//! shipped behavior matches what was measured. It is pure with respect to the
//! memory agent (depends only on `Sidecar` + `MemoryEntry`).

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::memory_types::MemoryEntry;
use crate::sidecar::{Sidecar, SidecarErrorKind};

/// System prompt instructing the model to rank candidates by usefulness.
pub const LLM_RERANK_SYSTEM: &str = "You re-rank stored MEMORIES by how useful each would be to surface to an AI coding agent for the CURRENT request. \
Order them best-first: a memory ranks high if a competent engineer would say knowing it specifically helps respond here (a relevant fact, preference, correction, or procedure). \
Off-topic, generic, or keyword-only matches rank low. \
Reply with ONLY a JSON array of candidate numbers, best first, e.g. [3,1,7]. Include only clearly useful candidates; omit ones that are not relevant. No prose.";

const TRANSIENT_FAILURE_BACKOFF: Duration = Duration::from_secs(30);
static FAILURE_BACKOFF_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);
static REPORTED_PERMANENT_FAILURES: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn failure_backoff_active() -> bool {
    let Ok(mut guard) = FAILURE_BACKOFF_UNTIL.lock() else {
        return false;
    };
    match *guard {
        Some(until) if until > Instant::now() => true,
        Some(_) => {
            *guard = None;
            false
        }
        None => false,
    }
}

fn record_failure_backoff() {
    if let Ok(mut guard) = FAILURE_BACKOFF_UNTIL.lock() {
        let candidate = Instant::now() + TRANSIENT_FAILURE_BACKOFF;
        if match *guard {
            Some(existing) => existing < candidate,
            None => true,
        } {
            *guard = Some(candidate);
        }
    }
}

fn report_permanent_failure_once(error: &anyhow::Error) {
    let message = error.to_string();
    let should_report = REPORTED_PERMANENT_FAILURES
        .lock()
        .map(|mut reported| reported.insert(message.clone()))
        .unwrap_or(true);
    if should_report {
        crate::logging::event_error(
            "Memory consensus judge permanently misconfigured; fix credentials or the configured memory model",
            [("error", message)],
        );
    }
}

/// Clear the process-wide judge circuit breaker after real auth state changes.
pub(crate) fn clear_failure_backoff() {
    if let Ok(mut guard) = FAILURE_BACKOFF_UNTIL.lock() {
        *guard = None;
    }
}

/// Cap the query length fed to the reranker. The query should already be the
/// focused (noise-stripped) view; this is a defensive bound. We keep the TAIL,
/// which carries the most recent intent.
const MAX_QUERY_CHARS: usize = 4000;

/// Per-candidate content cap so a single huge memory cannot dominate the prompt.
const MAX_CANDIDATE_CHARS: usize = 600;

fn truncate_tail(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    s.chars().skip(count - max).collect()
}

fn truncate_head(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

/// Build the listwise rerank prompt from a focused query and `(id, content)`
/// candidate pairs. Candidates are presented as a 1-based numbered list.
pub fn build_rerank_prompt(focused_query: &str, candidates: &[(String, String)]) -> String {
    let q = truncate_tail(focused_query, MAX_QUERY_CHARS);
    let mut p = String::with_capacity(256 + candidates.len() * 64);
    p.push_str("CURRENT REQUEST:\n");
    p.push_str(&q);
    p.push_str("\n\nCANDIDATE MEMORIES:\n");
    for (i, (_id, content)) in candidates.iter().enumerate() {
        let one_line = truncate_head(content, MAX_CANDIDATE_CHARS).replace('\n', " ");
        p.push_str(&format!("{}. {}\n", i + 1, one_line));
    }
    p.push_str("\nReturn candidate numbers ranked best-first as a JSON array.");
    p
}

/// Parse a ranked JSON array of 1-based candidate numbers into 0-based indices,
/// preserving order and dropping out-of-range / duplicate entries. Tolerates
/// surrounding prose by extracting the first `[`..`]` span.
///
/// Returns `None` when NO JSON array is found (unparseable / garbage response),
/// which the caller must treat as a *failure* (fall back to hybrid order), vs
/// `Some(vec![])` for a genuine empty array `[]` (model judged nothing relevant,
/// which the caller honors). These two cases have opposite correct behavior.
pub fn extract_ranking(resp: &str, n: usize) -> Option<Vec<usize>> {
    let (s, e) = (resp.find('[')?, resp.rfind(']')?);
    if e < s {
        return None;
    }
    let nums: Vec<i64> = serde_json::from_str(&resp[s..=e]).ok()?;
    let mut seen = HashSet::new();
    Some(
        nums.into_iter()
            .filter_map(|x| {
                let idx = usize::try_from(x).ok()?;
                if idx >= 1 && idx <= n && seen.insert(idx) {
                    Some(idx - 1)
                } else {
                    None
                }
            })
            .collect(),
    )
}

/// Backwards-compatible wrapper: parse a ranking, treating "no array found" the
/// same as "empty array" (both yield an empty Vec). Used by the offline
/// benchmark where the failure/empty distinction is not needed. Production uses
/// [`extract_ranking`] to distinguish the two.
pub fn parse_rerank_response(resp: &str, n: usize) -> Vec<usize> {
    extract_ranking(resp, n).unwrap_or_default()
}

/// Rerank `candidates` with a listwise LLM call.
///
/// Returns ALL candidates reordered best-first (callers truncate to their own
/// top-k). Candidates the model ranks are placed first in model order; any
/// candidate the model omits is appended afterwards in the original hybrid order
/// (so omitted-but-retrieved memories are never lost, just deprioritized).
///
/// How aggressively the reranker filters candidates.
///
/// The listwise LLM both *ranks* candidates and *omits* the ones it judges
/// irrelevant. These modes decide what to do with the omitted ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RerankMode {
    /// Precision-focused (default): inject ONLY the memories the model judged
    /// relevant, in model order. If the model keeps 2 of 50, return 2; if it
    /// keeps none, return none. Maximizes precision (nothing irrelevant is
    /// surfaced) at the cost of some recall.
    #[default]
    Precision,
    /// Recall-focused: model-ranked memories first, then the omitted candidates
    /// appended in hybrid order. The caller can then take a fixed top-k. Trades
    /// precision for recall (surfaces more, including model-rejected ones).
    Recall,
}

/// Rerank `candidates` with a listwise LLM call (precision-focused default).
///
/// Equivalent to [`rerank_candidates_with_mode`] with [`RerankMode::Precision`]:
/// returns ONLY the memories the model judged relevant, in model order (no
/// irrelevant padding). The caller still applies its own upper-bound cap (e.g.
/// `MAX_MEMORIES_PER_TURN`), so the injected set is `min(relevant_count, cap)`,
/// and empty when the model judges nothing relevant.
pub async fn rerank_candidates(
    sidecar: &Sidecar,
    focused_query: &str,
    candidates: Vec<(MemoryEntry, f32)>,
) -> Vec<MemoryEntry> {
    rerank_candidates_with_mode(sidecar, focused_query, candidates, RerankMode::Precision).await
}

/// Why a consensus rerank produced the result it did. Lets the caller attribute
/// "no-LLM memory mode" conversions exactly (see `memory_judge_metrics`).
///
/// Policy: the judge is the ONLY thing allowed to surface memory. On ANY judge
/// failure the rerank returns an EMPTY set (never hybrid order); the caller then
/// carries the previously judge-verified set so what surfaces is always
/// judge-vetted, never low-precision hybrid bloat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RerankOutcome {
    /// At least one judge produced a usable ballot; result is the judged set.
    Judged,
    /// Every judge failed (transport/timeout): returns EMPTY (caller carries the
    /// last judge-verified set).
    AllJudgesFailed,
    /// Single-judge response was unparseable garbage: returns EMPTY (caller
    /// carries the last judge-verified set).
    Unparseable,
    /// Single-judge transport error: returns EMPTY (caller carries the last
    /// judge-verified set).
    TransportError,
}

/// High-precision CONSENSUS rerank: run `votes` independent precision reranks
/// concurrently and keep ONLY the memories that at least `min_agree` of them
/// select. Two independent judges agreeing is what lifts injection precision to
/// ~1.0 (offline adjudication: single judge ~0.77 precision, 2-of-2 agreement
/// ~1.0, both with ~100% clean-rate on no-memory turns), at the cost of `votes`
/// LLM calls per fired turn. Output is ordered by descending agreement, then by
/// the best (lowest) rank any judge gave, so the most-agreed memories lead.
///
/// Robustness: judges that error/timeout simply contribute no votes (a blip
/// cannot force-inject). If EVERY judge fails to produce a usable response we
/// return EMPTY (the caller carries the last judge-verified set rather than
/// dropping to unvetted hybrid order). `votes <= 1` degenerates to a single
/// precision rerank.
pub async fn rerank_candidates_consensus(
    sidecar: &Sidecar,
    focused_query: &str,
    candidates: Vec<(MemoryEntry, f32)>,
    votes: usize,
    min_agree: usize,
) -> Vec<MemoryEntry> {
    rerank_candidates_consensus_attributed(sidecar, focused_query, candidates, votes, min_agree)
        .await
        .0
}

/// Like [`rerank_candidates_consensus`] but also returns *why* it produced the
/// result, so the live memory agent can attribute no-LLM conversions exactly.
pub async fn rerank_candidates_consensus_attributed(
    sidecar: &Sidecar,
    focused_query: &str,
    candidates: Vec<(MemoryEntry, f32)>,
    votes: usize,
    min_agree: usize,
) -> (Vec<MemoryEntry>, RerankOutcome) {
    if failure_backoff_active() {
        crate::logging::event_rate_limited(
            crate::logging::LogLevel::Info,
            "memory_consensus_judge_backoff",
            Duration::from_secs(60),
            "Memory consensus judge circuit breaker active; skipping LLM calls",
            Vec::<(&str, String)>::new(),
        );
        return (Vec::new(), RerankOutcome::AllJudgesFailed);
    }
    let votes = votes.max(1);
    let min_agree = min_agree.clamp(1, votes);
    if votes == 1 {
        return rerank_candidates_with_mode_attributed(
            sidecar,
            focused_query,
            candidates,
            RerankMode::Precision,
        )
        .await;
    }
    if candidates.is_empty() {
        // No candidates at all is not a conversion (there was nothing to judge).
        return (Vec::new(), RerankOutcome::Judged);
    }
    // NOTE: a single candidate is still JUDGED (no bypass). The judge is the only
    // thing allowed to surface memory, so even one item must clear it.

    let pairs: Vec<(String, String)> = candidates
        .iter()
        .map(|(e, _)| (e.id.clone(), e.content.clone()))
        .collect();
    let prompt = build_rerank_prompt(focused_query, &pairs);
    let n = candidates.len();

    // Fire `votes` independent reranks concurrently.
    let futures = (0..votes).map(|_| {
        let sidecar = sidecar.clone();
        let prompt = prompt.clone();
        async move {
            match sidecar.complete(LLM_RERANK_SYSTEM, &prompt).await {
                Ok(resp) => extract_ranking(&resp, n), // Some([]) = nothing relevant
                Err(e) => {
                    match crate::sidecar::classify_error(&e) {
                        SidecarErrorKind::Transient => {
                            record_failure_backoff();
                            crate::logging::event_rate_limited(
                                crate::logging::LogLevel::Warn,
                                "memory_consensus_judge_failed",
                                Duration::from_secs(60),
                                "Memory consensus judge transiently failed; circuit breaker armed",
                                vec![("error", e.to_string())],
                            );
                        }
                        SidecarErrorKind::Permanent => report_permanent_failure_once(&e),
                    }
                    None // transport error = no vote
                }
            }
        }
    });
    let ballots: Vec<Option<Vec<usize>>> = futures::future::join_all(futures).await;

    let usable = ballots.iter().filter(|b| b.is_some()).count();
    if usable == 0 {
        // Every judge failed (transport). Surface NOTHING from this rerank; the
        // caller carries the last judge-verified set rather than injecting
        // unvetted hybrid order.
        crate::logging::info(
            "Memory consensus rerank: all judges failed; surfacing nothing (caller carries verified set)",
        );
        return (Vec::new(), RerankOutcome::AllJudgesFailed);
    }

    let kept = tally_consensus(&ballots, n, min_agree);

    crate::logging::info(&format!(
        "Memory consensus rerank: {usable}/{votes} judges, {} of {n} candidates met >={min_agree} agreement",
        kept.len()
    ));

    (
        compose_reranked(candidates, &kept, RerankMode::Precision),
        RerankOutcome::Judged,
    )
}

/// Pure consensus tally: given per-judge ballots (each an optional best-first
/// list of candidate indices), the candidate count `n`, and the agreement bar
/// `min_agree`, return the kept candidate indices ordered by votes descending
/// then by best (lowest) rank any judge assigned. `None` ballots (failed judges)
/// contribute no votes. Factored out so it is unit-testable without a `Sidecar`.
fn tally_consensus(ballots: &[Option<Vec<usize>>], n: usize, min_agree: usize) -> Vec<usize> {
    let mut vote_count = vec![0usize; n];
    let mut best_rank = vec![usize::MAX; n];
    for ballot in ballots.iter().flatten() {
        for (rank, &idx) in ballot.iter().enumerate() {
            if idx < n {
                vote_count[idx] += 1;
                best_rank[idx] = best_rank[idx].min(rank);
            }
        }
    }
    let mut kept: Vec<usize> = (0..n).filter(|&i| vote_count[i] >= min_agree).collect();
    kept.sort_by(|&a, &b| {
        vote_count[b]
            .cmp(&vote_count[a])
            .then(best_rank[a].cmp(&best_rank[b]))
    });
    kept
}

/// Rerank `candidates` with a listwise LLM call, choosing precision vs recall.
///
/// In [`RerankMode::Precision`] returns only the model-kept relevant memories in
/// model order. In [`RerankMode::Recall`] returns model-kept first then the
/// omitted candidates in hybrid order (so a fixed top-k still fills).
///
/// Failure handling (never regress below the hybrid baseline):
/// Failure handling (judge-only: never surface unvetted memory):
/// - LLM transport error -> EMPTY (caller carries last judge-verified set).
/// - response with no parseable JSON array (garbage) -> EMPTY (caller carries).
/// - response with a genuine empty array `[]` (model judged nothing relevant)
///   -> Precision: empty; Recall: hybrid order (a real verdict, not a failure).
pub async fn rerank_candidates_with_mode(
    sidecar: &Sidecar,
    focused_query: &str,
    candidates: Vec<(MemoryEntry, f32)>,
    mode: RerankMode,
) -> Vec<MemoryEntry> {
    rerank_candidates_with_mode_attributed(sidecar, focused_query, candidates, mode)
        .await
        .0
}

/// Like [`rerank_candidates_with_mode`] but also reports *why* it produced the
/// result so callers can attribute no-LLM conversions exactly.
pub async fn rerank_candidates_with_mode_attributed(
    sidecar: &Sidecar,
    focused_query: &str,
    candidates: Vec<(MemoryEntry, f32)>,
    mode: RerankMode,
) -> (Vec<MemoryEntry>, RerankOutcome) {
    if candidates.is_empty() {
        // Nothing to judge: not a conversion.
        return (Vec::new(), RerankOutcome::Judged);
    }
    // NOTE: a single candidate is still JUDGED (no bypass) so the judge stays the
    // only thing that can surface memory.

    let pairs: Vec<(String, String)> = candidates
        .iter()
        .map(|(e, _)| (e.id.clone(), e.content.clone()))
        .collect();
    let prompt = build_rerank_prompt(focused_query, &pairs);
    let n = candidates.len();

    let order = match sidecar.complete(LLM_RERANK_SYSTEM, &prompt).await {
        // Case 1 failure: network/transport error. Surface NOTHING; the caller
        // carries the last judge-verified set (never unvetted hybrid order).
        Err(e) => {
            crate::logging::info(&format!(
                "Memory rerank failed ({e}); surfacing nothing (caller carries verified set)"
            ));
            return (Vec::new(), RerankOutcome::TransportError);
        }
        Ok(resp) => match extract_ranking(&resp, n) {
            Some(order) => order,
            None => {
                // Case 3: model replied but with no usable JSON array (garbage).
                // Treat as failure, not as "nothing relevant".
                crate::logging::info(
                    "Memory rerank: unparseable response; surfacing nothing (caller carries verified set)",
                );
                return (Vec::new(), RerankOutcome::Unparseable);
            }
        },
    };

    if order.is_empty() {
        // Case 2: model returned a genuine empty array -> it judged NOTHING
        // relevant. Precision mode honors that (inject nothing); Recall mode
        // still surfaces the hybrid set. This IS a judge verdict (Judged), not a
        // failure conversion.
        return match mode {
            RerankMode::Precision => (Vec::new(), RerankOutcome::Judged),
            RerankMode::Recall => (
                candidates.into_iter().map(|(e, _)| e).collect(),
                RerankOutcome::Judged,
            ),
        };
    }

    (
        compose_reranked(candidates, &order, mode),
        RerankOutcome::Judged,
    )
}

/// Pure composition step: given the candidates and the model's ranking
/// (0-based indices, best-first), produce the final entry list per `mode`.
/// Precision keeps only ranked entries; Recall appends omitted ones in hybrid
/// order. Factored out so it is unit-testable without a `Sidecar`.
fn compose_reranked(
    candidates: Vec<(MemoryEntry, f32)>,
    order: &[usize],
    mode: RerankMode,
) -> Vec<MemoryEntry> {
    let n = candidates.len();
    let ranked_set: HashSet<usize> = order.iter().copied().collect();
    let mut entries: Vec<Option<MemoryEntry>> =
        candidates.into_iter().map(|(e, _)| Some(e)).collect();

    let mut out: Vec<MemoryEntry> = Vec::with_capacity(n);
    // Model-ranked (relevant) candidates first, in model order.
    for &idx in order {
        if let Some(entry) = entries.get_mut(idx).and_then(Option::take) {
            out.push(entry);
        }
    }
    // Recall mode also appends the omitted candidates in original hybrid order.
    if mode == RerankMode::Recall {
        for (idx, slot) in entries.iter_mut().enumerate() {
            if !ranked_set.contains(&idx)
                && let Some(entry) = slot.take()
            {
                out.push(entry);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rerank_response_basic() {
        assert_eq!(parse_rerank_response("[3,1,2]", 3), vec![2, 0, 1]);
    }

    #[test]
    fn parse_rerank_response_dedups_and_bounds() {
        // 9 is out of range (n=3), duplicate 1 dropped, 0 invalid (1-based).
        assert_eq!(parse_rerank_response("[1, 9, 1, 2, 0]", 3), vec![0, 1]);
    }

    #[test]
    fn parse_rerank_response_tolerates_prose() {
        assert_eq!(
            parse_rerank_response("Here is the ranking: [2,1] (best first)", 2),
            vec![1, 0]
        );
    }

    #[test]
    fn parse_rerank_response_empty_on_garbage() {
        assert!(parse_rerank_response("no array here", 5).is_empty());
        assert!(parse_rerank_response("][", 5).is_empty());
    }

    #[test]
    fn extract_ranking_distinguishes_empty_array_from_no_array() {
        // Genuine empty array -> Some(empty): model judged nothing relevant.
        assert_eq!(extract_ranking("[]", 5), Some(vec![]));
        assert_eq!(extract_ranking("nothing relevant: []", 5), Some(vec![]));
        // No array at all -> None: unparseable, caller must treat as failure.
        assert_eq!(extract_ranking("I could not find anything", 5), None);
        assert_eq!(extract_ranking("][", 5), None);
        // Valid ranking -> Some(indices).
        assert_eq!(extract_ranking("[2,1]", 2), Some(vec![1, 0]));
    }

    fn mem(id: &str) -> MemoryEntry {
        let mut e = MemoryEntry::new(crate::memory_types::MemoryCategory::Fact, id);
        e.id = id.to_string();
        e
    }

    fn cands(ids: &[&str]) -> Vec<(MemoryEntry, f32)> {
        ids.iter()
            .rev()
            .enumerate()
            .map(|(i, id)| (mem(id), i as f32))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    #[test]
    fn compose_precision_keeps_only_ranked() {
        // Pool a,b,c,d; model keeps only c then a (order [2,0]).
        let pool = cands(&["a", "b", "c", "d"]);
        let out = compose_reranked(pool, &[2, 0], RerankMode::Precision);
        let ids: Vec<&str> = out.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["c", "a"],
            "precision returns ONLY model-kept, in model order"
        );
    }

    #[test]
    fn compose_recall_appends_omitted_in_hybrid_order() {
        let pool = cands(&["a", "b", "c", "d"]);
        let out = compose_reranked(pool, &[2, 0], RerankMode::Recall);
        let ids: Vec<&str> = out.iter().map(|e| e.id.as_str()).collect();
        // ranked (c,a) first, then omitted (b,d) in original order.
        assert_eq!(ids, vec!["c", "a", "b", "d"]);
    }

    #[test]
    fn build_prompt_numbers_candidates_one_based() {
        let cands = vec![
            ("a".to_string(), "first memory".to_string()),
            ("b".to_string(), "second memory".to_string()),
        ];
        let p = build_rerank_prompt("fix the scroll bug", &cands);
        assert!(p.contains("CURRENT REQUEST:\nfix the scroll bug"));
        assert!(p.contains("1. first memory"));
        assert!(p.contains("2. second memory"));
    }

    #[test]
    fn tally_consensus_requires_agreement() {
        // Two judges. Candidate 0 picked by both, 1 by one, 2 by neither.
        let ballots = vec![Some(vec![0, 1]), Some(vec![0])];
        // min_agree=2: only candidate 0 survives.
        assert_eq!(tally_consensus(&ballots, 3, 2), vec![0]);
        // min_agree=1: 0 and 1 survive (0 first: more votes).
        assert_eq!(tally_consensus(&ballots, 3, 1), vec![0, 1]);
    }

    #[test]
    fn tally_consensus_orders_by_votes_then_rank() {
        // 3 judges. c2 gets 3 votes, c0 gets 2, c1 gets 2 but ranked worse.
        let ballots = vec![Some(vec![2, 0, 1]), Some(vec![2, 0]), Some(vec![2, 1])];
        // votes: c2=3, c0=2, c1=2. c0 ranked above c1 (best_rank 1 vs 1? c0 best
        // rank 1, c1 best rank 1 too) -> stable by index. Top should be c2.
        let kept = tally_consensus(&ballots, 3, 2);
        assert_eq!(kept[0], 2, "unanimous candidate leads");
        assert!(kept.contains(&0) && kept.contains(&1));
    }

    #[test]
    fn tally_consensus_ignores_failed_ballots() {
        // One real judge + one failed (None). min_agree=1 still works off the
        // single usable ballot; a failed judge contributes no votes.
        let ballots = vec![Some(vec![1, 0]), None];
        assert_eq!(tally_consensus(&ballots, 2, 1), vec![1, 0]);
        // min_agree=2 with only one usable judge -> nothing meets the bar.
        assert!(tally_consensus(&ballots, 2, 2).is_empty());
    }

    #[test]
    fn tally_consensus_empty_when_no_votes() {
        let ballots = vec![Some(vec![]), Some(vec![])];
        assert!(tally_consensus(&ballots, 3, 1).is_empty());
    }

    #[test]
    fn transient_judge_failure_backoff_arms_and_auth_invalidation_clears_it() {
        clear_failure_backoff();
        assert!(!failure_backoff_active());
        record_failure_backoff();
        assert!(failure_backoff_active());
        clear_failure_backoff();
        assert!(!failure_backoff_active());
    }

    #[test]
    fn permanent_judge_failure_does_not_arm_backoff() {
        clear_failure_backoff();
        let error = anyhow::anyhow!("Claude API error (404 Not Found): not_found_error");
        assert_eq!(
            crate::sidecar::classify_error(&error),
            SidecarErrorKind::Permanent
        );
        report_permanent_failure_once(&error);
        assert!(!failure_backoff_active());
    }
}
