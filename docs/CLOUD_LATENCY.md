# Cloud latency diagnostics

Cloud operations emit info-level, content-free timing records to the ordinary
Jcode logs in `$JCODE_HOME/logs/jcode-YYYY-MM-DD.log` (default `~/.jcode/logs`).
They do not require trace mode. Each process writes its own records, so a local
client and a cloud daemon must both be updated to see both sides of a resume.
A new local binary does not update an already-running cloud daemon.

## Find expensive stages

From the repository root:

```bash
python3 scripts/cloud_latency_report.py --slow-ms 1000
python3 scripts/cloud_latency_report.py ~/.jcode/logs/jcode-2026-11-07.log
python3 scripts/cloud_latency_report.py downloaded-cloud-daemon.log --slow-ms 250
python3 scripts/cloud_latency_report.py --since 2026-11-08T00:28:00 --until 2026-11-08T00:38:00
```

The report ranks stage/operation/metric groups by maximum observed duration and
shows sample count, error count, p50, p95 and maximum. It reads files incrementally,
retaining numeric samples for exact percentiles. Defaults include all retained
logs, so narrow the files when comparing before/after builds. Percentiles describe
only the selected observed samples, not a statistically controlled benchmark.
The report prints only allowlisted label syntax and numeric fields, never raw log
lines. Do not combine overlapping stage/total metrics by summing them.
Time windows use the local wall-clock timestamps printed in the selected logs,
with an inclusive start and exclusive end. Do not pass UTC offsets. Undated or
invalid timestamp records are excluded when a window is selected. A time window
does not identify a binary: several old and new processes can write to one log,
so a post-reload window alone is not proof that every sample used the new build.

Most records use `[CLOUD_TIMING]`; mobile records use `mobile_sync stage=...`.
Common fields are `stage`, `operation`, `outcome`, and `duration_ms`. Additional
numeric `*_ms` fields appear as separate report rows. Request IDs correlate
client wire events and server work. Server records additionally have operation IDs,
a hashed session key, and history kind. Session keys are diagnostic correlation,
not an authorization mechanism. `none` means a metric was not available.

## Coverage and interpretation

| Path | Observable boundaries |
| --- | --- |
| Cloud control plane | HTTP send, body read/decode, host readiness polling/wait and terminal success/failure/cancellation |
| Native SSH attach | Process start through authenticated daemon handshake, including failures and cancelled attempts |
| TUI connect | Socket, readiness, Subscribe and bootstrap setup, with the failing phase retained on early return |
| History/todos transport | Request write, response wait, event JSON parse, payload byte count and request correlation |
| Server bootstrap and paging | Snapshot/build, encoding, writer-lock wait and socket write, plus fallback/failure/drop outcomes |
| Client application | History, page and todo event application, followed by one-shot frame composition readiness |
| Preview/inventory | Cache scope/read/freshness/write, SSH command, remote projection/inventory and local decode/load |
| Mobile transcript sync | Save queue/debounce, incremental/reconciliation pass, session upload/skip/error, HTTP and progress persistence |
| Handoff | Existing preparation/cache/probe timings plus machine-readable repository/snapshot/archive/upload/activation stages |

`request_wait_ms` starts after the request write completes and ends after response
JSON parsing. It includes remote queue/work, transfer and client scheduling, not
just network RTT. Server writer-lock versus write versus encode timings help
separate those causes. This measurement survives a cancelled socket-read future
because it is attached to the connection's outstanding request, not a particular
read attempt or executor thread. Separate connections never share correlation
queues. A fresh Subscribe completes with Done, while a target-aware Subscribe
retains its correlation until History. Explicit todo polls take precedence over
bootstrap snapshots. Todo events have no wire request ID, so their association is
best-effort FIFO, not proof that a specific request caused a pushed snapshot.
The correlation queue is bounded; old entries expire when subsequent requests
arrive. Expiry is a diagnostic observation, not an application timeout.

`frame_ready` measures from completed event application until a composed TUI frame
is ready. It is not a physical display/terminal flush timestamp. Updates may share
a frame. Unfocused idle terminals intentionally coalesce redraws for up to five
seconds, and hibernation can defer them further. This wall-clock wait is not the
CPU cost of rendering. Existing rendering diagnostics give detailed draw costs
when needed.
Fast streaming tokens are deliberately not logged individually. Key sync milestones
are logged consistently, while unrelated protocol events only receive a parse
record if slow or malformed, avoiding per-token logging overhead.

Remote projector stderr must pass a bounded, strict timing-record filter before
entering local logs. Arbitrary SSH stderr is not forwarded. Timings contain fixed
stage/outcome labels, numeric sizes/durations and bounded correlation fields, not
credentials, keys, paths, transcript contents, payloads or raw server error bodies.
Keep ordinary diagnostic log files private nevertheless, since other existing
subsystems may include information beyond these timing records.

Unchanged mobile reconciliation candidates are counted in a single
`mobile_sync candidates=... unchanged=... progress_updates=...` summary rather
than timed and logged individually. Changed-session and HTTP timings remain.
A repeatable no-network filesystem benchmark is available with:

```bash
justrust test --remote -p jcode-base --lib benchmark_unchanged_reconciliation -- --ignored --nocapture
```

It measures 1,300 unchanged sessions, not upload or end-to-end cloud latency.

Successful HTTP/1 mobile uploads drain small acknowledgment bodies to preserve
connection reuse. `http_upload_body` records success, timeout, size-limit or
read-error outcomes, bounded to 100ms and 64KiB. An already accepted upload remains
successful if its acknowledgment body is malformed or slow, avoiding duplicate
retries. HTTP/2 and later multiplexed responses skip this HTTP/1-specific drain.
`http_upload` includes this bounded cleanup, so its timing boundary differs from
older builds that stopped at response headers. The controlled keep-alive benchmark:

```bash
justrust test --remote -p jcode-base --lib benchmark_mobile_upload_keep_alive -- --ignored --nocapture
```

It injects a 30ms per-connection delay and an 8ms body delay. Connection counts
demonstrate reuse, but its elapsed times are not production API measurements.

A high total alone is not proof of the cause. Compare cache hits/misses, success
versus failure, cold versus warm connections and local versus daemon stage timing.
`workspace_command` records distinguish `git_probe`, `inventory_prepare`,
`inventory_projection`, `preview`, `cloud_status`, and `cloud_status_directories`.
Older `operation=none` samples cannot reliably identify which subprocess was slow.
These operations share transport code but have different workloads and deadlines,
so compare their distributions separately rather than attributing every slow SSH
command to inventory. The report counts HTTP/read/load errors, output bounds,
identity mismatches, and nonzero exits as failures, not normal cache misses/skips.
Do not claim a cold VM wake or a full end-to-end cloud resume is subsecond just
because one stage or a synthetic fixture is subsecond.
