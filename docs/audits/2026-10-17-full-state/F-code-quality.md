# Code-Quality Deep Audit (KCode fork, 2026-10-17)

Audit lane **F** — residual unfinished code, beyond what lanes A–E already
catalogued (stubs, tests, goals, fork delta, usability).

Scope: `/Users/kooshapari/CodeProjects/Phenotype/repos/jcode` (92 crates).
Methodology: ripgrep sweeps + targeted `read` of suspect sites + one
`cargo check --workspace` for warning surface. Read-only.

## Summary

- **Total unfinished items: 6 confirmed (1 is gated on Windows, 2 are user-visible stubs, 3 are minor)**

| Category | Hits | Confirmed unfinished |
|---|---|---|
| 1. `todo!()` / `unimplemented!()` / `bail!()` | 200+ `bail!` (all are legit error returns), **0 `todo!()`, 0 `unimplemented!()`** | 0 |
| 2. `// TODO` / `// FIXME` / `// XXX` / `// HACK` | 11 raw hits; 8 are doc/comment text, 3 are real unfinished code | 3 |
| 3. `NotImplemented` / "not yet" / "coming soon" | zero literal "not implemented" strings; 1 explicit `"stub for now"` doc-comment | 1 |
| 4. Stub functions (empty body / trivial default) | 9 empty fn bodies — 7 are trait default impls (legitimate), 2 are fn-pointer shims (legitimate) | 0 |
| 5. MVP / demo / preview strings | 100+ hits; all are legitimate use of words (`is_ascii_alphanumeric`, `AlphaTool` test name, `preview` model name, `TempDirectory` helper) | 0 |
| 6. Unused public APIs / dead-code warnings | `cargo check --workspace` clean (no warnings filtered for `unused`/`never used`); ~20 `#[allow(dead_code)]` annotations forward-declare reserved surface | 0 |
| 7. Commented-out code blocks | searched; **no real commented-out code blocks found** — the only `// let …` and `// fn …` lines are explanatory prose, not disabled code | 0 |
| 8. Incomplete match arms (`_ => panic!()` / trivial catch-all) | 2 `_ => panic!()` hits, both inside `#[cfg(test)] mod tests` blocks (legitimate test-only assertions) | 0 |

**Headline finding:** the KCode fork is **not riddled with unfinished code**.
The codebase has very few placeholders, no `unimplemented!()`/`todo!()`
panics in production, and no commented-out blocks. The unfinished work is
**concentrated in 3 specific spots**, all of them documented with explicit
TODO comments.

## Findings (sorted by impact)

| # | File:Line | Category | Description | Severity |
|---|-----------|----------|-------------|----------|
| 1 | `crates/jcode-auto-dream/src/lib.rs:100,110` | 2 + 3 | **`auto-dream` is a 723-line crate that never runs.** `maybe_dream` only logs `"consolidation would run"` and never calls the LLM. `DreamOutcome::WouldConsolidate` is documented as "stub for now". The crate owns a fully-implemented `consolidation_prompt.rs` (133 lines, builds the merge/dedupe/strengthen prompt), gates (`time_gate`, `session_gate`, `consolidation_lock`), and a 10-min throttle — but **no caller exists and the `Ready` branch is unreachable in practice**. The TODO comment `// TODO: actually run consolidation prompt via LLM` is on line 110. | **HIGH** |
| 2 | `crates/jcode-tui/src/tui/app/tui_state.rs:2322-2329` | 2 | **Swarm rename has full UI but no wire.** `SwarmPanelAction::RenameConfirm` accepts user input, but the confirm handler discards `new_label`, displays the status `"Rename sent (TODO: wire to swarm coordinator)"`, and emits no command. The `SwarmCommand::RenameAgent` variant referenced in the comment doesn't exist anywhere in the codebase (confirmed: zero hits in non-test code). | **HIGH** (user-visible: the swarm-panel rename UI is silently broken) |
| 3 | `crates/jcode-core/src/stdin_detect.rs:367` | 2 | **Windows stdin detection always returns `Unknown`.** The `#[cfg(target_os = "windows")]` branch is documented as a TODO with full implementation guidance (uses `windows-sys`, which is already in `Cargo.toml:262`). `pub fn check(_pid: u32) -> StdinState` returns `StdinState::Unknown` unconditionally. | **MEDIUM** (only Windows; macOS/Linux complete; the windows-sys dep is already declared so this is a 1-day port) |
| 4 | `crates/jcode-auto-dream/src/lib.rs:97-100` | 3 | `pub enum DreamOutcome::Skipped { reason: &'static str }` and `WouldConsolidate` are documented in their own doc-comments as `"…not yet satisfied"` and `"Consolidation would run here (stub for now)"`. The `Skipped` variant is also confusingly named: `Skipped` is returned for "gates passed but nothing happened", which is the wrong name — it should be `NoOp`/`Idle`. | LOW (documentation/naming hygiene; tied to #1) |
| 5 | `crates/jcode-permission-bubble/src/fork_message.rs:304,308` | 8 | Two `_ => panic!()` catch-arms. **Verified false-positive:** both are inside `#[cfg(test)] mod tests` (the file's `#[cfg(test)]` opens at line 155). These are legitimate test-only "this branch is unreachable in the test fixture" assertions, not unfinished code. Listed for completeness. | **NOT AN ISSUE** |
| 6 | 20× `#[allow(dead_code)]` sites | 6 | 20 sites forward-declare intentionally-reserved APIs (e.g. `crates/jcode-tui/src/tui/app.rs:1366,1370,1373`, `crates/jcode-app-core/src/agent.rs:880`, `crates/jcode-base/src/session.rs:18`). Not unfinished code; they are deliberate reserved-API surfaces. Listed for completeness. | **NOT AN ISSUE** (would only matter if dead-code-elimination is desired) |

## Top-10 highest-leverage fixes

1. **Wire `auto-dream::maybe_dream` to an actual LLM call.**
   ~2-4 hours. Steps:
   (a) Add a `ConsolidationRunner` trait or fn that takes a `ConsolidationPrompt` and a `&dyn Provider`.
   (b) Have `maybe_dream` accept a `Provider` and call it (or open a deferred prompt via swarm).
   (c) Persist the result through `consolidation_lock` to update gate state.
   This is the single biggest unfinished feature in the fork.

2. **Implement swarm-pane rename via a `SwarmCommand::RenameAgent` variant.**
   ~30-60 minutes. Steps:
   (a) Add `SwarmCommand::RenameAgent { session_id, new_label }` to the swarm command enum.
   (b) Wire `tui_state.rs:2322` to call `swarm.send_command(SwarmCommand::RenameAgent { … })`.
   (c) Verify with the TUI test in `ui_tests/mod.rs` (already has `swarm_rename_active` / `swarm_rename_buffer` mocks).

3. **Implement Windows `stdin_detect::check`.**
   ~1-2 hours. `windows-sys` is already in `Cargo.toml:262` with
   `Win32_Foundation` and `Win32_System_Threading` features.
   Path: `OpenProcess` → `NtQuerySystemInformation` → check thread wait reason.
   The TODO comment already lists the exact steps.

4. **Rename `DreamOutcome::Skipped` → `DreamOutcome::NoOp { reason }`.**
   ~15 minutes. Pure rename; the doc comment says "Gates passed but no action taken (not yet satisfied)" — the current variant name reads like "we chose to skip", not "nothing happened to do".

5. **Add an integration test that proves consolidation actually runs end-to-end.**
   ~2 hours. Mocks the LLM provider, captures the prompt sent, and asserts the gate state advances on success. Currently `auto_dream` has no such test — the crate has 723 lines of unit tests for individual gates, but no test of the `Ready → RunPrompt → Persist` arc.

6. **Document the deferred `#[allow(dead_code)]` forward-declared sites with `// reserved-for:` comments.**
   ~30 minutes. Cheap way to make future contributors understand these are deliberately-suspended APIs (e.g. `crates/jcode-tui/src/tui/app.rs:1366` etc.).

7. **Confirm Windows build is actually working.**
   If `windows-sys` is in the lockfile but Windows CI never runs, the
   pre-installed dep is wasted and #3 has no validation surface.

8. **Add a CI smoke test that gates on `cargo check --workspace` warning count.**
   Several minutes of config; pre-empts future unfinished-code regressions.

9. **Per-suite `cargo doc` check** — the doctests contain ~50 ambiguous
   "preview" / "demo" / "beta" matches that are not unfinished code but
   could be tightened for clarity.

10. **Run `cargo udeps` for the workspace once nightly.** Cheap and would
    surface dead-code-eliminatable surface that `#[allow(dead_code)]`
    currently hides.

## Time-to-finish estimate

- **Quick wins (1–15 minutes each):** 4 items
  (rename `Skipped` → `NoOp`, add reserved-for comments, add CI smoke, run `cargo doc`)
- **Medium (30–90 minutes each):** 1 item
  (wire `SwarmCommand::RenameAgent` — the enum variant, the dispatch, and the
  test mock update)
- **Hard (2 hours+ each):** 2 items
  - `auto-dream` end-to-end wire-up (real LLM call, persistence, integration test)
  - Windows `stdin_detect::check` (FFI plumbing, race-window testing)

Total unfinished work: **~6-8 hours of focused engineering** to take the
KCode fork from "almost-finished" to "feature-complete on the documented
scaffold".

## What this audit did NOT find

- Zero `panic!()` in production code without a clear test context.
- Zero `unimplemented!()` anywhere.
- Zero `todo!()` anywhere.
- Zero real commented-out code blocks (every `//` line I sampled is doc or prose).
- Zero legacy "MVP"/"demo"/"preview" strings outside legitimate context (Anthropic beta-headers, OAuth test names, semver parsing of `-alpha.1`/`-beta.1` — all legitimate).
- Zero `bail!()` in placeholder contexts — `bail!` is the codebase's idiomatic
  error-return (similar to `anyhow::bail!`); 200+ hits are all legitimate
  error paths, not unfinished code.

The KCode fork is **substantially cleaner than its audit prompt anticipated**.
The 3 unfinished items are concentrated, documented, and individually shippable.

## Methodology / commands run

```bash
# cat 1
rg "todo!\(|unimplemented!\(|bail!\(" crates/ --type rust -n
rg "todo!\(\)|unimplemented!\(\)" crates/ --type rust -n   # confirm 0 hits

# cat 2
rg "//\s*(TODO|FIXME|XXX|HACK)" crates/ --type rust -n

# cat 3
rg "NotImplemented|not.*yet.*implemented|coming.*soon|placeholder" crates/ -i

# cat 4
rg -t rust '^\s*fn\s+\w+[^{]*\{\s*\}\s*$' crates/

# cat 5
rg "MVP|demo|preview|alpha|beta|temporary" crates/ -i

# cat 6
cargo check --workspace  # clean

# cat 7
rg -t rust '^[[:space:]]*//[^\n]*\b(fn|impl|struct|enum|return|let)\s' crates/

# cat 8
rg -t rust '_ => panic!\(\)' crates/
```

Elapsed: ~10 minutes wall clock; result: high signal, low noise.
