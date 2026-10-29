# Cloud Acceptance run 40636491000 follow-up

## Observed failure

The actual GitHub Actions run `40636491000`, head
`4d5a1aa26a3f6f71bc762ed3835d806f259733cb`, finished on October 28, 2026
with 20 passing stages and two failing stages. Both failures passed on their
second repetition. Downloaded evidence is retained locally under
`~/.jcode/scratch/run40636491000/cloud-acceptance-40636491000-1/`.

- `group.suite.1/output.log` reports a missing source `group-root` lease after
  the simulated lost activation command. Its fixture `uncertain.err` is
  `Error: Server disconnected`, not evidence of the intended lost activation.
  The source `home/.jcode/logs/jcode-2026-10-28.log` records concurrent main
  connections and `Debug client error: daemon is busy; wait for all turns and
  background tasks to finish before group transfer` at `05:17:45.328` UTC.
- `tui.midturn_auto_poke.1` times out waiting for a fresh cloud continuation.
  Its cloud daemon log records the same busy rejection at `05:17:59.783` UTC,
  with concurrent readiness connections. The source log subsequently reports
  `/cloud failed: Error: Server disconnected`. The fenced source and installed
  destination notice alone do not establish successful activation/continuation.

## Root cause and narrow repair

Readiness `Ping` acquired the global runtime activity read reservation and
retained it through writing `Pong`. A caller could receive `Pong` and issue its
exclusive group-transfer RPC before the reservation dropped. A concurrent
readiness probe could also acquire that reservation between RPCs. Thus a daemon
with no active agent work could transiently reject migration as busy. The debug
handler's propagated error closed the connection, obscuring the busy diagnostic.

`Ping` only snapshots connection presence. Normal and debug socket handlers now
exclude that read-only request from mutation admission. All other request and
active-work reservations, exclusive transfer checks, lease fencing, and lost
activation recovery remain unchanged. No retry, skip, timeout increase, relaxed
assertion, or change to the earlier session-only idle fix was needed.

The new actual-socket regression
`readiness_probes_do_not_contend_with_group_transfer_admission` holds exclusive
transfer admission while both normal and debug `Ping` succeed, checks no agent
session or client count was allocated, and checks exclusive admission remains
available afterward. Existing busy-work rejection coverage remains enabled.

## Executed validation

The repaired executable was built with hosted
`justrust build --profile selfdev -p jcode --bin jcode`, run
`20261027-222521501-3600137` (`where remote on hosted`). Its reported version is
`jcode v0.99.1-dev (unknown)`, so the tested artifact is identified by SHA-256:

```text
81e54f64e4a0f596774ac41879ac45f89f3254c6e7c10a1b3251aa5aaf4c81c2
```

- Hosted `justrust check -p jcode-app-core --tests`: passed.
- Hosted focused actual-socket regression: 1 passed, 0 failed, 0 ignored.
- Hosted `justrust test -p jcode-app-core group_transfer -- --nocapture`:
  11 passed, 0 failed, 0 ignored, including the socket regression and
  `prepare_rejects_busy_gate_then_fences_and_abort_restores`.
- `python3 tests/test_cloud_acceptance_harness.py`: 13 passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- Actual focused acceptance command below: **8 passed, 0 failed**, group and
  midturn each repeated four times in fresh isolated fixtures.
- Actual full acceptance command below: **22 passed, 0 failed**, every layer
  repeated twice, including idle round trips and all seven TUI scenarios.

```bash
python3 scripts/test_cloud_acceptance.py --binary target/selfdev/jcode \
  --layers group tui --scenario midturn_auto_poke --repeat 4 \
  --artifacts "$JCODE_SCRATCH_DIR/cloud-ping-repair-focused"
python3 scripts/test_cloud_acceptance.py --binary target/selfdev/jcode \
  --repeat 2 --artifacts "$JCODE_SCRATCH_DIR/cloud-ping-repair-full"
```

Both artifact directories retain `report.json`, `junit.xml`, fixture homes,
daemon/provider/SSH logs, and TUI frames. The full matrix completed at
`2026-10-28T05:29:26Z`. These are the real CLI, daemon, bubblewrap and loopback
OpenSSH/TUI acceptance paths with a deterministic provider, not managed cloud
provisioning, production OAuth, public-network SSH, or live-model validation.
The shared daemon was not reloaded. Signing work was not changed.
