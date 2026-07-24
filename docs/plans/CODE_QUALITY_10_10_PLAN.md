# Code Quality 10/10 Plan

This document defines the quality target for jcode, the standards required to reach it, and the phased execution plan to get there without destabilizing the product.

## Goal

Raise jcode from its current state of roughly **7/10 overall code quality** to a sustained **9+/10 engineering standard**, with a practical target that feels like "10/10" in day-to-day development:

- clean builds
- clear module ownership
- small and maintainable files
- low-risk refactors
- strong tests
- predictable behavior under stress
- strict CI guardrails that prevent regressions

Because jcode is a fast-moving product, "10/10" does **not** mean "perfect". It means:

1. defects are easier to prevent than to introduce
2. contributors can quickly understand where code belongs
3. the repo resists architectural drift
4. risky areas are well-tested and observable
5. quality does not depend on memory or heroics

## Current Problems

The main issues observed in the codebase today are:

### 1. Oversized modules

Several files are dramatically larger than they should be for long-term maintainability. Major hotspots currently include:

- `src/provider/openai.rs`
- `src/provider/mod.rs`
- `src/agent.rs`
- `src/server.rs`
- `src/tui/ui.rs`
- `src/tui/info_widget.rs`
- `tests/e2e/main.rs`

These files are doing too much at once and create review, testing, and onboarding friction.

### 2. Warning and dead-code debt

The repository currently tolerates a significant warning budget instead of targeting warning-free builds. There are also multiple broad `allow(dead_code)` suppressions that hide drift.

### 3. Inconsistent strictness around failure paths

The codebase contains many `unwrap`, `expect`, `panic!`, `todo!`, and `unimplemented!` usages. Some are valid in tests, but production code should be more defensive and explicit.

### 4. Test concentration

There are many tests, which is good, but some test coverage is concentrated inside very large files and does not yet provide ideal fault isolation.

### 5. Guardrails are present but not yet strict enough

There is already useful quality infrastructure in the repository, but it should be tightened so quality improves automatically over time.

## Definition of Done for "10/10"

We will consider this program successful when the codebase reaches the following state:

### Build and lint quality

- `cargo check --all-targets --all-features` passes cleanly
- `cargo clippy --all-targets --all-features -- -D warnings` passes cleanly or is very close with narrow, justified exceptions
- `cargo fmt --all -- --check` passes
- warning count is near zero and actively ratcheted downward

### Structural quality

- no production file exceeds **1200 LOC** without a documented reason
- most production files are below **800 LOC**
- most functions stay below **100 LOC** unless complexity is clearly justified
- major domains have clear boundaries and ownership

### Reliability quality

- e2e tests are split by feature instead of concentrated in mega-files
- critical state transitions have targeted tests
- reload, streaming, tool execution, and swarm coordination have explicit failure-mode coverage
- long-running reliability checks exist for memory, socket lifecycle, and reconnect/reload behavior

### Safety quality

- production `unwrap` / `expect` usage is significantly reduced and justified where it remains
- broad `allow(dead_code)` suppressions are eliminated or reduced to narrow local allowances
- tool, shell, path, and credential boundaries are explicit and tested

### Contributor quality

- contributors can tell where code belongs
- refactor rules are documented
- CI makes regressions hard to merge
- architecture docs match reality

## Non-Negotiable Principles

1. **No big-bang rewrite.** Refactor incrementally.
2. **Behavior-preserving changes first.** Extract, move, split, and test before changing logic.
3. **Quality must be enforceable.** Prefer CI guardrails over informal expectations.
4. **Delete dead code aggressively.** Simpler code is higher-quality code.
5. **Keep the product shippable throughout the program.**

## Metrics to Track

These metrics should be checked repeatedly during the program:

- warning count
- clippy violations
- count of broad `allow(dead_code)` suppressions
- count of production `unwrap` / `expect`
- top 20 largest Rust files
- test runtime and flake rate
- startup time, memory, and reload reliability

## Phased Plan

## Phase 0: Prevent Further Decay

**Objective:** stop quality from getting worse.

Tasks:

- add stricter CI checks for clippy and all-target/all-feature builds
- ratchet warning policy downward
- document code quality standards and file-size goals
- establish a tracked todo list for the quality program

Success criteria:

- no new warnings merge unnoticed
- no new giant files are added casually
- contributors can see the roadmap and standards in-repo

## Phase 1: Warning and Dead-Code Burn-Down

**Objective:** restore signal quality in builds.

Tasks:

- remove unused variables, methods, and stale helpers
- replace broad `#![allow(dead_code)]` with narrow scoped allows where truly needed
- delete abandoned code paths
- reduce dead code in TUI, memory, and provider modules

Success criteria:

- warning count materially reduced
- dead-code suppression becomes the exception, not the default

## Phase 2: Decompose the Biggest Files

**Objective:** eliminate the primary maintainability hazard.

Priority order:

1. `tests/e2e/main.rs`
2. `src/server.rs`
3. `src/agent.rs`
4. `src/provider/mod.rs`
5. `src/provider/openai.rs`
6. `src/tui/ui.rs`
7. `src/tui/info_widget.rs`

Approach:

- extract pure helpers first
- extract types and state machines second
- extract domain-specific submodules third
- keep public interfaces stable during moves

Success criteria:

- each hotspot file becomes materially smaller
- functionality remains stable
- tests remain green during each split

## Phase 3: Strengthen Error Handling

**Objective:** make failure modes explicit and recoverable.

Tasks:

- reduce production `unwrap` / `expect`
- improve error context with `anyhow` / `thiserror`
- classify retryable vs user-facing vs internal invariant failures
- add tests for malformed streams, reconnects, and tool interruption paths

Success criteria:

- fewer panic-prone production paths
- clearer logs and more diagnosable failures

## Phase 4: Rebalance the Test Pyramid

**Objective:** make failures faster, narrower, and more actionable.

Tasks:

- split e2e suites by feature
- add more unit tests for parsing, protocol, and state transitions
- add snapshot or golden tests for stable render outputs
- add property tests for serialization, tool parsing, and patch/edit invariants
- improve test support utilities and isolation

Success criteria:

- lower test maintenance cost
- failures localize to one subsystem quickly

## Phase 5: Reliability and Performance Guardrails

**Objective:** keep architectural quality aligned with runtime quality.

Tasks:

- add or strengthen memory and stress checks
- add repeated reload / attach / detach reliability tests
- track startup and idle resource regressions
- improve structured diagnostics around reload, sockets, and provider streaming

Success criteria:

- regressions are caught before release
- long-running behavior is measurably stable

## Phase 6: Finish the Ratchet

**Objective:** make quality self-sustaining.

Tasks:

- move from warning budget to effectively warning-free builds
- enforce stricter clippy rules where practical
- document module ownership expectations
- review and refresh architecture docs after refactors land

Success criteria:

- repo quality remains high without special cleanup pushes
- the codebase resists drift by default

## Immediate Execution Order

The first concrete actions should be:

1. land this quality plan and a tracked todo list
2. tighten CI guardrails
3. begin warning/dead-code cleanup
4. split `tests/e2e/main.rs`
5. continue into `src/server.rs`

## Initial Target Refactors

### `tests/e2e/main.rs`
Split into:

- `tests/e2e/session_flow.rs`
- `tests/e2e/tool_execution.rs`
- `tests/e2e/reload.rs`
- `tests/e2e/swarm.rs`
- `tests/e2e/provider_behavior.rs`
- `tests/e2e/test_support/mod.rs`

### `src/server.rs`
Split further into:

- `src/server/state.rs`
- `src/server/bootstrap.rs`
- `src/server/socket.rs`
- `src/server/session_registry.rs`
- `src/server/event_subscriptions.rs`

### `src/agent.rs`
Split into:

- `src/agent/loop.rs`
- `src/agent/stream.rs`
- `src/agent/tool_exec.rs`
- `src/agent/interrupts.rs`
- `src/agent/messages.rs`
- `src/agent/retry.rs`

### `src/provider/mod.rs`
Split into:

- `src/provider/traits.rs`
- `src/provider/model_route.rs`
- `src/provider/pricing.rs`
- `src/provider/http.rs`
- `src/provider/capabilities.rs`

## Working Rules for the Refactor Program

- every step must compile or fail for a very obvious temporary reason
- prefer moving code without changing behavior
- avoid mixing cleanup and feature work in the same commit when possible
- when a file is touched, leave it cleaner than it was
- if a new broad allow-suppression is added, it must be documented in the PR

## Validation Matrix

Minimum validation during this program:

- `cargo check -q`
- `cargo test -q`
- targeted tests for touched areas
- `scripts/check_warning_budget.sh`
- `cargo fmt --all -- --check`

Stricter validation when touching core orchestration or provider code:

- `cargo check --all-targets --all-features`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets --all-features`
- `cargo test --test e2e`

## Ownership

This is an active engineering program, not a one-time cleanup document. The expectation is:

- the plan is updated as milestones are completed
- todo items are kept current
- progress is visible in the repo
- each completed phase leaves behind stronger guardrails than before
