# Production telemetry rollout acceptance, 2026-10-17

At 10:54 UTC Wrangler OAuth authentication and production D1 read access were
working. No new credentials were created or disclosed. The previous authorization
blocker no longer reproduced.

## Deployment

- Target: Worker `jcode-telemetry`, production custom domains
  `telemetry.jcode.sh` and `telemetry.solosystems.dev`.
- Applied additive migration `0031_usage_report_receipts.sql` to the existing
  `jcode-telemetry` database. No historical rows were deleted or rewritten.
- Dry-run verified bindings, including `FIREHOSE_USAGE`.
- Deployment succeeded at 10:55 UTC, version
  `3b15ed62-6418-42a4-a119-e748ab4ee267`.
- Wrangler confirmed the nightly `17 4 * * *` schedule.

## Actual production acceptance observations

A real pending canonical client report was submitted to the public `/v2/usage`
endpoint twice with its original event identity and occurrence time. Both
responses returned HTTP 200, `durable:true`, matching installation/event IDs,
and `firehose:true`. A direct production D1 query found exactly one matching
contract-v2 receipt, with transient payload columns cleared by the atomic
rollup trigger. No fabricated token usage was submitted and no pending files
were manually deleted.

The live client automatically reduced its pending spool from 1,528 reports at
10:55:38 UTC to 1,464 at 10:55:50 UTC, despite ongoing usage. Production canonical
rollups increased from 191 responses / 21,831,776 tokens to 271 responses /
34,489,718 tokens over that interval. This establishes actual delivery and
backlog progress, not a claim that the entire backlog was already empty.

The health endpoint at 10:55:51 UTC returned `ok:true`, database size
6,787,788,800 bytes, soft limit 8,000,000,000 bytes, and `over_soft_limit:false`.
Current headroom was about 1.21 GB below the soft limit. This is not a forecast
of future growth or proof that the scheduled cleanup has already run.

## Whole-result recheck

At 10:56:38 UTC, repeated the real pending-report acceptance path after the
complete rollout: both deliveries again returned matching durable acknowledgments
and backup success, and production D1 again contained exactly one compact receipt.
The backlog had fallen to 1,233 reports, with 573 canonical responses stored.
At 10:56:49 UTC, all production canonical groups satisfied nonnegative bucket
counts and `total = input + output + cache_read + cache_write` (zero invalid
groups). Canonical response count had grown to 640. The public endpoint rejected
an empty JSON request with HTTP 400 instead of accepting an unversioned event.
Health remained green at 6,787,952,640 bytes during this repeated check.
These checks exercise the actual deployed HTTP/ACK/database boundary, not only
the local adapter or packaging checks.

## Remaining limits

The deployment/authentication blocker is resolved. Historical missing reports,
possible interim legacy retry overcount, exact provider invoice reconciliation,
and older fleet clients are not resolved by this rollout. Receipt cleanup is
scheduled but its first production execution has not yet been observed. Monitor
backlog, growth, pruning throughput and pricing coverage as described in
`token-accounting-reliability.md`. Reports beyond the seven-day replay window
remain preserved locally for manual reconciliation.
