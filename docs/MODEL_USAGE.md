# Model usage metadata

The shared runtime supplies `ModelRoute.usage`. The public API and Rust SDK expose
it as `ModelRouteInfo.usage: Option<ModelUsage>` through `get_runtime_info()` and
pushed `RuntimeInfo` events. The existing `list_models()` return type is unchanged.
The TypeScript SDK exposes the equivalent optional `usage` field.

```json
{
  "count": 12,
  "last_used_unix_secs": 1789210000,
  "tracking_started_unix_secs": 1789209000,
  "selection_count": 8,
  "last_selected_unix_secs": 1789208000
}
```

## Units and coverage

- `count` counts **tracked agent turns with at least one persisted assistant
  response on this route**. A turn is anchored to a persisted input message.
  Tool, truncated-answer, and reload continuations count once per route. The
  session journal preserves the turn identity across handoff and restart. A new
  input starts a new turn. If a later request fails or is cancelled, earlier
  persisted responses still constitute use. This does not measure task success.
- `last_used_unix_secs` is the latest recorded persisted-response time. It can
  advance during a long turn without increasing the count.
- `tracking_started_unix_secs` marks this local ledger's coverage. It is absent
  until tracking has begun. Counts are not all-time historical estimates. A zero
  count means no recorded turns in this coverage, not "never used".
- `selection_count` and `last_selected_unix_secs` come from the legacy TUI
  `model_picker_usage.json`. They measure picker selections, not model requests.
  Effort variants are summed within a route. These counters remain separate.
- Missing `usage` means unknown metadata, including older servers. It must not
  be displayed as "never used".

The ledger is stored under `JCODE_HOME` (normally `~/.jcode`) as
`model-usage-v1.sqlite3`. SQLite serializes concurrent writers. Its primary key
combines durable session/input-turn ID and route identity, so repeated tool continuations do
not increase the count. Reads do not create or modify the ledger. Runtime
recording is shared by streaming and blocking paths, including desktop, TUI,
CLI and swarm agents. Debug sessions and hidden auxiliary model calls are not
counted. A serving route that cannot be identified unambiguously is not guessed.
Model fallback is attributed to the model actually serving the response.

Older sessions cannot be backfilled exactly: their messages store token counts
but not model/route identity, and only eight environment snapshots are retained.
Neither a session's latest model nor its last modification time provides exact
historical request attribution. No transcript scan is performed on picker open.

## Ranking and freshness

`jcode_sdk::compare_model_usage` and TypeScript `compareModelUsage` compare usage
best-first by tracked turn count, last-used time, historical selection count,
and last-selected time. Search relevance and explicit current/favorite policy
belong ahead of this comparator, followed by a stable model/route tie-breaker.
The TUI uses the same comparator and keeps its existing current/favorite policy.

Legacy protocol clients opt into `model_usage_updated` delta events with
`get_model_catalog.subscribe_usage_updates: true`. The flag defaults to false,
so old clients that cannot decode new enum variants receive no new event kind.
New API bridges and TUIs opt in. Each delta carries one route, avoiding a full
catalog rebuild and the busy Agent lock. The bridge updates its cached catalog
and publishes `RuntimeInfo`. Monotonic merging prevents late snapshots or
cross-session events from regressing counts within one tracking epoch.

## Rollout

A desktop hot reload alone does not update its daemon. Build and validate the
runtime on an isolated socket first. Publish the validated source build, then
use `jcode server promote <installed-version>` and `jcode server reload --json`
at a safe idle window. Do not use `server stop --force` or send kill signals.
The supported reload checkpoints sessions, but it still signals active
model generations. Defer activation while any session is processing.
