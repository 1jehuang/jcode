//! Sleep-time consolidation buffer — R9 Phase 1 (deterministic, no LLM).
//!
//! A durable, idempotent, interruptible proposal buffer. Recurring
//! observations (scoped rule candidates, recurring facts) accumulate here
//! across sessions; a later Phase-2 pass (wordsmith + validation gate,
//! NEEDS-BRAIN) promotes the best ones into durable memory or skill files.
//!
//! Design notes (see `exec/r9-sleep/REPORT.md` for the full rationale):
//!
//! - **New crate, zero coupling.** The buffer never reads `MemoryEntry`,
//!   never touches scoring/injection, and never parses transcripts itself.
//!   Producers (session-end hooks, R3-style transcript passes, ambient
//!   cycles) call [`SleepBuffer::propose`] with already-extracted strings.
//! - **Dedup is two-tier.** An exact normalized-text hash (`dedup_key`)
//!   merges byte-identical re-proposals idempotently; a 64-bit SimHash over
//!   token 1+2-grams merges near-duplicates (typo-level edits) WITHOUT an
//!   LLM. Hash math is std-only plus `sha2` (already in the lockfile).
//! - **Priority is a closed-form score.** `occurrences`, distinct-session
//!   count, source-tier weight, and age decay combine deterministically, so
//!   Phase 2 always works the highest-signal proposals first.
//! - **Idempotent + interruptible.** Every mutation is keyed, every
//!   no-op returns the existing record, and state is a single JSON file
//!   written atomically (temp file + rename). A killed cycle loses at most
//!   the in-flight unflushed proposal, and re-running converges.
//! - **Schedule default 12h.** [`DEFAULT_CONSOLIDATION_INTERVAL_MINUTES`]
//!   follows the Reddit dreaming cadence cited in PROGRAM.md R9; the cron
//!   shape reuses the existing ambient queue (`SKILL.md` in
//!   `.jcode/skills/sleep-consolidation/`), no new scheduler plumbing.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Schema version of the persisted buffer file. Bumped only on
/// intentionally breaking record-shape changes (with migration code).
pub const SLEEP_BUFFER_VERSION: u32 = 1;

/// Default cadence between consolidation cycles in minutes (12h).
///
/// Source: PROGRAM.md R9 cites the Reddit dreaming cadence (every ~12h for
/// heavy users) as the concrete default for sleep-time scheduling.
pub const DEFAULT_CONSOLIDATION_INTERVAL_MINUTES: u32 = 12 * 60;

/// Minimum SimHash Hamming similarity (fraction of equal bits) for two
/// proposals to count as near-duplicates. 0.875 = at most 8 of 64 bits
/// differ: typo-level / punctuation-level edits merge, genuinely different
/// sentences do not.
pub const NEAR_DUP_SIMILARITY_THRESHOLD: f64 = 0.875;

/// Minimum occurrences before a proposal is eligible for Phase-2 promotion.
pub const DEFAULT_PROMOTION_THRESHOLD: u32 = 2;

/// Proposal lifecycle state. Only `Pending` proposals are ranked for
/// promotion; the rest are terminal bookkeeping so Phase 2 stays
/// idempotent across restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    /// Awaiting enough signal for Phase-2 promotion.
    #[default]
    Pending,
    /// Promoted to durable memory / skill file by Phase 2.
    Promoted,
    /// Rejected by the Phase-2 validation gate (kept for audit).
    Rejected,
    /// Superseded by a merged near-duplicate (points at `merged_into`).
    Merged,
}

/// Where a proposal came from. Higher tiers weigh more in priority:
/// explicit user corrections outrank agent-distilled candidates, which
/// outrank bulk tool-ingested candidates (mirrors the R10 provenance-tier
/// ordering without depending on it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProposalSource {
    /// Human correction or explicit instruction.
    User,
    /// Deterministic agent-side extraction (transcript pass, hook).
    #[default]
    AgentDistilled,
    /// Bulk tool output ingestion.
    ToolIngested,
}

impl ProposalSource {
    /// Deterministic weight used by the priority score.
    pub fn weight(self) -> f64 {
        match self {
            ProposalSource::User => 3.0,
            ProposalSource::AgentDistilled => 2.0,
            ProposalSource::ToolIngested => 1.0,
        }
    }
}

/// One candidate consolidation: a scoped rule or recurring-fact proposal
/// awaiting Phase-2 wordsmithing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    /// Stable id: `prop_<millis>_<rand>` so concurrent proposers never
    /// collide even before the dedup pass merges them.
    pub id: String,
    /// Schema version at write time.
    pub version: u32,
    /// Exact-match key: sha256 of the normalized candidate text.
    /// Re-proposing identical text is a pure counter bump (idempotent).
    pub dedup_key: String,
    /// 64-bit SimHash over token 1+2-grams: near-duplicates merge
    /// deterministically without an LLM.
    pub simhash: u64,
    /// Candidate text as proposed (verbatim from the producer).
    pub text: String,
    /// Kind hint for Phase 2 (`scoped_rule`, `recurring_fact`, ...).
    /// Free-form: Phase 1 never interprets it.
    pub kind: String,
    /// How many times this candidate (or a duplicate) was proposed.
    pub occurrences: u32,
    /// Distinct sessions that proposed it. Width (not just volume) is the
    /// recurrence signal CodeYam-style approval needs.
    pub session_ids: Vec<String>,
    /// Strongest source tier seen across proposals (monotone: only
    /// upgrades, never downgrades, so priority is restart-stable).
    pub source: ProposalSource,
    /// First-seen wall clock (for age decay in priority).
    pub first_seen_at: DateTime<Utc>,
    /// Last-proposal wall clock (bump only).
    pub last_seen_at: DateTime<Utc>,
    /// Evidence pointers: session ids / transcript offsets backing this
    /// candidate. Capped (see [`MAX_CITATIONS`]) to bound file growth.
    pub citations: Vec<String>,
    pub status: ProposalStatus,
    /// Target proposal id when `status == Merged`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_into: Option<String>,
}

impl Proposal {
    /// Closed-form priority: higher is more promotion-worthy.
    ///
    /// ```text
    /// score = source_weight * (occurrences + distinct_sessions)
    ///         * decay(days_since_first_seen)
    /// decay(d) = 1 / (1 + d / 30)   // halves every ~30 days
    /// ```
    ///
    /// Pure function of the record: no I/O, no clock reads inside the
    /// ranking path except the caller-supplied `now`, so ordering is
    /// reproducible and unit-testable. Terminal (non-pending) proposals
    /// score negative infinity and never surface for promotion.
    pub fn priority_score(&self, now: DateTime<Utc>) -> f64 {
        if self.status != ProposalStatus::Pending {
            return f64::NEG_INFINITY;
        }
        let breadth = (self.occurrences as f64) + (distinct_count(&self.session_ids) as f64);
        let age_days = (now - self.first_seen_at).num_seconds().max(0) as f64 / 86_400.0;
        let decay = 1.0 / (1.0 + age_days / 30.0);
        self.source.weight() * breadth * decay
    }
}

/// Max citations retained per proposal (oldest dropped first on overflow).
pub const MAX_CITATIONS: usize = 16;

/// Outcome of a [`SleepBuffer::propose`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposeOutcome {
    /// Brand-new proposal created.
    Created { id: String },
    /// Exact-text re-proposal: counters bumped on the existing record.
    DedupedExact { id: String },
    /// Near-duplicate: merged into the existing similar proposal.
    MergedNearDup { into_id: String, new_id: String },
}

/// Durable on-disk shape: version tag + proposal map.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BufferFile {
    version: u32,
    proposals: HashMap<String, Proposal>,
}

/// Deterministic sleep-time consolidation buffer.
///
/// Backed by a single JSON file (`sleep_buffer.json` under the configured
/// dir, default `~/.jcode/sleep/`). All mutations go through keyed,
/// idempotent operations; [`SleepBuffer::flush`] persists atomically via
/// temp-file + rename so a crash mid-write never corrupts the file.
pub struct SleepBuffer {
    path: PathBuf,
    proposals: HashMap<String, Proposal>,
    dirty: bool,
}

impl SleepBuffer {
    /// Open (or create) the buffer at an explicit path. Missing or corrupt
    /// files start empty — fail-open, never fail-closed (a corrupt buffer
    /// must not take down the cycle that owns it). Corruption is reported
    /// via the returned `loaded_ok` flag on [`SleepBuffer::open_with_status`].
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        Self::open_with_status(path).map(|(buf, _)| buf)
    }

    /// Like [`SleepBuffer::open`] but also reports whether the persisted
    /// state loaded cleanly (`false` = started empty after missing/corrupt
    /// file or version mismatch).
    pub fn open_with_status(path: impl Into<PathBuf>) -> Result<(Self, bool)> {
        let path = path.into();
        let (proposals, loaded_ok) = match fs::read_to_string(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (HashMap::new(), true),
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("reading sleep buffer '{}'", path.display()));
            }
            Ok(text) => match serde_json::from_str::<BufferFile>(&text) {
                Ok(file) if file.version == SLEEP_BUFFER_VERSION => (file.proposals, true),
                _ => (HashMap::new(), false),
            },
        };
        Ok((
            Self {
                path,
                proposals,
                dirty: false,
            },
            loaded_ok,
        ))
    }

    /// Default on-disk location: `~/.jcode/sleep/sleep_buffer.json`
    /// (`$JCODE_HOME` honored). Directory is created on open.
    pub fn default_path() -> Result<PathBuf> {
        let base = if let Ok(home) = std::env::var("JCODE_HOME") {
            PathBuf::from(home)
        } else {
            dirs_home().ok_or_else(|| anyhow::anyhow!("No home directory"))?
        };
        Ok(base.join(".jcode").join("sleep").join("sleep_buffer.json"))
    }

    /// Open the buffer at the default location, creating parent dirs.
    pub fn open_default() -> Result<Self> {
        let path = Self::default_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating sleep dir '{}'", parent.display()))?;
        }
        Self::open(path)
    }

    /// File backing this buffer.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Number of proposals (all statuses).
    pub fn len(&self) -> usize {
        self.proposals.len()
    }

    /// True when no proposals are stored.
    pub fn is_empty(&self) -> bool {
        self.proposals.is_empty()
    }

    /// Whether there are unflushed mutations.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Fetch a proposal by id.
    pub fn get(&self, id: &str) -> Option<&Proposal> {
        self.proposals.get(id)
    }

    /// Propose a candidate consolidation.
    ///
    /// Idempotent in three tiers:
    /// 1. **Exact**: normalized text hashes to an existing `dedup_key` →
    ///    bump `occurrences`, extend `session_ids`/`citations`, upgrade
    ///    `source` tier, refresh `last_seen_at`. Returns
    ///    [`ProposeOutcome::DedupedExact`].
    /// 2. **Near-duplicate**: SimHash similarity ≥ threshold with an
    ///    existing pending proposal → mark the newcomer `Merged`, point
    ///    `merged_into` at the winner, and fold its counts/citations into
    ///    the winner. Returns [`ProposeOutcome::MergedNearDup`].
    /// 3. **Novel**: insert a fresh pending proposal. Returns
    ///    [`ProposeOutcome::Created`].
    ///
    /// Empty/whitespace-only text is rejected (error, no state change).
    /// The buffer is NOT auto-flushed: call [`SleepBuffer::flush`] to
    /// persist (lets callers batch a whole session-end pass into one
    /// atomic write — the interruptible unit).
    pub fn propose(
        &mut self,
        text: &str,
        kind: &str,
        session_id: &str,
        source: ProposalSource,
        citations: &[String],
    ) -> Result<ProposeOutcome> {
        let normalized = normalize_text(text);
        if normalized.is_empty() {
            anyhow::bail!("cannot propose empty candidate text");
        }
        let now = Utc::now();
        let dedup_key = dedup_key_for(&normalized);

        // Tier 1: exact match on dedup_key (any status except Merged
        // tombstones, which forward to their winner below).
        if let Some(id) = self
            .proposals
            .values()
            .find(|p| p.dedup_key == dedup_key && p.status != ProposalStatus::Merged)
            .map(|p| p.id.clone())
        {
            let p = self.proposals.get_mut(&id).expect("id from values()");
            bump_proposal(p, session_id, source, citations, now);
            self.dirty = true;
            return Ok(ProposeOutcome::DedupedExact { id });
        }

        let simhash = simhash64(&normalized);

        // Tier 2: near-duplicate against pending proposals only (promoted /
        // rejected records are terminal and never absorb new signal).
        let winner = self
            .proposals
            .values()
            .filter(|p| p.status == ProposalStatus::Pending)
            .filter(|p| hamming_similarity(p.simhash, simhash) >= NEAR_DUP_SIMILARITY_THRESHOLD)
            .max_by(|a, b| {
                a.occurrences
                    .cmp(&b.occurrences)
                    .then_with(|| a.first_seen_at.cmp(&b.first_seen_at).reverse())
                    // Final tiebreak on content-derived key (NOT id: id suffixes
                    // are random). Same-millisecond twin proposals must still
                    // converge identically on every run.
                    .then_with(|| b.dedup_key.cmp(&a.dedup_key))
            })
            .map(|p| p.id.clone());

        if let Some(winner_id) = winner {
            let new_id = new_proposal_id();
            // Fold the newcomer's single observation into the winner.
            {
                let w = self
                    .proposals
                    .get_mut(&winner_id)
                    .expect("winner id from values()");
                bump_proposal(w, session_id, source, citations, now);
            }
            self.proposals.insert(
                new_id.clone(),
                Proposal {
                    id: new_id.clone(),
                    version: SLEEP_BUFFER_VERSION,
                    dedup_key,
                    simhash,
                    text: text.to_string(),
                    kind: kind.to_string(),
                    occurrences: 1,
                    session_ids: vec![session_id.to_string()],
                    source,
                    first_seen_at: now,
                    last_seen_at: now,
                    citations: capped_citations(citations),
                    status: ProposalStatus::Merged,
                    merged_into: Some(winner_id.clone()),
                },
            );
            self.dirty = true;
            return Ok(ProposeOutcome::MergedNearDup {
                into_id: winner_id,
                new_id,
            });
        }

        // Tier 3: novel proposal.
        let id = new_proposal_id();
        self.proposals.insert(
            id.clone(),
            Proposal {
                id: id.clone(),
                version: SLEEP_BUFFER_VERSION,
                dedup_key,
                simhash,
                text: text.to_string(),
                kind: kind.to_string(),
                occurrences: 1,
                session_ids: vec![session_id.to_string()],
                source,
                first_seen_at: now,
                last_seen_at: now,
                citations: capped_citations(citations),
                status: ProposalStatus::Pending,
                merged_into: None,
            },
        );
        self.dirty = true;
        Ok(ProposeOutcome::Created { id })
    }

    /// Pending proposals sorted by descending [`Proposal::priority_score`].
    /// Ties break by earliest `first_seen_at` (older signal first), then
    /// by `dedup_key` (content-derived, so the order is identical on every
    /// run: proposal ids carry random suffixes and must never decide
    /// ordering).
    pub fn ranked_pending(&self, now: DateTime<Utc>) -> Vec<&Proposal> {
        let mut out: Vec<&Proposal> = self
            .proposals
            .values()
            .filter(|p| p.status == ProposalStatus::Pending)
            .collect();
        out.sort_by(|a, b| {
            b.priority_score(now)
                .partial_cmp(&a.priority_score(now))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.first_seen_at.cmp(&b.first_seen_at))
                .then_with(|| a.dedup_key.cmp(&b.dedup_key))
        });
        out
    }

    /// Top-`limit` pending proposals at or above `min_occurrences`
    /// occurrences: the Phase-2 work queue. Pure read; marks nothing.
    pub fn promotion_queue(
        &self,
        now: DateTime<Utc>,
        min_occurrences: u32,
        limit: usize,
    ) -> Vec<&Proposal> {
        self.ranked_pending(now)
            .into_iter()
            .filter(|p| p.occurrences >= min_occurrences)
            .take(limit)
            .collect()
    }

    /// Mark a proposal promoted (Phase 2, after the validation gate
    /// passes). Idempotent: already-promoted returns `false`, no change.
    pub fn mark_promoted(&mut self, id: &str) -> bool {
        self.transition(id, ProposalStatus::Promoted, None)
    }

    /// Mark a proposal rejected (Phase-2 gate failure). Idempotent.
    pub fn mark_rejected(&mut self, id: &str) -> bool {
        self.transition(id, ProposalStatus::Rejected, None)
    }

    /// Remove terminal proposals (`Promoted`, `Rejected`, `Merged`) older
    /// than `older_than`. Returns the number pruned. Pending proposals are
    /// never pruned: the buffer is the signal, not a cache.
    pub fn prune_terminal(&mut self, older_than: DateTime<Utc>) -> usize {
        let before = self.proposals.len();
        self.proposals
            .retain(|_, p| p.status == ProposalStatus::Pending || p.last_seen_at >= older_than);
        let pruned = before - self.proposals.len();
        if pruned > 0 {
            self.dirty = true;
        }
        pruned
    }

    /// Persist atomically (temp file in the same dir + rename). No-op when
    /// clean. After a successful flush, re-opening the path yields
    /// identical proposals — the idempotency contract tests assert this.
    pub fn flush(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let file = BufferFile {
            version: SLEEP_BUFFER_VERSION,
            proposals: self.proposals.clone(),
        };
        let text = serde_json::to_string_pretty(&file).context("serializing sleep buffer")?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating sleep dir '{}'", parent.display()))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        {
            let mut f = fs::File::create(&tmp)
                .with_context(|| format!("creating tmp buffer '{}'", tmp.display()))?;
            f.write_all(text.as_bytes())
                .context("writing tmp sleep buffer")?;
            f.sync_all().context("syncing tmp sleep buffer")?;
        }
        fs::rename(&tmp, &self.path).with_context(|| {
            format!(
                "renaming tmp buffer '{}' to '{}'",
                tmp.display(),
                self.path.display()
            )
        })?;
        self.dirty = false;
        Ok(())
    }

    fn transition(&mut self, id: &str, to: ProposalStatus, _via: Option<()>) -> bool {
        let Some(p) = self.proposals.get_mut(id) else {
            return false;
        };
        if p.status == to {
            return false; // idempotent no-op
        }
        // Terminal states never leave: rejected/promoted records are audit
        // history, and merged tombstones must keep forwarding.
        if p.status != ProposalStatus::Pending {
            return false;
        }
        p.status = to;
        p.last_seen_at = Utc::now();
        self.dirty = true;
        true
    }
}

// ---------------------------------------------------------------------------
// Normalization + hashing (all deterministic, std + sha2 only)
// ---------------------------------------------------------------------------

/// Lowercase, collapse all whitespace runs to single spaces, strip leading
/// / trailing space. The exact-match layer operates on this form so
/// `"  Foo\nBAR "` and `"foo bar"` dedup together.
pub fn normalize_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// sha256 hex of the normalized text: the exact-dedup key.
pub fn dedup_key_for(normalized: &str) -> String {
    let mut h = Sha256::new();
    h.update(normalized.as_bytes());
    hex_encode(&h.finalize())
}

/// 64-bit SimHash over token unigrams + bigrams with a deterministic
/// per-token hash (FNV-1a64 over UTF-8 bytes — no RandomState, so equal
/// inputs hash equal across processes and restarts).
pub fn simhash64(normalized: &str) -> u64 {
    let tokens: Vec<&str> = normalized.split(' ').filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        return 0;
    }
    // Bit accumulator: +1 per hash-bit 1, -1 per 0, weighted by n-gram order
    // (bigrams weigh 2x: phrase order is the stronger near-dup signal).
    let mut acc = [0i32; 64];
    let mut feed = |gram: &str, weight: i32| {
        let h = fnv1a64(gram.as_bytes());
        for (bit, slot) in acc.iter_mut().enumerate() {
            if (h >> bit) & 1 == 1 {
                *slot += weight;
            } else {
                *slot -= weight;
            }
        }
    };
    for tok in &tokens {
        feed(tok, 1);
    }
    for pair in tokens.windows(2) {
        // Join with a separator byte that cannot appear in normalized text
        // (normalization collapses all whitespace to single spaces, and we
        // use \x1f), so "a b"+"c" and "a"+"b c" hash distinctly.
        let bigram = format!("{}\x1f{}", pair[0], pair[1]);
        feed(&bigram, 2);
    }
    let mut out = 0u64;
    for (bit, v) in acc.iter().enumerate() {
        if *v > 0 {
            out |= 1 << bit;
        }
    }
    out
}

/// Fraction of equal bits in [0, 1].
pub fn hamming_similarity(a: u64, b: u64) -> f64 {
    let diff = (a ^ b).count_ones();
    f64::from(64 - diff) / 64.0
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut h = OFFSET;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(PRIME);
    }
    h
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

fn new_proposal_id() -> String {
    let ts = Utc::now().timestamp_millis();
    let rand: u32 = rand_u32();
    format!("prop_{ts}_{rand:08x}")
}

fn rand_u32() -> u32 {
    // DefaultHasher over time + pid + thread: unique enough for id suffixes
    // (dedup never relies on ids). No extra dependency for four bytes.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut h);
    std::process::id().hash(&mut h);
    std::thread::current().id().hash(&mut h);
    (h.finish() & 0xffff_ffff) as u32
}

fn distinct_count(ids: &[String]) -> usize {
    let mut seen: HashSet<&str> = HashSet::new();
    for id in ids {
        seen.insert(id.as_str());
    }
    seen.len()
}

fn bump_proposal(
    p: &mut Proposal,
    session_id: &str,
    source: ProposalSource,
    citations: &[String],
    now: DateTime<Utc>,
) {
    p.occurrences = p.occurrences.saturating_add(1);
    if !p.session_ids.iter().any(|s| s == session_id) {
        p.session_ids.push(session_id.to_string());
    }
    // Source tier is monotone: only upgrades (higher weight wins), so a
    // later low-tier re-proposal can never demote a user-backed candidate
    // and priority stays restart-stable.
    if source.weight() > p.source.weight() {
        p.source = source;
    }
    for c in citations {
        if !p.citations.iter().any(|e| e == c) {
            p.citations.push(c.clone());
        }
    }
    while p.citations.len() > MAX_CITATIONS {
        p.citations.remove(0);
    }
    p.last_seen_at = now;
}

fn capped_citations(citations: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for c in citations {
        if !out.iter().any(|e| e == c) {
            out.push(c.clone());
        }
    }
    while out.len() > MAX_CITATIONS {
        out.remove(0);
    }
    out
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests;
