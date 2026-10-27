# Harness details

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
