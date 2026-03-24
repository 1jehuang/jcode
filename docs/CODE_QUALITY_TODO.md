# Code Quality Program Todo List

This file tracks the execution backlog for the code-quality uplift program described in `docs/CODE_QUALITY_10_10_PLAN.md`.

Status values:

- `pending`
- `in_progress`
- `blocked`
- `done`

## Phase 0: Prevent Further Decay

- [ ] Add CI job for `cargo check --all-targets --all-features`
- [ ] Add CI job for `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] Keep warning policy on a downward ratchet
- [ ] Add documented file-size and function-size targets to contributor guidance

## Phase 1: Warning and Dead-Code Burn-Down

- [ ] Inventory all `#![allow(dead_code)]` locations and justify or remove them
- [ ] Reduce baseline warning count significantly from the current level
- [ ] Remove stale unused functions in `setup_hints.rs`
- [ ] Remove stale unused code in TUI support modules
- [ ] Audit broad suppressions and replace with narrow local allowances

## Phase 2: Decompose the Biggest Files

### Highest priority
- [ ] Split `tests/e2e/main.rs` by feature area
  - Started 2026-03-24: extracted feature modules `session_flow`, `transport`, `provider_behavior`, `binary_integration`, `safety`, and `ambient`
  - Remaining follow-up: extract shared helpers into `tests/e2e/test_support/mod.rs`
- [ ] Continue splitting `src/server.rs` into focused submodules
- [ ] Split `src/agent.rs` into orchestration, stream, interrupt, and tool-exec modules

### Next wave
- [ ] Split `src/provider/mod.rs` into traits, pricing, routes, and shared HTTP helpers
- [ ] Split `src/provider/openai.rs` into request, stream, tool, and response modules
- [ ] Split `src/tui/ui.rs` by render responsibility
- [ ] Split `src/tui/info_widget.rs` by widget/domain sections

## Phase 3: Error Handling Hardening

- [ ] Count production `unwrap` / `expect` separately from test-only usages
- [ ] Replace easy production `unwrap` / `expect` hotspots with explicit errors
- [ ] Add better error context for provider stream parsing failures
- [ ] Add better error context for reload and socket lifecycle failures

## Phase 4: Test Strategy Improvements

- [ ] Extract shared e2e test support helpers
- [ ] Add focused tests for reload state transitions
- [ ] Add focused tests for malformed provider stream chunks
- [ ] Add snapshot or golden tests for stable TUI render outputs
- [ ] Add property tests for protocol serialization and tool parsing

## Phase 5: Reliability and Performance Guardrails

- [ ] Add repeated reload reliability test coverage
- [ ] Add repeated attach/detach and reconnect coverage
- [ ] Track memory regression expectations in a documented budget
- [ ] Improve observability around reload, swarm, and tool execution paths

## Immediate Active Work

- [ ] Land the quality plan document
- [ ] Land this todo list
- [ ] Tighten CI guardrails
- [ ] Begin the first high-ROI cleanup or split
