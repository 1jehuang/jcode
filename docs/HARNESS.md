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

## Absolutely requires user action

`requires_user_action` is a condition on an unfinished todo, not another lifecycle status. Use it only when a specific action genuinely requires the user and no safe, authorized agent alternative exists. Examples include approval for an otherwise unauthorized side effect, account access only the user controls, or a necessary decision that cannot safely be inferred. Difficulty, a failed build, or a preference for clarification are not sufficient reasons.

```json
{
  "update": [
    {
      "id": "publish",
      "requires_user_action": {
        "action": "Approve publishing these commits",
        "reason": "Publishing includes unrelated local commits and requires your authorization"
      }
    }
  ]
}
```

The TUI labels this condition **✋ Absolutely requires user action** and shows the concrete action. Both `action` and `reason` must be nonempty. Keep the todo pending or in progress. Automatic continuation skips that item without scheduling timed review reminders; independent work remains actionable. Clear the condition with `"requires_user_action": null` when the user has taken the action, then reassess rather than assuming the task is complete.

## Hill-climbable measurements

A hill-climbable task has a concrete objective and a repeatable way to measure progress. Instead of stopping at an assertion that the work is done, an agent can make a change, measure the result, and iterate against that feedback.

Jcode's todo assessments record the feedback loop, its coverage, how directly it tests the requested outcome, and the evidence for stopping. This distinguishes an implementation change from a validated user workflow.

See the [todo tool implementation](../crates/jcode-app-core/src/tool/todo.rs).
