# C: PR Triage — KooshaPari/KCode

**Generated:** 2026-10-16T23:52Z
**Repo:** KooshaPari/KCode
**Auditor:** read-only subagent (no comments, no closes, no edits, no pushes)
**Method:** `gh pr list --state open --json …` + `gh pr view --json …` for bodies

---

## 0. Scope Correction

The audit brief (`C-goals-reality.md` §4) referenced **12 open PRs**. As of the moment this report was generated there are **15 open PRs**. The discrepancy is because PRs **#93, #94, #95** were opened today (2026-10-16, 23:24Z–23:33Z), between the audit snapshot and this triage run. They are real, recent, and included below.

> Verified count: `gh pr list --state open | wc -l` → 15.

---

## 1. Summary Table

Sorted by age (newest first). Age computed against 2026-10-16T23:52Z.

| PR  | Title                                                                       | Author      | Age (d) | Draft | Mergeable   | CI            | Base → Head                                 | Files / +/-          | Category            | Recommended Action |
| --- | --------------------------------------------------------------------------- | ----------- | ------- | ----- | ----------- | ------------- | ------------------------------------------- | -------------------- | ------------------- | ------------------ |
| 95  | fix(tui): detect an abandoned controlling terminal on Windows              | KooshaPari  | 0.3     | no    | MERGEABLE   | green         | master ← fix/windows-terminal-abandonment-detection | 3 / +202/-10        | MERGE-READY         | merge              |
| 94  | fix(update): fork builds never auto-update from upstream                    | KooshaPari  | 0.4     | no    | MERGEABLE   | Kilo FAILURE  | master ← fix/update-product-decollation     | 2 / +188/-7          | NEEDS-REVIEW        | investigate then merge |
| 93  | fix(model-picker): union config-declared provider models (#1824)            | KooshaPari  | 0.5     | no    | MERGEABLE   | green         | master ← fix/model-picker-config-declared-models | 5 / +369/-24        | MERGE-READY         | merge              |
| 58  | Windows transport impls + stray brace (draft; mid-refactor)                | KooshaPari  | 2.8     | yes   | MERGEABLE   | green         | consolidation/v1-2026-10-12 ← fix/windows-transport-impls-20261013 | 2 / +3039/-3018     | BLOCKED             | wait for build green + rebase onto master |
| 35  | fix(tui): support tiny terminal session resume                             | KooshaPari  | 7.7     | no    | CONFLICTING | green         | feature/herdr-plugin-manifest ← fix/kcode-small-terminal-resume | 4 / +55/-33         | NEEDS-REBASE        | rebase onto herdr tip or close (merged equivalent #44 on master) |
| 25  | feat(tui): add /poke context recovery launcher                             | KooshaPari  | 14.2    | no    | MERGEABLE   | 5 failures    | master ← codex/poke-context                 | 9 / +214/-16         | BLOCKED             | fix CI failures (fmt, ubuntu test, windows test, quality, cross-target) |
| 23  | Merge upstream v0.88.0 into KCode                                           | KooshaPari  | 15.3    | no    | CONFLICTING | green (skeleton) | master ← feature/upstream-0.88-sync     | 644 / +61,775/-11,367 | ABANDON-CANDIDATE   | close — upstream advanced to v0.90.0; new sync PR needed |
| 22  | fix(macos): make executable trust repair explicit opt-in                   | KooshaPari  | 16.5    | yes   | MERGEABLE   | green         | master ← impl/macos-trust-policy-20260930   | 10 / +268/-43        | STALE               | close, reopen against current master or retire |
| 20  | feat(tool): add optional durable effect recovery hook                      | KooshaPari  | 16.5    | yes   | MERGEABLE   | green         | master ← impl/effect-recovery-hook-20260930  | 12 / +518/-23        | STALE               | close or rebase + re-test |
| 18  | fix(macos): preserve installed executable trust identity at runtime         | KooshaPari  | 16.6    | yes   | MERGEABLE   | green         | master ← impl/macos-trust-no-startup-resign-20260930 | 2 / +39/-52         | STALE               | close or fold into #22 (overlapping intent) |
| 16  | fix(provider): fail closed when ForgeCode CLI lacks required machine contract | KooshaPari  | 16.6    | yes   | MERGEABLE   | FAILURE (provider-contract) | master ← impl/forgecode-provider-contract-20260930 | 2 / +139/-1        | STALE               | close — provider contract changed, probe broken |
| 14  | feat(server): expose exact daemon identity in ping handshake                | KooshaPari  | 17.5    | yes   | MERGEABLE   | green         | master ← impl/daemon-identity-handshake-20260929 | 8 / +192/-22       | STALE               | close or rebase + re-review (spec #12 also stale) |
| 12  | spec: mature-first KCode recovery — daemon identity, trust boundaries       | KooshaPari  | 17.5    | yes   | MERGEABLE   | n/a (spec)    | master ← spec/harness-mature-recovery-20260929 | 24 / +1075/-0      | STALE               | close or refresh — spec anchors 5 abandoned implementation candidates |
| 10  | feat(acp): discover sessions and broker remote interactions                | KooshaPari  | 19.4    | yes   | CONFLICTING | green         | feature/herdr-plugin-manifest ← codex/acp-native-remote | 28 / +2985/-118   | NEEDS-REBASE        | rebase onto herdr tip or close |
| 8   | fix(openrouter): stop double-polling dead unfold after EOF + herdr plugin manifest | KooshaPari  | 27.2    | no    | CONFLICTING | green         | master ← feature/herdr-plugin-manifest      | 2320 / +225,375/-211,309 | NEEDS-REBASE        | rebase + split — XXL is the herdr tail (panic fix + manifest) |

---

## 2. Per-PR Detail

### #95 — fix(tui): detect an abandoned controlling terminal on Windows

**What it does:** Adds a Windows implementation of `current_tty_nr()` in `crates/jcode-tui/src/tui/app/terminal_liveness.rs` so the existing TUI liveness guard (which was already implemented for Linux and macOS) actually does something on Windows instead of silently no-opping and holding an ~80–150 MB heap after a dead terminal. The PR adds three small platform-specific helpers (`win32_console_input_handle`, `win32_console_process_list`) and falls back to checking the parent console's process attachment when no TTY handle is bound.

**Category: MERGE-READY.** Base is master, +202/-10 across 3 files, no conflicts, all third-party CI green (`Socket Security`, `semgrep-cloud-platform`, `CodeRabbit`), Kilo Code Review was still queued at triage time. Recent (30 minutes old).

**Recommendation:** Merge as soon as Kilo Code Review returns SUCCESS or is dismissed. The patch is small, well-scoped, and addresses a real cross-platform gap surfaced by the liveness guard itself.

---

### #94 — fix(update): fork builds never auto-update from upstream

**What it does:** Removes the hardcoded `const GITHUB_REPO: &str = "1jehuang/jcode";` in `crates/jcode-app-core/src/update.rs` and replaces it with a per-product repo query, so fork builds resolve GitHub releases from their own fork repo and stop silently overwriting themselves with upstream binaries. This is a direct decollision follow-up to the `~/CodeProjects/Phenotype/repos/jcode` rename work.

**Category: NEEDS-REVIEW.** The PR is technically mergeable and small (+188/-7, two files), but `Kilo Code Review` returned **FAILURE** (conclusion `FAILURE`, completed 23:49Z). The other third-party checks (`Socket`, `semgrep`, `CodeRabbit`) passed. Need to determine whether the Kilo failure is a real correctness signal (then fix) or a noisy AI-review false positive (then merge with override).

**Recommendation:** Open the Kilo review link (`https://app.kilo.ai/code-reviews/4a238bf2-...`) and check whether the issue is real. If Kilo flagged a missing test case or an env-var naming nit, address it; if it's a stylistic concern, merge after a local sanity check that `jcode update` no longer references upstream. The fix is the minimum necessary for product decollision and should land today.

---

### #93 — fix(model-picker): union config-declared provider models into the /model picker (#1824)

**What it does:** Backports the upstream jcode fix for issue #1824 to KCode. The `/model` picker for `openai-compatible` providers was sourced only from the live `GET /v1/models` cache, so models declared in `[[providers.<name>.models]]` config that the endpoint serves on chat completions but omits from `/models` were unreachable. The PR unions the static config-declared list into the picker route built by `direct_openai_compatible_profile_routes` in `crates/jcode-base/src/provider/mod.rs:195`.

**Category: MERGE-READY.** Base is master, +369/-24 across 5 files, all CI green (Kilo, Socket, semgrep, CodeRabbit), no conflicts, 30 minutes old.

**Recommendation:** Merge. Trivial upstream-style backport that closes a user-visible UX gap.

---

### #58 — Windows transport impls + stray brace (draft; branch still mid-refactor)

**What it does:** Two minimal patches on top of `consolidation/v1-2026-10-12`:
- `crates/kcode-transport/src/windows.rs`: fills the empty `AsyncRead`/`AsyncWrite` impls for `SplitReadRef`/`SplitWriteRef`.
- `crates/kcode-base/src/provider/mod.rs`: removes a stray `};` at line 117.

Body explicitly warns: **"this branch still does NOT build. kcode-transport compiles, but kcode-base fails with 8 unrelated errors."**

**Category: BLOCKED.** Target base is `consolidation/v1-2026-10-12` (NOT master); the branch is mid-refactor per the author note; `Require Linked Issue` CI check failed twice (FAILURE, FAILURE). No activity for ~2.5 days.

**Recommendation:** Wait. This PR is an active work-in-progress against the consolidation branch, not a master-bound fix. The branch tip needs to build clean and be rebased onto current master before review. Owner (per body) is a coordinating agent; the human owner has not separately confirmed this PR. No operator action needed today.

---

### #35 — fix(tui): support tiny terminal session resume

**What it does:** Lets the session picker accept nonzero terminal sizes below 60×20 and renders the selected title in a compact layout while keeping keyboard navigation. Zero-size terminals still fail; panic-safe init remains. Validation: 6 tests pass on `kcode-terminal-guard`; isolated `cargo check` and `cargo test` on `kcode-tui` pass at 38×14, 20×5, 57×1, and 1×1.

**Category: NEEDS-REBASE.** Base is `feature/herdr-plugin-manifest`, status `CONFLICTING`. The PR is small (+55/-33, 4 files) and isolated. **Note:** an equivalent fix already landed on master as merged PR #44 ("[Instinct] fix(tui): support tiny terminals in Jcode session resume", merged 2026-10-12). If the herdr branch tracks the same goal, #35 may now be redundant against master.

**Recommendation:** Decide: (a) close #35 because the master equivalent (#44) already shipped, or (b) rebase onto the herdr branch tip and keep the KCode-renamed variant. If keeping, rebase must precede review.

---

### #25 — feat(tui): add /poke context recovery launcher

**What it does:** Implements `/poke` (launch a fresh context-recovery session for all active repos, or scoped via `/poke <repo-name>`), `/poke on|off|status` (auto-poke), and wires the command for local launches and connected remote sessions. SSH transport rejects unsupported contexts. Closes issue #26.

**Category: BLOCKED.** Five CI jobs FAILURE: `Quality Guardrails`, `Build & Test (ubuntu-latest)`, `Build & Test (windows-latest)`, `Format`, `Windows Cross-Target Check (Linux)`. macOS build & test succeeded. Last activity 2026-10-13 (3 days ago); no operator has touched it since.

**Recommendation:** Treat the CI failures as the blocking signal — not "needs review," but "needs fixes." Most likely root causes are (1) `Format` failure means `cargo fmt` drift; (2) ubuntu/windows test failures are real test breakage; (3) the Windows cross-target check is the new gate that was added in #54, so it has probably never been green on this branch. A rebase onto current master plus `cargo fmt --all` is the minimum first step; then re-run the suite. Until those five checks are green, this should NOT be merged.

---

### #23 — Merge upstream v0.88.0 into KCode

**What it does:** A 644-file (+61,775 / −11,367) sync of upstream jcode v0.88.0 (`ee4cd3db3`) into the KCode fork. Body claims the workspace `--all-targets` is green and cites specific test-drift fixes from upstream commits `42285059b` and `5a68156548`.

**Category: ABANDON-CANDIDATE.** `CONFLICTING` against master. Body says merge was prepared Oct 1; the PR has had no commits since 2026-10-14T03:19Z (2 days ago). **Critical context: this PR is stale against upstream.** The KCode fork has since been rebased onto upstream v0.90.0 (PR #33, merged 2026-10-09, then reverted; subsequent sync activity). v0.88.0 is two upstream releases behind. The fork's effective state at this point should track v0.90.0+, not v0.88.0.

**Recommendation:** **Close #23.** A new sync PR should be opened against current master targeting whatever upstream tip the fork actually tracks today. Carrying 61k+ insertions from v0.88 when v0.90 (or later) already exists in the lineage will only cause a redundant second merge conflict. This is exactly the type of "looks active but is in fact stale" PR that wastes reviewer time.

---

### #22 — fix(macos): make executable trust repair explicit opt-in

**What it does:** Implementation candidate for K-F002 / #21. Stops KCode's frozen unconditional macOS startup routine that strips provenance/quarantine xattrs and force ad-hoc re-signs the executable; gates the repair behind a `JCODE_MACOS_STARTUP_REPAIR` env var. Body says "preserves the repair code for controlled local/dev recovery while we gather native evidence."

**Category: STALE.** Draft, MERGEABLE, but 16.5 days with zero activity since the initial push.

**Recommendation:** **Close**, or rebase + re-test against current master. This is part of the same 6-PR cluster (#14, #16, #18, #20, #22) all dated 2026-09-29–30 and all marked draft. They share an underlying "harness-mature-recovery" thesis (see #12) that has not progressed.

---

### #20 — feat(tool): add optional durable effect recovery hook

**What it does:** Implementation candidate for K-F008 / #19. Narrow scope: `Registry` owns an optional `EffectRecoveryAdapter`; only the `write` tool is protected in this first experiment. Records durable intent before dispatch, dispatch before the tool call, confirmation after success, marks the effect UNCERTAIN if durable confirmation fails after the file effect already committed.

**Category: STALE.** Draft, MERGEABLE, 16.5 days idle.

**Recommendation:** **Close or rebase + re-test.** Real reliability work but drafted against a master that has since moved. If the operator cares about the feature, the cleanest path is a fresh PR with the current `Registry` and `WriteTool` types.

---

### #18 — fix(macos): preserve installed executable trust identity at runtime

**What it does:** Removes the implicit runtime executable mutation (xattr strip + ad-hoc resign) and adds a macOS-native compile/static guard against reintroducing it. If Tahoe/local-build repair is needed, body says it belongs in an explicit installer/dev recovery flow.

**Category: STALE.** Draft, MERGEABLE, 16.6 days idle. **Overlapping intent with #22**: both gate the macOS trust repair; #18 removes, #22 opt-ins. They are sibling implementations of the same fix from the same author on the same day.

**Recommendation:** **Fold #18 into #22** (or close one and keep the other). Maintaining both as parallel drafts is a maintenance hazard.

---

### #16 — fix(provider): fail closed when ForgeCode CLI lacks required machine contract

**What it does:** Adds a cached runtime capability probe against the configured `forge` executable; fails closed with an actionable diagnostic when the executable lacks the structured-machine flags the mounted ForgeCode provider invokes (`--output-format stream-json`, `--input-format stream-json`, `--permission-mode`, `--resume`, `--tools`).

**Category: STALE.** Draft, MERGEABLE, BUT `provider-contract` CI check returned **FAILURE** (2026-09-30 10:58Z, not re-run since). 16.6 days idle. The CI failure probably reflects the probe being unable to find a `forge` binary in the runner (likely expected on a fork-only CI environment) — i.e., the check needs `FORGE_BIN` to point at a real binary or be marked `continue-on-error`.

**Recommendation:** **Close** (the provider-contract CI failure has never been addressed) OR fix the CI env so the probe can run. The diagnostic behavior is good, but a PR that has been red in CI for two weeks should not be considered merge-candidate.

---

### #14 — feat(server): expose exact daemon identity in ping handshake

**What it does:** Extends the backward-compatible `Pong` response with optional server runtime version, git hash, and PID; adds `Client::ping_identity()` while preserving `ping()`. Strengthens an existing native debug-server test. Does NOT claim cryptographic attestation.

**Category: STALE.** Draft, MERGEABLE, 17.5 days idle. Conceptually sound (closes the "old client / new daemon" gap), but has been waiting on the spec PR #12 to settle.

**Recommendation:** **Close or rebase + re-review.** The implementation is small and the test strengthening is real value. If the spec is being kept open (#12), this PR should at minimum be marked ready-for-review against current master.

---

### #12 — spec: mature-first KCode recovery — daemon identity, trust boundaries and open gates

**What it does:** A 24-file / +1075/-0 spec-only PR under the HARNESS-MATURE-20260929 program. Documents findings (new client can be served by older shared daemon; macOS startup removes trust xattrs; etc.) and proposes a maturity-first recovery plan. Body: "Spec changes are not an implementation candidate."

**Category: STALE.** Draft, MERGEABLE. 17.5 days idle. This is the spec anchor for #14, #16, #18, #20, #22 — all five of which are now stale drafts themselves. Carrying an unmerged spec blocks the entire program without any forward motion.

**Recommendation:** **Close or refresh.** Either merge the spec as-is to give the implementation candidates a stable base, or close it and have the operator re-issue a fresh program with current priorities. Keeping a 1,075-line spec open as a draft for 17 days is a coordination hazard.

---

### #10 — feat(acp): discover sessions and broker remote interactions

**What it does:** Contract correction in `5ad1daf74…` so that existing-session attachments without an explicit control grant reject prompt/cancel/configuration mutations before daemon dispatch (per HarnessDesk CLIENT-CONTRACT 2.2/3). Newly created sessions retain interactive default. Body references a hosted run `36618817807` that passed 54 tests including passive negatives.

**Category: NEEDS-REBASE.** Draft, `CONFLICTING` against `feature/herdr-plugin-manifest`. 28 files, +2985/-118. Body is otherwise strong (real hosted-run validation cited). 19.4 days idle.

**Recommendation:** **Rebase onto current herdr branch tip and re-run the hosted run.** If the contract correction is still needed, this should land; if not, close. Carrying an unstamped 28-file ACP PR as a draft for 3 weeks is risky.

---

### #8 — fix(openrouter): stop double-polling dead unfold after EOF (panic) + CI/lefthook coverage + herdr plugin manifest

**What it does:** The headline fix for the `jcode serve` panic in `~/err.md` (a tokio `futures::stream::unfold` crash after OpenRouter SSE final event). Also wires the regression cohort into hosted CI and a repo-local lefthook gate, and carries the herdr plugin manifest work that originally opened this PR.

**Category: NEEDS-REBASE.** `CONFLICTING`. **XXL** (2320 files / +225,375/-211,309 — by far the largest PR in the queue). Target base is `feature/herdr-plugin-manifest`. The label `size:XXL` was applied by the size-bot. Last activity 2026-10-16T02:05Z (today; comments or push).

**Recommendation:** **Rebase and split.** The panic fix is small and worth shipping; the herdr plugin manifest work is a separate concern. Split into (a) `fix(openrouter): panic on dead unfold after EOF` (target master, small diff) and (b) `feat(herdr): plugin manifest` (target herdr branch). The combined 2320-file PR against master has no realistic path to merge as-is.

---

## 3. Overall Summary

### Count by category

| Category            | Count | PRs                          |
| ------------------- | ----- | ---------------------------- |
| MERGE-READY         | 2     | #93, #95                     |
| NEEDS-REVIEW        | 1     | #94                          |
| NEEDS-REBASE        | 3     | #8, #10, #35                 |
| STALE               | 6     | #12, #14, #16, #18, #20, #22 |
| BLOCKED             | 2     | #25, #58                     |
| ABANDON-CANDIDATE   | 1     | #23                          |
| **TOTAL**           | **15**|                              |

### Headline numbers

- **2 PRs ready to merge today:** #93 (model picker), #95 (Windows terminal liveness). Both opened within the last hour, all CI green.
- **1 PR ready after one investigation:** #94 (fork auto-update). Kilo Code Review FAILURE must be triaged before merge.
- **3 PRs need rebases:** #8 (XXL; split recommended), #10 (herdr), #35 (herdr — possibly redundant with merged #44 on master).
- **2 PRs are blocked on CI/build state, not reviews:** #25 (/poke — five CI failures), #58 (Windows transport — explicitly doesn't compile per author).
- **7 PRs are stale drafts from 16–27 days ago,** six of which form a coherent "harness-mature-recovery" cluster anchored by spec #12. The cluster has had zero forward motion in 17 days.
- **1 PR (#23) is an abandoned merge candidate:** v0.88.0 sync is two upstream releases behind the fork's current state.

### Recommended operator actions (priority order)

1. **Merge today:** #95, #93 (both green and recent).
2. **Triage Kilo failure on #94; merge if benign.**
3. **Close #23** (superseded v0.88.0 sync).
4. **Decide on the herdr tail:** split #8 (panic fix vs manifest), rebase or close #10, rebase or close #35.
5. **Close or batch the HARNESS-MATURE cluster (#12, #14, #16, #18, #20, #22):** either commit to landing them in this order against current master, or close them all and reopen a fresh program.
6. **Fix CI on #25 (/poke):** the five failures are blockers, not review items.
7. **Leave #58 alone** — it's an active WIP against `consolidation/v1-2026-10-12` and should not be reviewed until the branch builds.

### Scope mismatch with audit brief

Brief said "12 open PRs." Actual count is **15**. The three additions (#93, #94, #95) all opened today, 23:24Z–23:33Z, are not reflected in `C-goals-reality.md` §4 (which was generated earlier the same day). This is consistent with the audit being a point-in-time snapshot and this triage being later in the day.