# Swarm spawn issues (5) — ready to file against 1jehuang/jcode

Open each as a separate issue. All are grounded in exact file:line.

**Type summary**
| # | Title | Type | Labels |
|---|-------|------|--------|
| 1 | description advertises `model` but schema strips it | **bug** | `bug`, `swarm`, `good first issue` |
| 2 | context window not auto-discovered: LiteLLM `max_input_tokens` ignored + watchdog blind | **bug** | `bug`, `swarm`, `reliability`, `context-window` |
| 3 | `stop` not idempotent / no batch `teardown` | **bug** (missing idempotent path) + small feature | `bug`, `swarm`, `cleanup` |
| 4 | lane todo named by full prompt, not `label` | **bug** (wrong data in todo name) | `bug`, `swarm`, `todo` |
| 5 | `await_members` no blocking mode / no unified wait | **enhancement** (by-design, add opt-in) | `enhancement`, `swarm` |

---

## Issue 1 — swarm: tool description advertises `model`/`model: "inherit"` but schema strips it; error does not name the config keys

**Type:** bug
**Labels:** `bug`, `swarm`, `good first issue`

**Problem**
Spawning a swarm worker with `model: "inherit"` (or any `model` value) fails with:
> Agent-selected swarm models are disabled. Omit model to use the user-configured default.

The swarm tool's **description** documents `model` and `model: "inherit"` as valid parameters, but the tool **schema** strips the `model` property by default. The agent is told one thing by the prose and a different thing by the schema, passes the parameter anyway, and gets a generic error that does not name the config keys to change.

**Code**
- `crates/jcode-app-core/src/tool/swarm_model_schema.rs:10` — `apply_swarm_model_policy` does `properties.remove("model")` when `!agents.swarm_model_selection || agents.swarm_model_allowlist.is_empty()`.
- `crates/jcode-config-types/src/lib.rs:680-684` — `validate_swarm_model_request` returns the error when `!swarm_model_selection`.
- `crates/jcode-base/src/config/default_file.rs:609-613` — defaults are `swarm_model_selection = false`, `swarm_model_allowlist = []`.

**Repro**
1. Default config (`swarm_model_selection = false`).
2. `swarm spawn` with `model: "inherit"` (or any model).
3. All 5 spawns in a batch fail with the generic error.
4. Re-spawning without `model` succeeds.

**Suggested fix**
1. Make the swarm tool description for `model`/`model: "inherit"` conditional on the same policy as `apply_swarm_model_policy`. When selection is off, say workers inherit the coordinator's model and overrides require `agents.swarm_model_selection = true` plus an `agents.swarm_model_allowlist` entry.
2. Name the config keys in the error: `...disabled. Set agents.swarm_model_selection = true and add the exact route to agents.swarm_model_allowlist, or omit model.`

**Acceptance**
- With selection off, schema and description both omit/hide `model` (no prose/schema contradiction).
- The error, if still reachable, names the exact config keys.
- Regression test: default config → schema has no `model` property AND description makes no unconditional claim about `model: "inherit"`.

---

## Issue 2 — swarm: context window not auto-discovered — LiteLLM `max_input_tokens` ignored, worker budget miscomputed, watchdog blind

**Type:** bug
**Labels:** `bug`, `swarm`, `reliability`, `context-window`

**Problem**
Two compounding defects cause swarm workers running through LiteLLM (or any OpenAI-compatible proxy) to receive an **incorrect context budget** and to **stall undetected** during long inferences:

### Defect 2a: `max_input_tokens` field not parsed from `/v1/models`

LiteLLM's `/v1/models` endpoint **does** report the model's context window via `max_input_tokens`:

```json
{
  "id": "deepseek-reasoner",
  "object": "model",
  "owned_by": "openai",
  "mode": "chat",
  "max_input_tokens": 262144,
  "max_output_tokens": 262144
}
```

But Jcode's catalog parser only looks for `context_window` and `context_length`:

```rust
// crates/jcode-base/src/provider/models_catalog.rs:150-156
if let Some(ctx) = model
    .get("context_window")
    .or_else(|| model.get("context_length"))
    .and_then(|c| c.as_u64())
{
    limits.insert(slug.clone(), ctx as usize);
}
// ↑ max_input_tokens is NEVER checked → limits stays empty for LiteLLM models
```

Because the catalog is empty, `Provider::context_window()` falls back to `context_limit_for_model_with_provider` (the **static** table in `jcode-provider-core/src/models.rs:533`), which:
- Has `deepseek-v4` → 1M, `deepseek-flash` → 1M
- Does **NOT** have `deepseek-reasoner` → `None` → **200k default**

The actual model window is **1M** (confirmed via the running deployment). The worker compacts at 200k threshold instead of 1M, wastes tokens on premature compaction, and can hit the real model limit before Jcode thinks it should compact.

**Repro (2a)**
1. Configure LiteLLM at `http://localhost:4000/v1` with `deepseek-reasoner` (window 1M).
2. `curl -s http://localhost:4000/v1/models` → shows `max_input_tokens: 1048576`.
3. Spawn a swarm worker with `model: "deepseek-reasoner"`.
4. Worker's `context_window()` returns **200000** (static fallback), not 1048576.
5. Compaction triggers at ~160k instead of ~838k.

**Fix 2a:**
```rust
// models_catalog.rs:150 — add the third field
if let Some(ctx) = model
    .get("context_window")
    .or_else(|| model.get("context_length"))
    .or_else(|| model.get("max_input_tokens"))  // ← LiteLLM / OpenAI-proxy standard
    .and_then(|c| c.as_u64())
{
    limits.insert(slug.clone(), ctx as usize);
}
```

### Defect 2b: `Provider::context_window()` bypasses the catalog snapshot

Even after Fix 2a, the catalog data reaches `cached_context_limit`, but `Provider::context_window()` does **not** call it:

```rust
// crates/jcode-provider-core/src/lib.rs:506-509
fn context_window(&self) -> usize {
    context_limit_for_model_with_provider(&self.model(), Some(self.name()))
        .unwrap_or(DEFAULT_CONTEXT_LIMIT)
    // ↑ calls the static-table variant, NOT the cached variant
}
```

Whereas `context_limit_for_model_with_provider_and_cache` (models.rs:557) **does** check `cached_context_limit` first — that's the layer where catalog data would be used. But the `Provider` trait's `context_window()` never routes through it.

**Fix 2b:** Change `Provider::context_window()` to use the cached variant, threading the catalog snapshot through. Requires either a module-level dependency from `jcode-provider-core` → `jcode-base`, or a trait method to inject the cache lookup.

### Defect 2c: stall watchdog blind to long single inference

A swarm worker can sit in `running` for 10+ minutes in a single long "thinking"/first-token-wait step. The stall watchdog exists but does not catch it:

- `crates/jcode-app-core/src/server/swarm_watchdog.rs:49` — `stall_after()` reads `agents.swarm_stall_after_secs`.
- `crates/jcode-app-core/src/server/swarm_watchdog.rs:85-95` — `evaluate_stall` only flags a worker when `silent >= stall_after`, where `silent` is derived from `activity_age_secs`.
- `crates/jcode-app-core/src/agent/turn_streaming_mpsc.rs:388-390` — every provider stream event (throttled to every 2s) calls `session_metrics::record_activity(...)`, so a single long inference that is still streaming keeps the activity clock ticking.
- `crates/jcode-base/src/config/default_file.rs:650-651` — defaults: `swarm_stall_after_secs = 600`, `swarm_stall_nudge = false`.
- `crates/jcode-app-core/src/server/state.rs:924` — `queue_soft_interrupt_for_session` is a **soft** interrupt that queues for the next turn boundary; it does not cancel an in-flight inference step.

**Why the watchdog is blind**
A worker in a single long model call with no tool call and no new message boundary is still "active" by the current definition (it has recent streamed tokens or is mid-prefill). `evaluate_stall` therefore never fires, and even if it did, `swarm_stall_nudge` defaults to `false`, so the worker is only *flagged in status* with a one-shot coordinator notification — no nudge, no auto-retry, no auto-stop.

**Repro (2c)**
1. Spawn a worker on a very large file/crate (e.g. a 290k LOC TUI crate).
2. The worker enters a single long inference step (token count frozen or crawling, same file).
3. 10-12 minutes pass. `swarm list` shows `running`, no stall flag (or a late flag the coordinator misses while polling).
4. `swarm message` with `delivery=interrupt` does not preempt the in-flight step.
5. Only `swarm stop` (force) recovers.

**Fix 2c:**
1. Add a secondary stall criterion: turn is `running` but no *meaningful* progress (tool call, message boundary, thinking→output transition) for `N` seconds → stalled. Do not rely on raw streaming bytes alone.
2. Default `swarm_stall_nudge = true` (or auto-nudge when the coordinator is awake).
3. Optional hard cap: `agents.swarm_stall_max_secs` (e.g. 30 min) → auto-`retry`/`stop` a worker past that threshold. Today the watchdog only *notifies*; it never acts.

**Acceptance (all three)**
- 2a: `deepseek-reasoner` spawned via LiteLLM reports `context_window() = 1048576` (not 200000).
- 2b: A model with a known catalog `max_input_tokens` but **no** static table entry still gets the correct window.
- 2c: A worker in a single 15-min inference with no tool call is flagged stalled well before 30 min.
- `swarm_stall_nudge = true` (default) delivers a nudge to the stalled worker.
- `swarm stop` on a running worker cancels the in-flight inference and returns the final state without an error.
- No false stalls: a worker actively calling tools every < N seconds is not marked stalled.
- Regression test: mock `/v1/models` returning `{"max_input_tokens": 1048576}` → `cached_context_limit("test-model")` returns `Some(1048576)`.

---

## Issue 3 — swarm: `stop` is not idempotent and there is no batch `teardown`

**Type:** bug (the non-idempotent `stop` is a bug; batch `teardown` is a small feature riding along)
**Labels:** `bug`, `swarm`, `cleanup`

**Problem**
After workers finish and become `ready`, calling `swarm stop` by friendly name (or even full session ID) can return `Unknown swarm session` if the member record was already removed by the idle reaper or the `swarm_id` no longer matches. There is no "already gone = success" path, and no one-shot way to clean up all idle lanes in a swarm.

**Code**
- `crates/jcode-app-core/src/server/comm_session.rs:1063-1116` — `resolve_stop_target_session` resolves only against **live** `swarm_members`; `matches.len() == 0` → `Err("Unknown swarm session ...")`. It does not check terminal/removed members.
- `crates/jcode-app-core/src/server/swarm.rs:340` — `DEFAULT_SWARM_IDLE_WORKER_REAP_SECS = 30 * 60`.
- `crates/jcode-app-core/src/server/swarm.rs:355-367` — `idle_spawned_worker_reap_candidates` selects spawned workers whose status is `ready`/terminal and whose `last_status_change` is past `idle_after`; the reaper (`server.rs:299-387`) closes their client and **removes the member record**.

**Repro**
1. Spawn workers, let them report and go `ready`.
2. After a few minutes (or after the 30-min reap window), call `swarm stop` with the friendly name.
3. If the member was reaped or the `swarm_id` mismatched, you get `Unknown swarm session '<name>'`.
4. There is no `swarm teardown` / `swarm cleanup` action to stop all idle lanes in one call.

**Suggested fix**
1. Make `resolve_stop_target_session` idempotent: if the target matches a member with a terminal/removed status (`stopped`, `closed`, `crashed`, `failed`), return `Ok` (no-op) instead of `Err`. Distinguish "never existed" from "already gone" in the error/success message.
2. Add a `swarm teardown` (or `swarm cleanup`) action: stop all idle spawned lanes in the coordinator's swarm in one call; report per-lane "stopped" / "already gone"; overall `Ok`.

**Acceptance**
- `swarm stop` on a worker that is already `stopped`/reaped returns success (idempotent no-op), not an error.
- `swarm stop` on a name that never existed still errors, with a distinct message.
- `swarm teardown` stops all idle lanes in the swarm in one call and reports per-lane outcomes.

---

## Issue 4 — swarm: lane todo name uses the full spawn prompt (first 60 chars) instead of the short `label`

**Type:** bug
**Labels:** `bug`, `swarm`, `todo`

**Problem**
When spawning a worker, the coordinator's lane todo is named with the **first 60 characters of the spawn prompt** (the full message), not the short `label`. Long prompts become unwieldy todo names that pollute the coordinator's plan and break `todo update` by short name (the plan already contains a todo named with the prompt text, so a different short name appears as a "duplicate todo name" error).

**Code**
- `crates/jcode-app-core/src/tool/communicate.rs:1270-1283` — on `spawn`, `title` is computed as `prompt.lines().find(non-empty).chars().take(60)`; `label` is only used as a fallback when `title` is empty. `ensure_spawn_lane(&todos, &label, phase, &title, prompt, ...)` creates the lane with this long title as the todo name.

**Repro**
1. `swarm spawn` with `label: "providers-analyst"` and a long `initial_message` starting with "You are a codebase analyst. The repo root is /Users/mac/jcode ...".
2. The coordinator's todo plan now contains a todo named "You are a codebase analyst. The repo root is /Users/mac/jcode" (first 60 chars).
3. Trying `todo update` with the short name "providers-analyst" fails with "duplicate todo name" because the existing todo is named with the prompt text.

**Suggested fix**
1. Use `label` as the lane todo **name** (not the prompt). Keep the full prompt as the todo **description**/task body.
2. Concretely in `communicate.rs:1273-1277`: set `title = label`, and pass the prompt as the description. The `label` field already exists and is required for spawn.

**Acceptance**
- After `swarm spawn`, the coordinator's lane todo is named with the short `label` (≤ ~40 chars), not the prompt text.
- The full spawn prompt is available as the todo's description/task body.
- `todo update` by the short `label` name works without "duplicate todo name" errors.

---

## Issue 5 — swarm: `await_members` has no blocking mode and swarm dependency IDs are not accepted by `bg wait`

**Type:** enhancement (the non-blocking wake behavior is by design and works; this is adding an opt-in blocking/unified wait)
**Labels:** `enhancement`, `swarm`

**Problem**
`swarm await_members` is non-blocking: it registers a dependency and returns immediately with "Watching ... in the background. You'll be woken." The wake mechanism works, but there is no synchronous join, and the dependency ID returned is not accepted by `bg wait` (which only knows background tasks). A coordinator with nothing else to do ends up in a poll loop (`swarm list` / repeated `await_members`), and the two "waitable" subsystems (swarm deps vs. bg tasks) do not share a common handle.

**Code**
- `crates/jcode-app-core/src/server/background_tasks.rs:259-343` — `dispatch_swarm_await_completion` delivers a soft-interrupt + wake when the condition is met; it is inherently non-blocking.
- Test: `crates/jcode-app-core/src/tool/communicate_tests/end_to_end.rs:135-242` — `communicate_await_members_background_returns_immediately_and_notifies`.
- `bg wait` (the background-task subsystem) does not resolve swarm dependency IDs; it returns "Task not found" for them.

**Repro**
1. `swarm await_members` with `mode=all`.
2. It returns immediately with a dependency ID.
3. `bg wait` with that dependency ID → "Task not found".
4. The coordinator must either poll `swarm list` or wait for the wake; there is no blocking alternative.

**Suggested fix (low priority — the wake mechanism works)**
1. Add a `wait=true` (or `blocking`) option to `swarm await_members` that blocks until the condition is met or the timeout elapses.
2. Or introduce a unified "waitable handle" type that both `bg wait` and `swarm await_members` accept, so a coordinator can `wait` on either with one call.

**Acceptance**
- `swarm await_members` with `wait=true` blocks and returns the result when the condition is met (or on timeout).
- A single `wait` call accepts both background-task IDs and swarm dependency IDs (unified handle).
- Default (non-blocking) behavior is unchanged.
