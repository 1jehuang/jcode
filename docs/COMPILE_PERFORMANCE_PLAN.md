# Compile Performance Plan

This document tracks the plan to make jcode's self-dev / refactor loop much faster
without sacrificing full-feature builds.

See also:

- [`REFACTORING.md`](./REFACTORING.md)
- [`MODULAR_ARCHITECTURE_RFC.md`](./MODULAR_ARCHITECTURE_RFC.md)

## Goals

- Keep full-featured builds available for normal usage and self-dev reloads.
- Make common self-dev edits significantly cheaper to compile.
- Reduce how often customizations require recompilation at all.
- Measure improvements after each phase and stop churn that does not pay off.

## Current Baseline (2026-03-24)

Measured locally on the current tree:

- Warm `cargo check --quiet`: **~8.5s**
- Warm `scripts/dev_cargo.sh build --release -p jcode --bin jcode --quiet`: **~47.3s**

Additional observations from this audit:

- A previous warm-ish `cargo check` run landed around **~12.3s**.
- A less-warm `cargo check --timings` run landed around **~23.8s**.
- The previous local default `clang + mold` setup failed during release linking on this machine.
- `clang + lld` links the release `jcode` binary successfully here.

## Near-Term Targets

For common self-dev edits that do **not** touch broad shared interfaces:

- Warm `cargo check`: **< 5s**
- Warm `cargo build` / reload-oriented build: **< 20–30s**

For shared/core edits we should still aim to stay materially below today's baseline,
even if they cannot reach the same fast path.

## What Matters Most (ranked)

1. **Workspace / crate boundaries**
   - Rust caches best at the crate boundary.
   - Heavy untouched subsystems should remain compiled and reusable in full builds.
2. **Good boundary design**
   - High-churn logic should not live in broad fanout crates or unstable shared types.
3. **`sccache`**
   - Practical win for repeated local builds and CI.
4. **Fast, reliable linker configuration**
   - Especially important for `cargo build` and release/self-dev reload builds.
5. **Heavy subsystem isolation**
   - Embeddings, provider implementations, and large TUI/rendering code should stop
     churning unrelated builds.
6. **Narrower build targets for inner loops**
   - Avoid rebuilding extra bins/targets when not needed.
7. **Reduce the need to recompile at all**
   - Issue #32's customization records and extension points should make many changes
     config/hook/skill/data driven rather than source driven.

## Execution Plan

### Phase 1 — Tactical build speed wins

- Keep `.cargo/config.toml` conservative for local contributors.
- Use `scripts/dev_cargo.sh` for local self-dev builds:
  - enables `sccache` automatically if installed
  - prefers `clang + lld` on Linux x86_64
  - uses the dedicated Cargo `selfdev` profile for `jcode` self-dev build/reload paths
  - can still opt into `mold` via `JCODE_FAST_LINKER=mold`
- Route refactor-shadow builds through that wrapper.

### Phase 2 — Measurement and repeatability

Standard self-dev checkpoints now live behind `scripts/bench_selfdev_checkpoints.sh`, which runs:
- cold `cargo check`
- warm touched-file `cargo check`
- cold self-dev `jcode` build
- warm touched-file self-dev `jcode` build

Use it when capturing comparable before/after numbers for refactors.

- Add documented commands for cold/warm `check` and `build` timing.
- Prefer touched-file timings (for example `scripts/bench_compile.sh check --touch src/server.rs`) over no-op hot-cache reruns when judging ROI.
- Track timing deltas after each structural phase.
- Fix build/link blockers before treating any timing data as authoritative.
- 2026-03-25: upgraded `scripts/bench_compile.sh` to support repeated runs, summary stats,
  JSON output, and extra cargo-arg passthrough so compile-speed work can use consistent
  touched-file measurements instead of one-off ad hoc timings.
- 2026-03-25: upgraded `scripts/dev_cargo.sh` with `--print-setup` plus clearer cache/linker
  diagnostics so developers can confirm whether `sccache` / fast-linker paths are actually active.
- 2026-03-30: removed the per-build `build.rs` timestamp/build-number churn from local source
  builds. `JCODE_VERSION` for source builds is now stable per `Cargo.toml` version + git hash,
  while UI/version build-time display comes from the binary mtime at runtime. Validation on this
  machine: two no-op release-jcode runs measured **221.688s then 0.559s**, confirming the main
  crate no longer recompiles just because build metadata changed.
- 2026-04-09: introduced a dedicated Cargo `selfdev` profile for self-dev iteration. On this
  machine, the warm local `jcode` self-dev build path dropped from about **56.1s** for
  `scripts/dev_cargo.sh build --release -p jcode --bin jcode --quiet` to about **16.0s** for
  `scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode --quiet`, while keeping the
  normal release/distribution profile unchanged.
- 2026-04-18: added `scripts/bench_selfdev_checkpoints.sh` to standardize cold/warm self-dev
  checkpoints. First local checkpoint attempt on this machine surfaced two environment blockers:
  - cold checkpoints failed because `cargo clean` could not remove part of `target/release`
    (`Permission denied` on a fingerprint timestamp file)
  - warm `selfdev-jcode` touched-file measurement on `src/tool/read.rs` failed because the
    `sccache`-wrapped rustc process terminated with signal 15 during the `jcode` crate build
  - warm touched-file `cargo check` on `src/tool/read.rs` completed in **93.115s** then **9.430s**,
    which is useful as a rough upper/lower bound but not yet stable enough to treat as an
    authoritative checkpoint
  - follow-up required: fix the `target/release` permission issue, rerun cold checkpoints, and
    rerun warm self-dev measurements until they are stable enough to compare against future waves
- 2026-04-18: updated `scripts/bench_selfdev_checkpoints.sh` to keep running after individual
  checkpoint failures and report them in JSON/text output instead of aborting early. Verified local
  output on this machine with `--touch src/tool/read.rs --runs 1`:
  - warm touched-file `cargo check`: **9.582s**
  - warm touched-file `selfdev-jcode` build: **59.898s**
  - failed checkpoints reported cleanly: `cold_check`, `cold_selfdev_build`
- 2026-04-18: added `--skip-cold` to `scripts/bench_selfdev_checkpoints.sh` so warm-only
  checkpoints remain usable while cold-path cleanup is blocked locally. Verified local output on this
  machine with `--skip-cold --touch src/tool/read.rs --runs 1`:
  - warm touched-file `cargo check`: **9.339s**
  - warm touched-file `selfdev-jcode` build: **18.844s**
  - skipped checkpoints reported explicitly: `cold_check`, `cold_selfdev_build`
- 2026-04-18: additional warm-only checkpoint on a broader shared edit target with
  `--skip-cold --touch src/server.rs --runs 1`:
  - warm touched-file `cargo check`: **8.711s**
  - warm touched-file `selfdev-jcode` build: **18.969s**
- 2026-04-18: additional warm-only checkpoint on a heavy tool-path file with
  `--skip-cold --touch src/tool/communicate.rs --runs 1`:
  - warm touched-file `cargo check`: **8.496s**
  - warm touched-file `selfdev-jcode` build: **21.400s**
- 2026-04-18: additional warm-only checkpoint on a provider-heavy file with
  `--skip-cold --touch src/provider/openai.rs --runs 1`:
  - warm touched-file `cargo check`: **8.750s**
  - warm touched-file `selfdev-jcode` build: **21.386s**
- 2026-04-18: additional warm-only checkpoint on the shared provider module with
  `--skip-cold --touch src/provider/mod.rs --runs 1`:
  - warm touched-file `cargo check`: **9.772s**
  - warm touched-file `selfdev-jcode` build: **17.917s**
- 2026-04-18: additional warm-only checkpoint on the agent entry module with
  `--skip-cold --touch src/agent.rs --runs 1`:
  - warm touched-file `cargo check`: **7.318s**
  - warm touched-file `selfdev-jcode` build: **30.928s**
- 2026-04-18: additional warm-only checkpoint on the memory tool with
  `--skip-cold --touch src/tool/memory.rs --runs 1`:
  - warm touched-file `cargo check`: **7.787s**
  - warm touched-file `selfdev-jcode` build: **12.798s**
- 2026-04-18: additional warm-only checkpoint on session search with
  `--skip-cold --touch src/tool/session_search.rs --runs 1`:
  - warm touched-file `cargo check`: **7.009s**
  - warm touched-file `selfdev-jcode` build: **12.874s**
- 2026-04-18: additional warm-only checkpoint on the browser tool with
  `--skip-cold --touch src/tool/browser.rs --runs 1`:
  - warm touched-file `cargo check`: **13.693s**
  - warm touched-file `selfdev-jcode` build: **18.874s**
- 2026-04-28: diagnosed the repeated self-dev `jcode` lib build `SIGTERM` on this 16 GiB,
  no-swap workstation. `journalctl -u earlyoom` showed earlyoom sending `SIGTERM` to the root
  `rustc` at about **1.09 GiB RSS** when available memory crossed the 10% threshold. A direct
  no-`sccache` build reproduced the same signal, so `sccache` was only reporting the termination.
  `scripts/dev_cargo.sh` now enables adaptive low-memory overrides for `--profile selfdev` when
  Linux + earlyoom + no swap + <24 GiB RAM + <8 GiB currently available RAM are detected:
  `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_SELFDEV_INCREMENTAL=false`, and
  `CARGO_PROFILE_SELFDEV_CODEGEN_UNITS=16`. Use `JCODE_SELFDEV_LOW_MEMORY=off` to disable, or
  `JCODE_SELFDEV_LOW_MEMORY=on` to force. Validation: the original root build completed under
  those settings in **2m34s** after the interrupted partial build reused artifacts; a later
  benchmark with 9.4 GiB available showed that preserving the inherited selfdev profile can reduce
  warm edit builds from about **60s** to about **14s** when there is enough headroom.
- 2026-05-05: trimmed root compile surface by replacing broad `tokio/full` with explicit used
  features, aligning Jcode-owned `crossterm` dependencies on 0.29, and replacing `qr2term` with
  direct `qrcode` rendering. This removed the duplicate `crossterm 0.28` path from the `jcode`
  tree while preserving login QR output. Validation: `cargo check --profile selfdev -p jcode --bin
  jcode`, `cargo test --profile selfdev login_qr --lib -- --nocapture`, and coordinated
  `selfdev build` passed.
- 2026-05-05: removed unused `reqwest/blocking` from `jcode-provider-core`; static search showed
  no blocking API usage in that crate. Validation: `cargo check --profile selfdev -p
  jcode-provider-core` and full `cargo check --profile selfdev -p jcode --bin jcode` passed.
- 2026-05-03: added `JCODE_DEV_FEATURE_PROFILE` to `scripts/dev_cargo.sh` so compile-speed probes and
  narrow inner-loop builds can consistently select feature sets without repeating Cargo flags. Profiles:
  `default`, `minimal`/`none` (`--no-default-features`), `pdf` (`--no-default-features --features pdf`),
  `embeddings` (`--no-default-features --features embeddings`), and `full` (`--features embeddings,pdf`).
  The wrapper leaves explicit `--features` / `--no-default-features` cargo args untouched. Validation on
  this machine: `JCODE_DEV_FEATURE_PROFILE=minimal scripts/dev_cargo.sh check -p jcode --lib --quiet` passed.
- 2026-05-03: disabled Cargo auto-discovery for root binary targets and moved developer-only helper
  binaries (`tui_bench`, `session_memory_bench`, `mermaid_side_panel_probe`) behind the opt-in
  `dev-bins` feature. This keeps broad normal checks focused on production/test targets while preserving
  explicit probe coverage via `cargo check --all-targets -p jcode --features dev-bins`. Validation showed
  `cargo check --all-targets -p jcode` skips those three bins, while adding `--features dev-bins` includes them.
- 2026-05-03: moved the self-dev build/version/channel support implementation out of the root crate and
  into `crates/jcode-build-support`, leaving `src/build.rs` as a re-export facade. This cuts another
  stable, high-fanout support subsystem out of the root compile unit while preserving existing call sites
  (`crate::build::*`). Validation: `cargo check -p jcode-build-support`, `cargo test -p jcode-build-support`,
  and `cargo check -p jcode --lib` passed during the split.
- 2026-05-03: moved the pure keybinding parser/matcher/types from `src/tui/keybind.rs` into
  `jcode-tui-core::keybind`, leaving root TUI config-loading wrappers in place. This creates a reusable
  cache boundary for a low-coupling TUI helper module while preserving the existing `crate::tui::keybind::*`
  API. Validation: `cargo check -p jcode-tui-core`, `cargo test -p jcode-tui-core`, and
  `cargo check -p jcode --lib` passed.

Warm-only touched-file checkpoints captured so far on this machine:

| Touched file | Warm `cargo check` | Warm `selfdev-jcode` build |
| --- | ---: | ---: |
| `src/tool/session_search.rs` | 7.009s | 12.874s |
| `src/agent.rs` | 7.318s | 30.928s |
| `src/tool/memory.rs` | 7.787s | 12.798s |
| `src/tool/communicate.rs` | 8.496s | 21.400s |
| `src/server.rs` | 8.711s | 18.969s |
| `src/provider/openai.rs` | 8.750s | 21.386s |
| `src/tool/read.rs` | 9.339s | 18.844s |
| `src/provider/mod.rs` | 9.772s | 17.917s |
| `src/tool/browser.rs` | 13.693s | 18.874s |

Observed spread from these warm-only checkpoints:
- warm touched-file `cargo check`: **7.009s to 13.693s**
- warm touched-file `selfdev-jcode` build: **12.798s to 30.928s**
- fastest measured warm self-dev rebuilds so far are on smaller tool-path edits
- `src/agent.rs` currently stands out as the most expensive warm self-dev rebuild in this sample set
- `src/tool/browser.rs` currently stands out as the slowest warm `cargo check` in this sample set

### Phase 3 — Workspace boundary design

The refined layered target, dependency rules, and migration guidance live in
[`docs/MODULAR_ARCHITECTURE_RFC.md`](MODULAR_ARCHITECTURE_RFC.md). The crate list
below is the compile-performance-oriented destination sketch and should be read
as compatible with that RFC, not as the only acceptable final packaging.

Proposed destination layout:

- `jcode-core`
  - protocol, ids, message types, config primitives, shared utility types
- `jcode-server`
  - server lifecycle, reload, socket, swarm, daemon behaviors
- `jcode-agent`
  - agent turn loop, tool orchestration, stream handling
- `jcode-provider`
  - provider traits, shared provider types, routing/catalog support
- `jcode-embedding`
  - embedding model integration and related heavy inference dependencies
- `jcode-tui`
  - TUI rendering, widgets, state reduction, terminal UI support
- `jcode-tui-core`
  - low-level TUI helpers with minimal root coupling, including stream buffers and keybinding parsing
- `jcode-selfdev`
  - customization records, migration logic, self-dev productization
- `jcode-build-support`
  - self-dev build commands, source-state fingerprints, binary channel paths/manifests

### Phase 4 — First crate splits

Start with the highest-leverage cache boundaries:

1. `jcode-embedding`
2. provider support / provider implementation splits
3. self-dev/customization system once the new extension-point work lands
4. server / agent split along the seams already being extracted

### Phase 4a — First workspace boundary landed

- 2026-03-24: moved the heavy ONNX/tokenizer implementation into the new
  `crates/jcode-embedding` workspace crate.
- The main `src/embedding.rs` module now acts as a facade for process-local
  cache/stats/path/logging integration.
- This preserves the public `crate::embedding` API while creating a real Cargo
  cache boundary for the heaviest embedding dependencies.
- Follow-up: gather more realistic before/after timing data using controlled
  touched-file benchmarks rather than fully hot no-op rebuilds.
- 2026-05-05: made the `embeddings` feature opt-in instead of part of default
  features. The crate boundary was already in place, but ordinary `cargo check`
  / `cargo build` still compiled the `tract` / `tokenizers` subtree unless
  developers remembered `--no-default-features`. Default builds now keep `pdf`
  enabled but skip local embedding inference; full local inference remains
  available via `--features embeddings` or `JCODE_DEV_FEATURE_PROFILE=full`.
  Validation: `cargo tree -p jcode --edges normal --depth 1` includes
  `jcode-pdf` but not `jcode-embedding`; adding `--features embeddings` includes
  both; `cargo check -p jcode --quiet` passes.

- 2026-03-24: moved PDF extraction behind the new `crates/jcode-pdf` workspace
  crate and fixed the `--no-default-features` build path by making PDF support
  degrade gracefully when the feature is disabled.

- 2026-03-24: moved Azure bearer-token retrieval behind the new
  `crates/jcode-azure-auth` workspace crate so the Azure SDK no longer lives
  directly in the main crate.
- Note: touched-file timing for `src/auth/azure.rs` needs more instrumentation
  cleanup; one post-split sample was anomalous and should not be treated as a
  trustworthy ROI datapoint yet.

- 2026-03-24: moved email notification / IMAP reply transport behind the new
  `crates/jcode-notify-email` workspace crate.
- The main `src/notifications.rs` module now keeps the higher-level ambient,
  safety, and channel integration while SMTP/IMAP/mail parsing lives behind a
  dedicated crate boundary.
- This split is primarily meant to keep `lettre`, `imap`, `mail-parser`, and
  `native-tls` out of unrelated self-dev rebuilds; edits to `notifications.rs`
  itself still invalidate the main crate and are not the right sole ROI metric.

- 2026-03-25: landed the first provider boundary slice with
  `crates/jcode-provider-metadata`.
- Boundary decision: provider **metadata / profile catalogs / pure selection helpers** move into
  their own crate first, while env mutation, config-file I/O, and runtime integration remain in
  `src/provider_catalog.rs` as a facade.
- This is intentionally narrower than a full `Provider` trait split: it creates a real provider-side
  compile boundary without prematurely dragging streaming/message/runtime dependencies into a shared
  crate that would likely stay high-churn.

- 2026-03-25: landed the next provider-core slice with `crates/jcode-provider-core`.
- Boundary decision: move **shared HTTP client + route/cost/core provider value types** first,
  but keep the `Provider` trait itself in `src/provider/mod.rs` for now.
- Reason: the trait currently still mixes in `message.rs`, runtime/auth behavior, and provider-specific
  streaming/compaction concerns; moving it too early would likely create a noisy, still-high-churn core crate.

- 2026-03-25: landed the first provider-implementation support crate with
  `crates/jcode-provider-openrouter`.
- Boundary decision: move **OpenRouter-specific model catalog / endpoint cache / provider ranking /
  model-spec parsing support** into a dedicated crate, while keeping the actual `Provider` trait impl,
  auth wiring, and message/stream translation in `src/provider/openrouter.rs`.
- Reason: this creates a real provider-implementation compile boundary now, without introducing a crate
  cycle through `Provider`, `EventStream`, or `message.rs`.

- 2026-03-25: landed the next provider-implementation support crate with
  `crates/jcode-provider-gemini`.
- Boundary decision: move **Gemini Code Assist schema/types, model-list constants, and pure support helpers**
  into a dedicated crate, while keeping the actual `Provider` trait impl, auth calls, and runtime/network orchestration
  in `src/provider/gemini.rs`.
- Reason: this creates another real provider-side compile boundary without forcing the `Provider` / `EventStream`
  seam prematurely.

- 2026-03-30: moved the pure OpenAI tool-schema normalization helpers into
  `crates/jcode-provider-core/src/openai_schema.rs`.
- Boundary decision: move **pure schema adaptation / strict-normalization helpers** first, while keeping
  `build_tools(...)` and request-history rewriting in `src/provider/openai_request.rs` because those still depend on
  local tool/message types.
- Reason: this creates another provider-side cache boundary now without prematurely pulling `Message`, `ToolDefinition`,
  or the `Provider` trait into a shared crate.

- 2026-05-05: moved provider catalog-refresh diffing into
  `jcode-provider-core::catalog_refresh` and re-exported it from the root provider facade.
- Boundary decision: move the pure `ModelRoute` summary/diff logic first because it has no root-crate
  auth/runtime/config dependencies.
- 2026-05-05: split the stable provider pricing tables/helpers into
  `jcode-provider-core::pricing`, leaving `src/provider/pricing.rs` as a thin facade for root-only
  auth/env/OpenRouter-cache lookups.
- Reason: provider pricing is relatively stable table/math code, but it previously lived in the main crate
  beside high-churn provider runtime code. This creates a reusable cache boundary without moving the
  `Provider` trait or network implementations prematurely.
- Validation: `cargo test -p jcode-provider-core --quiet`, `cargo test -p jcode pricing:: --quiet`,
  `cargo check -p jcode --quiet`, and `cargo check -p jcode --features embeddings --quiet` pass.
- 2026-05-05: moved provider failover prompt/decision/classifier contracts and provider
  selection/fallback-order contracts into `jcode-provider-core`, leaving root provider modules as
  facades for env/runtime/account state. This continues shrinking `src/provider/mod.rs` support
  surfaces toward an eventual `jcode-provider` runtime crate.
- Validation: `cargo test -p jcode-provider-core --quiet`, focused root provider selection/failover
  tests, and `cargo check -p jcode --quiet` pass.
- 2026-05-05: moved the Copilot `PremiumMode` provider-control enum into `jcode-provider-core`
  and re-exported it from the root/Copilot facades. The `Provider` trait no longer needs to name
  the root `copilot` module for this control surface.
- Validation: `cargo check -p jcode-provider-core --quiet` and `cargo check -p jcode --quiet` pass.
- 2026-05-05: moved provider-native tool result DTOs/sender aliases into `jcode-provider-core`.
  The global `Provider` trait no longer has to expose types owned by the root Claude module.
- Validation: `cargo check -p jcode-provider-core --quiet` and `cargo check -p jcode --quiet` pass.
- 2026-05-05: moved stable provider model constants, static provider/model classification,
  Copilot model-name normalization, and fallback context-window heuristics into
  `jcode-provider-core::models`. Root `src/provider/models.rs` now layers dynamic account catalogs,
  runtime availability, and cache hydration on top of those core helpers.
- Validation: `cargo test -p jcode-provider-core models:: --quiet`,
  `cargo check -p jcode-provider-core --quiet`, and `cargo check -p jcode --quiet` pass.
- 2026-05-05: moved the global `Provider` trait and `EventStream` alias into `jcode-provider-core`.
  Root `src/provider/mod.rs` now re-exports the contract while continuing to own concrete provider
  implementations and `MultiProvider` composition. This is the main provider seam needed before a
  future `jcode-provider` runtime crate can be introduced safely.
- Validation: `cargo check -p jcode-provider-core --quiet` and `cargo check -p jcode --quiet` pass.

- 2026-05-05: moved the stable provider-facing `ToolDefinition` contract from `src/message.rs` into
  `jcode-message-types` and re-exported it from the root message facade. This is a prerequisite for
  shrinking the provider trait and tool registry surfaces away from root-crate-only message types.
- Validation: `cargo test -p jcode-message-types --quiet` and `cargo check -p jcode --quiet` pass.
- 2026-05-05: introduced `jcode-tool-types` for stable tool execution output DTOs and moved
  `ToolOutput` / `ToolImage` out of `src/tool/mod.rs`. Root tool modules continue using the same
  names via a facade re-export, but provider/agent/server seams can now depend on a narrow tool
  result contract without depending on the root tool registry.
- Validation: `cargo check -p jcode-tool-types --quiet`, `cargo test -p jcode-tool-types --quiet`,
  and `cargo check -p jcode --quiet` pass.
- 2026-05-05: added `jcode-tool-core` for runtime tool contracts and moved `Tool`, `ToolContext`,
  `ToolExecutionMode`, and `StdinInputRequest` out of `src/tool/mod.rs`. `jcode-tool-types` stays
  DTO-only, while channel/runtime-bearing context lives in the runtime-contract crate instead of
  contaminating pure type crates.
- 2026-05-05: also moved the shared tool intent schema helper into `jcode-tool-core`, keeping the
  root `src/tool/mod.rs` module focused on registry composition rather than shared schema contracts.
- Validation: `cargo check -p jcode-tool-core --quiet`, `cargo check -p jcode-tool-types --quiet`,
  and `cargo check -p jcode --quiet` pass.
- 2026-05-05: moved provider streaming contracts `StreamEvent` and `ConnectionPhase` from
  `src/message.rs` into `jcode-message-types`, again preserving root facade re-exports. Together
  with `ToolDefinition`, this materially reduces the root-only surface of the provider trait and
  prepares a future `jcode-provider` crate.
- Validation: `cargo check -p jcode-message-types --quiet`, `cargo test -p jcode-message-types --quiet`,
  and `cargo check -p jcode --quiet` pass.
- 2026-05-05: moved core conversation DTOs `Message`, `ContentBlock`, `Role`, and `CacheControl`
  into `jcode-message-types`, while keeping root-only redaction/generated-image/session helpers in
  `src/message.rs`. Provider and agent contracts can now refer to message data through the lower
  type crate rather than the root crate facade.
- Validation: `cargo check -p jcode-message-types --quiet`, `cargo test -p jcode-message-types --quiet`,
  and `cargo check -p jcode --quiet` pass.
- 2026-05-05: moved pure message helpers for fresh-user-turn detection, stable message hashing,
  tool ID sanitization, and the missing-tool-output constant into `jcode-message-types`. Root keeps
  secret redaction and generated-image visual context because those still depend on regex/env/fs/base64
  integration details.
- Validation: `cargo check -p jcode-message-types --quiet`, focused root message helper tests, and
  `cargo check -p jcode --quiet` pass.
- 2026-05-05: moved the provider split-system dynamic-context insertion helper and its tests into
  `jcode-message-types`. This removes another pure message transformation from `src/provider/mod.rs`
  and keeps preparing the provider trait for an eventual runtime crate split.
- Validation: `cargo test -p jcode-message-types dynamic_context --quiet`,
  `cargo check -p jcode-message-types --quiet`, and `cargo check -p jcode --quiet` pass.

- 2026-05-05: moved the server lightweight-control request classifier from
  `src/server/client_lifecycle.rs` into `jcode-protocol::Request::is_lightweight_control_request`.
  This is a small but directionally important server seam: protocol-shape policy belongs with the
  protocol contract, while the large client lifecycle module keeps runtime dispatch.
- Validation: `cargo check -p jcode-protocol --quiet` and `cargo check -p jcode --quiet` pass.

- 2026-03-30: moved the workspace-map subsystem into the new `crates/jcode-tui-workspace` crate.
- Boundary decision: move **workspace map data/model + widget rendering** first, while keeping the surrounding
  `info_widget`, app state, and higher-level TUI composition in the main crate.
- Reason: this is a safe first `jcode-tui` foothold because the workspace map code is already mostly self-contained and
  avoids the much riskier `App` / renderer / markdown / mermaid seams.

### Phase 5 — Reduce invalidation pressure

- Continue shrinking giant hotspot files.
- Keep high-churn code out of stable low-level crates.
- Avoid changing shared broad fanout types casually.

### Phase 6 — Reduce recompilation demand via issue #32

- Store customization intent, provenance, validation, and migration hints.
- Add extension points so more user changes live in:
  - config
  - hooks
  - skills
  - prompt overlays
  - routing/theme/layout data
- Prefer those over direct Rust source edits whenever possible.
- 2026-03-30: landed the first prompt-overlay seam for system-prompt customization without a rebuild.
  jcode now loads `~/.jcode/prompt-overlay.md` and `./.jcode/prompt-overlay.md` into the
  static prompt, which is a low-risk first step toward the broader issue #32 customization plan.

## Scenario Measurements (2026-03-24)

Touched-file `cargo check` samples gathered during this batch:

- `src/server.rs`: ~8.7s
- `src/tool/read.rs`: ~8.8s
- `src/auth/azure.rs` before Azure crate split: ~7.0s
- `src/provider/openrouter.rs` before Azure crate split: ~6.5s
- `src/provider/openrouter.rs` after Azure crate split: ~6.0s
- `src/notifications.rs` after notification-email crate split: ~11.4s
- `src/channel.rs` after notification-email crate split: ~4.8s
- `src/provider_catalog.rs` after provider-metadata split: ~5.8s
- `src/provider/mod.rs` after provider-core type split: ~50.1s
- `src/provider/openrouter.rs` after openrouter-support crate split: ~5.6s
- `src/provider/gemini.rs` after gemini-support crate split: ~5.5s

Notes:

- The post-split touched-file measurement for `src/auth/azure.rs` produced an anomalous
  result and should not be treated as a reliable ROI datapoint yet.
- The post-split `src/notifications.rs` timing is not by itself a negative signal: touching
  that root module still rebuilds the main crate, while the intended win is that unrelated edits
  stop dragging mail transport dependencies through the same compile unit.
- No-op fully hot-cache reruns can look unrealistically fast; use touched-file scenarios
  when evaluating structural compile-speed changes.
- Provider metadata timings should be interpreted as a first provider-side foothold, not the final
  provider ROI story; the larger wins should come from future provider-core / implementation splits.
- The `src/provider/mod.rs` touched-file timing remains high because touching that root file still rebuilds the
  main crate and the auth/runtime-heavy trait logic. This stage is about carving out stable reusable pieces first,
  not claiming that the provider root is solved.
- The `src/provider/openrouter.rs` touched-file sample is more encouraging because the heavy OpenRouter-specific
  catalog/ranking/cache support now lives in its own crate while the main module stays a thinner wrapper.
- The `src/provider/gemini.rs` touched-file sample is similarly encouraging: the serde-heavy Code Assist schema and
  pure model-list/support helpers now live outside the main crate while the runtime wrapper remains local.

## Dependency Hygiene Wins (2026-03-24)

- `global-hotkey` is now gated behind `target_os = "macos"` instead of being compiled on all
  platforms.
- This is a smaller win than a crate split, but it removes an unnecessary dependency subtree from
  Linux self-dev builds because the hotkey listener implementation is macOS-only.
- Validation: on Linux, `cargo tree -i global-hotkey` is now empty.

## Next-Boundary Assessment

The next obvious heavy dependency boundaries are less clearly safe/local than the ones already landed:

- provider support remains high-value, but `src/provider/mod.rs` and related implementations are
  broad enough that the next split should be designed carefully instead of rushed.
- a future `jcode-provider-core` / provider-implementation split is still the most promising next
  compile-speed move, but it needs boundary design first so high-churn shared types do not create
  a new invalidation hotspot.

Current provider-boundary stance:

- **Done:** `jcode-provider-metadata` for stable login/profile catalog data and pure selection logic.
- **Done:** `jcode-provider-core` for shared HTTP client plus route/cost/core provider value types.
- **Done:** `jcode-provider-openrouter` for OpenRouter-specific catalog/cache/ranking/model-spec support.
- **Done:** `jcode-provider-gemini` for Gemini Code Assist schema/types and pure model support helpers.
- **Done:** `jcode-provider-core::openai_schema` for pure OpenAI schema adaptation / strict-normalization helpers.
- **Not done yet:** `Provider` trait / `EventStream` extraction and fully standalone provider impl crates.
- **Reason:** the trait side still depends on `message.rs`, auth flows, runtime behavior, and provider-specific
  streaming logic; the current staged split avoids turning that unstable seam into a low-value high-churn crate.

That means the best next batch should likely target either:
- a carefully designed trait seam, or
- another provider implementation support split with similarly clean boundaries.

Current TUI-boundary stance:

- **Done:** `jcode-tui-workspace` for workspace-map model + widget rendering.
- **Not done yet:** broader `jcode-tui` extraction for markdown, mermaid, info widgets, and the shared renderer.
- **Reason:** the remaining high-value TUI files are larger but still more tightly coupled to `App`, config, images,
  side-panel state, and rendering orchestration, so they need staged extraction rather than a rushed top-level split.

## Developer Workflow Guidance

### Fast local cargo wrapper

Use:

```bash
scripts/dev_cargo.sh check --quiet
scripts/dev_cargo.sh build --release -p jcode --bin jcode --quiet
scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode --quiet
scripts/dev_cargo.sh --print-setup
```

For narrower feature-set probes, set `JCODE_DEV_FEATURE_PROFILE` instead of spelling out Cargo flags:

```bash
JCODE_DEV_FEATURE_PROFILE=minimal scripts/dev_cargo.sh check -p jcode --lib --quiet
JCODE_DEV_FEATURE_PROFILE=pdf scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode --quiet
JCODE_DEV_FEATURE_PROFILE=full scripts/dev_cargo.sh check -p jcode --lib --quiet
```

This is especially useful because default `jcode` enables both `embeddings` and `pdf`; in the current
dependency graph, the root tree is about **3740** lines with defaults, **1133** with PDF-only, and **1106**
with no default features. Use these profiles for measurements and local probes, while keeping full/default
builds in CI and release paths where feature coverage matters.

Developer-only root binaries are opt-in to keep `--all-targets` inner loops from compiling extra probe
entrypoints by default:

```bash
cargo run --features dev-bins --bin tui_bench -- --help
cargo run --features dev-bins --bin session_memory_bench -- --help
cargo run --features dev-bins --bin mermaid_side_panel_probe -- --help
cargo check --all-targets -p jcode --features dev-bins --quiet
```

The wrapper:

- uses `sccache` automatically when available
- prefers `lld` locally on Linux x86_64
- uses the fast `selfdev` Cargo profile for self-dev build/reload workflows
- can inject a named feature profile via `JCODE_DEV_FEATURE_PROFILE` unless explicit feature args are present
- avoids hard-forcing a linker mode that may be broken on a given machine
- can print the currently selected cache/linker setup with `--print-setup`

Override linker mode explicitly when needed:

```bash
JCODE_FAST_LINKER=lld scripts/dev_cargo.sh build --release -p jcode --bin jcode
JCODE_FAST_LINKER=mold scripts/dev_cargo.sh build --release -p jcode --bin jcode
JCODE_FAST_LINKER=system scripts/dev_cargo.sh build --release -p jcode --bin jcode
```

For compile timing, prefer repeatable touched-file measurements over no-op hot-cache reruns:

```bash
scripts/bench_compile.sh check --runs 3 --touch src/server.rs
scripts/bench_compile.sh check --runs 3 --touch src/tool/read.rs
scripts/bench_compile.sh release-jcode --runs 3
scripts/bench_compile.sh selfdev-jcode --runs 3
scripts/bench_compile.sh build -- --package jcode --bin test_api
scripts/bench_selfdev_checkpoints.sh --touch src/server.rs --runs 3
```

`bench_compile.sh` now supports:

- `--runs <n>` for repeated timings with min/median/avg/max summaries
- `--touch <path>` to simulate a local edit before each timed run
- `--json` for scriptable output
- `-- <extra cargo args>` to narrow the measured target/package/bin/features

`bench_selfdev_checkpoints.sh` builds on that foundation to produce a single standard
self-dev checkpoint bundle for cold/warm check + build comparisons.

## Stop Conditions

After each structural phase we should re-measure and ask:

- Did warm `check` time improve materially?
- Did warm `build` / reload-oriented build time improve materially?
- Did we reduce rebuild scope for common self-dev edits?

If not, we should avoid continuing high-churn refactors on compile-time grounds alone.
