# Modular Architecture RFC

Status: Draft

This RFC describes a modular target architecture for jcode that matches the current codebase, preserves the existing product model, and gives us a safe migration path from today's mostly-monolithic root crate to a layered workspace.

It is intentionally aligned with:

- [`REFACTORING.md`](./REFACTORING.md)
- [`COMPILE_PERFORMANCE_PLAN.md`](./COMPILE_PERFORMANCE_PLAN.md)
- [`SERVER_ARCHITECTURE.md`](./SERVER_ARCHITECTURE.md)
- [`MULTI_SESSION_CLIENT_ARCHITECTURE.md`](./MULTI_SESSION_CLIENT_ARCHITECTURE.md)

## Goals

- Document the architecture that exists today, not an idealized version.
- Define a target layered and crate architecture that improves maintainability and compile times.
- Establish dependency rules that prevent the workspace from collapsing back into a monolith.
- Provide a phased migration plan that fits the refactoring roadmap and compile-performance plan.
- Preserve runtime behavior: one shared server, reconnecting clients, session-local self-dev capability, and stable tool/provider flows.

## Non-Goals

- A big-bang rewrite.
- Renaming every module or crate immediately.
- Forcing every subsystem into a separate crate before its boundaries are ready.
- Changing the core product architecture from single-server, multi-client.

## Executive Summary

Today, jcode is best described as a **modular monolith with a growing workspace shell**:

- The root `jcode` crate still owns most runtime orchestration and product behavior.
- Several heavy or relatively self-contained subsystems have already moved into workspace crates.
- The codebase has strong module-level separation in some areas, but several broad root modules still act as architectural chokepoints.

The target architecture is a **layered workspace**:

1. **Foundation layer** for stable shared types and runtime primitives.
2. **Domain/runtime layer** for session, agent, provider, and server logic.
3. **Interface layer** for CLI, TUI, self-dev, and optional heavy integrations.
4. **Composition layer** where the top-level `jcode` package wires the product together.

The most important design rule is this:

> High-churn orchestration code must depend on stable lower layers, while stable lower layers must never depend back on runtime/UI/product-specific code.

That rule serves both architecture quality and compile-speed goals.

## Current Architecture

### Current runtime model

At the product level, the runtime architecture is already clear:

- `jcode` is a **single-server, multi-client** application.
- The server owns sessions, swarm state, background tasks, provider state, and shared services.
- Clients are primarily TUI frontends that attach to server-owned sessions.
- Self-dev is session-local capability on the shared server, not a separate architecture.

That model should stay intact.

### Current code organization

The current code organization is mixed:

- **Root crate `jcode`** still contains most product logic.
- **Workspace crates** already isolate several heavy or stable seams.
- **Subdirectories under `src/`** increasingly reflect domain boundaries, especially for `agent`, `cli`, `server`, `tool`, and `tui`.

Current workspace members from `Cargo.toml`:

- root package: `jcode`
- `crates/jcode-agent-runtime`
- `crates/jcode-embedding`
- `crates/jcode-pdf`
- `crates/jcode-azure-auth`
- `crates/jcode-notify-email`
- `crates/jcode-provider-metadata`
- `crates/jcode-provider-core`
- `crates/jcode-provider-openrouter`
- `crates/jcode-provider-gemini`
- `crates/jcode-tui-workspace`

### What the root crate still owns

The root crate still directly owns most of the following concerns:

- CLI parsing and dispatch
- server orchestration and socket lifecycle
- session state and persistence
- agent turn execution and tool orchestration
- provider traits and several provider implementations
- protocol/message/config types
- tool registry and many tool implementations
- TUI application state and rendering
- auth, memory, safety, ambient mode, and product glue

This is why the root crate is still the primary compile and architecture hotspot.

### Existing extracted workspace seams

These splits already exist and should be treated as real architectural footholds, not temporary accidents:

| Crate | Current role |
|---|---|
| `jcode-agent-runtime` | shared interrupt and lightweight runtime primitives for agent execution |
| `jcode-embedding` | ONNX/tokenizer-based embedding implementation and heavy inference deps |
| `jcode-pdf` | PDF text extraction |
| `jcode-azure-auth` | Azure bearer token retrieval |
| `jcode-notify-email` | SMTP/IMAP/mail transport |
| `jcode-provider-metadata` | provider/login catalog and profile metadata |
| `jcode-provider-core` | shared provider value types, route/cost types, shared HTTP client, schema helpers |
| `jcode-provider-openrouter` | OpenRouter-specific catalog/cache/support helpers |
| `jcode-provider-gemini` | Gemini schema/model/support helpers |
| `jcode-tui-workspace` | workspace-map data/model/widget rendering |

These are already aligned with the compile-performance plan's strategy: isolate heavy dependencies and stable helper surfaces first.

### Current chokepoints

The root crate still has several broad, high-fanout modules that make both maintenance and incremental compilation harder. Current sizes observed from the tree:

- `src/server.rs`: ~1731 lines
- `src/provider/mod.rs`: ~2283 lines
- `src/session.rs`: ~2730 lines
- `src/protocol.rs`: ~1198 lines
- `src/main.rs`: ~55 lines

This supports the current plan direction:

- CLI decomposition is already mostly underway and should continue.
- Server, provider, session, and TUI state boundaries remain the most important structural work.
- The top-level binary entrypoint is already close to the desired thin composition shape.

### Current architecture in one picture

```mermaid
flowchart TD
  J[jcode root crate]

  J --> CLI[CLI and startup]
  J --> Server[Server orchestration]
  J --> Session[Session and persistence]
  J --> Agent[Agent turn loop and tools]
  J --> Provider[Provider trait and runtime impls]
  J --> TUI[TUI app and rendering]
  J --> Coreish[Protocol, message, config, ids]
  J --> Product[Auth, memory, safety, ambient, notifications]

  J --> AR[jcode-agent-runtime]
  J --> Emb[jcode-embedding]
  J --> PDF[jcode-pdf]
  J --> Azure[jcode-azure-auth]
  J --> Mail[jcode-notify-email]
  J --> PMeta[jcode-provider-metadata]
  J --> PCore[jcode-provider-core]
  J --> POR[jcode-provider-openrouter]
  J --> PGem[jcode-provider-gemini]
  J --> TW[jcode-tui-workspace]
```

## Architectural Problems To Solve

### 1. The root crate is still the product and the platform

Today the root crate acts as all of the following at once:

- domain model holder
- runtime orchestrator
- UI host
- provider abstraction layer
- integration shell
- compile boundary for unrelated edits

That makes it hard to reason about ownership and easy to create accidental coupling.

### 2. Stable types and high-churn orchestration still live together

Broadly reused types like protocol structures, message forms, IDs, route metadata, and config types should be more stable than server, TUI, or provider orchestration logic. Today many of these still live in the same crate and sometimes in the same dependency fanout path.

### 3. Some boundary slices exist, but the center remains too wide

The existing workspace crates are good first splits, but they mostly isolate leaves. The center of gravity is still inside the root crate, especially around:

- session state
- provider trait and runtime behavior
- server lifecycle
- tool registry wiring
- TUI app state and reducers

### 4. Compile-speed and architecture incentives are the same problem

The compile-performance plan is correct that crate boundaries matter most. The same boundaries that reduce invalidation pressure also improve ownership and testability.

## Target Architecture

### Layered model

The target is a layered workspace with a thin composition root. Arrows below mean
"depends on".

```mermaid
flowchart TD
  App[jcode top-level package]

  subgraph L2[Layer 2: interfaces and product surfaces]
    TUI[jcode-tui]
    SelfDev[jcode-selfdev]
    CLI[jcode-cli or root CLI modules]
  end

  subgraph L1[Layer 1: domain/runtime]
    Server[jcode-server]
    Agent[jcode-agent]
    Provider[jcode-provider]
    Session[jcode-session]
  end

  subgraph L0[Layer 0: foundation and support]
    Core[jcode-core]
    AR[jcode-agent-runtime]
    Emb[jcode-embedding]
    PDF[jcode-pdf]
    Azure[jcode-azure-auth]
    Mail[jcode-notify-email]
    PMeta[jcode-provider-metadata]
    PCore[jcode-provider-core]
    POR[jcode-provider-openrouter]
    PGem[jcode-provider-gemini]
    TW[jcode-tui-workspace]
  end

  App --> Server
  App --> TUI
  App --> SelfDev
  App --> CLI

  CLI --> Server
  CLI --> TUI
  CLI --> Core

  TUI --> Core
  TUI --> TW

  SelfDev --> Server
  SelfDev --> Core

  Server --> Agent
  Server --> Provider
  Server --> Session
  Server --> Core
  Server --> Mail

  Agent --> Provider
  Agent --> Session
  Agent --> Core
  Agent --> AR

  Provider --> Core
  Provider --> PCore
  Provider --> PMeta
  Provider --> POR
  Provider --> PGem
  Provider --> Azure

  Session --> Core
  Session --> Emb
  Session --> PDF
```

The exact crate names can evolve, but the dependency direction should not.

## Target crate responsibilities

### `jcode-core`

Purpose: stable shared types and utilities with minimal dependencies.

Should contain:

- IDs and naming primitives
- protocol DTOs that are not server-implementation-specific
- message/content/tool-definition types shared across runtime layers
- config primitives and enums that do not require runtime services
- small shared utility types with high reuse

Should not contain:

- TUI code
- server lifecycle code
- provider network code
- tokio task orchestration unless truly unavoidable
- product-specific wiring

Notes:

- This is the most important future extraction because it enables the rest.
- `src/protocol.rs`, `src/id.rs`, and carefully selected parts of `config.rs` and `message.rs` are the likely first feeders.

### `jcode-session`

Purpose: session domain model, persistence, and state transitions.

Should contain:

- session model and persisted metadata
- session storage/loading/snapshot logic
- reducer-like state transitions for session-owned data
- memory extraction hooks that are session-domain concerns

Should not contain:

- socket handling
- TUI state
- provider HTTP details
- direct server daemon lifecycle logic

Notes:

- This crate is not explicitly named in the current compile-performance plan, but the current size and fanout of `src/session.rs` make session extraction a natural stabilizing move.
- If introducing `jcode-session` feels too early, the same boundary should still be established internally first and extracted later.

### `jcode-provider`

Purpose: provider contracts and runtime-facing provider orchestration.

Should contain:

- the `Provider` trait once it depends only on lower-layer types
- provider routing abstractions
- runtime-facing provider composition
- shared streaming abstractions for provider results

Should not contain:

- provider-specific heavy catalogs and schema helpers that already live well in leaf crates
- server or TUI logic

Notes:

- Existing crates `jcode-provider-core`, `jcode-provider-metadata`, `jcode-provider-openrouter`, and `jcode-provider-gemini` remain useful under this layer.
- The key migration step is shrinking the `Provider` trait's dependency surface so it no longer depends on root-crate-only message/runtime types.

### `jcode-agent`

Purpose: agent turn engine and tool orchestration.

Should contain:

- turn-loop engine
- stream handling and response recovery
- tool execution orchestration
- compaction integration
- prompt assembly inputs that are agent-domain concerns

Should not contain:

- server socket lifecycle
- TUI state
- provider-specific leaf implementations

Notes:

- This aligns directly with the refactoring roadmap's "Agent Turn-Loop Unification" phase.
- `jcode-agent-runtime` remains the low-level runtime primitive crate below it.

### `jcode-server`

Purpose: daemon lifecycle and multi-client coordination.

Should contain:

- socket listeners and debug socket handling
- client attach/detach lifecycle
- swarm coordination
- reload/update server behaviors
- server-owned registries and shared service wiring

Should not contain:

- TUI rendering
- provider implementation details beyond service interfaces
- session persistence internals that belong in `jcode-session`

Notes:

- The current `src/server/` submodule tree is already the right shape for this extraction.
- `src/server.rs` should continue shrinking into a facade/composition module.

### `jcode-tui`

Purpose: client UI state, reducers, and rendering.

Should contain:

- app state and reducers
- remote client behavior and reconnect logic
- renderer/widget orchestration
- TUI-specific command/input handling

Should not contain:

- server daemon code
- session persistence internals
- provider network logic

Notes:

- This aligns directly with the refactoring roadmap's "TUI State/Reducer Split" phase.
- `jcode-tui-workspace` can remain a leaf crate or become a child dependency of `jcode-tui`.

### `jcode-selfdev`

Purpose: self-dev workflows, customization records, reload/build productization.

Should contain:

- self-dev state and tooling policy
- build/reload orchestration specific to self-dev workflows
- customization record and migration logic as it lands

Should not contain:

- generic server lifecycle not specific to self-dev
- general TUI rendering

Notes:

- This aligns with the compile-performance plan's issue-#32 direction and with the already-unified shared-server model.

### `jcode` top-level package

Purpose: composition root and shipping product package.

Should eventually be responsible for:

- binary entrypoints
- feature/default selection
- wiring the runtime graph together
- packaging and product defaults

It should not remain the long-term home of most implementation logic.

## Dependency Rules

These rules are the core of the RFC.

### Rule 1: Dependencies flow downward only

A higher layer may depend on a lower layer. A lower layer may not depend on a higher layer.

- foundation cannot depend on domain/runtime, interfaces, or product crates
- domain/runtime cannot depend on TUI or self-dev UI/product layers
- leaf adapters must not pull UI or server concerns downward

### Rule 2: No TUI types below the interface layer

- `ratatui`, `crossterm`, renderer state, viewport state, widget models, and clipboard/image/UI helper types must stay out of server, agent, provider, and core crates
- server-to-client data crosses the boundary via protocol/event types, not TUI structs

### Rule 3: No server daemon types in core or provider-support crates

- socket/session attachment state, fanout senders, debug socket helpers, and daemon lifecycle code must not appear in `jcode-core`, `jcode-provider-core`, or provider leaf crates

### Rule 4: Provider implementation crates depend on contracts, not on the server or TUI

- provider leaf crates may depend on `jcode-core`, `jcode-provider`, and `jcode-provider-core`
- they must not depend on `jcode-server` or `jcode-tui`

### Rule 5: Async/network-heavy dependencies do not belong in `jcode-core`

`jcode-core` should stay cheap to compile and highly reusable.

Avoid putting these there unless absolutely necessary:

- `reqwest`
- provider SDKs
- UI crates
- ONNX/tokenizer stacks
- mail/PDF dependencies

### Rule 6: Stable contracts should change more slowly than orchestration

Before extracting a crate, first shrink and stabilize its public surface.

Examples:

- move pure data types before moving stateful runtime code
- move pure helper functions before moving integration shells
- keep facades in the root crate during transitions if they reduce churn

### Rule 7: Avoid cross-cutting "utils" crates

Do not create a dumping-ground crate.

If code has a clear owner, it belongs with that owner:

- protocol/data types -> `jcode-core`
- session persistence -> `jcode-session`
- provider route/schema helpers -> provider crates
- rendering helpers -> `jcode-tui`

### Rule 8: The root package may compose many crates, but peer crates should stay narrow

The top-level `jcode` package can wire multiple domains together. Peer crates should not casually depend on each other sideways when a lower-level contract would do.

### Rule 9: New crate boundaries should follow both ownership and invalidation logic

A crate split is worth doing when it improves at least one of these substantially, and ideally both:

- clearer ownership and testability
- lower compile invalidation for common edits

### Rule 10: Preserve behavior with facades during migration

During migration, it is acceptable for the root crate to keep temporary facade modules that re-export or forward into extracted crates. That is preferable to risky behavior changes.

## Recommended Target Mapping From Today's Code

This is the recommended direction from the current tree, not a one-shot move list.

| Current area | Likely target |
|---|---|
| `src/id.rs`, protocol/message/config primitives | `jcode-core` |
| `src/session.rs`, parts of `storage`, restart snapshot concerns | `jcode-session` |
| `src/agent/*`, parts of `compaction`, tool orchestration seams | `jcode-agent` |
| `src/server/` + shrinking `src/server.rs` facade | `jcode-server` |
| `src/provider/mod.rs` trait/contracts plus provider composition seams | `jcode-provider` |
| existing provider helper crates | remain leaf/provider support crates |
| `src/tui/*` + `jcode-tui-workspace` | `jcode-tui` + leaf workspace widget crate |
| `src/cli/*` | stay in root initially or become `jcode-cli` later if justified |
| `src/tool/selfdev/*`, self-dev workflow/productization | `jcode-selfdev` |

## Phased Migration Plan

This migration is intentionally incremental and aligned with existing docs.

### Phase 0: Codify the architecture now

Deliverables:

- this RFC
- cross-links from refactoring and compile-performance docs
- dependency rules documented before more splits land

Why now:

- the repo already has enough workspace structure that undocumented drift is becoming more expensive

### Phase 1: Finish internal module decomposition in the root crate

Aligns with `REFACTORING.md` phases 2 through 6.

Focus areas:

- continue CLI decomposition until `main()` stays parse + runtime bootstrap only
- continue shrinking `src/server.rs` into a thin facade over `src/server/*`
- unify agent turn-loop variants behind one engine
- continue TUI state/reducer separation
- continue provider state isolation and pure helper extraction

Exit criteria:

- root modules are organized by ownership, not by convenience
- candidate extraction seams are obvious and lower-risk

### Phase 2: Extract `jcode-core`

This is the highest-leverage shared boundary.

First moves should be narrow and stable:

- IDs
- small protocol DTOs
- tool definition and message content forms that are broadly shared
- config enums/primitives that do not need runtime services

Avoid moving unstable orchestration APIs too early.

Exit criteria:

- server, agent, provider, and TUI code can all depend on the same lower-level shared types without depending on the root crate

### Phase 3: Extract runtime/domain crates

Primary targets:

1. `jcode-provider`
2. `jcode-agent`
3. `jcode-server`
4. `jcode-session`

Recommended order:

- start with whichever boundary is already most internally modular after Phase 1
- in practice, provider and server look like the strongest current candidates because they already have meaningful submodule trees and leaf support crates
- session may remain internal slightly longer if its public surface is still too entangled

Exit criteria:

- the root crate no longer defines the main provider, server, and agent contracts directly

### Phase 4: Extract `jcode-tui`

Focus:

- move client app/reducer/rendering code out of the root crate once protocol and runtime service boundaries are stable
- keep server events and client view-state concerns separated by protocol types

This phase should happen after enough shared contract extraction exists to avoid TUI depending back on root implementation details.

Exit criteria:

- TUI can evolve rapidly without dragging broad server/provider recompilation

### Phase 5: Extract `jcode-selfdev`

Focus:

- isolate self-dev workflow code and future customization/productization work
- keep shared-server runtime behavior intact
- move issue-#32 style no-rebuild customization logic here when it becomes concrete

Exit criteria:

- self-dev product behavior is explicit and no longer scattered across server/CLI/tool glue

### Phase 6: Shrink the root package into a composition shell

Desired end state:

- `src/main.rs` remains thin
- `jcode::run()` is mostly wiring
- the top-level package primarily assembles runtime services and default product configuration

### Continuous work across all phases

These should continue throughout the migration:

- keep carving heavy leaf dependencies into workspace crates where boundaries are safe
- measure touched-file compile timings after structural changes
- protect behavior with facades, tests, and refactor verification scripts
- prefer data-driven customization over source edits where issue #32 applies

## Migration Priorities

If we must prioritize, use this order:

1. stabilize and extract shared lower-level types
2. keep shrinking server/provider/session/agent hotspots internally
3. extract runtime contracts and orchestration crates
4. extract TUI
5. extract self-dev productization

This ordering gives the best overlap between architecture safety and compile-speed payoff.

## Acceptance Criteria

We should consider this RFC materially implemented when most of the following are true:

- the root package is primarily a composition shell
- shared cross-cutting types live in a lower-level crate rather than the root crate
- server, agent, provider, and TUI have clear ownership boundaries
- provider support crates no longer need root-crate-only types
- TUI depends on protocol/service contracts rather than runtime internals
- common self-dev edits avoid recompiling unrelated heavy subsystems whenever possible
- architecture docs match the actual crate graph

## Practical Guidance For Future Changes

When deciding where new code should go:

1. Ask who owns the behavior.
2. Ask which layers should be allowed to know about it.
3. Ask whether putting it in the root crate will increase invalidation for unrelated edits.
4. Prefer the narrowest stable owner that does not create an artificial abstraction.

Short version:

- if it is shared data, push downward
- if it is orchestration, keep it above stable contracts
- if it is UI, keep it out of runtime crates
- if it is heavy and isolated, make it a leaf crate

## Open Questions

These do not block the RFC, but they should be revisited as migration proceeds:

- Should `jcode-session` become an explicit crate, or remain an internal boundary until later?
- Should CLI remain in the top-level package permanently, or eventually become `jcode-cli`?
- Should `message` and `protocol` remain together in `jcode-core`, or split into separate contract crates if they evolve at different rates?
- Should `jcode-tui-workspace` remain a separate leaf crate long-term, or fold into `jcode-tui` once the larger TUI extraction lands?

## Recommendation

Adopt this RFC as the architectural north star for refactors and crate splits.

In practice that means:

- keep following the current refactoring roadmap
- keep using the compile-performance plan's measured, crate-boundary-first strategy
- treat every new extraction as part of one layered architecture, not as an isolated cleanup
