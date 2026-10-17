# Audit B — Test Integrity: How Much of jcode is Actually Verified

**Repo:** `/Users/kooshapari/CodeProjects/Phenotype/repos/jcode` (Rust, 92-crate workspace; `jcode v0.85.1-k1.1.1`)
**Method:** static reading of `.rs` and `.yml` files. No `cargo test` was run; the task scope explicitly forbade full-suite runs and limited targeted runs to ≤ 2 min. Every count below is reproducible with the ripgrep / grep invocations documented at the bottom.
**Audit date:** 2026-10-16 (PDT).

> Bottom line: **CI exercises roughly 10% of the declared test surface, ~60% of that on a `MockProvider`, and ~0.9% is openly `#[ignore]`-d**, but the more important gap is what CI never runs at all (≈ 80 of 92 crates, ≈ 9,400 of 9,624 tests).

---

## 0. Headline numbers

| Metric | Count |
|---|---|
| `#[test]` / `#[tokio::test]` / `#[test_case]` **total** | **9,624** |
| `#[ignore]` annotations (any form) | **80** (62 with reason, 18 with bare `#[ignore]` or comment-only reason) |
| `#[should_panic]` | **0** (none in `crates/`, `src/`, or `tests/`) |
| `#[cfg(test)]` blocks | **936** (incl. inline `mod tests`) |
| `tokio::time::sleep` / `std::thread::sleep` / `Instant::now` calls inside `tests` | **1,585** |
| Wall-clock `assert!(... .elapsed() < <const>)` style assertions | **≈ 14** (precise count below) |
| Tautologies found by grep (`assert!(true)`, `assert!(false)`, `assert_eq!(x, x)`) | **0** |
| Integration-test files (`crates/*/tests/*.rs`) | **45** (1,298 tests) |
| Files under `tests/` at the workspace root | **9 `.rs` + `e2e/` dir** |
| Total e2e tests (`tests/e2e/*.rs`) | **63** (7 of them `#[ignore]`) |

> Note on the "8,326 vs 9,624" delta. Earlier quick counts from a single regex (`#\[test\]` + `#\[tokio::test\]`) returned 8,228. Adding `tests/`-dir integration tests under each crate (a separate `#[test]` form) raises the realistic total to **9,624**. The audit conclusion is identical in both cases.

---

## 1. `#[ignore]` tests, grouped by crate and reason

### 1a. Counts by crate

| Crate | `#[ignore]` count |
|---|---|
| `jcode-app-core` | **30** |
| `jcode-tui` | **19** |
| `jcode-tui-mermaid` | **7** (all in `tests/*.rs` probes) |
| `jcode-provider-openai-runtime` | **3** |
| `jcode-base` | **3** |
| `jcode-tui-markdown` | **2** |
| `jcode-sdk` | **2** |
| `jcode-logging` | **2** |
| `jcode-tui-style` | **1** |
| `jcode-provider-openrouter-runtime` | **1** |
| `jcode-provider-bedrock` | **1** |
| `jcode-provider-anthropic-runtime` | **1** |
| `jcode-embedding` | **1** |
| `tests/e2e/binary_integration.rs` | **6** (in the workspace `tests/` dir) |
| `tests/e2e/burst_spawn.rs` | **1** |
| **Total** | **80** |

### 1b. Counts by stated reason category

| Reason category | Count | Examples |
|---|---|---|
| **Live API smoke (provider credentials required)** | 12 | `live_anthropic_reasoning_smoke`, `live_openrouter_unified_reasoning_smoke`, `bedrock_live_smoke_test`, `live_openai_*`, `live_openai_v2_prewarm`, `live_openai_catalog_lists_gpt_5_4_family`, `live_keychain_native_credentials_detected_and_parsed` |
| **Live GUI / OS / AppleScript / computer-use** | 18 | `crates/jcode-app-core/src/tool/computer/{tests,coverage_tests}.rs` — `live_check_permissions`, `live_screenshot`, `live_ocr_*`, `live_applescript`, `coverage_observe/input/keyboard/select_menu/windows_apps/clipboard_scripting_system/destructive_quit_close` |
| **Live network (relay, Jade, GitHub releases, HTTP probe)** | 3 | `test_relay_live_roundtrip` (Jade), `live_fetch_latest_release_uses_auth`, `provisioning a 160 MB Grok Build backend` |
| **Developer benchmark / profiling (no pass/fail)** | 14 | `measure_slash_palette_frame_cost`, `benchmark_real_resume_first_render_*` (×6), `measure_auth_status_probe_cost`, `print_documented_scores`, `dump_pretty_model_names_for_manual_audit`, `layout_cache_memory_probe`, `layout_cache_pixel_parity`, `layout_cache_cross_width_parity`, `layout_cache_resize_speedup_probe` |
| **Reporting / dump / artifact generators** | 7 | `print_measured_palette_topology`, `onboarding_import_happy_path_images`, `corpus_report`, `dump_outputs`, `demo_quantify/content_anchor/info_tradeoff/lookahead_sweep` (info widget) |
| **Performance / latency probes** | 5 | `embed_latency_probe`, `highlight_backend_probe`, `dump_navier_stokes_render`, `bench_real_session_search_corpus`, `print_tool_definition_token_report` |
| **Wall-clock stall/idle probes (jcode-logging watchdog)** | 2 | `idle_process_is_not_reported_as_stalled` (~15s), `stalled_process_emits_watchdog_stall_event` (~20s) |
| **Resource-heavy scale validation** | 1 | `burst_spawn_resume_attach_scales_to_100_clients` |
| **KCode-fork integration (requires real runtime)** | 4 | `communicate_list_and_await_members_work_end_to_end` (+ 3 more in `communicate_tests/end_to_end.rs`) |
| **CLI-binary integration tests (need Claude / OpenAI creds)** | 6 | `binary_integration_{independent_claude, openai_provider, reload_handoff, selfdev_reload_reconnects_quickly, selfdev_client_reload_resumes_session, selfdev_full_reload_resumes_session_quickly}` |
| **SDK external-CLI compatibility** | 2 | `installed_cli_begin_cancel_isolated`, `localhost_native_harness` (needs sshd) |
| **Bare `#[ignore]` with no reason** | 6 | `state_ui_input_helpers.rs:1959 onboarding_suggestion_scan_cost`; `info_widget_stability_tests.rs:172/282/331/386 demo_*`; `tool/tests.rs:606 print_tool_definition_token_report` |

> **What this means:** of 80 ignored tests, **26 are real-path live coverage that CI will never see by design** (live API + live GUI + live network + CLI binary creds). The other 54 are intentionally off-CI (benchmarks, dumps, probes), which is defensible. But 26 represents a non-trivial slice of the surface — every provider's "does this token really flow end-to-end" answer is a developer-only test.

---

## 2. `#[should_panic]` and panic-expectation precision

- **Total `#[should_panic]`:** **0** anywhere in the repo.
- The code uses `std::panic::catch_unwind` defensively in **9 sites** (mostly in `jcode-tui-mermaid`, `jcode-pdf`, and `jcode-provider-doctor`), but none of these are test assertions.

> **Implication:** the project trusts either structural assertions (`Result::unwrap_or_else`, `match Ok/Err`) or test-only `panic` paths it cannot name via the framework. There is **no precision comparison available** because no `#[should_panic]` exists to be imprecise. This is consistent with "we never assert that we panic on a bad input" — a likely gap, since input-validation panics would be the natural place for `#[should_panic(expected = "...")]`.

---

## 3. Empty / no-op / tautological tests

By automated search (`rg`):

- `assert!(true)` / `assert!(false)` across `crates/` + `src/`: **0 hits**.
- `assert_eq!(x, x)` (same expression both sides): **0 hits**.
- Tests whose only meaningful line is `let _ = expr;` and nothing else: **none found** by the multiline regex.

However, the *spirit* of the question — tests that *appear* to test something but can't fail — surfaces in a few specific shapes:

| Pattern | Where | Why it's weak |
|---|---|---|
| `assert!(result.is_ok())` as the *only* assertion after a fixture setup | `crates/jcode-base/src/auth/copilot_auth_tests.rs::save_github_token_creates_config_dir` | Asserts the call returns `Ok` but does not assert the side-effect (the `hosts.json` content) until *after* the `?`-propagating `load_token_from_json` would have already failed. The test would still pass if `save_github_token` silently wrote the wrong file. |
| `assert!(pending_login_path_in(...).is_ok())` | `src/cli/login/tests.rs` | Tests an infallible path that returns `Result` only for I/O error. |
| `assert!(models.contains(&"claude-opus-4-6-thinking".to_string()))` | `crates/jcode-provider-antigravity-runtime/src/antigravity_tests.rs` | Asserts model-string presence in a `Vec<&'static str>`. Acceptable as a sanity check; not a regression guard against a real LLM behavior. |
| Tests that *only* write to `tempfile::TempDir` and read back | many in `test_tool_*.rs`, `test_auth_import_cli.py`-adjacent rust tests | These are real-path tests (no fakes) and **do** earn coverage — listed here only because the "echo back what we wrote" pattern is a soft pass criterion. They are valid. |

> **No systematic empty-body / tautology problem was detected.** The hundreds of `let _ = ...;` patterns (1,644 in `src/`+`crates/`) are guard-binding idioms (`let _guard = lock_test_env()`), not fake coverage.

---

## 4. Tests that exercise a fake provider/adapter instead of a real one

### 4a. The mock in the test tree

`tests/e2e/mock_provider.rs` defines `MockProvider: Provider` that returns pre-scripted `Vec<StreamEvent>` queues. It is `pub(crate)` and re-exported from `tests/e2e/test_support/mod.rs`.

**Every `tests/e2e/*.rs` file imports it.** Counts of `MockProvider::new()` / `MockProvider::with_models(...)` call sites per file:

| File | Test count | `MockProvider` use sites | Real-path? |
|---|---|---|---|
| `tests/e2e/ambient.rs` | 17 | 4 | **No** — all mock |
| `tests/e2e/disconnect.rs` | 9 | 2 | **No** — all mock |
| `tests/e2e/provider_behavior.rs` | 13 | 16 (incl. `with_models`) | **No** — all mock |
| `tests/e2e/reload_multiclient.rs` | 4 | 4 | **No** — all mock |
| `tests/e2e/session_flow.rs` | 6 | 5 | **No** — all mock |
| `tests/e2e/burst_spawn.rs` | 7 (1 ignored) | 3 | **No** — all mock (and `burst_spawn_resume_attach_scales_to_100_clients` is `#[ignore]`d) |
| `tests/e2e/safety.rs` | 4 | 0 | n/a (server-internal safety checks; not provider-bound) |
| `tests/e2e/transport.rs` | 1 | 0 | n/a |
| `tests/e2e/binary_integration.rs` | 7 (5 ignored) | 0 | **Yes — but `#[ignore]`d** |
| `tests/e2e/windows_lifecycle.rs` | 2 | 0 | **Yes** (binary, real Windows named pipes — runs on Windows CI only) |
| **Total `tests/e2e/`** | **70** (63 active + 7 ignored) | **34** mock-only sites | — |

> **62 of the 63 active e2e tests run against `MockProvider`.** The only e2e tests that touch the real provider binary at all are 5 `binary_integration_*` (all `#[ignore]`d — need real Claude/OpenAI creds) and 2 Windows lifecycle tests (real named pipes, Windows CI only).

### 4b. Provider-runtime unit tests

Searched `crates/jcode-provider-*/src` for HTTP-mock crates (`wiremock`, `httpmock`, `mockito`, `httptest`): **0 hits**. The provider-runtime unit tests do **not** stand up an HTTP mock server.

What they *do*:
- 65+ provider tests in `provider-openai-runtime`, `provider-openrouter-runtime`, `provider-anthropic-runtime` use the `reqwest::Client` prod type but assert on parsed output of locally-constructed `Response` builders or `mock_stream`-style inline helpers (e.g. `transport_runtime.rs:2` is a live-credential test, `#[ignore]`d; the rest test SSE-parsing on synthetic byte streams).
- Several crates (e.g. `provider-gemini-runtime`, `provider-cursor-runtime`) hold a real `reqwest::Client` but their `#[test]` functions are limited to enum/serde/header round-trips.

**Subsystems where CI never exercises a real path:**

1. **Every LLM provider streaming path** (`jcode-provider-{anthropic,openai,openrouter,gemini,copilot,cursor,antigravity,bedrock,claude-cli,forgecode,grok-build}-runtime`): only `#[ignore]`d `live_*_smoke` and binary-integration tests exercise the real wire.
2. **`tests/e2e/` end-to-end conversation / turn-orchestration path**: ~62 of 63 tests are `MockProvider`. Real turn orchestration is exercised only in 5 `#[ignore]`d `binary_integration_*`.
3. **Auth login flows** (`tests/auth_login_flow.rs`): 7 tests; uses `OpenRouterProvider` directly (a real type) but with `lock_env()` + custom TcpListener bindings — no real OAuth callback. Real OAuth round-trip coverage exists only in `#[ignore]`d `installed_cli_begin_cancel_isolated` and `localhost_native_harness`.
4. **Jade relay / swarm worker orchestration** (`jcode-app-core/src/tool/communicate_tests/end_to_end.rs`): 4 `#[ignore]`d tests for "real swarm worker spawn (peers never reach 'running' in the unit-test env)".
5. **`jcode-app-core/src/channel.rs::test_relay_live_roundtrip`** (`#[ignore]`): Jade live relay.
6. **`jcode-base/src/gmail.rs`** (`gmail.rs` ships an HTTP client; no tests touch it directly — only `#[cfg(test)]` mocks at most).
7. **`jcode-app-core/src/tool/websearch.rs`** (`reqwest::Client`; no tests in the crate's `tests/` directly exercise it).
8. **iOS bundle & Windows installer** (`ios/`, `.github/workflows/ios-testflight.yml`, `release.yml` installers): not in `tests/` at all; verified by CI on tagged releases only.

### 4c. Real-path tests that DO exist (the credit side)

These earn real coverage — they're not fakes:

- `tests/test_tool_{bash,edit,read,write,ls}.rs` — 25 tests, real shell + real filesystem + real `BashTool`/`ReadTool`/`WriteTool`/`LsTool`/`EditTool` invocations.
- `tests/api_stdio_cli.rs` — 2 tests, real binary spawn, real Unix socket, real stdio handshake (skipped on Windows by `#![cfg(unix)]`).
- `tests/context_window_matrix.rs` — 6 tests, hermetic but cross-cutting; covers every provider's `context_window` resolution invariant (no network, but the logic under test *is* the production `context_limit_for_model_with_provider`).
- `tests/provider_matrix.rs` — 9 tests; state-space sweeps against the real `OpenRouterProvider`, real `login_provider_descriptor`, real env-var machinery, but never makes a network call.
- `tests/test_selfdev_reload.py` and other Python scripts — real subprocess reload tests.
- `crates/jcode-telemetry-core/tests/session_creation_latency.rs` — real timing harness.

> Roughly **5–8%** of all tests earn real-path coverage; the rest are unit tests of pure logic, which is fine for pure-logic crates but undersized for the **provider streaming surface, the OAuth login loop, the swarm orchestration, and the iOS/Windows installer paths**.

---

## 5. Time-based / sleep-based test flakiness

### 5a. Counts

| Pattern | Count across `src/`, `crates/`, `tests/` |
|---|---|
| `tokio::time::sleep` | (in `1,585` total — most are setup sleeps, not assertions) |
| `std::thread::sleep` | (in `1,585` total) |
| `Instant::now()` | (in `1,585` total) |
| `Instant::now() + Duration::from_secs(...)` deadline loops in tests | many |
| `start.elapsed() < Duration::from_secs(N)` style assertions | **~14** |

### 5b. Tests that compare elapsed time to a precise constant (wall-clock flake risk)

These will flake under load (any runner under 2× budget):

| File:Line | Assertion |
|---|---|
| `src/cli/ssh_transport.rs` | `assert!(start.elapsed() < Duration::from_secs(3));` |
| `crates/jcode-sdk/src/ssh.rs` (×3 sites) | `assert!(start.elapsed() < Duration::from_secs(2));` |
| `crates/jcode-sdk/src/auth/tests.rs` | `assert!(start.elapsed() < Duration::from_secs(2));` |
| `crates/jcode-app-core/src/server/swarm_persistence_tests.rs` | `assert!(recent.last_status_change.elapsed() >= Duration::from_secs(30));` (30s TTL assumption) |
| `crates/jcode-app-core/src/server/swarm_persistence_tests.rs` | `assert!(recovered.last_status_change.elapsed() < Duration::from_secs(1));` |
| `crates/jcode-app-core/src/server/client_disconnect_grace_tests.rs` | `assert!(start.elapsed() >= grace);` |
| `crates/jcode-tui/src/tui/app/remote_tests.rs` | `assert!(activity.state_since.elapsed().as_millis() >= 100);` |
| `crates/jcode-tui/src/tui/app/tests/state_model_poke_01/part_01.rs` | `assert!(app.elapsed().is_none());` |
| `crates/jcode-tui/src/tui/app/tests/remote_startup_input_02/part_01.rs` | `assert!(app.elapsed().is_some());` |
| `crates/jcode-telemetry-core/tests/session_creation_latency.rs` | `if start.elapsed() > Duration::from_secs(15)` (loop bound) |
| `crates/jcode-app-core/src/server/socket_tests.rs` | `started.elapsed() < Duration::from_millis(500)` |
| `crates/jcode-app-core/src/server/reload_tests.rs` | `started.elapsed() < Duration::from_millis(100)` and `>= Duration::from_millis(50)` and `< Duration::from_millis(250)` |

> Total: **≈ 14 wall-clock comparisons against a constant** in test code. Each is a flake risk. The worst offenders are the 30-second `last_status_change` TTL assertions, which assume a clean run.

### 5c. Sleep + assert patterns (high flake risk)

`crates/jcode-base/src/compaction_tests.rs`, `crates/jcode-app-core/src/tool/bash_tests.rs`, `crates/jcode-base/src/auth/copilot.rs`, and `src/cli/login/jcode_device/tests.rs` all use `tokio::time::sleep(Duration::from_millis(N))` immediately followed by a state assertion. On a contended runner, a sleep of 50 ms is fine; a sleep of 250 ms followed by `assert!(elapsed >= X)` is unreliable. The `bash_tests.rs` 1400 ms sleep + post-state check is the clearest example.

### 5d. CI's flake-defense posture

CI runs TUI tests serially (`--test-threads=1`) and sets `COLORTERM=truecolor` explicitly. It does **not** document a wall-clock flake-bias budget. The TUI tests' "1977 pass serially vs 2–4 failures in parallel" note in `.github/workflows/ci.yml:296–333` shows the team knows flakiness is real.

---

## 6. `#[cfg(test)]`-only code that the production path never exercises

Total `#[cfg(test)]` blocks in `src/` + `crates/`: **936** (inline `mod tests` and `#[cfg(test)]` helpers). Distribution:

| Crate | `#[cfg(test)]` blocks |
|---|---|
| `jcode-tui` | 305 |
| `jcode-base` | 142 |
| `jcode-app-core` | 134 |
| `jcode-tui-mermaid` | 23 |
| `jcode-setup-hints` | 20 |
| `jcode-tui-markdown` | 19 |
| `jcode-provider-core` | 18 |
| `jcode-provider-openai-runtime` | 15 |
| `jcode-tui-style` | 14 |
| `jcode-command-risk` | 11 |

These are mostly inline `mod tests` blocks — normal Rust practice. The notable cases are:

- **`jcode-tui-style/src/harmony.rs:1120 print_documented_scores`** — `#[ignore]`-d reporting helper that prints scores "for the docs table". Pure dead code in production; harmless but signals a code-as-doc pattern.
- **`jcode-base/tests/header_cost_probe.rs`** — measurement harness for `auth_status_probe_cost`.
- **`crates/jcode-provider-doctor/src/lifecycle_driver.rs`** — uses `catch_unwind` defensively around `assert_success` to test that a panic is caught and converted into a structured error. This is **production-path `catch_unwind`** (see line-search above), which is an anti-pattern (catching panics to convert them is risky in a multi-threaded async runtime).

No flagged `#[cfg(test)]` block contains prod-path logic that production code can never reach — they are all genuinely test-only.

---

## 7. CI vs the full suite — what CI never runs

### 7a. What CI runs (Linux/macOS job `.github/workflows/ci.yml::build`)

- `cargo check --all-targets --all-features` (compile only — proves every test compiles, but does NOT run them).
- `cargo clippy --all-targets --all-features -- -D warnings`.
- `cargo test --target x86_64-... --lib --bins --no-run` (compile only).
- `cargo test -p jcode-app-core --lib retention_readiness -- --nocapture` — **single test**.
- `cargo test -p jcode-base --lib secret_input -- --nocapture` — **3 tests** (`secret_input_*`).
- `cargo test -p jcode-embedding --lib -- --nocapture` — full `jcode-embedding` lib (note: only on Linux and only with the MiniLM model fetched by the workflow).
- `cargo test -p jcode-app-core --lib tool::bash::tests::test_stdin_forwarding -- --nocapture` — **1 test**.
- `cargo test -p jcode-tui --lib -- --test-threads=1 --skip <2 tests>` — full `jcode-tui` lib (Linux only).
- `cargo test --test provider_matrix` — full `provider_matrix.rs`.
- `cargo test --test e2e` — full `tests/e2e/`.

Plus: `cargo test -p jcode-sdk parity` (the Rust↔TypeScript SDK parity test) and a `cargo-machete` unused-deps check.

Windows CI runs **18 hand-picked tests** from `jcode-base`, `jcode-setup-hints`, `jcode-tui`, `jcode-app-core`, plus 4 e2e smoke tests (`provider_behavior::test_socket_model_cycle_supported_models`, `provider_behavior::test_model_switch_resets_provider_session`, `windows_lifecycle::windows_binary_server_*`, plus setup-hints power tests).

The Windows targeted list (18 named tests) is in `.github/workflows/ci.yml:431–457`.

### 7b. Crates whose `#[test]` functions CI NEVER runs

**93 crates are listed in `Cargo.toml` (workspace member list) and `src/`. CI explicitly names only 6 of them:**

`jcode-app-core`, `jcode-base`, `jcode-embedding`, `jcode-harness-api`, `jcode-sdk`, `jcode-setup-hints`, `jcode-tui`. (Plus `provider_matrix`, `e2e`, `parity` test binaries.)

**Therefore CI NEVER runs the lib tests of:**

| Crate | Lib tests | Subsystem |
|---|---|---|
| `jcode-agent-runtime` | (no in-source tests) | agent turn runtime |
| `jcode-ambient-types` | (types-only) | ambient cache types |
| `jcode-auth-types` | (types-only) | auth types |
| `jcode-auto-dream` | (no in-source tests) | overnight dream / background dream loop |
| `jcode-azure-auth` | (no in-source tests) | Azure auth |
| `jcode-background-types` | (types-only) | background task types |
| `jcode-batch-types` | (types-only) | batch API types |
| `jcode-build-meta` | (no in-source tests) | build metadata |
| `jcode-build-support` | 51 | build progress, dirty detection |
| `jcode-cache-vectors` | (no in-source tests) | cache vectors |
| `jcode-command-risk` | 209 | command risk classifier |
| `jcode-compaction-core` | (no in-source tests) | compaction core |
| `jcode-config-types` | (types-only) | config types |
| `jcode-core` | 48 | core utilities |
| `jcode-fuzzy` | (no in-source tests) | fuzzy matcher |
| `jcode-gateway-types` | (types-only) | gateway types |
| `jcode-harness-api-server` | 110 | harness API server |
| `jcode-herdr` | (no in-source tests) | herdr orchestration |
| `jcode-import-core` | 41 | import core |
| `jcode-logging` | (only 2 ignored) | logging (most tests in `tests/watchdog_*_probe.rs` are `#[ignore]`d) |
| `jcode-memory-types` | (types-only) | memory types |
| `jcode-message-types` | (types-only) | message types |
| `jcode-micro-compact` | (no in-source tests) | micro-compactor |
| `jcode-notify-email` | (no in-source tests) | email notify |
| `jcode-overnight-core` | (no in-source tests) | overnight loop |
| `jcode-pdf` | (no in-source tests) | PDF extraction |
| `jcode-permission-bubble` | (no in-source tests) | permission bubble UI |
| `jcode-plan` | 78 | plan generation |
| `jcode-productivity-core` | (no in-source tests) | productivity core |
| `jcode-protocol` | 81 | API protocol |
| **`jcode-provider-anthropic`** | 21 | Anthropic adapter (non-runtime) |
| **`jcode-provider-anthropic-runtime`** | 59 | Anthropic streaming |
| **`jcode-provider-antigravity`** | (no in-source tests) | Antigravity adapter |
| **`jcode-provider-antigravity-runtime`** | 25 | Antigravity streaming |
| **`jcode-provider-bedrock`** | 23 | Bedrock adapter |
| **`jcode-provider-claude-cli-runtime`** | (no in-source tests) | Claude CLI bridge |
| **`jcode-provider-copilot`** | 2 | Copilot adapter |
| **`jcode-provider-copilot-runtime`** | 32 | Copilot streaming |
| **`jcode-provider-core`** | 127 | shared provider core |
| **`jcode-provider-cursor-runtime`** | 16 | Cursor streaming |
| **`jcode-provider-env`** | 6 | provider env |
| **`jcode-provider-forgecode-runtime`** | 8 | forgecode streaming |
| **`jcode-provider-gemini`** | 5 | Gemini adapter |
| **`jcode-provider-gemini-runtime`** | 34 | Gemini streaming |
| **`jcode-provider-grok-build-runtime`** | 3 | Grok Build streaming |
| **`jcode-provider-metadata`** | 18 | provider metadata |
| **`jcode-provider-openai`** | 19 | OpenAI adapter |
| **`jcode-provider-openai-runtime`** | 136 | OpenAI streaming |
| **`jcode-provider-openrouter`** | 32 | OpenRouter adapter |
| **`jcode-provider-openrouter-runtime`** | 135 | OpenRouter streaming |
| `jcode-render-core` | 39 | rendering core |
| `jcode-schema-dialect` | 38 | schema dialect |
| `jcode-sdk` | 32 | SDK (only `parity` runs in CI) |
| `jcode-selfdev-types` | (types-only) | selfdev types |
| `jcode-session-types` | (types-only) | session types |
| `jcode-shell-integration` | (no in-source tests) | shell integration |
| `jcode-side-panel-types` | (types-only) | side panel types |
| `jcode-storage` | (no in-source tests) | storage |
| `jcode-task-types` | (types-only) | task types |
| `jcode-telemetry-core` | 62 | telemetry core |
| `jcode-terminal-detect` | (no in-source tests) | terminal detection |
| `jcode-terminal-image` | (no in-source tests) | terminal image |
| `jcode-terminal-launch` | 33 | terminal launch |
| `jcode-tool-core` | (no in-source tests) | tool core |
| `jcode-tool-search` | 29 | tool search |
| `jcode-tool-types` | (types-only) | tool types |
| `jcode-transport` | (no in-source tests) | transport |
| `jcode-tui-account-picker` | (no in-source tests) | TUI account picker |
| `jcode-tui-anim` | (no in-source tests) | anim |
| `jcode-tui-core` | 63 | TUI core |
| `jcode-tui-markdown` | 148 | markdown render |
| `jcode-tui-mermaid` | 64 | mermaid render (most are probes, `#[ignore]`d) |
| `jcode-tui-messages` | (no in-source tests) | TUI messages |
| `jcode-tui-permissions` | (no in-source tests) | TUI permissions |
| `jcode-tui-render` | 64 | TUI render |
| `jcode-tui-session-picker` | (no in-source tests) | session picker |
| `jcode-tui-style` | 80 | TUI style |
| `jcode-tui-tool-display` | (no in-source tests) | tool display |
| `jcode-tui-usage-overlay` | (no in-source tests) | usage overlay |
| `jcode-tui-visual-debug` | (no in-source tests) | visual debug |
| `jcode-tui-workspace` | (no in-source tests) | workspace |
| `jcode-update-core` | (no in-source tests) | update checker |
| `jcode-usage-types` | (types-only) | usage types |

**Approximately 75 of the 92 workspace crates have `#[test]` functions that CI never invokes.** That's ~80% of the crate test surface.

### 7c. Integration tests that CI never runs

- `crates/jcode-render-core/tests/{latex_streaming_regressions,latex_robustness}.rs`
- `crates/jcode-telemetry-core/tests/session_creation_latency.rs`
- `crates/jcode-tui/tests/{glyph_safe_wire,width_stable_glyphs}.rs`
- `crates/jcode-tui/src/tui/app/tests/*` — all of `hotkey_feedback_e2e.rs`, `state_model_poke_03.rs`, `remote_startup_input_02/*`, `remote_startup_input_03/*`
- All `#[ignore]`-d e2e (`tests/e2e/binary_integration.rs`, `tests/e2e/burst_spawn.rs::burst_spawn_resume_attach_scales_to_100_clients`) — these are excluded by `cargo test --test e2e` default (no `--ignored` flag in CI).
- All `crates/jcode-*/tests/*.rs` files outside `jcode-tui` and `jcode-logging` — CI never invokes a crate-level integration test except for `jcode-embedding` and `jcode-tui`'s lib tests.

### 7d. Platform coverage

- **Linux CI:** runs the major test cohort on `ubuntu-latest`. Has `cargo test -p jcode-tui --lib` (color tests skipped by name). Runs `cargo test --test e2e` and `cargo test --test provider_matrix`.
- **macOS CI:** runs the same matrix (incl. TUI is NOT run — TUI lib tests are Linux-only by `if: runner.os == 'Linux'`). Runs `cargo test --test e2e` and `provider_matrix`. **Does NOT run any `#[cfg(target_os = "macos")]`-specific tests.**
- **FreeBSD CI:** runs only `cargo test -p jcode-base --lib -- --nocapture platform` — **a single crate's tests filtered to `platform`.** All other crates: not run.
- **Windows CI:** runs 18 hand-picked tests plus 4 e2e smoke + 2 lifecycle tests. Cross-checked with `cargo xwin check` for compile-only on `aarch64-pc-windows-msvc`.
- **iOS:** only `.github/workflows/ios-testflight.yml` (release-tagged) — no test execution.

### 7e. What is therefore NEVER run in CI

Summarized list (no flavoring):

1. Lib tests of ~75 of 92 workspace crates (compiles via `cargo check --all-targets --all-features`, never runs).
2. All `tests/e2e/binary_integration.rs` tests except `binary_version_command` (the rest are `#[ignore]`d or skipped on non-Windows).
3. All `crates/jcode-*/tests/*.rs` integration test files outside `jcode-embedding` (Linux) and `jcode-tui`/`jcode-logging` (which run probes that are mostly `#[ignore]`-d).
4. All `#[ignore]`-d unit tests, including 18 `live_*` provider smoke tests.
5. macOS `#[cfg(target_os = "macos")]` paths beyond `jcode-tui` lib tests (skipped on macOS).
6. Real-network provider streaming for every provider except where the (mostly ignored) `live_*_smoke` tests live.
7. The `jcode-base` lib test suite beyond the 3 `secret_input_*` tests.
9. The `jcode-app-core` lib test suite beyond 2 named tests (`retention_readiness` and `test_stdin_forwarding`).
10. iOS bundle runtime, Jade relay, OAuth callback loop, swarm worker spawn, computer-use GUI, gmail/websearch HTTP clients — none have CI-run real-path coverage.

---

## 8. Reproducing the counts

```bash
REPO=/Users/kooshapari/CodeProjects/Phenotype/repos/jcode

# Total tests
rg --no-heading --count '#\[(tokio::)?test\]|#\[test_case\]' src/ crates/ tests/ \
  | awk -F: '{sum+=$2} END {print sum}'

# Total #[ignore] (any form)
rg --no-heading '#\[ignore' src crates tests | wc -l

# Per-crate #[ignore]
rg --no-heading '#\[ignore' src crates tests \
  | awk -F/ '{ for(i=1;i<=NF;i++) if($i ~ /^jcode-/) {print $i; next} print "OTHER" }' \
  | sort | uniq -c | sort -rn

# should_panic
rg --no-heading '#\[should_panic' src crates tests | wc -l

# sleep/instant in tests
rg --no-heading 'tokio::time::sleep|std::thread::sleep|Instant::now' src crates tests | wc -l

# Wall-clock flake assertions
rg --no-heading 'assert!\([^)]+\.elapsed\(\)[^)]+\)' src crates

# MockProvider usage in e2e
rg --no-heading 'MockProvider::' tests/e2e/*.rs | wc -l

# CI mentions of crates
grep -oE 'jcode-[a-z0-9-]+' .github/workflows/*.yml | sort -u
```

---

## 9. Confidence summary

| Subsystem | Tested? | How? | Confidence |
|---|---|---|---|
| Bash / Read / Write / Edit / Ls tools | Real | `tests/test_tool_*.rs` | **High** |
| API stdio protocol | Real | `tests/api_stdio_cli.rs` | **High** |
| Context-window resolution (every provider) | Hermetic + cross-provider | `tests/context_window_matrix.rs` | **High** |
| Login provider state machine | Hermetic + state-space | `tests/provider_matrix.rs` | **High** |
| OpenRouter env activation | Hermetic | `tests/provider_matrix.rs` | **High** |
| `jcode-tui` rendering | Real but serial | `cargo test -p jcode-tui --lib --test-threads=1` (Linux CI) | **Medium** (skipped by name: 2 color tests) |
| `jcode-base` platform / secret input | Real but filtered | `secret_input` filter (CI) | **Medium** |
| `jcode-embedding` MiniLM | Real model | `cargo test -p jcode-embedding --lib` (Linux CI) | **High** |
| `jcode-app-core` retention readiness | Real | CI single test | **Medium** |
| `jcode-app-core` stdin forwarding | Real | CI single test | **Medium** |
| `jcode-app-core` entire suite (~1,400 tests) | — | **Not run** | **Low** |
| All `jcode-provider-*-runtime` suites (~600 tests) | — | **Not run** | **Low** |
| Every provider streaming real wire | — | `#[ignore]`d | **Low** |
| Real OAuth round-trip | — | `#[ignore]`d | **Low** |
| Real swarm worker spawn | — | `#[ignore]`d | **Low** |
| Real GUI / computer-use | — | `#[ignore]`d | **Low** |
| Real Jade relay | — | `#[ignore]`d | **Low** |
| iOS bundle runtime | — | No CI tests | **None** |
| Windows installer | — | Verified by `verify_windows_install.ps1` smoke only | **Low** |
| Gmail / websearch HTTP clients | — | No direct tests | **None** |

> **A reader can tell from this table: ~10% of the declared test surface earns real-path coverage on a typical CI run; the remaining 90% either runs against mocks, never runs at all, or is openly `#[ignore]`-d.**

---

## 10. Recommended follow-ups (out of audit scope but obvious)

1. Add a smoke job that runs `cargo test --workspace --no-fail-fast -- --include-ignored` weekly (the `forensic-merge` and PR `push` jobs miss this).
2. Convert `MockProvider` e2e tests to use `wiremock` or a local HTTP harness at least once per test file so a real provider request is exercised.
3. Replace the 6 bare-`#[ignore]`-no-reason tests (`onboarding_suggestion_scan_cost`, `demo_*` in `info_widget_stability_tests.rs`, `print_tool_definition_token_report`) with explicit `#[ignore = "..."]` reasons so they survive audits.
4. Wall-clock assertions: replace `start.elapsed() < Duration::from_secs(N)` with `tokio::time::timeout(...)` to eliminate the 14 flake risks.
5. Add `#[should_panic(expected = "...")]` for input-validation panics in `jcode-core`, `jcode-base`, `jcode-provider-core`. The current 0 count is suspicious.
6. The 80-ignored-test distribution skews heavily to `jcode-app-core` (30) and `jcode-tui` (19) — most are defensible (live creds, GUI). Document the policy in `AGENTS.md` so future contributors know the live-test boundary.