# Harness details

## Session subprocess environment

Clients can replace the current session's subprocess environment overrides using
`SetSessionEnv` (or the SDK's `setSessionEnv` / `set_session_env` helper). The map
replaces, rather than merges with, previous overrides. An empty map clears it.
Keys must be nonempty and contain neither `=` nor NUL, values cannot contain NUL,
and `JCODE_*` keys are reserved (case-insensitively).

These overrides apply to session-owned Bash subprocess launches, including queued
background launches. Each accepted job keeps its own snapshot. Updating or
clearing a session's map does not alter already accepted jobs or running
processes. Overrides do not mutate the daemon's process environment. They are
not a general override of provider authentication/configuration, shared MCP
servers, or unrelated daemon services.

Forks, transfers, and swarm children receive independent copies of their parent's
current overrides. Subsequent parent updates do not update existing children.
Clearing conversation history carries overrides to the replacement session ID.
Attaching an existing session uses that target's overrides, never the temporary
connecting session's map.
Session teardown removes the live registry entry, not environment copies already
held by running jobs. Independent sessions do not receive another session's map.

**Overrides are ephemeral and are explicitly lost on daemon restart or reload.**
They are not saved in session JSON, journals, reload recovery, or conversation
history. Loading a saved session does not restore them. Clients must reapply
overrides before submitting resumed work, including after reconnecting to a
restarted daemon. Automatic recovery may run without them: this feature adds no
fail-closed recovery barrier and must not be treated as one.

This is environment isolation, not a security sandbox or secret-redaction system.
The registry does not intentionally log or persist values, but a command can print
its environment into tool output, write it to disk, or transmit it elsewhere.
Subprocesses inherit ordinary daemon environment variables unless overridden,
and OS process inspection or another program running as the same user may expose
secrets. Only supply credentials to trusted commands and trusted child agents.

## Session context ceilings

The harness API supports a persistent compaction ceiling for each session:

```json
{"v":1,"id":42,"req":"set_context_cap","session_id":"session-id","max_context_tokens":600000}
```

The reply reports both the normalized override and the effective budget:

```json
{"v":1,"reply_to":42,"ev":"context_cap_changed","session_id":"session-id","max_context_tokens":600000,"effective_budget":600000}
```

The effective budget is the smaller of the model's context window and the session
ceiling. If no session ceiling is set, `[compaction] max_context_tokens` supplies
the ceiling instead. A global value of `0` means the model window is uncapped.
A session override replaces the global ceiling, so it can raise or lower it, but
can never exceed the model window.

Pass `0` or `null` to clear an override and return to the current global default.
Overrides are isolated between sessions, saved with the session, and restored on
resume after a daemon restart. Older sessions without this field inherit the
global default. Model changes recalculate the budget without discarding the
override.

Changes requested during a running turn wait for the session lock without blocking
the connection's request reader. Lowering the ceiling does not interrupt the
active request or immediately rewrite the transcript. Existing compaction
thresholds apply to subsequent requests. Use `compact` to request compaction
explicitly. The ceiling is a compaction budget, not an exact outbound token limit.

Rust SDK: `client.set_context_cap(session_id, Some(600_000))`.
TypeScript SDK: `await client.setContextCap(sessionId, 600_000)`.
Both return the effective budget. Live `token_usage` events also carry an optional
`effective_budget` from the daemon, allowing clients to render a context meter
without treating the model's full window as the active compaction budget.

## Token optimization

`agentgrep` is jcode's grep tool, built for agents. Matches include file structure, such as functions and their locations, so an agent can understand the surrounding code without reading an entire file. Results are adaptively trimmed based on what the agent has already seen to reduce repeated context.

See the [agentgrep implementation](../crates/jcode-app-core/src/tool/agentgrep.rs).

## KV cache

Provider-side prompt caching lets repeated prompt prefixes reuse work from earlier requests. Jcode monitors provider-reported cache usage and compares request signatures to detect meaningful cache misses. The monitor runs inside the agent, so clients receive the same classified cache-miss events.

Cache reuse depends on the provider and the request. Monitoring makes misses visible rather than guaranteeing that every request will hit the cache.

See the [KV cache monitor](../crates/jcode-base/src/kv_cache_monitor.rs).

## Todo planning hierarchy

Define plans as **phases → lanes → items**, including plans with only one phase or lane:

```json
{
  "phases": [
    {
      "phase": 1,
      "lanes": [
        {
          "lane": "implementation",
          "items": [
            {
              "id": "implement",
              "content": "Implement the change",
              "status": "in_progress",
              "priority": "high",
              "confidence": "validated"
            }
          ]
        }
      ]
    }
  ]
}
```

Phases execute in declared order. Lanes within a phase can run in parallel, while items within a lane run in order. Items inherit phase and lane membership from their enclosing containers. Update existing items by `id` rather than resending the hierarchy. Flat saved todo lists remain compatible internally with storage, swarm coordination, and rendering.

## Completely blocked

No way forward.

`completely_blocked` is a condition on an unfinished todo, not another lifecycle status. Its object contains only a nonempty `reason` string explaining the blocker.

```json
{
  "update": [
    {
      "id": "recover-source",
      "completely_blocked": {
        "reason": "Required source data no longer exists"
      }
    }
  ]
}
```

The TUI labels this condition **✋ Completely blocked** and shows the reason. Keep the todo pending or in progress. Automatic continuation skips that item without scheduling timed review reminders; independent work remains actionable. Clear the condition with `"completely_blocked": null` when a way forward exists, then reassess rather than assuming the task is complete.

## Live worker status in todo lanes

An assigned worker's todo lane shares the opened session's live status renderer: activity spinner, observed input/output tokens, streaming throughput, connection, elapsed time, and current tool intent. Missing telemetry is not guessed from context size or lifetime totals. Waiting and stopped workers show their available reason instead of a live spinner. A completely blocked condition pauses its item without hiding independent work in the same lane.

Passive worker status is displayed with the todo lanes rather than in a second list beside the pinned todo band. Focused worker navigation and transcript controls remain available.

## Hill-climbable measurements

A hill-climbable task has a concrete objective and a repeatable way to measure progress. Instead of stopping at an assertion that the work is done, an agent can make a change, measure the result, and iterate against that feedback.

Jcode's todo assessments record the feedback loop, its coverage, how directly it tests the requested outcome, and the evidence for stopping. This distinguishes an implementation change from a validated user workflow.

See the [todo tool implementation](../crates/jcode-app-core/src/tool/todo.rs).
