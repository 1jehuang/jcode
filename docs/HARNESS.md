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
