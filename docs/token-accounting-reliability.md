# Token accounting reliability and verification

## What is being counted

The authoritative accounting unit is a provider response, not a user turn or a
session. An agent turn can make many responses. Compaction and sidecar requests
also consume tokens and must be reported separately. Do not add `usage_report`
rollups to `session_end` totals: they describe overlapping traffic.

`daily_model_usage` is the response-level source for new clients. The legacy
`token-value` dashboards estimate historical usage from session-end snapshots.
Those snapshots cannot reliably attribute a session that switched models, and
missing session ends cannot be reconstructed from those snapshots alone. Do not
present either source as a complete provider invoice or as cash expenditure.

## Required reliability properties

- Persist a usage report before starting asynchronous delivery. Keep its event
  identity and original occurrence time unchanged across retries and restarts.
- Remove a pending report only after a positive durable database acknowledgment.
  A successful backup firehose write alone is not a durable accounting receipt.
- Deduplicate and update counters atomically on the server. A lost HTTP response
  after a successful commit must not charge the same report again.
- Preserve the individual input, output, cache-read and cache-write buckets in
  the backup path. Preserve source, model, provider and occurrence time too.
- Treat provider input semantics explicitly. Some providers include cached
  input inside input tokens, while others report disjoint buckets. Neither total
  tokens nor prices may count the same cache read twice.
- Attribute delayed reports to the day the usage occurred, not the retry day.
- Bound deduplication retention together with the accepted replay window. Never
  prune receipts and then silently accept an old retry as a new response.
- Respect telemetry opt-out, including pending data. Persist only content-free
  counters and coarse routing metadata, never prompts or credentials.

## Release verification

1. Apply required database migrations before deploying the server. Test against
   an isolated database first. Do not prune raw history as part of this rollout.
2. Verify schema readiness, migration order and configured usage backup binding.
3. Exercise the actual ingestion handler with a fixed report identity:
   submit twice, then verify one counter increment and identical accounting day.
4. Simulate failure before the database commit, a database rollback, and a lost
   response after commit. Retrying must converge to one increment.
5. Exercise client persistence across process restart, non-durable HTTP success,
   timeout, and concurrent clients. Pending records must remain until committed.
6. Compare a known response's provider counters to its outbound report and stored
   rollup, including cached-only responses and partial/error streams that carry
   usage. Test both inclusive and disjoint cache conventions.
7. Run `npm test --prefix telemetry-worker` and the focused Rust telemetry tests.
   Unit tests are not evidence that the deployed worker or installed CLI changed.
8. After deployment, verify real durable acknowledgments and recent rollup
   updates. Check database growth and pending-client backlog, not just HTTP 200s.

## Limits and incident recovery

These protections apply to usage that the provider actually reports and the
client observes. They cannot recover a billed request for which no usage was
returned, a machine crash before usage was captured, opted-out telemetry,
permanently lost local storage, or records discarded by older clients.

A fallback analytics dataset is useful for incident analysis, but its retention,
sampling and asynchronous delivery mean it is not proof of invoice-grade
completeness. A green database probe proves present availability, not the absence
of earlier ingestion gaps. Preserve raw backup records and pending reports while
investigating, and label any reconstructed or unpriced usage explicitly.

If production credentials are unavailable, report that deployment and historical
loss verification are blocked. Do not equate passing local tests with a repaired
production pipeline, and do not claim historical gaps were recovered without
reconciliation evidence.
