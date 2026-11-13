# CI master-run integrity

A push to `master` must always reach a real verdict. This note records the rule
and why it exists, so a later "tidy up concurrency" change does not silently
reintroduce the failure.

## The concurrency contract

```yaml
concurrency:
  group: ci-${{ github.workflow }}-${{ github.ref }}-${{ github.event_name == 'push' && github.sha || 'pr' }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}
```

- **Pull requests may cancel in-flight runs.** A force-push or a new commit
  supersedes the old review, and that is desirable.
- **Pushes to `master` may not cancel anything.** The sha is part of the group
  for pushes as well, because GitHub also replaces a *pending* run in the same
  group when `cancel-in-progress` is `false`.
- Workflow-level concurrency sits above job-level, so a cancelled run leaves
  every job `cancelled`. `cancelled` is not a code failure, and the Aggregate
  gate reads it as a missing verdict, not a pass.

## What went wrong on 2026-11-12

One shared group plus `cancel-in-progress: true` meant each new push to `master`
killed the verification already running:

| run | sha | evidence |
|---|---|---|
| 43318753096 | `864bd675` | all 9 gating jobs started 22:19:14, all cancelled 22:19:28 |
| 43318791123 | `8aa3ca33` | created and started 22:19:27 — 1 s before the cancel above |
| 43318791123 | `8aa3ca33` | cancelled in turn: jobs completed 22:30:44-22:30:53, while run 43320550368 started 22:30:36 |
| 43320550368 | `6d0e812a` | no successor push, every job `success` — the control |

Branch protection then dropped `Build & Test (linux)`, `Cargo Check & Parity`,
`Clippy`, `Quality Guardrails`, `TypeScript SDK` and
`Windows Cross-Target Check (Linux)` because their last observed conclusion on
`master` was `cancelled`.

## Rule for the Aggregate gate

Only `success` gates a merge. A job that is genuinely allowed to stay skipped is
left out of the gate's `needs:`, and that is the only sanctioned exemption.
