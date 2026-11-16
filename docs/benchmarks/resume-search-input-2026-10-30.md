# Resume search input latency, October 30, 2026

## Change

Fuzzy metadata matching previously ran twice on the input thread for each search
edit: once to choose transcript candidates and again to filter visible sessions.
Transcript file reads were asynchronous, but this matching was not. Large literal
index scans also contributed measurable input latency.

The picker now performs fuzzy matching in its cancellable, 150 ms debounced
worker. Literal metadata matching remains immediate only when the total search
index is at most 1 MiB. Larger indexes use the worker for literal matching too.
Literal background hits retain a separate ranking tier above fuzzy hits. All
literal transcript scans finish before fuzzy transcript fallback scans begin.
Native JSONL journals are searched before their snapshot fallback.

## Measurements

Hosted Rust manual benchmark, 100 sessions with approximately 64 KiB indexes and
a nonmatching `modal cli` query:

- Equivalent old synchronous matching work: 188.49 ms.
- New complete search key handler: 1.61 ms.
- Command: `justrust test -p jcode-tui --lib benchmark_resume_search_keystroke_latency -- --ignored --nocapture`.
- Hosted run: `20261030-232109807-2794365`.

Local real-terminal smoke test against the actual session store, using isolated
120-column by 40-row PTYs and typing `modal cli` one character at a time:

| Binary | Key to next terminal write, milliseconds |
| --- | --- |
| Baseline `238464fe0` | 46.58, 39.34, 44.91, 144.63, 333.90, 151.51, 329.88, 335.63, 323.24 |
| Candidate | 9.96, 7.35, 7.52, 8.18, 8.46, 0.78, 7.38, 5.83, 5.77 |

The PTY timings measure the next terminal write, not a parsed query-specific
frame. Background redraws can affect this proxy. Neither test selected a session
or changed the shared daemon. These are single-run observations, not percentile
claims.

## Correctness

`justrust test -p jcode-tui --lib session_picker -- --nocapture` passed 163 tests
(10 ignored), hosted run `20261030-232058378-2795113`. Coverage includes deferred
fuzzy matching, deferred literal matching for large indexes, query cancellation,
metadata plus transcript token matching, progressive preview-scroll preservation,
and an exact `modal cli` transcript hit ranking ahead of newer `model cli`
fuzzy metadata.
