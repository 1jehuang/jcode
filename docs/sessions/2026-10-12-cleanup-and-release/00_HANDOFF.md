# HANDOFF — Jcode Cleanup-and-Release Arc

**Written:** 2026-10-15T21:35Z by orchestrator (B.8.87 audit + handoff, replaces per-operator handoff request)
**Audience:** Fresh orchestrator context resuming full project ownership of `KooshaPari/Jcode`
**Read time target:** 5 min scan, then act

---

## TL;DR

Master at `60ef2d2a3` (B.8.86 audit, 0 ahead/behind origin). 0 errors, 0 warnings, 3/3 `restore_session` tests pass.

**2 GATES FAILING** (was 1 in standing memory):
1. `check_code_size_budget.py` — pre-existing, `crates/jcode-tui/src/tui/mod.rs` 2072→2077 (from `f396d0fd7`, since B.8.74)
2. `cargo fmt --check` — **NEW FINDING, 14 files with drift, undetected because master CI has not run since 2026-10-14T10:20:15Z (35h+ ago) and that run FAILED**

**P7 release (v0.96.0-k1.0.0) is BLOCKED** on fmt cleanup. Audit plan explicitly says "After fmt-cleanup merges" (B.8.6).

12 open PRs, all operator-owned (2 MERGEABLE+ready, 4 CONFLICTING, 6+ DRAFT). Rooster (Tier 3 recon) status=ready, report captured below.

---

## CRITICAL NEW FINDINGS (this session)

### Finding 1: 14 cargo fmt drift files on master

Reproducible on stable rustfmt. The list:

| # | File | Line |
|---|---|---|
| 1 | `crates/jcode-app-core/build.rs` | 22 |
| 2 | `crates/jcode-app-core/src/tool/bash.rs` | 513, 530 |
| 3 | `crates/jcode-app-core/src/tool/discover.rs` | 670 |
| 4 | `crates/jcode-base/src/provider_activity_oauth.rs` | 123 |
| 5 | `crates/jcode-command-risk/src/heredoc.rs` | 26 |
| 6 | `crates/jcode-command-risk/src/redirection.rs` | 37, 50 |
| 7 | `crates/jcode-provider-grok-build-runtime/src/bin/fake_acp.rs` | 5, 137 |
| 8 | `crates/jcode-sdk/src/auth.rs` | 393, 414 |
| 9 | `crates/jcode-tui/src/tui/app/auth_account_picker.rs` | 758 |
| 10 | `crates/jcode-tui/src/tui/ui_inline_interactive.rs` | 439 |

PR #75 (`a1e953097`, "chore(fmt): cargo fmt cleanup + clippy followup") was merged in B.8.84 and IS an ancestor of master, but the drift was reintroduced by later PRs (#76/#77/#78/#80 etc.) without a follow-up `cargo fmt` pass.

**Fix:** `cargo fmt` is a one-liner. Suggested action: `cargo fmt && git add -u && git commit -m "chore(fmt): drift cleanup"`. Then a fresh CI run will confirm clean.

### Finding 2: Master CI is broken / silent for 35h+

- Last master push CI: `id=38548850946` (run #405), started 2026-10-14T10:20:15Z, conclusion **"failure"**
- Last commit tested: `29c85cbdf7` (the ratchet refresh commit)
- Master is now 42 commits AHEAD of that (`60ef2d2a3` HEAD) — 7 PRs merged since + 7 audit docs, **none CI-validated directly on master**
- Subsequent CI runs (`38644605215`, etc.) are all on `fix/ci-unused-deps` branch, NOT on master
- This means **every PR merged since B.8.66 has been validated only on its own branch, never on master itself**
- The ratchet commit's own message says "all ratchets pass on master" but the CI failed — suggests the Format check (or some other gate) is the actual failure, NOT a ratchet

**Fix candidates (TBD by fresh context):**
- Check the failed CI run's job logs to identify the actual failure
- Trigger a re-run on master with the 14-fmt-fix commit
- Investigate whether push-to-master CI is configured or if the workflow only runs on PRs

### Finding 3: 7/9 gates pass (not 7/8 as standing memory said)

Two failures:
1. `check_code_size_budget.py` — known, `tui/mod.rs` 2077 (pre-existing, 5-line overshoot from B.8.74)
2. `cargo fmt --check` — NEW (Finding 1)

Both must be addressed before P7 release is safe.

### Finding 4: All 12 open PRs are operator-owned (no external contributors)

This matches B.8.5's finding (the only "ready" PR that's independently mergeable is #25, and #34 is chained to #8). No action needed from the orchestrator — the operator owns the decisions.

---

## State snapshot

| Item | Value |
|---|---|
| Master HEAD | `60ef2d2a3` (B.8.86 audit) |
| origin/master | `60ef2d2a3` (0 ahead/behind) |
| Working tree | clean (0 untracked, 0 modified, 0 stash) |
| Current Cargo.toml version | `0.85.1-k1.1.1` |
| Latest KCode tag | `v0.90.0-k1.0.0` (on `feature/rebase-onto-v0.90.0` branch, NOT on master) |
| All KCode tags | `v0.90.0-k1.0.0`, `v0.85.1-k1.1.0`, `v0.85.0-k1.0.0` |
| Total tags | 220 |
| `cargo check --workspace` | 0 errors, 0 warnings |
| `restore_session` tests | 3/3 pass |
| `check_warning_budget.sh` | current=0 baseline=0 |
| `check_code_size_budget.py` | FAIL (tui/mod.rs 2077) |
| `cargo fmt --check` | FAIL (14 files) |
| Operator's most recent branch | `chore/xirp-custom-harness-discovery-20261014` (3 min old, XIRP portal work) |

---

## Recent arc (B.8.77 — B.8.87)

| ID | When (Z) | What |
|---|---|---|
| B.8.77 | 2026-10-13 | Corrected panic-prone ratchet definition (A/B/C) |
| B.8.78 | 2026-10-13 | Master re-baseline (3 PRs merged) + Ratchet A → 0 |
| B.8.79 | 2026-10-13 | PR #69 merged |
| B.8.80 | 2026-10-13 | PR #74 ratchet tightening 140→62 |
| B.8.81 | 2026-10-13 | 4 untracked operator dirs analyzed, m3.1 fix anomaly, external PR flood |
| B.8.82 | 2026-10-14 | m3.1 fix ANOMALY RESOLVED — fix IS in master under SHA `6eb00b35d` |
| B.8.83 | 2026-10-14 | Resolution arc — 4 dirs committed, cargo check green, PRs #64/#67/#75 ready |
| **B.8.84** | 2026-10-15T08:02Z | **3-PR merge arc — #64, #67, #75 squash-merged via harness-compliant path** |
| B.8.85 | 2026-10-15T08:16Z | Fast-forward to peer-merged PR #78 (file-size ratchet defense) |
| **B.8.86** | 2026-10-15T08:36Z | **Race with operator's PR #80, cancelled, accepted operator's fix** |
| **B.8.87** | 2026-10-15T21:35Z | **THIS — bulk recon + handoff** |

---

## Open decisions / queue

| # | Item | Blocked by | Owner |
|---|---|---|---|
| 1 | P7 release `v0.96.0-k1.0.0` (per audit B.8.6, "After fmt-cleanup merges") | **Fmt drift cleanup** (Finding 1) | Operator signal + me |
| 2 | Fmt drift cleanup (14 files) | None — one-line fix | Me (low-risk) |
| 3 | Master CI re-trigger / diagnosis | None | Me (diagnosis) |
| 4 | PR #25 (`/poke context`, MERGEABLE+ready, UNSTABLE CI) | UNSTABLE CI; the fmt fix may unblock it | Operator-owned |
| 5 | PR #34 (decollide Phase 2, MERGEABLE+ready, chained to #8) | PR #8 must land first | Operator-owned |
| 6 | PR #8 mega (+225k/-211k, 2254 files) | Conflicts with master; rebase needed | Operator-owned (substantial work) |
| 7 | 4 external items (Dependabot, phinbox, external PRs, billing) | External | Operator-only |
| 8 | 4 CONFLICTING (#8, #23, #10, #35) | Rebase work | Operator-owned |

**Recommended next action (fresh context, ordered):**

1. **Format cleanup** — `cargo fmt && git commit -m "chore(fmt): drift cleanup" && git push origin master` (TX: fix CI first, unblock everything else)
2. **Diagnose master CI failure** — fetch logs of run #38548850946, find which gate failed, re-trigger
3. **Re-run all 9 gates** to confirm 9/9 pass (or document any new failures)
4. **P7 release prep** — bump `Cargo.toml` workspace version `0.85.1-k1.1.1` → `0.96.0-k1.0.0`, run `cargo check --workspace`, commit, tag `v0.96.0-k1.0.0`, push tag
5. **Audit B.8.88** documenting the fmt cleanup + P7 release

---

## Reproduction commands (run these in order to verify state)

```bash
cd ~/CodeProjects/Phenotype/repos/jcode
git fetch origin master
git rev-parse HEAD                            # expect: 60ef2d2a3...
git rev-parse origin/master                   # expect: 60ef2d2a3...
git status                                    # expect: clean
cargo --version                               # for record
cargo fmt --check                             # expect: 14 files drift
cargo check --workspace 2>&1 | tail -3        # expect: 0 errors, 0 warnings
cargo test --package jcode-app-core restore_session 2>&1 | tail -3   # expect: 3 passed
python3 scripts/check_code_size_budget.py     # expect: FAIL tui/mod.rs 2077
bash scripts/check_warning_budget.sh          # expect: current=0 baseline=0
bash scripts/check_panic_budget.py            # expect: 0 if passing
bash scripts/check_swallowed_error_budget.py  # expect: 0 if passing
gh api repos/KooshaPari/Jcode/actions/runs?per_page=3  # master CI status
gh pr list --repo KooshaPari/Jcode --state open --json number,mergeable,isDraft,additions | jq -r '.[] | "\(.number) \(.mergeable // "UNKNOWN") draft=\(.isDraft) +\(.additions)"' | head -20
```

**13 gate scripts in `scripts/`:** `check_code_size_budget.py`, `check_dependency_boundaries.py`, `check_donut_animates_live.py`, `check_guardrails.sh`, `check_module_files.py`, `check_panic_budget.py`, `check_powershell_syntax.ps1`, `check_startup_budget.sh`, `check_swallowed_error_budget.py`, `check_test_size_budget.py`, `check_warning_budget.sh`, `check_wildcard_reexport_budget.py` + `cargo fmt --check` (not a script but a CI gate).

The 9 operator CI gates are 9 of these 13 (likely all except the 4 platform-specific: `donut_animates_live`, `powershell_syntax`, `module_files`, `startup_budget`).

---

## Subagent / fleet status at handoff

- **rooster** (Tier 3 `opencode-go:deepseek-v4.1-flash`, 11m+, recon) — `status=ready`, DM received with 5-line report at 21:31:45Z. No further work needed.
- **No active subagents** at handoff time.

---

## Operator preferences (carry forward from B.8.77-B.8.86 standing memory)

| Signal | Meaning |
|---|---|
| `proc` | Take action on next actionable gate via harness-compliant path |
| `go` | Proceed with next operator-driven action |
| `do all` | Run the full queued/auto-actionable set |
| Silent | Wait — do not auto-act |

| Rule | Detail |
|---|---|
| Direct-push squash-merge | When `gh pr merge` is fail-closed (no `elicitate`): `git -c merge.ff=true merge --squash origin/<branch>` + commit + `git push origin master`. `gh pr close` works for closure. |
| Always `git fetch origin master` | Before any push — peer moves origin/master frequently (saw 2-3 commits move during 4-5 min cargo checks) |
| Fix forward, never revert | Use fix-forward commits, never `git reset` or force push |
| Standup order matters | Smallest PR first, then docs, then cleanup; cargo check after each merge |
| Subsumption is normal | Rebased branches squash-merge onto newer master, much of the work is already present |
| Race conditions common | Always fetch + `gh pr list` before starting work; operator's fixes are nearly always strictly better |
| Workspace size cap | 500 lines hard, 350 target |
| Branch hygiene | Delete branch on remote + local after merge (use `-d`, never `-D`) |

**4-tier hybrid model routing (ACTIVE):**
- **Tier 1 (default, ~60%):** `openai-compatible:MiniMax-M3`, low effort
- **Tier 2 (capability, ~30%):** `openai-compatible:MiniMax-Reasoning-1`, medium effort (design decisions, deep research, multi-file refactors)
- **Tier 3 (cost-optimized, ~10%):** `opencode-go:deepseek-v4.1-flash`, low effort (bulk recon, log tails, file reads)
- **Tier 4 (last-resort):** native `codex` CLI, only when Tier 1-3 fully blocked

---

## Pointer map

| Resource | Path |
|---|---|
| Project memory (THIS file's parent contract) | `~/AGENTS.md` |
| Standing harness memory | `~/.jcode/memories/global/harness-agents.md` |
| Main audit doc | `docs/sessions/2026-10-12-cleanup-and-release/01_AUDIT.md` |
| Session overview | `docs/sessions/2026-10-12-cleanup-and-release/00_SESSION_OVERVIEW.md` |
| All session docs | `docs/sessions/2026-10-12-cleanup-and-release/*.md` |
| Gate scripts | `scripts/check_*.{py,sh}` |
| Operator's KCode fork (separate repo) | `~/CodeProjects/Phenotype/repos/jcode-rebase-v0.90/` (also referred to as the KCode fork) |
| Live fleet registry | `http://localhost:3180` (Paperclip org chart) |
| Live fleet watcher | `~/bin/herdr-watch.sh` daemon |
| PR/issue routing | `gh pr list --repo KooshaPari/Jcode` (operator's fork only; do NOT use upstream `1jehuang/jcode` for PR/issue ops) |

---

## Pitfalls to avoid (battle-tested this arc)

1. **Race conditions with operator are real and frequent** — saw 2 in 14h (B.8.86 + during B.8.84 cargo check). ALWAYS `git fetch origin master` + `gh pr list` BEFORE starting work. If operator did it, accept their version and clean up — do not push a redundant PR.
2. **`gh pr create` "No commits between" error** can indicate a race with another PR, not just an empty diff.
3. **Master CI is currently silent** — if you see "0 checks on master" in `gh pr checks`, that's because push-to-master CI hasn't been triggering. Don't assume green means green.
4. **Operator's branches may have weird bases** — e.g., `feature/decollide-kcode-r2` is based on `feature/herdr-plugin-manifest` (PR #8), NOT master. Always check `git merge-base` before merging.
5. **Subsumption on squash-merge** — when a feature branch is squashed onto a newer master, the diff stat is misleading. Use `git diff --stat master...origin/<branch>` for the actual current delta.
6. **Cargo check timing** — first run after peer merges is slow (4-5 min for new deps), subsequent incremental runs are fast (<2 min).
7. **Don't use upstream `1jehuang/jcode` for PR/issue ops** — it has 48 PRs and 46 issues, vs. the fork's 12 PRs / 20 issues. The operator's working repo is the fork.
8. **Format drift is reintroduced quickly** — every PR merge should be followed by `cargo fmt` verification (until a pre-commit hook exists).

---

## Resumption script (copy/paste for fresh context)

```bash
# 1. Verify state
cd ~/CodeProjects/Phenotype/repos/jcode && \
  git fetch origin master && \
  git rev-parse HEAD origin/master && \
  git status && \
  cargo fmt --check 2>&1 | tail -3

# 2. Read this handoff
cat docs/sessions/2026-10-12-cleanup-and-release/00_HANDOFF.md

# 3. Read the audit (skim to most recent B.8.86)
head -100 docs/sessions/2026-10-12-cleanup-and-release/01_AUDIT.md
# Then grep for "B.8.86" to jump to most recent work

# 4. Check operator's most recent activity
git log --all --oneline -20 | head -20

# 5. Run the recommended next action (fmt cleanup)
cargo fmt && git diff --stat
# If clean, commit and push (after gate verification)
```

---

**End of handoff. Fresh context: read TL;DR first, then CRITICAL findings, then run reproduction commands, then act on Recommended next action #1 (fmt cleanup).**
