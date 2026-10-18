# Reload storm latency (2026-10-17)

When the shared server reloads, every open TUI reconnects at once and resumes
its session. This records what each phase costs, what was fixed, and where the
remaining time goes, measured from the live daemon log
(`~/.jcode/logs/jcode-*.log`) after each deploy.

## How to measure

Each reload starts at the last `Server listening` line. Useful lines:

| Phase | Log line |
|---|---|
| Server per-connection setup | `[TIMING] handle_client setup` (`provider_fork`, `agent_new`) |
| Provider fork per runtime | `[TIMING] provider fork: runtimes=... openrouter=` |
| Session parse on resume | `[TIMING] restore_session:` `load=`, `session_load: ... source=prewarm` |
| Restore -> History steps | `[TIMING] resume attach laps` (only when >100ms) |
| Server wrote History | `[TIMING] send_history write` |
| Client applied History | `Applied delta History` / `remote bootstrap: history after` |
| Client exec startup | `=== Startup Profile (...)` |

`SESSION_LIFECYCLE ... history_sent_ms` includes the deferred
`persist_restored()` that runs after History is already on the wire, so it can
overstate server latency by up to ~1s. Use `send_history write` instead.

## Fixed

| Change | Before | After (live) |
|---|---|---|
| Fresh-route memo, no-op catalog bump, resume skips route (`6ff59d6af`, `ce5a995a5`, `6c4cdfc66`) | fork p50 ~2.7s | ~0.1s |
| Defer presence markers (`3f04119ad`) | mark_active p50 43ms | 0 |
| Non-blocking placeholder close, defer telemetry session (`358263d18`) | close 0.46-2.1s | assign p50 7ms |
| `--resume` id via startup stub (`3b5d50d24`) | extra full parse 0.35-0.6s | 21-25ms |
| Parallel `messages` parse (`40240fe1e`) | 35MB load 159ms idle | 59ms |
| Startup prewarm of large sessions (`42ddac817`, `68a0b9257`, `e0d216c76`) | restore load 100-300ms | 0-1ms for all >=2MB |
| OpenRouter default-model ranking memo (`56f37a0d9`) | fork openrouter p50 32-46ms | 4-15ms |

## Where the time goes now (18:59:34 restart storm, 11 windows)

- Server, exec-connected -> History written: 106-216ms.
- Client, History written -> applied: 2-266ms (gorilla 201ms on a 2.6KB delta,
  guppy 266ms), so this is client work, not payload size.
- Client exec startup before connect: Startup Profile p50 151ms, max 535ms
  (`app_new_for_remote` up to 439ms, `selfdev_git_hash` ~87ms on self-dev).
- End to end `history after`: p50 421ms, p90 454ms, max 750ms.

## Open

- Intermittent 0.4-2s wait in `rename_swarm_member_session` for a swarm map
  write lock (seen at 17:40, 17:51, 18:45, 18:51). `3ba3c18d0` logs which map
  and how long; it has not recurred in four storms since.
- Client-side: background transcript render racing History apply, and
  `app_new_for_remote` / first frame on exec.

## Hypotheses tested and rejected (client)

- Redundant full transcript re-wrap after an identical delta History
  (`e6aaa600e`, reverted in `5e7f9fda7`): live, each client still did exactly one
  full body build before and after. That build is the first render of the
  transcript (CPU-bound, e.g. 5469 wrapped lines = 562ms under storm load,
  106ms idle), not a re-render. Faster first render needs viewport-first or
  incremental wrapping.
- `app_new_for_remote` (`[TIMING] app_new_for_remote`, `8893eff8f`/`4be384241`):
  5-171ms in the 19:17 storm, spread across startup-input apply and the
  startup-stub read under high load; no single structural cost.
- First-frame header build (`[TIMING] header_sections`): 31-251ms at 19:15 but
  nothing over 20ms at 19:17, so load-dependent.

## Local transcript shown late (fixed)

The reload fast-start transcript is rendered off-thread, but it was only
polled from the remote `handle_tick`, and during remote startup the tick is
1s. Ready transcripts sat unapplied for 0.5-0.6s until server History
arrived anyway. `d1f221cf3` polls at the top of every event-loop iteration and
`bd5cdd457` ticks every 10ms while the transcript is pending.

| Storm | `startup_transcript_applied` since app start |
|---|---|
| 19:23 (before) | p50 681ms, max 782ms |
| 19:26 (after) | p50 143ms, max 295ms (all before History) |

## Next: first full body wrap

The frame that first paints the transcript still builds the whole body:
19:26 storm clover 201ms, giraffe 264ms, mosquito 160ms, lizard 669ms
(5469 wrapped lines). A tail-first build (paint the last screenful, then
stitch older messages via the existing suffix-reuse path) would cut this,
but prompt numbering and the delta-History prefix both assume the full
display list, so it needs care.
