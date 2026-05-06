---
type: doc
name: project-overview
description: High-level overview of the project, its purpose, and key components
category: overview
generated: 2026-05-06
status: filled
scaffoldVersion: "2.0.0"
---

# Project Overview

Jcode is an open source agentic coding environment focused on terminal-first development, multi-agent coordination, tool use, memory, self-development, and desktop/mobile companion experiences. This repository is currently on the `feature/embedded-skills-harness` branch and also serves as a `jcode-harness` product fork with embedded offline skills, an offline clean-code quality gate, and `/init` swarm bootstrap support.

> **Detailed Analysis**: For generated symbol counts and dependency graphs, see `codebase-map.json` when present. Treat generated context as advisory and verify against `Cargo.toml` and the current tree.

## Quick Facts
- Root: `<project-root>`
- Primary languages: Rust for the application/workspace, Python for automation and black-box tests, JavaScript for the telemetry worker and related tooling.
- Root package: `jcode` in `Cargo.toml`.
- Main binaries: `jcode` at `src/main.rs`, `jcode-harness` at `src/bin/harness.rs`, `test_api` at `src/bin/test_api.rs`, plus dev-only binaries behind features.
- Desktop crate: `crates/jcode-desktop`.
- Current branch: `feature/embedded-skills-harness`.

## Entry Points
- `src/lib.rs` for the root library.
- `src/main.rs` for the primary CLI/TUI application.
- `src/bin/harness.rs` for the automation-facing `jcode-harness` binary.
- `src/project_init.rs` for `/init` static scaffolding and queued swarm bootstrap behavior.
- `src/skill.rs` and `src/skill_pack.rs` for embedded skills and skill registry behavior.
- `src/server/` for server, debug socket, swarm, and session orchestration.
- `crates/jcode-desktop/src/main.rs` for the desktop wrapper.
- `telemetry-worker/src/worker.js` for telemetry ingestion.
- `scripts/` for developer automation, CI helpers, profiling, quality budgets, and release/install flows.

## Key Exports
Use `cargo metadata --no-deps`, `cargo doc --workspace --no-deps`, and targeted `agentgrep` searches for current crate-level APIs. The root package and workspace crates expose Rust APIs through each `src/lib.rs`; automation-facing behavior is primarily through `jcode`, `jcode-harness`, scripts, and tests.

## File Structure & Code Organization
- `src/` - Root application/runtime modules for the main Jcode CLI/TUI, server, tools, skills, init, providers, storage, and harness binary.
- `crates/` - Modular Rust crates for runtime contracts, type crates, providers/auth, storage, TUI rendering/components, side-panel, update, terminal launch, mobile, and desktop support.
- `docs/` - Product, architecture, embedded skills, clean-code, release gates, bootstrap, and operating documentation.
- `scripts/` - Benchmarks, debug socket tests, CI checks, profiling, security/preflight, release, install, and self-dev helpers.
- `tests/` - Rust e2e tests and Python integration/regression tests that drive Jcode through sockets and subprocesses.
- `telemetry-worker/` - Cloudflare Wrangler telemetry worker and D1 migrations.
- `.jcode/` - Project-local init plans/reports, MCP plan, skills plan, and side-panel status.
- `.context/` - Generated AI context and agent playbooks.
- `assets/`, `packaging/`, `ios/`, `figma/`, `mockups/` - release, product, mobile, and design assets.

## Technology Stack Summary
Jcode is primarily a Rust 2024 Cargo workspace. Python scripts provide automation and black-box tests. JavaScript is used for the telemetry worker and some design/tooling surfaces. Developer workflows rely on focused Cargo checks, self-dev builds, debug sockets, and targeted Python/Rust tests.

## Getting Started Checklist
1. Review `AGENTS.md`, `README.md`, `CONTRIBUTING.md`, and `.jcode/init/SWARM_ANALYSIS_REPORT.md`.
2. Inspect workspace topology with `cargo metadata --no-deps` or `cargo check -p <crate>`.
3. Use `selfdev build` when available, or fallback to `scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode`.
4. Run focused tests near the touched crate/module.
5. For UI/session changes, validate with debug socket tester sessions.
6. For embedded skills or harness changes, use release-gate commands from `docs/JCODE_HARNESS_RELEASE_GATES.md`.
