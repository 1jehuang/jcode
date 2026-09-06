# Concurrency telemetry rollout, 2026-09-06

## What changed

Legacy session-marker counts are not recoverable as exact historical concurrency.
Their raw rows remain unchanged. `concurrency_event_quality` classifies them as
untrusted and `trusted_concurrency_events` excludes them. Do not backfill a v2
version onto old rows or infer correctness from a plausible-looking count.

New measurements come from owned, OS-locked runtime Agent lifetimes, not TUI
connections or the old process-global telemetry slot. Root and child counts are
separate. Children include manual splits as well as spawned agents. Idle live
Agents count, saved sessions do not. This measures simultaneous open runtime
sessions, not simultaneous model requests or unique people.

## Production deployment

- Applied additive migration `0026_concurrency_tracking.sql`.
- Deployed Worker version `24b27b8a-f8b6-469a-a83b-7050b952262f`.
- Added the dedicated `jcode_concurrency_firehose` backup dataset.
- Verified the real Rust tracking API delivered four start/end pairs to D1,
  including exact end peaks of **3, 3, 3, 1**, with root/child peaks **2/1**
  for the overlapping group and **1/0** after every prior owner closed.
- All verification events were explicitly CI-tagged. None entered the user view.

**The Worker rollout alone does not update installed Jcode binaries.** Users
must run a release containing the Agent ownership changes before they contribute
v2 data. The shared development daemon was not restarted as part of this repair.
Until client rollout, an empty trusted-user report is expected, not zero actual
concurrency. Existing clients continue to produce legacy/untrusted measurements.

## Validation and remaining boundary

- 82 Worker tests passed, including real SQLite ingestion/query checks and the
  reduced compound-SELECT budget required by production D1.
- 60 telemetry-core tests and 100 repeated parallel lease/crash stress runs passed.
- Six focused Agent ownership, viewer-attachment, and failed-resume tests passed.
- The documented report ran successfully against production D1. A rebuilt
  final-source Rust probe repeated the expected results and CI exclusion.
- A full executable build was stopped under host memory pressure. An additional
  existing resume/attachment regression run was terminated while linking before
  its tests ran. Neither is claimed as passing. No public client release or
  shared-daemon replacement was performed.

## Query later

```sh
cd telemetry-worker
npm run concurrency
# Machine-readable variant:
npm run concurrency -- --json
```

The report describes coverage, missing ends, and per-runtime/per-installation
observed peaks over events received in the last 30 days. It is not a live-online
counter or a time-weighted measure. Root and child peak values can occur at
different times and must not be added together. See `README.md` for the schema,
validation rules, retention, and Analytics Engine fallback mapping.

## Repeat a controlled production smoke check

This explicitly sends synthetic CI telemetry to production, isolated under a
disposable installation ID. Do not remove the CI tag to make a report nonempty.

```sh
# From the repository root, after deploying the Worker:
scripts/dev_cargo.sh run -p jcode-telemetry-core --example concurrency_probe \
  -- --emit-ci-telemetry
```

Use the printed installation ID in read-only Wrangler queries against
`concurrency_event_quality`. Require eight rows, four starts, four ends, all
`quality='trusted'` and `is_ci=1`, with the expected peaks printed by the probe.
Also require zero rows for that installation in `trusted_concurrency_events`.
Local probe success by itself is not evidence that the events arrived.
