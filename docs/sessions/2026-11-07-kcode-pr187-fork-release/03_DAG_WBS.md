# DAG / WBS — work breakdown and dependency graph

## Dependency graph

```
A. PR #187 (fork delivery + release gate)
   A1 embed timeout 600 -> 1800        (independent)
   A2 ARM64 gating: 4 publish paths    (independent)
   A3 SDK consumers: drop unconditional ARM64 demand   (depends on A2)
   A4 test-size ratchet rebaseline     (depends on current master state)
   A5 cargo fmt 2 files                (independent)
   A6 S7630 script injection fix       (independent)
   A7 fork release gate + fork_version validator   (independent)
   A8 ci.yml fork-PR runner isolation  (independent)
   A9 gpt-5.6-sol scope fix            (independent; blocks green Build & Test)
       |
       v
   A10 resolve merge conflicts vs master (3 rounds, must follow all edits)
       |
       v
   A11 45 review threads triaged -> 16/16 CI green -> squash-merge f59329a0f

B. PR #256 (publish-github-packages.yml hardening)   depends on A11 merged
   B1 8x inputs.jcode_release_tag -> env RELEASE_TAG
   B2 3x github.actor -> env GHCR_USER
   B3 remove redundant crane auth login exposing token in cmdline
   B4 repo-wide run: interpolation scan -> 0

C. Desk host CI stability   (parallel track, independent of A/B)
   C1 diagnose SIGKILL of runner listener by WSLExport task
   C2 rewrite export-wsl.ps1 (worker guard, .partial promote, runner restart)
   C3 dry-run verify exit 2 on active workers
   C4 reinstall runners v2.338.2 + runsvc.sh + reconfigure
   C5 verify both runners online with desk-linux label
   C6 investigate 02:13:42 distro reboot  -> UNRESOLVED
```

## WBS with critical path

| ID | Task | Est | Depends on | Status |
|---|---|---|---|---|
| A1 | Embedding cohort timeout 600 -> 1800s | 10m | — | done |
| A2 | ARM64 gating across 4 publish paths | 20m | — | done |
| A3 | Fix 2 SDK consumers demanding ARM64 unconditionally | 10m | A2 | done |
| A4 | Rebaseline test-size ratchet, exact entries | 10m | master | done |
| A5 | cargo fmt 2 unformatted files | 5m | — | done |
| A6 | S7630: move release tag to env | 10m | — | done |
| A7 | Fork release gate + fork_version parse validator | 15m | — | done |
| A8 | ci.yml fork-PR runner guard + permissions | 10m | — | done |
| A9 | gpt-5.6-sol scope fix | 20m | — | done |
| A10 | Resolve merge conflicts (3 rounds) | 30m | A1..A9 | done |
| A11 | Triage 45 threads, reach 16/16, squash-merge | 30m | A10 | done |
| B1 | 8x release_tag -> env | 10m | A11 | done |
| B2 | 3x github.actor -> env | 10m | A11 | done |
| B3 | Remove crane auth login line | 5m | A11 | done |
| B4 | Repo-wide interpolation scan = 0 | 5m | B1..B3 | done |
| C1 | Diagnose runner SIGKILL | 15m | — | done |
| C2 | Rewrite export-wsl.ps1 | 20m | C1 | done |
| C3 | Dry-run verify exit 2 | 10m | C2 | done |
| C4 | Reinstall + reconfigure 2 runners | 20m | — | done |
| C5 | Verify online + labels | 5m | C4 | done |
| C6 | Identify distro reboot trigger | 20m | — | **open** |

## Critical path

`A9 (scope fix) -> A10 (conflict resolution) -> A11 (CI green + merge) -> B1/B2/B3 -> B4`

The single gating dependency is **A11**: nothing in track B can land until #187 is green and merged.
Track C is independent and was pursued in parallel, but it *caused* the instability that made A11's
CI runs unreliable (see `05_KNOWN_ISSUES.md`).

## Blockers encountered and resolution

| Blocker | Resolution |
|---|---|
| CI jobs killed with no steps executed | Traced to WSLExport task SIGKILLing the runner listener (C1) |
| Runners in ENOENT relaunch loop | Runner dirs deleted; reinstalled from v2.338.2 (C4) |
| Duplicate `env:` key silently dropped RELEASE_TAG | Caught during B verification; merged mapping keys |
| Step used `$RELEASE_TAG` without defining it | Caught during B verification; added step env |
| Distro reboot at 02:13:42 killed jobs | **Unresolved** (C6) |
