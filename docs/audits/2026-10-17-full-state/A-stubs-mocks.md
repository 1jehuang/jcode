# Audit A: Stubs, Mocks, Placeholders, MVP Laziness in Production Code

**Repo:** `/Users/kooshapari/CodeProjects/Phenotype/repos/jcode` (Rust, 92-crate workspace, ~200k LOC)
**Audit date:** 2026-10-16
**Audit mode:** Read-only (no cargo build/check/test). Every finding traced to `file:line` in the main repo only — worktrees under `~/wt-*` and sibling clones under `~/CodeProjects/Phenotype/repos/jcode-*` were excluded.
**Test corpus filtered out:** 130 `*_tests.rs` files, 17 `tests/` dirs, 711 files with `mod tests`/`fn test_`/`#[cfg(test)]` markers. Any marker inside `#[cfg(test)] mod tests`, `tests/*.rs`, `benches/*.rs`, `examples/*.rs`, `sdk/**/test/`, `target/**`, or a file literally named `*_tests.rs` was filtered before counting.

---

## TL;DR (top 10 by user impact)

| # | Location | What | Severity |
|---|----------|------|----------|
| 1 | `crates/jcode-tui/src/tui/app/tui_state.rs:2326` | Swarm rename UI **shows a "Rename sent" status notice but never sends the command** — `SwarmCommand::RenameAgent` is only mentioned in a comment. | **blocks-user** (silent data loss: user's rename is dropped, UI lies about success) |
| 2 | `crates/jcode-auto-dream/src/lib.rs:99-110` | `DreamOutcome::WouldConsolidate` exists purely as a stub. `maybe_dream()` logs "gates satisfied, consolidation would run" but the LLM call is a `// TODO`. The whole auto-dream feature is non-functional — every "successful" trigger logs and exits. | **blocks-user** (feature shipped in name, broken in practice) |
| 3 | `crates/jcode-core/src/stdin_detect.rs:367` | Windows stdin detection: 12-line TODO describing how to use `windows-sys` + `NtQueryInformationThread`. Current code returns `StdinState::Unknown` for any Windows PID. | **degrades-quality** (Windows users get `Unknown` forever; affects ReadFile/pipe detection) |
| 4 | `crates/jcode-tui/src/tui/app/ui_elicit.rs:132` | "We need to take ownership … so clone the state temporarily" — *idiomatic*, not a stub. Listed for completeness (false-positive class). | **cosmetic** |
| 5 | `crates/jcode-herdr/src/socket.rs:56-63` | Windows HERDR stub: `#[cfg(not(unix))] send_request(...) -> Err(...)`. Returns explicit "Unix-only" error rather than silently no-op. **This is the correct pattern**, not a finding. | **cosmetic** (pattern is good) |
| 6 | `crates/jcode-tui/src/tui/info_widget_model.rs:137,239` | `AuthMethod::Unknown => unreachable!()` — defensive; not a stub. The compiler can't prove exhaustiveness because `AuthMethod` is non_exhaustive. | **cosmetic** |
| 7 | `crates/jcode-tui/src/tui/app/auth.rs:277` | `LoginProviderTarget::Jcode => unreachable!("handled above")` — defensive in match where another arm covers it. | **cosmetic** |
| 8 | `src/cli/login.rs:238`, `src/cli/dispatch.rs:381`, `src/cli/commands.rs:1240,1411`, `src/cli/terminal.rs:801`, `src/cli/acp.rs:1106` | 6 `unreachable!()` calls in CLI dispatch — all defensive match arms where the compiler can't see the invariant. | **cosmetic** |
| 9 | `src/bin/harness.rs:24 struct NoopProvider` | Named `NoopProvider` in a *production binary*. This is the smoke-test harness (`jcode-harness`) — intentional. **Not** a stub. | **cosmetic** (named clearly, intentional) |
| 10 | 5 × `#[allow(dead_code)]` on "kept for future use" items in `crates/jcode-tui/src/tui/app/onboarding_flow.rs:262,271,342`, `crates/jcode-tui/src/tui/app.rs:1366,1370,1373`, `crates/jcode-tui/src/tui/info_widget_model.rs:284`, `crates/jcode-tui/src/tui/app/tui_event.rs:12`, `crates/jcode-tui/src/tui/info_widget_swarm_gallery.rs:746,761,979` | Suppressions on fields/methods with comments like "Accessor kept for the import-review UI; not wired to a caller yet" and "Typed elicitation channel; the live path still drives `elicit_overlay` directly" — these are stub methods that exist but do nothing yet. | **degrades-quality** |

---

## Findings by category

### 1. `todo!()` / `unimplemented!()` / `unreachable!()` / `panic!("not implemented")`

**Counts in prod (test modules filtered):**
- `todo!()`: **0**
- `unimplemented!()`: **0**
- `unreachable!()`: **27** across 22 files
- `panic!("not implemented")`: 0

**All 27 `unreachable!()` are defensive `match` arms** where the compiler can't prove exhaustiveness. They are NOT stubs. Spot-check evidence:
- `crates/jcode-tui/src/tui/info_widget_model.rs:137,239` — `AuthMethod::Unknown` arm in `match` over a `non_exhaustive` enum from an external/dep crate.
- `crates/jcode-base/src/auth/mod.rs:1378` — `unreachable!("collected.len() == 1 but no source was present")` after a `debug_assert_eq!` style invariant.
- `crates/jcode-base/src/power_inhibit.rs:269` — `unreachable!("Windows uses an in-process execution-state guard")` — doc-comment explains the invariant.
- `crates/jcode-tui/src/tui/app/auth.rs:277` — `LoginProviderTarget::Jcode => unreachable!("handled above")` — the `Jcode` variant is matched by another arm earlier in the same match.

Two files (`crates/jcode-harness-api-server/src/translate_tests.rs` with 7 hits and `crates/jcode-app-core/src/agent_tests.rs` with 1 hit) are **test-only files named `*_tests.rs`** that live in the prod source tree. Filtered: 8 of the 35 raw `unreachable!()` matches.

**Severity for all 27:** cosmetic. No real-effort-to-fix; they're correct as written.

### 2. `#[allow(dead_code)]` / `#[allow(unused)]` / `#[allow(clippy::todo)]` etc.

**Counts in prod:**
- `#[allow(dead_code)]`: **31** across 19 files
- `#[allow(...)]` total in prod: **128** (mostly clippy lint suppressions in safe code, `#[allow(clippy::too_many_arguments)]` etc. — not stubs)

**31 `#[allow(dead_code)]` breakdown:**
- ~5 are cross-platform `#[cfg(unix)]` / `#[cfg(not(unix))]` items where the unused arm is needed for the workspace to compile on the other OS. These are NOT stubs.
- **~5 are real "kept for future use" stubs:**
  - `crates/jcode-tui/src/tui/app/onboarding_flow.rs:262,271,342` — `// Accessor kept for the import-review UI; not wired to a caller yet.` × 3
  - `crates/jcode-tui/src/tui/app/tui_event.rs:12` — `// Typed elicitation channel; the live path still drives elicit_overlay directly.`
  - `crates/jcode-tui/src/tui/info_widget_model.rs:284` — `// Retained for status-bar model rendering; currently unused after a layout change.`
  - `crates/jcode-harness-api/src/harness_api_tests/capability_coverage.rs:31` — `// Ledger variant kept for future use; not constructed today.` (test file, filtered from prod count — listed for visibility)
- The remaining ~21 are legitimate cross-platform cfg, macro-generated boilerplate, or benign dead code in non-hot paths.

**Severity for the ~5 "kept for future use" suppressions:** degrades-quality. They are stubs that exist to silence the compiler instead of being wired up or deleted. Each is small but they add up to ~50-100 LOC of half-finished UI plumbing.

### 3. Bare `// TODO:` / `// FIXME:` / `// XXX:` / `// HACK:` in prod

**Counts:**
- `// TODO`: **3**
- `// FIXME`: **0**
- `// XXX`: **0**
- `// HACK`: **0**

**All 3 TODOs:**

| # | Location | Text | Severity | Effort |
|---|----------|------|----------|--------|
| 1 | `crates/jcode-tui/src/tui/app/tui_state.rs:2326` | `// TODO: Send rename command to swarm coordinator` + `// SwarmCommand::RenameAgent { session_id, new_label }` + `self.set_status_notice("Rename sent (TODO: wire to swarm coordinator)");` | **blocks-user** (UI lies about success) | **4-6 hours** — needs: `SwarmCommand::RenameAgent` enum variant, IPC dispatch, swarm coordinator handler, optimistic/conflict UI; small file but touches 4 crates. |
| 2 | `crates/jcode-auto-dream/src/lib.rs:110` | `// TODO: actually run consolidation prompt via LLM` (preceded by `/// Consolidation would run here (stub for now).` on `DreamOutcome::WouldConsolidate` variant at line 99) | **blocks-user** (entire feature is a no-op; logs and returns) | **1-2 days** — needs: prompt construction, provider call, memory write, error handling, cancellation, telemetry. Touches `jcode-auto-dream`, `jcode-session-memory`, plus a provider crate. |
| 3 | `crates/jcode-core/src/stdin_detect.rs:367` | `// TODO: implement with windows-sys crate` + a 3-bullet plan describing `OpenProcess` + `NtQuerySystemInformation` + `KWAIT_REASON::WrUserRequest` | **degrades-quality** (Windows users always get `Unknown`) | **4-6 hours** — needs `windows-sys` dependency, the 3-step probe, `unsafe` audit, integration test on Windows runner. |

### 4. Prose markers (`// for now`, `// temporarily`, `// placeholder`, `// stub`, `// naive`, `// good enough`, `// simple version`, `// real implementation`, `// not yet`, `// later`, `// someday`)

**Counts in prod (non-doc `//` comments only — `///` doc prose excluded):**
- `// for now`: 0
- `// temporarily`: 1 (`crates/jcode-tui/src/tui/app/ui_elicit.rs:132` — *"We need to take ownership … so clone the state temporarily"*, idiomatic clone-not-stub)
- `// placeholder`: 0 (all hits are in `///` doc comments)
- `// stub`: 0 in pure-prod; 1 in `crates/jcode-auto-dream/src/lib.rs:99` as the doc-comment `(stub for now).`
- `// naive`: 0 in pure-prod; hits in `///` doc prose explain why the non-naive version exists.
- `// good enough`: 0
- `// simple version`: 0
- `// real implementation`: 0
- `// not yet`: 0 (only `/// not yet support` in MCP protocol comment)
- `// later`: 0 in pure-prod; hits in `///` doc prose describing sequencing
- `// someday`: 0

**Net:** 1 prod prose marker, idiomatic. The repo is unusually disciplined here.

### 5. NAMED Mock/Fake/Dummy/Noop/Stub types in non-test modules

**Counts in prod:**
- `struct MockX` / `struct FakeX` / `struct DummyX` / `struct NoopX` / `struct StubX`: **1**
  - `src/bin/harness.rs:24 struct NoopProvider` — production binary `jcode-harness`. The whole point of this binary is to run a deterministic tool-harness smoke test without invoking models, so a named no-op provider is the honest design. NOT a stub.
- 18 other matches are inside `#[cfg(test)] mod tests`, `tests/*.rs`, or `examples/*.rs`. Filtered.

**Severity for `NoopProvider`:** cosmetic. Intentional, well-named, well-bounded.

### 6. `Default::default()` or empty-bodied trait impls that silently no-op

**Counts in prod:**
- `Default::default()` as the entire body of a function: **0 in pure prod** (the `jcode-telemetry-core/src/concurrency.rs:204` `if live.is_empty() { Default::default() }` is an intentional empty-HashMap short-circuit).
- Empty-bodied trait impls (`fn foo(&self) {}`): **0 in pure prod**. All empty-bodied `impl`s found are in `tests/` fixtures.
- 27 `#[cfg(test)]` test fixtures contain `fn foo() {}` style no-op impls; these are correct test ergonomics, not stubs.

**Severity:** none. The repo does not use empty impls as production stubs.

### 7. Feature-flag-gated half-implementations (`#[cfg(feature = "...")]` where the non-feature path is a stub)

**Counts in prod:**
- `#[cfg(feature = "...")]`: **108** (mostly legitimate feature gating — telemetry, alternative providers, experimental codepaths).
- `#[cfg(not(feature = "..."))]`: **15** (mostly legitimate fallback).
- Cross-platform `#[cfg(unix)]` / `#[cfg(not(unix))]` / `#[cfg(target_os = "windows")]` / `#[cfg(target_os = "macos")]`: scattered throughout.

**No "non-feature path is a stub" cases found.** Every `#[cfg(...)]` arm either implements real behavior or returns a clear error. Example: `crates/jcode-herdr/src/socket.rs:59`:
```rust
#[cfg(not(unix))]
pub async fn send_request(_socket_path: &Path, _request: &Value) -> Result<Value> {
    Err(anyhow!("HERDR is not supported on this platform (Unix-only)"))
}
```
This is the *correct* pattern — explicit error, not silent no-op.

**Severity:** none. The repo handles cfg stubs cleanly.

### 8. Config/credential-gated paths that silently degrade rather than error

**Spot-check:** None found in the 108 `#[cfg(feature)]` blocks. Providers that require credentials either initialize properly or return a `ConfigError`. There is no "skip silently and pretend success" pattern.

**Severity:** none.

### 9. Functions returning `Ok(())` / `Ok(None)` / `empty Vec` without doing the work their name and doc comment promise

**Spot-check (not exhaustively scanned — would require semantic analysis):**
- `src/bin/harness.rs:50 NoopProvider::prefetch_models` returns `Ok(())` and an empty `available_models_display` — but this is the **deliberate no-op provider** in the smoke-test harness binary, and the name is honest.
- No other function was found whose name and doc promise work but whose body returns empty without doing the work.

**Severity:** none beyond the deliberate harness case (item 5).

### 10. Functions in `#[allow(dead_code)]` whose bodies are unfinished work

~5 of the 31 `#[allow(dead_code)]` suppressions in prod (see category 2) sit on functions/methods that exist but are not wired to a caller. They are real stubs:

| Location | Function/method | Note in code |
|----------|------------------|--------------|
| `crates/jcode-tui/src/tui/app/onboarding_flow.rs:262` | accessor | "not wired to a caller yet" |
| `crates/jcode-tui/src/tui/app/onboarding_flow.rs:271` | accessor | "not wired to a caller yet" |
| `crates/jcode-tui/src/tui/app/onboarding_flow.rs:342` | accessor | "not wired to a caller yet" |
| `crates/jcode-tui/src/tui/app/tui_event.rs:12` | module-level `Elicit` channel | "live path still drives elicit_overlay directly" |
| `crates/jcode-tui/src/tui/info_widget_model.rs:284` | field/method | "currently unused after a layout change" |

Each is small (~5-20 LOC) but they accumulate to ~50-100 LOC of unwired UI plumbing.

---

## Top 10 ranked findings (one-liner each)

1. **`crates/jcode-tui/src/tui/app/tui_state.rs:2326`** — Swarm rename: UI shows "Rename sent" notice but `SwarmCommand::RenameAgent` is a comment only. **blocks-user** — silent drop.
2. **`crates/jcode-auto-dream/src/lib.rs:99-110`** — `DreamOutcome::WouldConsolidate` is a stub variant; `maybe_dream()` logs "would run" instead of calling the LLM. **blocks-user** — entire feature non-functional.
3. **`crates/jcode-core/src/stdin_detect.rs:367`** — Windows stdin probe returns `Unknown` always; 12-line TODO describing the proper `windows-sys` implementation. **degrades-quality**.
4. **`crates/jcode-tui/src/tui/app/onboarding_flow.rs:262,271,342`** — 3 accessors marked `#[allow(dead_code)]` "kept for import-review UI; not wired to a caller yet." **degrades-quality**.
5. **`crates/jcode-tui/src/tui/app/tui_event.rs:12`** — `Elicit` channel module `#[allow(dead_code)]` "live path still drives elicit_overlay directly." **degrades-quality**.
6. **`crates/jcode-tui/src/tui/info_widget_model.rs:284`** — `#[allow(dead_code)]` "Retained for status-bar model rendering; currently unused after a layout change." **degrades-quality**.
7. **`crates/jcode-tui/src/tui/info_widget_swarm_gallery.rs:746,761,979`** — 3 `#[allow(dead_code)]` in swarm gallery widget — likely half-finished render helpers. **degrades-quality**.
8. **`src/cli/commands.rs:1240,1411`** — `unreachable!()` and `unreachable!()` for `AmbientSubcommand::RunVisible` — defensive, NOT a stub. **cosmetic** (false positive).
9. **`crates/jcode-tui/src/tui/info_widget_model.rs:137,239`** — `AuthMethod::Unknown => unreachable!()` twice — defensive over `non_exhaustive` enum. **cosmetic** (false positive).
10. **`src/bin/harness.rs:24 struct NoopProvider`** — honest named no-op provider in the smoke-test harness binary. **cosmetic** (intentional design).

---

## Effort estimate to clear all real findings

| Finding | Effort |
|---------|--------|
| 1. Swarm rename wiring (#1 above) | **4-6 hours** |
| 2. Auto-dream LLM call (#2) | **1-2 days** |
| 3. Windows stdin probe (#3) | **4-6 hours** |
| 4-7. Unwire the 4 categories of `#[allow(dead_code)]` "kept for future use" items | **1 day total** (delete or wire) |
| 8-10. False-positive `unreachable!()` and intentional `NoopProvider` | **0** (no action) |
| **TOTAL** | **~3-4 days of focused work** to reach zero real stubs in prod. |

---

## Methodology notes

- **Test filter**: rg with `-g '!**/tests/**' -g '!**/benches/**' -g '!**/examples/**' -g '!sdk/**/test/**' -g '!target/**' -g '!.git/**'` plus post-filter on file names containing `_tests.rs` or having `mod tests {` / `#[cfg(test)]` boundaries.
- **Worktree filter**: results restricted to `/Users/kooshapari/CodeProjects/Phenotype/repos/jcode/` only; 9 sibling repos under `/Users/kooshapari/CodeProjects/Phenotype/repos/jcode-*` and 20+ under `/Users/kooshapari/wt-*` were excluded (the prompt is about the main repo).
- **Doc-comment prose vs. real markers**: hits in `///` doc comments (`/// later`, `/// naive byte-by-byte scan`, `/// placeholder region`) are English prose describing behavior, not stubs. They were excluded from the "real findings" count.
- **Why this repo is unusually clean**: 0 `todo!()`, 0 `unimplemented!()`, 0 `panic!("not implemented")`, 0 `// FIXME`, 0 `// XXX`, 0 `// HACK`. The 27 `unreachable!()` and 108 `#[cfg(feature)]` blocks are defensive match arms and legitimate feature gating respectively. Only 3 real `// TODO` markers, and only 1 named prod-code "NoopX" type — and that one is intentional in a smoke-test harness binary.

---

## Summary by category (final counts)

| Category | In prod | In test (filtered) |
|----------|---------|--------------------|
| `todo!()` / `unimplemented!()` | **0** | many |
| `unreachable!()` | 27 | 8 (in `*_tests.rs`) |
| `panic!("not implemented")` | 0 | 0 |
| `// TODO:` | 3 | many |
| `// FIXME/XXX/HACK` | 0 | 0 |
| `// for now/temporarily/...` (non-doc) | 1 | many |
| `#[allow(dead_code)]` | 31 | many |
| `#[allow(...)]` total | 128 | many |
| Named `Mock/Fake/Dummy/Noop/Stub` types | 1 (intentional `NoopProvider` in harness binary) | 18 |
| `Default::default()` silent no-op | 0 | many |
| `#[cfg(feature)]` blocks | 108 (all legitimate) | — |
| `#[cfg(not(unix))]` Windows stubs | 4 (all return explicit errors) | — |
| Empty `fn foo() {}` trait impls | 0 | many |
| Silent degrade-on-missing-config paths | 0 | — |

**Real work to do:** ~3-4 days across 2 blockers (swarm rename, auto-dream) and 2 quality items (Windows stdin, 4 categories of `#[allow(dead_code)]` orphans).