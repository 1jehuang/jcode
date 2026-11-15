# Memory Regression Budget

Status: active guardrail
Updated: 2026-11-15

This document defines the current memory regression budget for jcode.

For live server triage and cause-specific remediation, see
[Jcode Server Memory Incident Runbook](./MEMORY_INCIDENT_RUNBOOK.md).

For idle-session cache reclamation, its configuration, measured RAM indicator,
and retained-state boundaries, see [Session Hibernation](./SESSION_HIBERNATION.md).

The goal is not to freeze memory usage forever. The goal is to make memory changes:
- measurable
- reviewable
- intentionally justified

Where possible, budgets below are tied to counters and caps already exposed by the codebase rather than guessed RSS numbers.

## How to collect the metrics

Use existing debug surfaces instead of ad hoc instrumentation:

- TUI aggregate memory profile: `:debug memory`
- TUI memory sample history: `:debug memory-history`
- Markdown cache profile: `:debug markdown:memory`
- Mermaid cache profile: `:debug mermaid:memory`
- Agent/session memory profile via debug socket: `agent:memory`
- Fast server incident classification: `server:memory-incident`
- Full server attribution: `server:memory`
- Process-lifetime timeline: `python scripts/analyze_runtime_memory_log.py --days 1`

Primary sources in code:
- `src/tui/app/debug_cmds.rs`
- `src/tui/memory_profile.rs`
- `src/session.rs`
- `src/tui/markdown.rs`
- `src/tui/mermaid.rs`
- `src/runtime_memory_log.rs`

## Budget model

We use two kinds of budgets:

1. Hard caps
- These are explicit limits already enforced by caches.
- Regressions here mean the code changed its bound or bypassed it.

2. Ratchet expectations
- These are expected relationships between memory counters.
- Regressions here are allowed only with explanation and updated docs/tests.

## Efficiency investigations and stopping criteria

Prefer eliminating redundant ownership over reducing useful retained state.
Preserve conversation history, exact persistence/replay, snapshot isolation,
rendered output, responsiveness, and cache utility. Cache eviction, compression
with extra decoding work, and delayed delivery are tradeoffs, not automatic wins.

For an optimization round:

1. Rank attributed owners before editing. Prefer an expected retained gain of
   at least 10 MiB per server or 1 MiB per typical client. These are investigation
   priorities, not runtime caps. Smaller obvious low-complexity waste may qualify.
2. Pin and identify both binaries. Repeat the same workload against isolated
   sockets and homes, with settled samples at equal session/turn counts. Do not
   infer savings from restarting the production server.
3. Distinguish live allocator bytes, process PSS/private pages, unique backing
   allocations, string capacities, and serialized JSON sizes. JSON size is an
   attribution proxy, not a guaranteed heap bound. Exclude busy profiles explicitly.
4. Test exact equality, legacy serialization, replay, and mutation isolation.
   Verify the installed binary and live diagnostic schema after activation.
5. Stop the round after two ranked follow-up passes find no further safe material
   candidate, or when remaining candidates require wider redesign, unavailable
   attribution, or behavior/latency tradeoffs. Record deferred owners and the
   evidence that would justify reopening them. This is not proof that all future
   optimization potential is exhausted.

### Shared compaction acceptance example (2026-11-15)

Sixteen resumed sessions, each with a 1 MiB encrypted compaction payload, grew
live allocator usage by **52.358–52.729 MiB before** and **36.578–36.702 MiB after**
sharing immutable session/persistence snapshots, over two pinned runs per binary.
The pinned revisions also include concurrent changes, so this is a workload
comparison rather than a one-patch benchmark. Ownership tests and payload
counters independently verify the eliminated metadata copy.
Process-private growth varied more, so this is not a fixed RSS savings claim.
Both candidate runs retained 16,777,744 bytes of canonical payload capacity and
zero additional persistence-metadata payload capacity. The independent compaction manager summary was initially a separate owner.
A subsequent narrower change shares only the immutable encrypted body between
manager summaries and stored states, retaining independently mutable summary text.

Borrowed exact comparison also avoids constructing an owned exported payload
solely to detect unchanged state. An optional probe of 128 comparisons with a
9 MiB payload took 67,228 microseconds for export-and-compare and 21,172 for the
borrowed comparison in one hosted test run. This measures that operation, not
application-wide latency or retained RSS. Run the focused probe with
`justrust test -p jcode-base --lib persisted_state_comparison_probe -- --ignored --nocapture`.

The follow-up owner pass found about 0.57 MiB combined AGENTS/skill snapshot
string capacity across 41 ready live sessions. A local completed-turn fixture
retained 26 default tools with 32,697 serialized JSON bytes, not a heap bound or
a measurement of custom MCP catalogs. Immediately post-reload, locked tool
snapshots are cold, so their zero count is not representative of warmed turns.
The approximately 10.3 MiB search index already shares incremental Bloom-filter
backing storage. No cache reduction was made to manufacture further savings.
The encrypted-body follow-up (`fe1773bb6`, sharing regressions in `63bf3a240`)
repeated the isolated sixteen-session workload with two runs per pinned binary.
Live-allocator growth fell from **36.261–36.400 MiB to 20.163–20.309 MiB**.
Private-page growth was also lower but noisier, so this is not a guaranteed
production PSS reduction. The pinned builds contain concurrent changes, while
pointer-sharing, copy-on-write and byte-identical JSON tests independently
verify the removed encrypted-body allocation. No history or cache was discarded.
Custom tool catalogs and transcript/provider-cache payload duplication remain
separate candidates requiring their own ownership and mutation analysis.

## Hard caps

### Markdown cache budget

Source: `src/tui/markdown.rs`

| Metric | Budget | Why |
|---|---:|---|
| `highlight_cache_entries` | `<= 256` | Explicit cache cap (`HIGHLIGHT_CACHE_LIMIT`) |

Required review action if violated:
- explain why the cache limit changed
- update this doc
- update any affected tests or benchmarks

### Mermaid cache budget

Sources:
- `src/tui/mermaid.rs`
- `src/tui/mermaid_cache_render.rs`

| Metric | Budget | Why |
|---|---:|---|
| `render_cache_entries` | `<= 64` | Explicit render-cache cap (`RENDER_CACHE_MAX`) |
| `image_state_entries` | `<= 12` | Explicit protocol-state cap (`IMAGE_STATE_MAX`) |
| `source_cache_entries` | `<= 8` | Explicit decoded-source cap (`SOURCE_CACHE_MAX`) |
| `active_diagrams` | `<= 128` | Explicit active-diagram cap (`ACTIVE_DIAGRAMS_MAX`) |
| `cache_disk_png_bytes` | `<= 50 MiB` | Explicit on-disk cache cap (`CACHE_MAX_SIZE_BYTES`) |
| `cache_disk_max_age_secs` | `<= 259200` | 3-day expiry (`CACHE_MAX_AGE_SECS`) |

Required review action if violated:
- document the new limit and reason
- verify eviction still works
- verify no unbounded growth path was introduced

## Ratchet expectations

### Session and transcript memory

Source: `src/session.rs`, `src/tui/memory_profile.rs`

These are not strict caps yet, but they are expected relationships.

| Metric relationship | Expectation |
|---|---|
| `provider_messages_cache.count` vs `messages.count` | Should remain in the same order of magnitude for a single session, and normally track the transcript closely |
| `session_provider_cache_json_bytes` vs `canonical_transcript_json_bytes` | Should remain comparable for normal chat flows, not explode independently |
| `transient_provider_materialization_json_bytes` | Should return to zero or near-zero outside active materialization-heavy paths |
| `display_large_tool_output_bytes` | Large values require explanation because they usually mean raw tool output is being retained too aggressively in the UI |

Required review action if violated:
- show before/after memory profiles
- explain which retention path grew
- prefer fixing duplication before raising any budget

### Runtime memory log expectations

Source: `src/runtime_memory_log.rs`

Runtime memory logs are the regression detection mechanism, not just a debug feature.

Expected behavior:
- server/client logs should be sufficient to explain large changes in:
  - session/transcript totals
  - provider cache totals
  - TUI display totals
  - side panel totals
- new large memory owners should emit attributable signals instead of appearing only as unexplained RSS growth

Required review action if violated:
- add or improve attribution before accepting the memory increase

## Review checklist for memory-affecting changes

When changing memory-heavy code, capture and include:

1. Which counters changed?
- aggregate `:debug memory`
- targeted `:debug markdown:memory` / `:debug mermaid:memory`
- `agent:memory` when session/provider cache behavior changes

2. Was a hard cap changed?
- if yes, explain why the old cap was insufficient

3. Did duplication increase?
- canonical transcript
- provider cache
- materialized provider view
- display copy
- side-panel copy

4. Did observability remain adequate?
- if memory grew, can logs/profiles explain where?

## Current initial budget summary

These are the concrete enforced limits today:

- Markdown highlight cache entries: 256
- Mermaid render cache entries: 64
- Mermaid protocol image-state entries: 12
- Mermaid decoded source-cache entries: 8
- Mermaid active diagrams: 128
- Mermaid on-disk PNG cache: 50 MiB, max age 3 days

Any intentional change to those limits must update this document in the same PR.

## November 15, 2026 ownership follow-up

- Message and stored-message payloads now share immutable backing storage and
  detach on mutation. Legacy JSON arrays and independent snapshot mutation are
  preserved. An isolated 32-snapshot allocator probe over a 720,896-byte body
  measured 23,080,144 bytes of growth for owned-vector copies versus 3,440 bytes
  for shared snapshots. This is an ownership probe, not production RSS savings.
- Pinned 16-session local-provider workloads did not establish a steady-state
  streaming RAM reduction from transcript sharing alone. Provider transports
  already release request-preparation transcript snapshots before network waits.
- The OpenAI-compatible/OpenRouter retry transport separately retained a full
  JSON request and a deep per-attempt clone. The attempt now borrows the retry
  owner's immutable request, preserving serialization and retry behavior.
  Two private 16-session runs with 1 MiB of tool results per session measured
  allocator-live growth of 74,415,664 and 74,265,760 bytes before, versus
  57,186,656 and 57,134,672 bytes after. The second pair reversed run order.
  These are isolated real transport measurements, not a claim about current
  production totals or a fixed RSS reduction.
- The retry-body change passed 12 wire tests, 11 retry-filter tests and four
  Azure retry tests on hosted builds. No cache limits, history retention,
  batching, timers or output quality were reduced.
