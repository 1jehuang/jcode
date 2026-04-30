# Crate Ownership and Modularization Boundaries

This document defines the target structure for keeping `jcode` modular without turning shared crates into a dumping ground. It is intentionally practical: use it when deciding whether to move a type, helper, or behavior out of the root crate.

## Goals

Primary goal: make normal development and selfdev builds faster by shrinking the root crate's recompilation surface. Structural cleanliness is valuable because it supports that compile-time goal.

- Move stable DTOs and protocol-safe state into small crates so changes in root behavior do not recompile those contracts, and changes in contracts recompile only focused dependents.
- Keep dependency-light crates dependency-light so they compile quickly and do not pull large runtime/TUI/provider graphs into unrelated builds.
- Keep root-only behavior, storage, process, TUI, server, and provider runtime logic in the root crate until a full dependency boundary can move without increasing dependency fan-out.
- Avoid cyclic dependencies and hidden coupling through broad `jcode-core` re-exports.
- Preserve serde compatibility and root re-exports during migrations unless all call sites are intentionally updated.
- Measure success by compile impact: fewer root edits, fewer root-owned DTOs, smaller dependency fan-out, and faster `cargo check --profile selfdev` / `selfdev build` after common changes.

## Ownership rules

### Type crates own stable data contracts

A `*-types` crate should contain:

- Plain data structures used by multiple crates or protocol layers.
- Serialization shape and small pure helper methods tied to the data contract.
- No filesystem, network, process, TUI, provider client, global state, or storage access.
- Dependencies limited to serde, chrono, and other type crates where necessary.

Examples: `jcode-session-types`, `jcode-side-panel-types`, `jcode-selfdev-types`, `jcode-background-types`.

### Domain behavior modules own root runtime behavior

Root modules should keep behavior when it needs:

- `crate::storage`, `crate::config`, `crate::logging`, `crate::server`, or process spawning.
- Provider HTTP clients and auth managers.
- Tokio runtime, background tasks, channels, global caches, file locks, or PID registries.
- TUI rendering and crossterm/ratatui state.

If a type has inherent methods that need these APIs, either leave the type in root or move behavior and dependencies together into a domain crate. Do not move only the struct if that forces illegal inherent impls in root.

### `jcode-core` is for genuinely shared primitives

`jcode-core` should contain:

- Cross-domain primitives that do not have an obvious domain crate yet.
- Very small, dependency-light helpers used by many crates.
- Temporary DTO staging only when creating a new domain type crate would be premature.

`jcode-core` should not accumulate every extracted DTO indefinitely. Once a cluster grows, split it into a focused domain crate.

### Compile-speed decision rule

Prefer a split when it reduces root crate churn or dependency fan-out. Do not split just to make files look tidier if the new crate adds dependencies, increases rebuild fan-out, or forces frequent cross-crate edits. A good split has at least one of these compile-time benefits:

- Common root behavior edits no longer touch stable type definitions.
- A type-only change can be checked by compiling a small type crate plus focused dependents.
- Heavy dependencies stay out of DTO crates.
- Multiple downstream crates can use a small contract without depending on the root crate.

### Re-export policy

During migrations:

1. Move the type to the target crate.
2. Keep the old root path as `pub use ...` to preserve call sites.
3. Validate focused tests and selfdev build/reload.
4. Later, remove obsolete root re-exports only after downstream crates can depend directly on the domain crate.

## Move checklist

Before moving a type:

- [ ] Does it have inherent methods?
- [ ] Do those methods require root-only APIs?
- [ ] Does its serde representation stay identical?
- [ ] Are all field visibilities still appropriate?
- [ ] Does the target crate already have needed dependencies?
- [ ] Is the target crate still acyclic?
- [ ] Can root keep a compatibility re-export?
- [ ] Is there a focused test filter that covers the moved type?
- [ ] Did `cargo check --profile selfdev` pass for the target crate and root binary?
- [ ] Did selfdev build and reload pass on a clean HEAD?

## Test policy

Prefer focused filters for validation. Broad filters often select unrelated stateful, timing-sensitive, or benchmark tests.

Known broad-filter hazards observed during modularization:

- `side_panel` selects unrelated pinned UI/layout and latency benchmark tests.
- `usage` selects app-display tests in addition to pure usage tests.
- `session::` selects live-attach server tests and picker behavior beyond session persistence.

Document precise filters next to each domain crate/module. Broad filters are still useful for periodic sweeps, but they should not block a DTO-only extraction when precise tests and compile checks pass.

## Current `jcode-core` export audit

| Module | Current contents | Preferred long-term home | Notes |
| --- | --- | --- | --- |
| `ambient_usage_types` | Ambient scheduler usage records/rate limit DTOs | `jcode-ambient-types` or `jcode-ambient-core` | Pure DTOs. Good candidate to split once more ambient types are ready. |
| `catchup_types` | Catch-up persisted state and rendered brief DTOs | `jcode-catchup-types` or stay in core | Small and low churn. Split only if catch-up grows. |
| `copilot_usage_types` | Local Copilot usage counters | `jcode-usage-types` | Belongs with usage report DTOs. |
| `gateway_types` | Paired device and pairing code persisted records | `jcode-gateway-types` | Pairing/token behavior remains root. |
| `goal_types` | Goal state, milestones, status, updates | `jcode-goal-types` or `jcode-task-types` | Larger domain. Worth splitting if goal/tool work grows. |
| `memory_types` | Memory activity DTOs | `jcode-memory-types` | Memory has enough domain weight for its own type crate. |
| `todo_types` | Todo item DTO | `jcode-task-types`, `jcode-todo-types`, or core | Tiny. Could join goal/catchup task-state crate. |
| `usage_types` | Provider usage report DTOs | `jcode-usage-types` | Should join Copilot usage and account usage DTOs. |
| `env` | Environment variable helpers | stay in core | General utility, no domain crate needed. |
| `id` | ID helpers | stay in core | General utility. |
| `panic_util` | Panic formatting helpers | stay in core | General runtime utility. |
| `stdin_detect` | stdin detection helpers | stay in core | General platform/runtime utility. |
| `util` | Misc utilities | audit later | Should not become a catch-all. |

## Target domain type crates

High-value next splits:

1. `jcode-usage-types`
   - `usage_types`
   - `copilot_usage_types`
   - pure account usage DTOs if/when separated from root formatting/runtime helpers

2. `jcode-gateway-types`
   - `gateway_types`
   - possibly `GatewayConfig` after deciding whether config owns it
   - mobile gateway protocol-safe DTOs if needed by mobile crates

3. `jcode-ambient-types`
   - `ambient_usage_types`
   - ambient state/request/result DTOs, but only after root-only `AmbientState::load/save/record_cycle` methods are separated into root free functions or a persistence layer

4. `jcode-memory-types`
   - `memory_types`
   - any memory protocol/activity DTOs used across server/TUI/tools

5. Optional task-state crate
   - `goal_types`
   - `todo_types`
   - `catchup_types` if the product model wants these grouped

## Big module refactor targets

These are not simple DTO moves. Refactor behavior boundaries first.

### `src/session.rs`

Target split:

- metadata/session model
- persistence and journal replay
- startup stubs and remote startup snapshots
- memory profiling/cache attribution
- rendering lives in existing `session/render.rs`
- crash recovery lives in existing `session/crash.rs`

### `src/ambient.rs`

Target split:

- visible cycle context I/O
- state persistence
- directive persistence
- schedule queue and locking
- prompt building
- manager/runtime orchestration

Do not move `AmbientState` as a DTO until load/save/record behavior is separated from the struct.

### `src/usage.rs`

Target split:

- API fetch providers
- provider response parsing
- local caches/sync
- display formatting
- account selection/guidance
- public report DTOs in `jcode-usage-types`

### `src/gateway.rs`

Target split:

- registry persistence
- pairing/token auth
- HTTP route handling
- WebSocket auth/extraction
- WebSocket relay
- public gateway DTOs in `jcode-gateway-types`

## Definition of “optimal enough”

The structure is good enough when:

- Each type crate has a clear domain and minimal dependency set.
- `jcode-core` contains only true primitives or documented temporary staging modules.
- Root modules no longer mix large DTO blocks, persistence, runtime orchestration, and rendering in one file.
- Every domain has focused validation commands.
- Selfdev build/reload works cleanly after every structural change.
