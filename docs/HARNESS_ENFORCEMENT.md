# Harness execution enforcement

Feature flag `features.harness_enforcement` (default `true`, env `JCODE_HARNESS_ENFORCEMENT=0` to disable). Code: `crates/jcode-app-core/src/harness.rs`, agent glue in `agent/harness_glue.rs`.

## Outer layer (agent loop, both `run_turn` and `run_turn_streaming_mpsc`)

- **Pre-turn router.** Before the first request of a user turn, the prompt is matched (word boundaries) against every skill's `triggers`/`tags` and exact `name`. Up to 3 matches are activated in session state. Descriptions are not matched: with hundreds of skills installed they would route nearly every prompt.
- **Ephemeral policy.** Every request gets exactly one `<harness_execution_policy>` user message after any memory or plan-limit message: the ACTION OVER MEMORY invariant, active skills, bound tools, and up to 8 `## HARD CONSTRAINTS` bullets. It goes through the same request-only path as memory injection and is never saved to history.
- **Turn-completion gate.** A text-only stop on an actionable turn (not small talk, 3+ words) with zero tool calls this turn is intercepted. The unverified assistant message is dropped from history, `[HARNESS INTERVENTION]: ...` is appended, and the loop continues. Capped at 2 per user turn.
- **Telemetry.** On turn end the log gets `[harness] {"turn_id","auto_routed_skills","tools_called","gate_blocks","stop_interventions",...}`.

## Inner layer (`Registry::execute`, so batch and codemode subcalls are covered)

- Runs after the user `pre_tool` hook. Gates `codemode`, `mcp`, `mcp_call`, `mcp_search` and `mcp__<server>__<tool>` the same way.
- `codemode` accepts `phase` (`discover_bridge | inspect_schema | execute_query`) and `target`. MCP calls infer phase from the action and tool name (`*schema*`, `list_tables`, `describe` count as inspection, `*sql*`/`*query*` count as queries), and `target` defaults to the server.
- `execute_query` on a target that was never inspected returns `BLOCKED_BY_HARNESS: Target schema/state not yet verified ...`. Phase/payload mismatches (an unknown phase, a missing target, `discover_bridge` that calls a tool, `execute_query` that only searches) are blocked too.

## Skills

- `skill_manage load` is idempotent: a second load returns `Skill '<name>' is already active; proceed directly with execution.` After a compaction the next load resends the body.
- Activation, whether automatic or manual, counts for the whole session. A skill loaded before the work started is never re-required.
- `required_tools` are listed in the policy block. Deferred ones (rarely used built-ins and `mcp__*`) are bound by a `tool_reference`, so they are callable without `load_tools` or an MCP search.

### SKILL.md layout

```markdown
---
name: crm-audit
description: Audit CRM contact data
triggers: [crm, contacts]        # or tags:, comma string also accepted
required_tools: [codemode, mcp]  # alias: required-tools
---
# Examples and procedure (verbose, top)
...

## HARD CONSTRAINTS
- Short imperative rules, one per bullet. Must be the last section.
```

Tests: `cargo test --test harness` (agent-level traces) and `cargo test -p jcode-app-core harness::` (unit).
