---
name: sleep-consolidation
description: Sleep-time memory consolidation cycle. Use on a ~12h cadence (ambient schedule or cron) to accumulate recurring rule/fact candidates into the deterministic proposal buffer, and to surface the promotion queue for Phase-2 review. Never invents memories; only counts and ranks what sessions already proposed.
allowed-tools: bash, read, write, grep, agentgrep
---

# Sleep Consolidation (R9 Phase 1 — deterministic, no LLM)

You are the sleep-time consolidation cycle. You do NOT wordsmith, invent,
or write memories. You count, deduplicate, and prioritize candidate
consolidations that sessions have already proposed, so a later Phase-2
pass (with a model + human approval) can promote the best ones.

Phase 2 (wordsmith + validation gate) is NEEDS-BRAIN and OUT OF SCOPE
for this skill. If asked to rewrite candidates into fluent rules or to
write them into memory files, stop and say so.

## State

- Buffer file: `~/.jcode/sleep/sleep_buffer.json` (honors `$JCODE_HOME`).
- Crate: `jcode-sleep-buffer` (`SleepBuffer::open_default`, `propose`,
  `promotion_queue`, `mark_promoted`, `mark_rejected`, `flush`).
- Every mutation is idempotent; every flush is atomic (tmp + rename).
  Re-running a cycle converges. A killed cycle loses at most unflushed
  in-memory proposals.

## Cycle procedure

1. Open the buffer: `SleepBuffer::open_default()`. If it reports a
   corrupt/missing file, it starts empty (fail-open) — note it, continue.
2. Collect: gather candidate strings ALREADY extracted by producers
   (session-end hook payloads, transcript-rule passes, ambient cycle
   outputs). Do NOT parse raw transcripts here; if no producer output
   exists, record an empty cycle and skip to step 5.
3. Accumulate: call `propose(text, kind, session_id, source, citations)`
   per candidate. Outcomes:
   - `Created` — novel candidate, now pending.
   - `DedupedExact` — recurring candidate, counters bumped (this is the
     CodeYam recurrence signal: count, do not paraphrase).
   - `MergedNearDup` — typo-level variant folded into the winner.
   Empty text errors are expected for blank producer lines — skip them.
4. Rank: read `promotion_queue(now, min_occurrences=2, limit=20)`.
   Report the queue ordered by score (occurrences + session breadth,
   source-tier weight, 30-day age decay). Do NOT promote anything.
5. Flush once at the end (`flush()`), then reschedule: queue the next
   cycle ~12h out via the existing ambient scheduled queue
   (`schedule_ambient` with `wake_in_minutes: 720`, context naming this
   skill). The 12h default follows the Reddit dreaming cadence; tighten
   only with evidence of proposal starvation.

## Rules

- No memory writes. No skill-file writes. No prompt changes.
- Terminal states (`promoted`/`rejected`/`merged`) are audit history:
  never edit them, never propose into them.
- Pending proposals are never deleted by this cycle. `prune_terminal`
  only (old terminal records, with a cutoff date stated in the report).
- Keep producer kinds opaque (`scoped_rule`, `recurring_fact`, ...):
  Phase 1 never interprets them.

## Cycle report shape

- Proposals added / deduped-exact / merged-near-dup (counts).
- Current pending total; top of the promotion queue with scores,
  occurrences, distinct-session counts, oldest-first ties.
- Buffer health: file size, corrupt-on-open flag, pruned terminal count.
- Next scheduled wake (should be ~12h).
