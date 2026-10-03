//! Deterministic transcript rule proposals (Phase 1, no model calls).
//!
//! Converged-design distillation runner, proposal stage only:
//!
//! 1. **Episodic buffer.** [`RuleBuffer`] accumulates [`Episode`] observations
//!    cheaply across sessions. Recording is unconditional; promotion is not.
//! 2. **Promotion trigger.** [`propose_candidates`] groups episodes by exact
//!    normalized trigger key and emits a [`RuleCandidate`] per group whose
//!    *distinct-session* count reaches the recurrence threshold (default 3,
//!    per the field consensus for count-based promotion proposals).
//! 3. **Approval store.** [`ApprovalStore`] is file-backed
//!    (`~/.jcode/rules/rule-approvals.json`). A human or agent approves or
//!    rejects; rejections are retained as negative signal.
//! 4. **Rejected-edit buffer.** [`pending_candidates`] filters proposals
//!    against every already-decided candidate id, so a rejected rule is never
//!    proposed twice.
//!
//! Grounding: research `MEMORY-SYSTEMS.md` section 3 (d1 Phase 1 scope) —
//! RecMem (buffer, never promote every turn), SkillOpt (rejected edits kept
//! as negatives), Recuris (proposals cite the evidence session IDs),
//! CODESKILL (coding-agent distillation domain match).
//!
//! Deliberately out of scope: semantic clustering (exact normalized-key
//! matching only — anything fancier needs a model and belongs in Phase 2),
//! wordsmithing the rule text, and the validation gate. A candidate carries
//! the raw first-seen trigger text plus cited evidence; wording and proving
//! happen later. This module never calls a model and never touches memory
//! scoring or injection.
//!
//! Fail-open on a missing store: first run has no decisions yet, so a
//! missing approvals file loads as empty and proposals still flow. A
//! *corrupt* file is an error, never silent loss — silently dropping past
//! rejections would re-propose refused rules, defeating the buffer.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Default recurrence threshold: groups seen in this many distinct sessions
/// propose a candidate. Matches the N>=3 cluster rule from the distillation
/// survey (MEMORY-SYSTEMS.md section 3.3).
pub const DEFAULT_RECURRENCE_THRESHOLD: usize = 3;

/// Cap on cited session IDs stored per candidate. Counts stay exact; only the
/// cited list is capped. Bounded so one viral trigger cannot bloat the file.
pub const MAX_EVIDENCE_SESSIONS: usize = 64;

/// Triggers longer than this are clipped before storage. Triggers are meant
/// to be short patterns, not pasted transcripts.
pub const MAX_TRIGGER_LEN: usize = 500;

/// On-disk file name for the approval store under `~/.jcode/rules/`.
pub const APPROVALS_FILE_NAME: &str = "rule-approvals.json";

/// Taxonomy of what a rule candidate is about. Kept coarse on purpose: the
/// proposal stage detects recurrence, it does not ontologize.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// Something the agent got wrong and was corrected on.
    Correction,
    /// A stated user preference worth remembering.
    Preference,
    /// A repeated multi-step way of doing something.
    Procedure,
    /// A stable fact about the project or environment.
    Fact,
}

impl RuleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RuleKind::Correction => "correction",
            RuleKind::Preference => "preference",
            RuleKind::Procedure => "procedure",
            RuleKind::Fact => "fact",
        }
    }
}

/// Scope a rule candidate applies to. Intentionally mirrors the memory
/// project/global split without depending on the memory modules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleScope {
    Project,
    Global,
}

impl RuleScope {
    pub fn as_str(self) -> &'static str {
        match self {
            RuleScope::Project => "project",
            RuleScope::Global => "global",
        }
    }
}

/// One recorded observation from a session: something happened that might,
/// if it recurs, deserve a standing rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Episode {
    pub episode_id: String,
    pub session_id: String,
    pub kind: RuleKind,
    pub scope: RuleScope,
    /// Raw trigger text as observed. Normalization happens at proposal time
    /// so the buffer stays a faithful log.
    pub trigger: String,
    /// RFC 3339 wall-clock time of recording.
    pub recorded_at: String,
}

impl Episode {
    /// Build an episode stamped with the current time and a fresh id.
    pub fn now(kind: RuleKind, scope: RuleScope, session_id: &str, trigger: &str) -> Self {
        Self {
            episode_id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            kind,
            scope,
            trigger: clip_trigger(trigger),
            recorded_at: now_rfc3339(),
        }
    }

    /// Normalized key this episode groups under. Empty when the trigger
    /// carries no signal (proposals skip such episodes).
    pub fn trigger_key(&self) -> String {
        normalize_trigger(&self.trigger)
    }
}

/// Canonicalize a trigger for exact-match recurrence counting: lowercase,
/// collapse all whitespace runs to one space, trim. Case and spacing
/// variants of the same correction are the same trigger; anything subtler
/// is Phase-2 semantic work, not this pass.
pub fn normalize_trigger(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn clip_trigger(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() <= MAX_TRIGGER_LEN {
        return trimmed.to_string();
    }
    let mut end = MAX_TRIGGER_LEN;
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Deterministic candidate id from the group key. Inline FNV-1a (not
/// `DefaultHasher`, whose SipHash keys are random per process) so ids are
/// stable across runs and across machines — the rejected-edit buffer keys
/// on them, so stability is load-bearing.
pub fn candidate_id_for(kind: RuleKind, scope: RuleScope, trigger_key: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in format!("{}|{}|{}", kind.as_str(), scope.as_str(), trigger_key).as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("rc-{hash:016x}")
}

/// Cross-session accumulation buffer. Episodes are appended, never mutated;
/// grouping happens at proposal time so threshold changes do not need a
/// rebuild.
#[derive(Clone, Debug, Default)]
pub struct RuleBuffer {
    episodes: Vec<Episode>,
}

impl RuleBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one episode. Episodes with empty session ids are still stored
    /// (the buffer is a faithful log); they simply never reach threshold
    /// because proposal counting requires a non-empty session id.
    pub fn record(&mut self, episode: Episode) {
        self.episodes.push(episode);
    }

    pub fn len(&self) -> usize {
        self.episodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.episodes.is_empty()
    }

    pub fn episodes(&self) -> &[Episode] {
        &self.episodes
    }

    pub fn episodes_for_session(&self, session_id: &str) -> Vec<&Episode> {
        self.episodes
            .iter()
            .filter(|episode| episode.session_id == session_id)
            .collect()
    }
}

/// A scoped rule candidate: a recurrent trigger pattern plus the evidence
/// justifying it. Emitted deterministically; wording comes later.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleCandidate {
    /// Stable id from (kind, scope, normalized trigger). Re-proposing the
    /// same group yields the same id, which is what makes the rejected-edit
    /// buffer work.
    pub candidate_id: String,
    pub kind: RuleKind,
    pub scope: RuleScope,
    /// First-seen raw trigger text, human-readable. Wordsmithing is out of
    /// scope; this is the evidence verbatim.
    pub trigger_pattern: String,
    /// Normalized key used for grouping.
    pub trigger_key: String,
    /// Total episodes in the group (may exceed session count when one
    /// session repeats the trigger).
    pub occurrence_count: usize,
    /// Distinct non-empty sessions in the group. The promotion signal.
    pub session_count: usize,
    /// Cited evidence, sorted and deduplicated, capped at
    /// [`MAX_EVIDENCE_SESSIONS`].
    pub evidence_sessions: Vec<String>,
    pub first_seen: String,
    pub last_seen: String,
}

/// Deterministic pass over the buffer: group by (kind, scope, normalized
/// trigger key), emit one candidate per group with `session_count >=
/// threshold`. Counting is over distinct sessions, not episodes — one
/// session hammering a trigger five times is one data point, not five.
/// Episodes with empty normalized triggers or empty session ids never
/// promote. Output order is deterministic: session count desc, then
/// occurrence count desc, then candidate id.
pub fn propose_candidates(buffer: &RuleBuffer, threshold: usize) -> Vec<RuleCandidate> {
    propose_candidates_filtered(buffer, threshold, &HashSet::new())
}

/// [`propose_candidates`] minus any group whose candidate id is in
/// `suppressed`. Feed [`ApprovalStore::decided_ids`] (or
/// [`ApprovalStore::suppressed_ids`] for rejections only) here so decided
/// rules are never re-proposed.
pub fn propose_candidates_filtered(
    buffer: &RuleBuffer,
    threshold: usize,
    suppressed: &HashSet<String>,
) -> Vec<RuleCandidate> {
    let threshold = threshold.max(1);
    let mut groups: BTreeMap<(RuleKind, RuleScope, String), GroupAccum> = BTreeMap::new();
    for episode in &buffer.episodes {
        if episode.session_id.is_empty() {
            continue;
        }
        let key = episode.trigger_key();
        if key.is_empty() {
            continue;
        }
        groups
            .entry((episode.kind, episode.scope, key))
            .or_default()
            .add(episode);
    }
    let mut candidates: Vec<RuleCandidate> = groups
        .into_iter()
        .filter(|(_, accum)| accum.sessions.len() >= threshold)
        .map(|((kind, scope, key), accum)| accum.candidate(kind, scope, key))
        .filter(|candidate| !suppressed.contains(&candidate.candidate_id))
        .collect();
    candidates.sort_by(|a, b| {
        b.session_count
            .cmp(&a.session_count)
            .then(b.occurrence_count.cmp(&a.occurrence_count))
            .then(a.candidate_id.cmp(&b.candidate_id))
    });
    candidates
}

#[derive(Default)]
struct GroupAccum {
    sessions: BTreeSet<String>,
    occurrences: usize,
    first_pattern: String,
    first_seen: String,
    last_seen: String,
}

impl GroupAccum {
    fn add(&mut self, episode: &Episode) {
        self.sessions.insert(episode.session_id.clone());
        self.occurrences += 1;
        if self.first_pattern.is_empty() {
            self.first_pattern = episode.trigger.clone();
            self.first_seen = episode.recorded_at.clone();
        }
        self.last_seen = episode.recorded_at.clone();
    }

    fn candidate(self, kind: RuleKind, scope: RuleScope, key: String) -> RuleCandidate {
        let mut evidence: Vec<String> = self.sessions.into_iter().collect();
        evidence.truncate(MAX_EVIDENCE_SESSIONS);
        RuleCandidate {
            candidate_id: candidate_id_for(kind, scope, &key),
            kind,
            scope,
            trigger_pattern: self.first_pattern,
            trigger_key: key,
            occurrence_count: self.occurrences,
            session_count: evidence.len().min(self.occurrences),
            evidence_sessions: evidence,
            first_seen: self.first_seen,
            last_seen: self.last_seen,
        }
    }
}

/// The human/agent verdict on a candidate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Decision {
    Approved {
        by: String,
        at: String,
    },
    Rejected {
        by: String,
        at: String,
        reason: String,
    },
}

/// A candidate plus its verdict. The candidate snapshot is stored inline so
/// the log stays interpretable even if proposal formatting later changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecidedRule {
    pub candidate: RuleCandidate,
    pub decision: Decision,
}

/// File-backed approval log: approved rules plus the rejected-edit buffer.
/// Serialized as one JSON object `{approved: [...], rejected: [...]}`.
/// Rejections are retained (SkillOpt pattern): they are the negative signal
/// that stops the proposer from surfacing the same bad rule twice.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalStore {
    #[serde(default)]
    pub approved: Vec<DecidedRule>,
    #[serde(default)]
    pub rejected: Vec<DecidedRule>,
}

impl ApprovalStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Approve a candidate. Upserts by candidate id: re-approving moves the
    /// record to the back with the fresh verdict instead of duplicating.
    /// Approving removes any prior rejection of the same id (a human
    /// overrode the earlier verdict).
    pub fn approve(&mut self, candidate: RuleCandidate, by: &str) {
        let id = candidate.candidate_id.clone();
        self.rejected
            .retain(|rule| rule.candidate.candidate_id != id);
        self.approved
            .retain(|rule| rule.candidate.candidate_id != id);
        self.approved.push(DecidedRule {
            candidate,
            decision: Decision::Approved {
                by: by.to_string(),
                at: now_rfc3339(),
            },
        });
    }

    /// Reject a candidate with a reason. Upserts by candidate id like
    /// [`ApprovalStore::approve`]. The reason is the negative signal: future
    /// readers (human or Phase-2 distiller) can see *why* it was refused.
    pub fn reject(&mut self, candidate: RuleCandidate, by: &str, reason: &str) {
        let id = candidate.candidate_id.clone();
        self.approved
            .retain(|rule| rule.candidate.candidate_id != id);
        self.rejected
            .retain(|rule| rule.candidate.candidate_id != id);
        self.rejected.push(DecidedRule {
            candidate,
            decision: Decision::Rejected {
                by: by.to_string(),
                at: now_rfc3339(),
                reason: reason.to_string(),
            },
        });
    }

    pub fn is_decided(&self, candidate_id: &str) -> bool {
        self.approved
            .iter()
            .chain(self.rejected.iter())
            .any(|rule| rule.candidate.candidate_id == candidate_id)
    }

    /// Ids that must never be re-proposed: the rejected-edit buffer.
    pub fn suppressed_ids(&self) -> HashSet<String> {
        self.rejected
            .iter()
            .map(|rule| rule.candidate.candidate_id.clone())
            .collect()
    }

    /// Every decided id, approved or rejected. The usual filter for
    /// [`propose_candidates_filtered`]: approved rules are already captured
    /// elsewhere, rejected ones are refused — neither should resurface.
    pub fn decided_ids(&self) -> HashSet<String> {
        self.approved
            .iter()
            .chain(self.rejected.iter())
            .map(|rule| rule.candidate.candidate_id.clone())
            .collect()
    }

    /// Load from `path`. A missing file is an empty store, not an error:
    /// first run has no decisions yet. A corrupt file is an error (fail
    /// loud — silently dropping past rejections would re-propose refused
    /// rules, defeating the buffer).
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read approvals {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parse approvals {}", path.display()))
    }

    /// Save atomically: write temp file in the same directory, then rename.
    /// Creates parent directories as needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create approvals dir {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self).context("serialize approvals")?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)
            .with_context(|| format!("write approvals tmp {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("publish approvals {}", path.display()))?;
        Ok(())
    }

    /// Default on-disk location: `~/.jcode/rules/rule-approvals.json`.
    /// `None` when the home directory cannot be resolved.
    pub fn default_path() -> Option<PathBuf> {
        crate::storage::user_home_path("")
            .ok()
            .map(|home| home.join(".jcode/rules").join(APPROVALS_FILE_NAME))
    }

    /// Load from [`ApprovalStore::default_path`]. Missing home dir or
    /// unreadable file yields an empty store (fail-open: proposals must keep
    /// flowing even when the decision log is unavailable).
    pub fn load_default() -> Self {
        Self::default_path()
            .and_then(|path| Self::load(&path).ok())
            .unwrap_or_default()
    }
}

/// The closed loop in one call: propose from the buffer, minus everything
/// already decided in the store. What comes back needs a verdict.
pub fn pending_candidates(
    buffer: &RuleBuffer,
    threshold: usize,
    store: &ApprovalStore,
) -> Vec<RuleCandidate> {
    propose_candidates_filtered(buffer, threshold, &store.decided_ids())
}

/// Recurrence histogram for introspection: (kind, scope, trigger key) to
/// distinct-session count, over the same eligible episodes proposals use.
/// Lets callers report "near misses" (groups at threshold - 1) without
/// reimplementing grouping.
pub fn recurrence_counts(buffer: &RuleBuffer) -> HashMap<(RuleKind, RuleScope, String), usize> {
    let mut counts: HashMap<(RuleKind, RuleScope, String), BTreeSet<String>> = HashMap::new();
    for episode in &buffer.episodes {
        if episode.session_id.is_empty() {
            continue;
        }
        let key = episode.trigger_key();
        if key.is_empty() {
            continue;
        }
        counts
            .entry((episode.kind, episode.scope, key))
            .or_default()
            .insert(episode.session_id.clone());
    }
    counts
        .into_iter()
        .map(|(key, sessions)| (key, sessions.len()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(session: &str, trigger: &str) -> Episode {
        Episode {
            episode_id: format!("ep-{session}-{trigger}"),
            session_id: session.to_string(),
            kind: RuleKind::Correction,
            scope: RuleScope::Project,
            trigger: trigger.to_string(),
            recorded_at: "2026-10-03T00:00:00.000Z".to_string(),
        }
    }

    fn buffer_with(trigger: &str, sessions: &[&str]) -> RuleBuffer {
        let mut buffer = RuleBuffer::new();
        for session in sessions {
            buffer.record(episode(session, trigger));
        }
        buffer
    }

    #[test]
    fn recurrence_counts_distinct_sessions_not_episodes() {
        let buffer = buffer_with("run cargo fmt before committing", &["a", "b", "c"]);
        let counts = recurrence_counts(&buffer);
        assert_eq!(counts.len(), 1);
        assert_eq!(
            counts[&(
                RuleKind::Correction,
                RuleScope::Project,
                normalize_trigger("run cargo fmt before committing")
            )],
            3
        );
        // Same session repeating five times is still one data point.
        let buffer = buffer_with(
            "run cargo fmt before committing",
            &["a", "a", "a", "a", "a"],
        );
        let counts = recurrence_counts(&buffer);
        assert_eq!(
            counts[&(
                RuleKind::Correction,
                RuleScope::Project,
                normalize_trigger("run cargo fmt before committing")
            )],
            1
        );
    }

    #[test]
    fn threshold_gates_candidate_emission() {
        let buffer = buffer_with("always pin dependency versions", &["s1", "s2"]);
        assert!(propose_candidates(&buffer, 3).is_empty());
        let buffer = buffer_with("always pin dependency versions", &["s1", "s2", "s3"]);
        let candidates = propose_candidates(&buffer, 3);
        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(candidate.session_count, 3);
        assert_eq!(candidate.occurrence_count, 3);
        assert_eq!(
            candidate.evidence_sessions,
            vec!["s1".to_string(), "s2".to_string(), "s3".to_string()]
        );
    }

    #[test]
    fn buffer_accumulates_across_sessions_until_threshold() {
        let mut buffer = RuleBuffer::new();
        buffer.record(episode("s1", "check the incident runbook first"));
        assert!(propose_candidates(&buffer, 3).is_empty());
        buffer.record(episode("s2", "check the incident runbook first"));
        assert!(propose_candidates(&buffer, 3).is_empty());
        buffer.record(episode("s3", "check the incident runbook first"));
        let candidates = propose_candidates(&buffer, 3);
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].trigger_pattern,
            "check the incident runbook first"
        );
    }

    #[test]
    fn normalization_groups_case_and_whitespace_variants() {
        let mut buffer = RuleBuffer::new();
        buffer.record(episode("s1", "Run  cargo fmt   before committing"));
        buffer.record(episode("s2", "run cargo fmt before committing"));
        buffer.record(episode("s3", "RUN CARGO FMT BEFORE COMMITTING\n"));
        let candidates = propose_candidates(&buffer, 3);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].session_count, 3);
    }

    #[test]
    fn candidate_emission_shape_and_determinism() {
        let mut buffer = RuleBuffer::new();
        for session in ["s2", "s1", "s3", "s1"] {
            buffer.record(episode(session, "update SYSTEM_TWEAKS on system changes"));
        }
        let first = propose_candidates(&buffer, 3);
        let second = propose_candidates(&buffer, 3);
        assert_eq!(first, second);
        let candidate = &first[0];
        // Evidence cited, sorted, deduplicated.
        assert_eq!(
            candidate.evidence_sessions,
            vec!["s1".to_string(), "s2".to_string(), "s3".to_string()]
        );
        assert_eq!(candidate.session_count, 3);
        assert_eq!(candidate.occurrence_count, 4);
        assert!(!candidate.candidate_id.is_empty());
        // Stable id across independently built buffers with the same group.
        let other = buffer_with(
            "update SYSTEM_TWEAKS on system changes",
            &["s1", "s2", "s3"],
        );
        assert_eq!(
            propose_candidates(&other, 3)[0].candidate_id,
            candidate.candidate_id
        );
    }

    #[test]
    fn groups_split_by_kind_scope_and_trigger() {
        let mut buffer = RuleBuffer::new();
        for session in ["s1", "s2", "s3"] {
            buffer.record(episode(session, "same words"));
            let mut global = episode(session, "same words");
            global.scope = RuleScope::Global;
            buffer.record(global);
            let mut pref = episode(session, "same words");
            pref.kind = RuleKind::Preference;
            buffer.record(pref);
            buffer.record(episode(session, "different words entirely"));
        }
        let candidates = propose_candidates(&buffer, 3);
        // (correction,project), (correction,global), (preference,project) for
        // "same words", plus (correction,project) for the other trigger.
        assert_eq!(candidates.len(), 4);
    }

    #[test]
    fn empty_triggers_and_empty_sessions_never_promote() {
        let mut buffer = RuleBuffer::new();
        for n in 0..5 {
            buffer.record(episode(&format!("s{n}"), "   \n\t  "));
            let mut no_session = episode("", "real trigger words here");
            no_session.episode_id = format!("nosess-{n}");
            buffer.record(no_session);
        }
        assert!(propose_candidates(&buffer, 1).is_empty());
        assert!(recurrence_counts(&buffer).is_empty());
    }

    #[test]
    fn approval_round_trip_through_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("approvals.json");
        // Missing file loads empty, not an error.
        assert_eq!(ApprovalStore::load(&path).unwrap(), ApprovalStore::new());

        let buffer = buffer_with(
            "quote paths with spaces in shell commands",
            &["s1", "s2", "s3"],
        );
        let candidate = propose_candidates(&buffer, 3).pop().unwrap();
        let mut store = ApprovalStore::new();
        store.approve(candidate.clone(), "agent");
        store.save(&path).unwrap();

        let reloaded = ApprovalStore::load(&path).unwrap();
        assert_eq!(reloaded.approved.len(), 1);
        assert_eq!(reloaded.approved[0].candidate, candidate);
        assert!(matches!(
            reloaded.approved[0].decision,
            Decision::Approved { .. }
        ));
        assert!(reloaded.is_decided(&candidate.candidate_id));
    }

    #[test]
    fn rejection_suppresses_reproposal_but_approval_does_not_resurface() {
        let buffer = buffer_with("never force-push to main", &["s1", "s2", "s3", "s4"]);
        let candidate = propose_candidates(&buffer, 3).pop().unwrap();

        let mut store = ApprovalStore::new();
        store.reject(candidate.clone(), "human", "too blunt, needs scope");
        // Rejected id suppresses the proposal (rejected-edit buffer).
        assert!(pending_candidates(&buffer, 3, &store).is_empty());
        assert_eq!(
            store.suppressed_ids(),
            HashSet::from([candidate.candidate_id.clone()])
        );

        // Rejection round-trips through the file with its reason intact.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("approvals.json");
        store.save(&path).unwrap();
        let reloaded = ApprovalStore::load(&path).unwrap();
        assert_eq!(reloaded.rejected.len(), 1);
        let Decision::Rejected { reason, .. } = &reloaded.rejected[0].decision else {
            panic!("expected rejection");
        };
        assert_eq!(reason, "too blunt, needs scope");
        assert!(pending_candidates(&buffer, 3, &reloaded).is_empty());

        // A later approval overrides the rejection (human changed mind), and
        // approved rules do not resurface as pending either.
        let mut store = reloaded;
        store.approve(candidate.clone(), "human");
        assert!(store.rejected.is_empty());
        assert!(pending_candidates(&buffer, 3, &store).is_empty());
        // Re-deciding the same id upserts instead of duplicating.
        store.approve(candidate.clone(), "human");
        assert_eq!(store.approved.len(), 1);
    }

    #[test]
    fn undecided_candidates_stay_pending() {
        let mut buffer = RuleBuffer::new();
        for session in ["s1", "s2", "s3"] {
            buffer.record(episode(session, "first recurring pattern"));
            buffer.record(episode(session, "second recurring pattern"));
        }
        let store = ApprovalStore::new();
        let pending = pending_candidates(&buffer, 3, &store);
        assert_eq!(pending.len(), 2);
    }

    #[test]
    fn corrupt_approvals_file_is_an_error_not_silent_loss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("approvals.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(ApprovalStore::load(&path).is_err());
    }
}
