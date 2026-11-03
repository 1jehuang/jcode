# Memory Regression Budget

Status: active guardrail
Updated: 2026-04-18

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
