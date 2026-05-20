//! Background compaction for conversation context management
//!
//! When context reaches 80% of the limit, kicks off background summarization.
//! User continues chatting while summary is generated. When ready, seamlessly
//! swaps in the compacted context.
//!
//! The CompactionManager does NOT store its own copy of messages. Instead,
//! callers pass `&[Message]` references when needed. The manager tracks how
//! many messages from the front have been compacted via `compacted_count`.
//!
//! ## Compaction Modes
//!
//! - **Reactive** (default): compact when context hits a fixed threshold (80%).
//! - **Proactive**: compact early based on predicted EWMA token growth rate.
//! - **Semantic**: compact based on embedding-detected topic shifts and
//!   relevance scoring. Falls back to proactive if embeddings are unavailable.

use crate::message::{ContentBlock, Message, Role};
use crate::provider::Provider;
use crate::provider::openai_request::{
    openai_encrypted_content_fallback_summary, openai_encrypted_content_is_sendable,
};
use anyhow::Result;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinHandle;

pub use jcode_compaction_core::{
    CHARS_PER_TOKEN, COMPACTION_THRESHOLD, CRITICAL_THRESHOLD, CompactionAction, CompactionEvent,
    CompactionStats, DEFAULT_TOKEN_BUDGET, EMBED_MAX_CHARS_PER_MSG, EMBEDDING_HISTORY_WINDOW,
    EMERGENCY_TOOL_RESULT_MAX_CHARS, MANUAL_COMPACT_MIN_THRESHOLD, MIN_TURNS_TO_KEEP,
    RECENT_TURNS_TO_KEEP, SEMANTIC_EMBED_CACHE_CAPACITY, SUMMARY_PROMPT, SYSTEM_OVERHEAD_TOKENS,
    Summary, TOKEN_HISTORY_WINDOW, build_compaction_prompt, build_emergency_summary_text,
    compacted_summary_text_block, content_char_count, emergency_truncate_tool_results,
    estimate_compaction_tokens, mean_embedding, message_char_count, safe_compaction_cutoff,
    semantic_cache_key, semantic_goal_text, semantic_message_text, summary_payload_char_count,
};

/// Result from background compaction task
struct CompactionResult {
    summary_text: String,
    openai_encrypted_content: Option<String>,
    covers_up_to_turn: usize,
    duration_ms: u64,
    summarized_messages: usize,
}

struct CompactionOutcomeLog<'a> {
    trigger: &'a str,
    pre_tokens: u64,
    post_tokens: u64,
    messages_compacted: usize,
    messages_dropped: Option<usize>,
    duration_ms: u64,
    all_messages: &'a [Message],
}

/// Manages background compaction of conversation context.
///
/// Does NOT own message data. The caller owns the messages and passes
/// references into methods that need them. After compaction, the manager
/// records `compacted_count` — the number of leading messages that have
/// been summarized and should be skipped when building API payloads.
pub struct CompactionManager {
    /// Number of leading messages that have been compacted into the summary.
    /// When building API messages, skip the first `compacted_count` messages.
    compacted_count: usize,

    /// Active summary (if we've compacted before)
    active_summary: Option<Summary>,

    /// Rolling char estimate for the active (non-compacted) message suffix.
    ///
    /// In the common append-only case this is maintained incrementally, so token
    /// estimation does not need to rescan the entire active history every time.
    active_message_chars: usize,

    /// When true, the incremental char estimate must be recomputed from the
    /// caller's full message list before it can be trusted.
    active_message_chars_dirty: bool,

    /// Background compaction task handle
    pending_task: Option<JoinHandle<Result<CompactionResult>>>,

    /// User-facing trigger label for the currently running background compaction.
    pending_trigger: Option<String>,

    /// Turn index (relative to uncompacted messages) where pending compaction will cut off
    pending_cutoff: usize,

    /// Total turns seen (for tracking)
    total_turns: usize,

    /// When true, session restore/reseed has just loaded old history and
    /// compaction must stay disabled until a genuinely new message is added.
    suppress_compaction_until_new_message: bool,

    /// Token budget
    token_budget: usize,

    /// Provider-reported input token usage from the latest request.
    /// Used to trigger compaction with real token counts instead of only heuristics.
    observed_input_tokens: Option<u64>,

    /// Last compaction event (if any)
    last_compaction: Option<CompactionEvent>,

    // ── Mode & strategy ────────────────────────────────────────────────────
    /// Active compaction mode (set from config at construction)
    mode: crate::config::CompactionMode,

    /// Config snapshot for mode-specific parameters
    compaction_config: crate::config::CompactionConfig,

    // ── Proactive mode state ───────────────────────────────────────────────
    /// Rolling window of observed token counts, one entry per turn snapshot.
    /// Used to compute EWMA growth rate for proactive compaction.
    token_history: VecDeque<u64>,

    /// Total turns elapsed since the last successful compaction.
    /// Used as a cooldown anti-signal.
    turns_since_last_compact: usize,

    // ── Semantic mode state ────────────────────────────────────────────────
    /// Per-turn embedding snapshots for topic-shift detection.
    /// Each entry is the L2-normalized embedding of the last assistant message
    /// of that turn (truncated to EMBED_MAX_CHARS_PER_MSG for speed).
    embedding_history: VecDeque<Vec<f32>>,

    /// Local cache for semantic compaction embeddings keyed by truncated-text hash.
    /// Stores both successful embeddings and failed lookups (`None`) so repeated
    /// semantic scans do not redo the same work.
    semantic_embed_cache: HashMap<u64, (Option<Vec<f32>>, u64)>,

    /// Monotonic recency counter for the semantic embedding cache LRU.
    semantic_embed_cache_counter: u64,
}

impl CompactionManager {
    pub fn new() -> Self {
        let cfg = crate::config::config().compaction.clone();
        let mode = cfg.mode.clone();
        Self {
            compacted_count: 0,
            active_summary: None,
            active_message_chars: 0,
            active_message_chars_dirty: false,
            pending_task: None,
            pending_trigger: None,
            pending_cutoff: 0,
            total_turns: 0,
            suppress_compaction_until_new_message: false,
            token_budget: DEFAULT_TOKEN_BUDGET,
            observed_input_tokens: None,
            last_compaction: None,
            mode,
            compaction_config: cfg,
            token_history: VecDeque::with_capacity(TOKEN_HISTORY_WINDOW + 1),
            turns_since_last_compact: 0,
            embedding_history: VecDeque::with_capacity(EMBEDDING_HISTORY_WINDOW + 1),
            semantic_embed_cache: HashMap::with_capacity(SEMANTIC_EMBED_CACHE_CAPACITY),
            semantic_embed_cache_counter: 0,
        }
    }

    /// Reset all compaction state
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn with_budget(mut self, budget: usize) -> Self {
        self.token_budget = budget;
        self
    }

    /// Update the token budget (e.g., when model changes)
    pub fn set_budget(&mut self, budget: usize) {
        self.token_budget = budget;
    }

    /// Get current token budget
    pub fn token_budget(&self) -> usize {
        self.token_budget
    }

    /// Notify the manager that a message was added.
    ///
    /// Legacy callers that do not provide the message content keep turn counts
    /// correct, but mark the rolling char estimate dirty so the next token
    /// estimate will resync from the provided history slice.
    pub fn notify_message_added(&mut self) {
        self.total_turns += 1;
        self.suppress_compaction_until_new_message = false;
        self.active_message_chars_dirty = true;
    }

    /// Notify the manager that a message was added and update the rolling char
    /// estimate incrementally.
    pub fn notify_message_added_with(&mut self, message: &Message) {
        self.notify_message_added_blocks(&message.content);
    }

    pub fn notify_message_added_blocks(&mut self, content: &[ContentBlock]) {
        self.total_turns += 1;
        self.suppress_compaction_until_new_message = false;
        self.active_message_chars = self
            .active_message_chars
            .saturating_add(content_char_count(content));
        self.active_message_chars_dirty = false;
    }

    /// Backward-compatible alias for `notify_message_added`.
    /// Accepts (and ignores) the message — callers that haven't been
    /// updated yet can still call `add_message(msg)`.
    pub fn add_message(&mut self, message: Message) {
        self.notify_message_added_with(&message);
    }

    /// Seed the manager from already-existing history that was restored from
    /// disk or otherwise replayed into memory.
    ///
    /// This updates turn counts but deliberately suppresses compaction until a
    /// genuinely new message is added after the restore. Restoring history must
    /// not itself trigger compaction.
    pub fn seed_restored_messages(&mut self, count: usize) {
        self.total_turns = count;
        self.suppress_compaction_until_new_message = count > 0;
        self.active_message_chars = 0;
        self.active_message_chars_dirty = count > 0;
    }

    /// Seed the manager from already-existing history with an exact rolling char
    /// estimate for the active suffix.
    pub fn seed_restored_messages_with(&mut self, all_messages: &[Message]) {
        self.total_turns = all_messages.len();
        self.suppress_compaction_until_new_message = !all_messages.is_empty();
        self.active_message_chars = all_messages.iter().map(message_char_count).sum();
        self.active_message_chars_dirty = false;
    }

    pub fn seed_restored_stored_messages_with(
        &mut self,
        all_messages: &[crate::session::StoredMessage],
    ) {
        self.total_turns = all_messages.len();
        self.suppress_compaction_until_new_message = !all_messages.is_empty();
        self.active_message_chars = all_messages
            .iter()
            .map(|message| content_char_count(&message.content))
            .sum();
        self.active_message_chars_dirty = false;
    }

    /// Restore a previously persisted compacted view.
    pub fn restore_persisted_state(
        &mut self,
        state: &crate::session::StoredCompactionState,
        total_messages: usize,
    ) {
        self.pending_task = None;
        self.pending_trigger = None;
        self.pending_cutoff = 0;
        self.observed_input_tokens = None;
        self.last_compaction = None;
        self.token_history.clear();
        self.turns_since_last_compact = 0;
        self.embedding_history.clear();
        self.semantic_embed_cache.clear();
        self.semantic_embed_cache_counter = 0;
        self.total_turns = total_messages;
        self.compacted_count = state.compacted_count.min(total_messages);
        self.active_message_chars = 0;
        self.active_message_chars_dirty = total_messages > self.compacted_count;
        self.active_summary = Some(Summary {
            text: state.summary_text.clone(),
            openai_encrypted_content: state.openai_encrypted_content.clone(),
            covers_up_to_turn: state.covers_up_to_turn,
            original_turn_count: state.original_turn_count,
        });
        self.suppress_compaction_until_new_message = total_messages > 0;
    }

    /// Restore persisted compaction state and compute the active-suffix char
    /// estimate from the provided full message list.
    pub fn restore_persisted_state_with(
        &mut self,
        state: &crate::session::StoredCompactionState,
        all_messages: &[Message],
    ) {
        self.restore_persisted_state(state, all_messages.len());
        self.active_message_chars = self
            .active_messages(all_messages)
            .iter()
            .map(message_char_count)
            .sum();
        self.active_message_chars_dirty = false;
    }

    pub fn restore_persisted_stored_state_with(
        &mut self,
        state: &crate::session::StoredCompactionState,
        all_messages: &[crate::session::StoredMessage],
    ) {
        self.restore_persisted_state(state, all_messages.len());
        let start = self.compacted_count.min(all_messages.len());
        self.active_message_chars = all_messages[start..]
            .iter()
            .map(|message| content_char_count(&message.content))
            .sum();
        self.active_message_chars_dirty = false;
    }

    /// Export the currently active compacted view for persistence.
    pub fn persisted_state(&self) -> Option<crate::session::StoredCompactionState> {
        self.active_summary
            .as_ref()
            .map(|summary| crate::session::StoredCompactionState {
                summary_text: summary.text.clone(),
                openai_encrypted_content: summary.openai_encrypted_content.clone(),
                covers_up_to_turn: summary.covers_up_to_turn,
                original_turn_count: summary.original_turn_count,
                compacted_count: self.compacted_count,
            })
    }

    /// Drop provider-native OpenAI compaction state when it can no longer be
    /// replayed within OpenAI's per-string request limit. The compacted prefix
    /// remains compacted, but future requests use a small text fallback instead
    /// of bricking the session with an oversized `encrypted_content` field.
    pub fn discard_oversized_openai_native_compaction(&mut self) -> bool {
        let Some(summary) = self.active_summary.as_mut() else {
            return false;
        };
        let Some(encrypted_content) = summary.openai_encrypted_content.as_ref() else {
            return false;
        };
        if openai_encrypted_content_is_sendable(encrypted_content) {
            return false;
        }

        let encrypted_content_len = encrypted_content.len();
        crate::logging::warn(&format!(
            "[compaction] Discarding oversized OpenAI native compaction payload ({} chars)",
            encrypted_content_len,
        ));
        summary.openai_encrypted_content = None;
        let fallback = openai_encrypted_content_fallback_summary(encrypted_content_len);
        if summary.text.trim().is_empty() {
            summary.text = fallback;
        } else if !summary
            .text
            .contains("OpenAI native compaction state was discarded")
        {
            summary.text.push_str("\n\n");
            summary.text.push_str(&fallback);
        }
        self.observed_input_tokens = None;
        true
    }

    // ── Token snapshot (proactive mode) ────────────────────────────────────

    /// Record the observed token count after a completed turn.
    ///
    /// Called by the agent after `update_compaction_usage_from_stream`.
    /// Pushes the value into the rolling history window used by the proactive
    /// and semantic modes. Also increments the cooldown counter.
    pub fn push_token_snapshot(&mut self, tokens: u64) {
        self.token_history.push_back(tokens);
        if self.token_history.len() > TOKEN_HISTORY_WINDOW {
            self.token_history.pop_front();
        }
        self.turns_since_last_compact += 1;
    }

    /// Record an embedding snapshot for the current turn (semantic mode).
    ///
    /// `text` should be a short representation of the turn's assistant output
    /// (first EMBED_MAX_CHARS_PER_MSG chars). Silently skipped if the
    /// embedding model is unavailable.
    pub fn push_embedding_snapshot(&mut self, text: &str) {
        let snippet: String = text.chars().take(EMBED_MAX_CHARS_PER_MSG).collect();
        if let Some(emb) = self.cached_semantic_embedding(&snippet) {
            self.embedding_history.push_back(emb);
            if self.embedding_history.len() > EMBEDDING_HISTORY_WINDOW {
                self.embedding_history.pop_front();
            }
        }
    }

    // ── Anti-signal guard (shared by proactive + semantic) ──────────────────

    /// Returns `true` when any anti-signal fires and we should NOT compact
    /// proactively right now.
    ///
    /// Anti-signals are universal guards applied before the mode-specific
    /// trigger logic. They prevent wasted work and respect user intent.
    fn anti_signals_block(&self, all_messages: &[Message]) -> bool {
        let cfg = &self.compaction_config;

        // 1. Already compacting — never double-trigger.
        if self.pending_task.is_some() {
            return true;
        }

        // 2. Context below the proactive floor — too early regardless of trend.
        let usage = self.context_usage_with(all_messages);
        if usage < cfg.proactive_floor {
            return true;
        }

        // 3. Not enough token history to project from.
        if self.token_history.len() < cfg.min_samples {
            return true;
        }

        // 4. Growth has stalled: last stall_window snapshots show no increase.
        //    If tokens haven't grown, there's no urgency.
        if self.token_history.len() >= cfg.stall_window {
            let recent: Vec<u64> = self
                .token_history
                .iter()
                .rev()
                .take(cfg.stall_window)
                .cloned()
                .collect();
            let oldest = recent[recent.len() - 1];
            let newest = recent[0];
            if newest <= oldest {
                return true;
            }
        }

        // 5. Cooldown: too soon after the last compaction.
        if self.turns_since_last_compact < cfg.min_turns_between_compactions {
            return true;
        }

        false
    }

    // ── Proactive mode trigger ──────────────────────────────────────────────

    /// Returns `true` if the proactive strategy thinks we should compact now.
    ///
    /// Uses an EWMA over the token history to project forward `lookahead_turns`
    /// turns. If the projected token count would exceed the 80% threshold,
    /// it's time to compact before we get there.
    fn should_compact_proactively(&self, all_messages: &[Message]) -> bool {
        if self.anti_signals_block(all_messages) {
            return false;
        }

        let cfg = &self.compaction_config;
        let budget = self.token_budget as f64;
        let threshold = COMPACTION_THRESHOLD as f64 * budget;

        // Compute EWMA of per-turn token deltas.
        // We need at least 2 snapshots to get a delta.
        let snapshots: Vec<u64> = self.token_history.iter().cloned().collect();
        if snapshots.len() < 2 {
            return false;
        }

        let alpha = cfg.ewma_alpha as f64;
        let mut ewma_delta: f64 = (snapshots[1] as f64) - (snapshots[0] as f64);
        ewma_delta = ewma_delta.max(0.0);
        for i in 2..snapshots.len() {
            let delta = ((snapshots[i] as f64) - (snapshots[i - 1] as f64)).max(0.0);
            ewma_delta = alpha * delta + (1.0 - alpha) * ewma_delta;
        }
        let Some(current) = snapshots.last().copied().map(|value| value as f64) else {
            return false;
        };
        let projected = current + ewma_delta * cfg.lookahead_turns as f64;

        crate::logging::info(&format!(
            "[compaction/proactive] current={:.0} ewma_delta={:.1}/turn projected@{}turns={:.0} threshold={:.0}",
            current, ewma_delta, cfg.lookahead_turns, projected, threshold
        ));

        projected >= threshold
    }

    // ── Semantic mode trigger ───────────────────────────────────────────────

    /// Returns `true` if the semantic strategy detects a topic shift or
    /// predicts we should compact now.
    ///
    /// Topic-shift detection: compares the mean embedding of the oldest half
    /// of the history window against the newest half. A low cosine similarity
    /// between the two clusters indicates a topic boundary was crossed —
    /// the previous topic is complete and safe to summarize.
    ///
    /// Falls back to proactive logic if embeddings are unavailable.
    fn should_compact_semantic(&self, all_messages: &[Message]) -> bool {
        if self.anti_signals_block(all_messages) {
            return false;
        }

        // Need enough embedding history to split into two halves.
        let history_len = self.embedding_history.len();
        if history_len < 4 {
            // Fall back to proactive trigger.
            return self.should_compact_proactively(all_messages);
        }

        let cfg = &self.compaction_config;
        let half = history_len / 2;

        let old_embeddings: Vec<&Vec<f32>> = self.embedding_history.iter().take(half).collect();
        let new_embeddings: Vec<&Vec<f32>> = self.embedding_history.iter().skip(half).collect();

        let dim = old_embeddings[0].len();

        // Compute mean embedding for each half.
        let mean_old = mean_embedding(&old_embeddings, dim);
        let mean_new = mean_embedding(&new_embeddings, dim);

        let similarity = crate::embedding::cosine_similarity(&mean_old, &mean_new);

        crate::logging::info(&format!(
            "[compaction/semantic] topic similarity (old vs new half) = {:.3} (threshold={:.2})",
            similarity, cfg.topic_shift_threshold
        ));

        if similarity < cfg.topic_shift_threshold {
            crate::logging::info(
                "[compaction/semantic] Topic shift detected — triggering proactive compaction",
            );
            return true;
        }

        // No topic shift — still fall back to proactive growth check.
        self.should_compact_proactively(all_messages)
    }

    /// Build a relevance-scored keep set for semantic compaction.
    ///
    /// Embeds the last `goal_window_turns` messages to represent the current
    /// goal, then scores all active messages by cosine similarity. Returns the
    /// cutoff index: messages before the cutoff will be summarized, messages at
    /// or after are kept verbatim.
    ///
    /// Messages above `relevance_keep_threshold` anywhere in the history are
    /// pulled out of the summarize set. Falls back to the standard recency
    /// cutoff if embeddings fail.
    fn semantic_cutoff(&mut self, active: &[Message]) -> usize {
        let goal_window_turns = self.compaction_config.goal_window_turns;
        let relevance_keep_threshold = self.compaction_config.relevance_keep_threshold;
        let standard_cutoff = active.len().saturating_sub(RECENT_TURNS_TO_KEEP);
        if standard_cutoff == 0 {
            return 0;
        }

        // Build goal text from recent turns.
        let goal_turns = goal_window_turns.min(active.len());
        let goal_text = semantic_goal_text(&active[active.len() - goal_turns..]);

        if goal_text.is_empty() {
            return standard_cutoff;
        }

        let goal_emb = match self.cached_semantic_embedding(&goal_text) {
            Some(embedding) => embedding,
            None => return standard_cutoff,
        };

        // Score each candidate message (those before standard_cutoff).
        let mut high_relevance_count = 0usize;
        let mut earliest_high_relevance = standard_cutoff;

        for (idx, msg) in active[..standard_cutoff].iter().enumerate() {
            let text = semantic_message_text(msg);

            if text.is_empty() {
                continue;
            }

            if let Some(embedding) = self.cached_semantic_embedding(&text) {
                let sim = crate::embedding::cosine_similarity(&goal_emb, &embedding);
                if sim >= relevance_keep_threshold {
                    high_relevance_count += 1;
                    earliest_high_relevance = earliest_high_relevance.min(idx);
                }
            }
        }

        if high_relevance_count == 0 {
            return standard_cutoff;
        }

        // Find the latest high-relevance message before standard_cutoff.
        // We can't have gaps in the summarized range (tool call integrity),
        // so we move the cutoff up to just before the earliest high-relevance
        // message in the tail of the compaction range.
        let adjusted_cutoff = earliest_high_relevance;

        // Ensure we actually compact something meaningful.
        if adjusted_cutoff < 2 {
            return standard_cutoff;
        }

        crate::logging::info(&format!(
            "[compaction/semantic] relevance scoring: {} high-relevance msgs kept, cutoff {} -> {}",
            high_relevance_count, standard_cutoff, adjusted_cutoff
        ));

        adjusted_cutoff
    }

    /// Get the active (uncompacted) messages from a full message list.
    /// Skips the first `compacted_count` messages.
    fn active_messages<'a>(&self, all_messages: &'a [Message]) -> &'a [Message] {
        // If session restore/replay leaves the manager with bookkeeping from a
        // longer message vector, never fall back to the full transcript. That
        // makes already-compacted messages active again and can drive repeated
        // emergency compaction loops. Clamp to the end instead: all available
        // messages are covered by the summary until new turns arrive.
        let start = self.compacted_count.min(all_messages.len());
        &all_messages[start..]
    }

    fn clamp_compacted_count_to_messages(
        &mut self,
        all_messages: &[Message],
        reason: &str,
    ) -> bool {
        // Some backward-compatible call paths intentionally poll/apply without
        // caller-owned message history. An empty slice there means "unknown",
        // not necessarily an empty transcript, so do not treat it as an
        // authoritative upper bound.
        if all_messages.is_empty() {
            return false;
        }
        if self.compacted_count <= all_messages.len() {
            return false;
        }

        crate::logging::warn(&format!(
            "[compaction/invariant] compacted_count_exceeded_messages reason={} compacted_count={} messages_len={} total_turns={} has_summary={} summary_chars={} observed_input_tokens={:?}",
            reason,
            self.compacted_count,
            all_messages.len(),
            self.total_turns,
            self.active_summary.is_some(),
            self.summary_chars(),
            self.observed_input_tokens,
        ));
        self.compacted_count = all_messages.len();
        self.active_message_chars = 0;
        self.active_message_chars_dirty = false;
        true
    }

    fn log_compaction_state(&self, phase: &str, trigger: &str, all_messages: &[Message]) {
        let active_len = self.active_messages(all_messages).len();
        crate::logging::info(&format!(
            "[compaction/state] phase={} trigger={} messages_len={} active_messages={} compacted_count={} total_turns={} token_budget={} token_estimate={} effective_tokens={} observed_input_tokens={:?} has_summary={} summary_chars={} pending_cutoff={} is_compacting={}",
            phase,
            trigger,
            all_messages.len(),
            active_len,
            self.compacted_count,
            self.total_turns,
            self.token_budget,
            self.token_estimate_with(all_messages),
            self.effective_token_count_with(all_messages),
            self.observed_input_tokens,
            self.active_summary.is_some(),
            self.summary_chars(),
            self.pending_cutoff,
            self.pending_task.is_some(),
        ));
    }

    fn log_compaction_outcome(&self, outcome: CompactionOutcomeLog<'_>) {
        let tokens_saved = outcome.pre_tokens.saturating_sub(outcome.post_tokens);
        let grew = outcome.post_tokens > outcome.pre_tokens;
        let level = if grew { "warn" } else { "info" };
        let line = format!(
            "[compaction/outcome] level={} trigger={} duration_ms={} pre_tokens={} post_tokens={} tokens_saved={} grew={} messages_len={} active_messages={} compacted_count={} total_turns={} messages_compacted={} messages_dropped={} summary_chars={} observed_input_tokens={:?}",
            level,
            outcome.trigger,
            outcome.duration_ms,
            outcome.pre_tokens,
            outcome.post_tokens,
            tokens_saved,
            grew,
            outcome.all_messages.len(),
            self.active_messages(outcome.all_messages).len(),
            self.compacted_count,
            self.total_turns,
            outcome.messages_compacted,
            outcome.messages_dropped.unwrap_or(0),
            self.summary_chars(),
            self.observed_input_tokens,
        );
        if grew {
            crate::logging::warn(&line);
        } else {
            crate::logging::info(&line);
        }
    }

    fn active_message_chars_with(&self, all_messages: &[Message]) -> usize {
        if self.active_message_chars_dirty
            || self.active_messages_count() != self.active_messages(all_messages).len()
        {
            self.active_messages(all_messages)
                .iter()
                .map(message_char_count)
                .sum()
        } else {
            self.active_message_chars
        }
    }

    /// Get current token estimate using the caller's message list
    pub fn token_estimate_with(&self, all_messages: &[Message]) -> usize {
        estimate_compaction_tokens(
            self.active_summary.as_ref(),
            self.active_message_chars_with(all_messages),
            self.token_budget,
        )
    }

    /// Get current token estimate (backward compat — uses 0 messages, only summary + observed)
    pub fn token_estimate(&self) -> usize {
        estimate_compaction_tokens(self.active_summary.as_ref(), 0, self.token_budget)
    }

    /// Store provider-reported input token usage for compaction decisions.
    pub fn update_observed_input_tokens(&mut self, tokens: u64) {
        self.observed_input_tokens = Some(tokens);
    }

    /// Best-effort current token count using the caller's messages.
    pub fn effective_token_count_with(&self, all_messages: &[Message]) -> usize {
        let estimate = self.token_estimate_with(all_messages);
        let observed = self
            .observed_input_tokens
            .and_then(|tokens| usize::try_from(tokens).ok())
            .unwrap_or(0);
        estimate.max(observed)
    }

    /// Best-effort token count without message data (uses only observed tokens)
    pub fn effective_token_count(&self) -> usize {
        let estimate = self.token_estimate();
        let observed = self
            .observed_input_tokens
            .and_then(|tokens| usize::try_from(tokens).ok())
            .unwrap_or(0);
        estimate.max(observed)
    }

    /// Get current context usage as percentage (using caller's messages)
    pub fn context_usage_with(&self, all_messages: &[Message]) -> f32 {
        self.effective_token_count_with(all_messages) as f32 / self.token_budget as f32
    }

    /// Get current context usage (without messages, uses observed tokens only)
    pub fn context_usage(&self) -> f32 {
        self.effective_token_count() as f32 / self.token_budget as f32
    }

    /// Check if we should start compaction
    pub fn should_compact_with(&self, all_messages: &[Message]) -> bool {
        use crate::config::CompactionMode;
        if self.suppress_compaction_until_new_message {
            return false;
        }
        let active = self.active_messages(all_messages);
        match self.mode {
            CompactionMode::Reactive => {
                self.pending_task.is_none()
                    && self.context_usage_with(all_messages) >= COMPACTION_THRESHOLD
                    && active.len() > RECENT_TURNS_TO_KEEP
            }
            CompactionMode::Proactive => {
                active.len() > RECENT_TURNS_TO_KEEP && self.should_compact_proactively(all_messages)
            }
            CompactionMode::Semantic => {
                active.len() > RECENT_TURNS_TO_KEEP && self.should_compact_semantic(all_messages)
            }
        }
    }

    /// Start background compaction if needed
    pub fn maybe_start_compaction_with(
        &mut self,
        all_messages: &[Message],
        provider: Arc<dyn Provider>,
    ) {
        if !self.should_compact_with(all_messages) {
            return;
        }

        let active = self.active_messages(all_messages);

        // Calculate cutoff within active messages.
        // Semantic mode uses relevance scoring; other modes use recency.
        let mut cutoff = match self.mode {
            crate::config::CompactionMode::Semantic => self.semantic_cutoff(active),
            _ => active.len().saturating_sub(RECENT_TURNS_TO_KEEP),
        };
        if cutoff == 0 {
            return;
        }

        // Adjust cutoff to not split tool call/result pairs
        cutoff = safe_compaction_cutoff(active, cutoff);
        if cutoff == 0 {
            return;
        }

        // Snapshot messages to summarize (must clone for the async task)
        let messages_to_summarize: Vec<Message> = active[..cutoff].to_vec();
        let msg_count = messages_to_summarize.len();
        let existing_summary = self.active_summary.clone();
        let mode_label = self.mode_trigger_label().to_string();
        let estimated_tokens = self.effective_token_count_with(all_messages);
        crate::logging::info(&format!(
            "[TIMING] compaction_start: trigger={}, active_messages={}, cutoff={}, estimated_tokens={}, has_existing_summary={}",
            mode_label,
            active.len(),
            cutoff,
            estimated_tokens,
            existing_summary.is_some(),
        ));

        self.pending_cutoff = cutoff;
        self.pending_trigger = Some(mode_label.clone());

        // Spawn background task that notifies via Bus when done
        self.pending_task = Some(tokio::spawn(async move {
            let start = std::time::Instant::now();
            let result =
                generate_compaction_artifact(provider, messages_to_summarize, existing_summary)
                    .await;
            let duration_ms = start.elapsed().as_millis() as u64;
            crate::logging::info(&format!(
                "Compaction ({}) finished in {:.2}s ({} messages summarized)",
                mode_label,
                duration_ms as f64 / 1000.0,
                msg_count,
            ));
            crate::bus::Bus::global().publish(crate::bus::BusEvent::CompactionFinished);
            result.map(|mut result| {
                result.duration_ms = duration_ms;
                result.summarized_messages = msg_count;
                result
            })
        }));
    }

    /// Ensure context fits before an API call.
    ///
    /// Starts background compaction if above 80%. If context is critically full
    /// (>=95%), also performs an immediate hard-compact (drops old messages) so
    /// the next API call doesn't fail with "prompt too long".
    pub fn ensure_context_fits(
        &mut self,
        all_messages: &[Message],
        provider: Arc<dyn Provider>,
    ) -> CompactionAction {
        let was_compacting = self.is_compacting();
        self.maybe_start_compaction_with(all_messages, provider);
        let bg_started = !was_compacting && self.is_compacting();

        let usage = self.context_usage_with(all_messages);
        if usage >= CRITICAL_THRESHOLD {
            crate::logging::warn(&format!(
                "[compaction] Context at {:.1}% (critical threshold {:.0}%) — performing synchronous hard compact",
                usage * 100.0,
                CRITICAL_THRESHOLD * 100.0,
            ));
            match self.hard_compact_with(all_messages) {
                Ok(dropped) => {
                    let post_usage = self.context_usage_with(all_messages);
                    crate::logging::info(&format!(
                        "[compaction] Hard compact dropped {} messages, context now at {:.1}%",
                        dropped,
                        post_usage * 100.0,
                    ));
                    return CompactionAction::HardCompacted(dropped);
                }
                Err(reason) => {
                    crate::logging::error(&format!(
                        "[compaction] Hard compact failed at critical threshold: {}",
                        reason
                    ));
                }
            }
        }

        if bg_started {
            CompactionAction::BackgroundStarted {
                trigger: self
                    .pending_trigger
                    .clone()
                    .unwrap_or_else(|| self.mode_trigger_label().to_string()),
            }
        } else {
            CompactionAction::None
        }
    }

    /// Backward-compatible wrapper
    pub fn maybe_start_compaction(&mut self, _provider: Arc<dyn Provider>) {
        // Without messages, we can only check observed tokens
        // This is a no-op if no messages are provided
        // Callers should migrate to maybe_start_compaction_with
    }

    /// Force immediate compaction (for manual /compact command).
    pub fn force_compact_with(
        &mut self,
        all_messages: &[Message],
        provider: Arc<dyn Provider>,
    ) -> Result<(), String> {
        if self.pending_task.is_some() {
            return Err("Compaction already in progress".to_string());
        }

        let active = self.active_messages(all_messages);

        if active.len() <= RECENT_TURNS_TO_KEEP {
            return Err(format!(
                "Not enough messages to compact (need more than {}, have {})",
                RECENT_TURNS_TO_KEEP,
                active.len()
            ));
        }

        if self.context_usage_with(all_messages) < MANUAL_COMPACT_MIN_THRESHOLD {
            return Err(format!(
                "Context usage too low ({:.1}%) - nothing to compact",
                self.context_usage_with(all_messages) * 100.0
            ));
        }

        let mut cutoff = active.len().saturating_sub(RECENT_TURNS_TO_KEEP);
        if cutoff == 0 {
            return Err("No messages available to compact after keeping recent turns".to_string());
        }

        cutoff = safe_compaction_cutoff(active, cutoff);
        if cutoff == 0 {
            return Err("Cannot compact - would split tool call/result pairs".to_string());
        }

        let messages_to_summarize: Vec<Message> = active[..cutoff].to_vec();
        let msg_count = messages_to_summarize.len();
        let existing_summary = self.active_summary.clone();

        self.pending_cutoff = cutoff;
        self.pending_trigger = Some("manual".to_string());

        self.pending_task = Some(tokio::spawn(async move {
            let start = std::time::Instant::now();
            let result =
                generate_compaction_artifact(provider, messages_to_summarize, existing_summary)
                    .await;
            let duration_ms = start.elapsed().as_millis() as u64;
            crate::logging::info(&format!(
                "Compaction finished in {:.2}s ({} messages summarized)",
                duration_ms as f64 / 1000.0,
                msg_count,
            ));
            crate::bus::Bus::global().publish(crate::bus::BusEvent::CompactionFinished);
            result.map(|mut result| {
                result.duration_ms = duration_ms;
                result.summarized_messages = msg_count;
                result
            })
        }));

        Ok(())
    }

    /// Backward-compatible force_compact (for callers that still have their own message vec).
    /// This variant works with the old API where CompactionManager had its own messages.
    /// Callers should migrate to force_compact_with.
    pub fn force_compact(&mut self, _provider: Arc<dyn Provider>) -> Result<(), String> {
        Err(
            "force_compact requires messages — use force_compact_with(messages, provider)"
                .to_string(),
        )
    }

    /// Check if background compaction is done and apply it, updating rolling
    /// token-estimate state from the provided full message list.
    pub fn check_and_apply_compaction_with(&mut self, all_messages: &[Message]) {
        self.clamp_compacted_count_to_messages(all_messages, "check_and_apply_start");
        let task = match self.pending_task.take() {
            Some(task) => task,
            None => return,
        };

        // Check if done without blocking
        if !task.is_finished() {
            // Not done yet, put it back
            self.pending_task = Some(task);
            return;
        }

        // Get result
        match futures::executor::block_on(task) {
            Ok(Ok(result)) => {
                let trigger = self
                    .pending_trigger
                    .clone()
                    .unwrap_or_else(|| self.mode_trigger_label().to_string());
                self.log_compaction_state("apply_start", &trigger, all_messages);
                let pre_tokens = self.effective_token_count_with(all_messages) as u64;
                let compacted_chars: usize = self
                    .active_messages(all_messages)
                    .iter()
                    .take(self.pending_cutoff)
                    .map(message_char_count)
                    .sum();
                let summary = Summary {
                    text: result.summary_text,
                    openai_encrypted_content: result.openai_encrypted_content,
                    covers_up_to_turn: result.covers_up_to_turn,
                    original_turn_count: self.pending_cutoff,
                };

                // Advance the compacted count — these messages are now summarized
                self.compacted_count = self.compacted_count.saturating_add(self.pending_cutoff);
                if !all_messages.is_empty() {
                    self.compacted_count = self.compacted_count.min(all_messages.len());
                }
                self.active_message_chars = self
                    .active_message_chars_with(all_messages)
                    .saturating_sub(compacted_chars);
                self.active_message_chars_dirty = false;

                // Store summary
                self.active_summary = Some(summary);
                self.discard_oversized_openai_native_compaction();
                self.observed_input_tokens = None;
                let post_tokens = self.effective_token_count_with(all_messages) as u64;
                self.last_compaction = Some(CompactionEvent {
                    trigger: trigger.clone(),
                    pre_tokens: Some(pre_tokens),
                    post_tokens: Some(post_tokens),
                    tokens_saved: Some(pre_tokens.saturating_sub(post_tokens)),
                    duration_ms: Some(result.duration_ms),
                    messages_dropped: None,
                    messages_compacted: Some(result.summarized_messages),
                    summary_chars: self
                        .active_summary
                        .as_ref()
                        .map(|summary| summary.text.len()),
                    active_messages: Some(self.active_messages_count()),
                });
                crate::logging::info(&format!(
                    "[TIMING] compaction_complete: trigger={}, duration={}ms, pre_tokens={}, post_tokens={}, tokens_saved={}, messages_compacted={}, summary_chars={}, active_messages={}",
                    self.last_compaction
                        .as_ref()
                        .map(|event| event.trigger.as_str())
                        .unwrap_or("unknown"),
                    result.duration_ms,
                    pre_tokens,
                    post_tokens,
                    pre_tokens.saturating_sub(post_tokens),
                    result.summarized_messages,
                    self.active_summary
                        .as_ref()
                        .map(|summary| summary.text.len())
                        .unwrap_or(0),
                    self.active_messages_count(),
                ));
                self.log_compaction_outcome(CompactionOutcomeLog {
                    trigger: &trigger,
                    pre_tokens,
                    post_tokens,
                    messages_compacted: result.summarized_messages,
                    messages_dropped: None,
                    duration_ms: result.duration_ms,
                    all_messages,
                });

                // Reset cooldown counter so proactive/semantic modes don't
                // fire again immediately after a successful compaction.
                self.turns_since_last_compact = 0;

                self.pending_cutoff = 0;
                self.pending_trigger = None;
            }
            Ok(Err(e)) => {
                crate::logging::error(&format!("[compaction] Failed to generate summary: {}", e));
                self.pending_trigger = None;
                self.pending_cutoff = 0;
            }
            Err(e) => {
                crate::logging::error(&format!("[compaction] Task panicked: {}", e));
                self.pending_trigger = None;
                self.pending_cutoff = 0;
            }
        }
    }

    /// Backward-compatible completion check without caller history.
    pub fn check_and_apply_compaction(&mut self) {
        self.check_and_apply_compaction_with(&[]);
        self.active_message_chars_dirty = true;
    }

    /// Take the last compaction event (if any)
    pub fn take_compaction_event(&mut self) -> Option<CompactionEvent> {
        self.last_compaction.take()
    }

    /// Get messages for API call (with summary if compacted).
    /// Takes the full message list from the caller.
    pub fn messages_for_api_with(&mut self, all_messages: &[Message]) -> Vec<Message> {
        self.check_and_apply_compaction_with(all_messages);
        self.discard_oversized_openai_native_compaction();

        let active = self.active_messages(all_messages);

        match &self.active_summary {
            Some(summary) => {
                let summary_block = summary
                    .openai_encrypted_content
                    .as_ref()
                    .map(|encrypted_content| ContentBlock::OpenAICompaction {
                        encrypted_content: encrypted_content.clone(),
                    })
                    .unwrap_or_else(|| ContentBlock::Text {
                        text: compacted_summary_text_block(&summary.text),
                        cache_control: None,
                    });

                let mut result = Vec::with_capacity(active.len() + 1);

                result.push(Message {
                    role: Role::User,
                    content: vec![summary_block],
                    timestamp: None,
                    tool_duration_ms: None,
                });

                // Clone only the active (non-compacted) messages
                result.extend(active.iter().cloned());

                result
            }
            None => active.to_vec(),
        }
    }

    /// Backward-compatible messages_for_api (no messages available).
    /// Returns only summary if present, or empty vec.
    pub fn messages_for_api(&mut self) -> Vec<Message> {
        self.check_and_apply_compaction();
        self.discard_oversized_openai_native_compaction();

        // Without caller messages, we can only return the summary
        match &self.active_summary {
            Some(summary) => {
                let summary_block = summary
                    .openai_encrypted_content
                    .as_ref()
                    .map(|encrypted_content| ContentBlock::OpenAICompaction {
                        encrypted_content: encrypted_content.clone(),
                    })
                    .unwrap_or_else(|| ContentBlock::Text {
                        text: compacted_summary_text_block(&summary.text),
                        cache_control: None,
                    });
                vec![Message {
                    role: Role::User,
                    content: vec![summary_block],
                    timestamp: None,
                    tool_duration_ms: None,
                }]
            }
            None => Vec::new(),
        }
    }

    /// Check if compaction is in progress
    pub fn is_compacting(&self) -> bool {
        self.pending_task.is_some()
    }

    /// Get the active compaction mode
    pub fn mode(&self) -> crate::config::CompactionMode {
        self.mode.clone()
    }

    /// Change the active compaction mode for this session at runtime.
    pub fn set_mode(&mut self, mode: crate::config::CompactionMode) {
        self.mode = mode.clone();
        self.compaction_config.mode = mode;
    }

    fn mode_trigger_label(&self) -> &'static str {
        self.mode.as_str()
    }

    /// Get the number of compacted (summarized) messages
    pub fn compacted_count(&self) -> usize {
        self.compacted_count
    }

    /// Get the character count of the active summary (0 if none)
    pub fn summary_chars(&self) -> usize {
        self.active_summary
            .as_ref()
            .map(summary_payload_char_count)
            .unwrap_or(0)
    }

    /// Get the current number of active, un-compacted messages.
    pub fn active_messages_count(&self) -> usize {
        self.total_turns.saturating_sub(self.compacted_count)
    }

    /// Get stats about current state (without message data)
    pub fn stats(&self) -> CompactionStats {
        CompactionStats {
            total_turns: self.total_turns,
            active_messages: 0, // unknown without messages
            has_summary: self.active_summary.is_some(),
            is_compacting: self.is_compacting(),
            token_estimate: self.token_estimate(),
            effective_tokens: self.effective_token_count(),
            observed_input_tokens: self.observed_input_tokens,
            context_usage: self.context_usage(),
        }
    }

    /// Get stats with full message data
    pub fn stats_with(&self, all_messages: &[Message]) -> CompactionStats {
        let active = self.active_messages(all_messages);
        CompactionStats {
            total_turns: self.total_turns,
            active_messages: active.len(),
            has_summary: self.active_summary.is_some(),
            is_compacting: self.is_compacting(),
            token_estimate: self.token_estimate_with(all_messages),
            effective_tokens: self.effective_token_count_with(all_messages),
            observed_input_tokens: self.observed_input_tokens,
            context_usage: self.context_usage_with(all_messages),
        }
    }

    fn cached_semantic_embedding(&mut self, text: &str) -> Option<Vec<f32>> {
        let key = semantic_cache_key(text);

        if let Some((cached, recency)) = self.semantic_embed_cache.get_mut(&key) {
            let counter = self.semantic_embed_cache_counter;
            self.semantic_embed_cache_counter = counter.wrapping_add(1);
            *recency = counter;
            return cached.clone();
        }

        let embedding = crate::embedding::embed(text).ok();
        self.insert_semantic_embedding_cache(key, embedding.clone());
        embedding
    }

    fn insert_semantic_embedding_cache(&mut self, key: u64, embedding: Option<Vec<f32>>) {
        if self.semantic_embed_cache.len() >= SEMANTIC_EMBED_CACHE_CAPACITY {
            let oldest_key = self
                .semantic_embed_cache
                .iter()
                .min_by_key(|(_, (_, recency))| *recency)
                .map(|(&key, _)| key);
            if let Some(oldest_key) = oldest_key {
                self.semantic_embed_cache.remove(&oldest_key);
            }
        }

        let counter = self.semantic_embed_cache_counter;
        self.semantic_embed_cache_counter = counter.wrapping_add(1);
        self.semantic_embed_cache.insert(key, (embedding, counter));
    }

    /// Poll for compaction completion and return an event if one was applied.
    pub fn poll_compaction_event_with(
        &mut self,
        all_messages: &[Message],
    ) -> Option<CompactionEvent> {
        self.check_and_apply_compaction_with(all_messages);
        self.take_compaction_event()
    }

    pub fn poll_compaction_event(&mut self) -> Option<CompactionEvent> {
        self.check_and_apply_compaction();
        self.take_compaction_event()
    }

    /// Emergency hard compaction: drop old messages without summarizing.
    /// Takes the caller's full message list to inspect content.
    ///
    /// When the remaining turns (after keeping `RECENT_TURNS_TO_KEEP`) still
    /// exceed the token budget, progressively keeps fewer turns down to
    /// `MIN_TURNS_TO_KEEP`.
    pub fn hard_compact_with(&mut self, all_messages: &[Message]) -> Result<usize, String> {
        if self.clamp_compacted_count_to_messages(all_messages, "hard_compact_start") {
            self.log_compaction_state("hard_compact_clamped", "hard_compact", all_messages);
        }

        let active = self.active_messages(all_messages);

        if active.len() <= MIN_TURNS_TO_KEEP {
            return Err(format!(
                "Not enough messages to compact (have {}, need more than {})",
                active.len(),
                MIN_TURNS_TO_KEEP
            ));
        }

        let pre_tokens = self.effective_token_count_with(all_messages) as u64;
        self.log_compaction_state("hard_compact_start", "hard_compact", all_messages);
        let active_char_counts: Vec<usize> = active.iter().map(message_char_count).collect();
        let mut remaining_suffix_chars = vec![0usize; active_char_counts.len() + 1];
        for idx in (0..active_char_counts.len()).rev() {
            remaining_suffix_chars[idx] =
                remaining_suffix_chars[idx + 1].saturating_add(active_char_counts[idx]);
        }

        let mut turns_to_keep = RECENT_TURNS_TO_KEEP.min(active.len().saturating_sub(1));
        let mut cutoff;
        loop {
            cutoff = active.len().saturating_sub(turns_to_keep);
            cutoff = safe_compaction_cutoff(active, cutoff);

            if cutoff > 0 {
                let remaining_tokens = remaining_suffix_chars[cutoff] / CHARS_PER_TOKEN;
                if remaining_tokens <= self.token_budget {
                    break;
                }
            }

            if turns_to_keep <= MIN_TURNS_TO_KEEP {
                cutoff = active.len().saturating_sub(MIN_TURNS_TO_KEEP);
                cutoff = safe_compaction_cutoff(active, cutoff);
                break;
            }
            turns_to_keep = (turns_to_keep / 2).max(MIN_TURNS_TO_KEEP);
        }

        if cutoff == 0 {
            return Err("Cannot compact — would split tool call/result pairs".to_string());
        }

        let dropped_count = cutoff;
        let summary_text = build_emergency_summary_text(
            self.active_summary
                .as_ref()
                .map(|summary| summary.text.as_str()),
            dropped_count,
            pre_tokens,
            self.token_budget,
            &active[..cutoff],
        );

        let summary = Summary {
            text: summary_text,
            openai_encrypted_content: None,
            covers_up_to_turn: cutoff,
            original_turn_count: cutoff,
        };

        self.compacted_count = self
            .compacted_count
            .saturating_add(cutoff)
            .min(all_messages.len());
        self.active_message_chars = remaining_suffix_chars[cutoff];
        self.active_message_chars_dirty = false;
        self.active_summary = Some(summary);
        self.observed_input_tokens = None;
        let post_tokens = self.effective_token_count_with(all_messages) as u64;
        self.last_compaction = Some(CompactionEvent {
            trigger: "hard_compact".to_string(),
            pre_tokens: Some(pre_tokens),
            post_tokens: Some(post_tokens),
            tokens_saved: Some(pre_tokens.saturating_sub(post_tokens)),
            duration_ms: Some(0),
            messages_dropped: Some(dropped_count),
            messages_compacted: Some(dropped_count),
            summary_chars: self
                .active_summary
                .as_ref()
                .map(|summary| summary.text.len()),
            active_messages: Some(self.active_messages_count()),
        });
        self.log_compaction_outcome(CompactionOutcomeLog {
            trigger: "hard_compact",
            pre_tokens,
            post_tokens,
            messages_compacted: dropped_count,
            messages_dropped: Some(dropped_count),
            duration_ms: 0,
            all_messages,
        });

        Ok(dropped_count)
    }

    /// Backward-compatible hard_compact
    pub fn hard_compact(&mut self) -> Result<usize, String> {
        Err("hard_compact requires messages — use hard_compact_with(messages)".to_string())
    }

    /// Emergency truncation: shorten large tool results in active messages.
    ///
    /// When hard compaction isn't sufficient (the remaining few turns are
    /// individually too large), this truncates tool result content so the
    /// conversation can fit within the token budget.
    ///
    /// Returns the number of tool results that were truncated.
    pub fn emergency_truncate_with(&mut self, all_messages: &mut [Message]) -> usize {
        let start = self.compacted_count.min(all_messages.len());
        let active = &mut all_messages[start..];
        let truncated = emergency_truncate_tool_results(active, EMERGENCY_TOOL_RESULT_MAX_CHARS);

        if truncated > 0 {
            self.observed_input_tokens = None;
        }
        truncated
    }
}

impl Default for CompactionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate summary using the provider
async fn generate_compaction_artifact(
    provider: Arc<dyn Provider>,
    messages: Vec<Message>,
    mut existing_summary: Option<Summary>,
) -> Result<CompactionResult> {
    let start = Instant::now();
    if let Some(summary) = existing_summary.as_mut()
        && let Some(encrypted_content) = summary.openai_encrypted_content.as_ref()
        && !openai_encrypted_content_is_sendable(encrypted_content)
    {
        let encrypted_content_len = encrypted_content.len();
        crate::logging::warn(&format!(
            "[compaction] Existing OpenAI native compaction payload is oversized ({} chars); falling back to text summary",
            encrypted_content_len,
        ));
        summary.openai_encrypted_content = None;
        let fallback = openai_encrypted_content_fallback_summary(encrypted_content_len);
        if summary.text.trim().is_empty() {
            summary.text = fallback;
        } else if !summary
            .text
            .contains("OpenAI native compaction state was discarded")
        {
            summary.text.push_str("\n\n");
            summary.text.push_str(&fallback);
        }
    }

    if let Ok(native) = provider
        .native_compact(
            &messages,
            existing_summary
                .as_ref()
                .map(|summary| summary.text.as_str()),
            existing_summary
                .as_ref()
                .and_then(|summary| summary.openai_encrypted_content.as_deref()),
        )
        .await
    {
        if let Some(encrypted_content) = native.openai_encrypted_content.as_ref()
            && !openai_encrypted_content_is_sendable(encrypted_content)
        {
            crate::logging::warn(&format!(
                "[compaction] OpenAI native compaction returned oversized encrypted_content ({} chars); falling back to text summary",
                encrypted_content.len(),
            ));
        } else {
            return Ok(CompactionResult {
                summary_text: native.summary_text.unwrap_or_default(),
                openai_encrypted_content: native.openai_encrypted_content,
                covers_up_to_turn: messages.len(),
                duration_ms: start.elapsed().as_millis() as u64,
                summarized_messages: messages.len(),
            });
        }
    }

    let max_prompt_chars = provider.context_window().saturating_sub(4000) * CHARS_PER_TOKEN;
    let prompt = build_compaction_prompt(&messages, existing_summary.as_ref(), max_prompt_chars);

    // Generate summary using simple completion
    let summary = provider
        .complete_simple(
            &prompt,
            "You are a helpful assistant that summarizes conversations.",
        )
        .await?;

    Ok(CompactionResult {
        summary_text: summary,
        openai_encrypted_content: None,
        covers_up_to_turn: messages.len(),
        duration_ms: start.elapsed().as_millis() as u64,
        summarized_messages: messages.len(),
    })
}

pub async fn build_transfer_compaction_state(
    provider: Arc<dyn Provider>,
    messages: Vec<Message>,
    existing_state: Option<crate::session::StoredCompactionState>,
) -> Result<Option<crate::session::StoredCompactionState>> {
    let existing_summary = existing_state.as_ref().map(|state| Summary {
        text: state.summary_text.clone(),
        openai_encrypted_content: state.openai_encrypted_content.clone(),
        covers_up_to_turn: state.covers_up_to_turn,
        original_turn_count: state.original_turn_count,
    });

    if messages.is_empty() {
        return Ok(existing_state.map(|mut state| {
            state.compacted_count = 0;
            state
        }));
    }

    let prior_turns = existing_state
        .as_ref()
        .map(|state| state.original_turn_count.max(state.covers_up_to_turn))
        .unwrap_or(0);
    let result = generate_compaction_artifact(provider, messages.clone(), existing_summary).await?;
    let total_turns = prior_turns + messages.len();

    Ok(Some(crate::session::StoredCompactionState {
        summary_text: result.summary_text,
        openai_encrypted_content: result.openai_encrypted_content,
        covers_up_to_turn: total_turns,
        original_turn_count: total_turns,
        compacted_count: 0,
    }))
}

#[cfg(test)]
#[path = "compaction_tests.rs"]
mod tests;
