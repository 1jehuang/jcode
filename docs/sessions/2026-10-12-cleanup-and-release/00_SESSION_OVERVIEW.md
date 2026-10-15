# 2026-10-12 Cleanup and Release — Session Overview

## Operator directive (verbatim, 2026-10-12T23:10:15Z)

> as a side task local, desktop (other local node), and github cloud need cleanup, e.g. merging and finishing all branches and the same for stashes or any other refs, then dealing w\ local worktree sprawl the same way and finishing any unstaged or unfinsihed local work before same deltion and compression to 1 checkout\clean state against origin along w\ new release followng full green\quality pass

## Decomposed goals

1. **Merge / finish / close** all open branches, PRs, issues, stashes, refs
2. **Finish** any unstaged / uncommitted local work first
3. **Then** delete the obsolete worktrees, branches, refs
4. **Compress** to a single clean checkout against `origin/master`
5. **Release** new version after a full green quality pass (test + lint + fmt + type-check)

## Scope (5 surfaces)

| Surface | Owner | State | Risk |
|---|---|---|---|
| **Local jcode repo** (`/Users/kooshapari/CodeProjects/Phenotype/repos/jcode`) | self | 31 worktrees, 40+ branches, 1 stash | medium (most branches are operator's active work) |
| **Other local node** (`kooshapari-desk` 100.96.135.160, Windows) | remote agent | unknown state via herdr | high (no direct access, depends on remote agent) |
| **GitHub cloud (KooshaPari/Jcode)** | self/operator | 12 open PRs, 20 open issues | medium (most are operator's active work) |
| **GitHub cloud (KooshaPari/KCode fork)** | self/operator | 3 open PRs | low |
| **Release pipeline** | codex w8:pT "Audit and canonicalize release" | already running | **collide risk** |

## Collision check

- `w8:pT` (codex, working): **"Audit and canonicalize release"** — already doing release work
- `w8:p18` (jcode, working): "Execute the recovered channel-corpus killed-thre…" — long-running revive work
- `w8:p1B/p1C/p1D` (jcode, done): Revive work in `/private/tmp/revive-work`
- `w7:p1E` (jcode, focused): **"KCode"** — the operator's focused pane
- 12+ other jcode/codex panes doing active product work (PortfolioDesign, Khostty, PhenoFabric, Omniroute, Mux, ShareCLI, AgilePlus, KCode, CockPit, PhenoShared, HeliosLite, Parpoura, etc.)

**Coordination imperative**: do NOT touch worktrees/branches/panes that are active in the snapshot above. The cleanup must be:
- (a) announced to the operator
- (b) coordinated with the w8:pT codex release agent
- (c) sequenced so destructive ops happen after the active work finishes or moves out

## Destructive command constraints (per AGENTS.md)

The following are policy-blocked without operator approval:
- `git push --force` / `--force-with-lease`
- `git reset --hard`
- `git clean -fd`
- `git branch -D` (use `-d` for safe delete if merged, else ask)
- `gh repo delete`

Will request approval for each destructive op; will not silently advance.

## Phasing plan (proposed)

| Phase | Goal | Time est. | Risk |
|---|---|---|---|
| 0 | Audit + plan (this doc) | now | none |
| 1 | Finish own work (close my open PRs, drop my stashes) | 30 min | low |
| 2 | Close stale / abandoned issues (with comment + label) | 1-2 hr | medium |
| 3 | Inventory + propose disposition for each open PR | 2-3 hr | medium |
| 4 | Coordinate with w8:pT codex release agent | 1 hr | high (coordination) |
| 5 | Cargo fmt --all (issue #38) | 30 min + 1 hr review | medium |
| 6 | Full green quality pass (test, lint, type-check) | 1-3 hr | high (cold compile) |
| 7 | Bump version, tag, GitHub release | 30 min | low |
| 8 | Compress to 1 checkout (delete worktrees, branches, refs) | 1 hr | high (irreversible) |
| 9 | Sync other local node (kooshapari-desk) | 1 hr | high (remote) |

**Estimate total**: 8-12 hr across 1-2 days.

## Stance

- **NOT** touching operator's active work (pane states above)
- **NOT** doing anything destructive without approval
- **WILL** do safe read-only audit + sequencing
- **WILL** ask for sign-off on each phase before destructive ops
- **WILL** coordinate with w8:pT codex agent before any release-related work

## Status

- 23:11 — session doc opened, audit complete, plan drafted
- 23:11 — sending coordination message to operator's focused pane (w7:p1E)
- next: wait for operator's response before phase 1
