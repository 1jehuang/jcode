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

## Mobile and workspace follow-up measurements

Read-only samples over an existing managed SSH master on November 8, 2026
measured machine status at 6.4–7.1ms, directory status at 6.85–6.90ms (27
entries), and the selected active repository's git probe at 179–226ms. These
did not reproduce the historical 8-second workspace outliers. They are remote
stage timings, not cold connection or end-to-end UI latency.

The bounded authoritative directory scanner also has a worst-case synthetic
benchmark: an identical frozen 97MB snapshot fell from 3.575s to 1.190s, and
journal metadata scanning from 0.675s to 0.078s, with identical metadata. This
does not establish that scanning caused the observed live outliers.

Mobile push now reuses the shared HTTP client while retaining its five-second
request deadline and delivery semantics. Eight loopback pushes with an
artificial 60ms connection setup delay improved from 622ms/eight connections
to 71ms/one connection. This is not an APNs delivery measurement. Static
`operation=mobile_push` timings cover total, HTTP send and body processing.
The report counts `rejected` and `undelivered` as failures, but not `no_devices`.

Mobile command execution drains stdout and bounded stderr concurrently and
supervises the whole child lifetime. An isolated child emitting 1.4MB of
Unicode stderr previously hit the fixture's 301ms timeout and now completes
in 3ms. This demonstrates removal of a pipe deadlock, not faster model turns.
`operation=mobile_runner` timings cover claim, transfer, turn, sync, complete
and poll wait, including errors and cancellation. The five-second poll cadence
is unchanged.

The runner now claims immediately on startup and immediately after completing a
command. It sleeps five seconds only after an empty claim, retaining the idle
poll rate. Errors back off for 20 seconds rather than 20 seconds plus the old
unconditional five-second wait. This removes scheduling gaps at startup and
between queued commands, not the up-to-five-second wait for a command arriving
during an idle poll interval. The current API provides no long-poll or command
notification channel, so that remaining latency needs coordinated API work.

`operation=mobile_runner stage=queue_wait` records `claimed_at - created_at`
using the two API-provided timestamps, avoiding phone/daemon clock skew. It
includes machine wake, previous queued work and polling delays, but excludes
claim-response transport and turn execution. Legacy responses without those
timestamps still execute normally. Invalid or reversed timestamps omit the
metric rather than changing command handling. Logs contain only the duration,
not command IDs, text or timestamps. Paused-clock regression comparisons prove
the eliminated five-second scheduling gaps, not production end-to-end latency.

### Optional command wakeups

The daemon can subscribe to `GET /v1/mobile/commands/events` using its normal
account bearer header over a WebSocket. The only accepted application frame is
`{"type":"commands_available"}`. It is a hint to perform the existing atomic
HTTP claim, never an executable command. Initial subscription triggers a queue
recheck to cover commands queued between an empty claim and socket registration.
Bursts coalesce in a bounded watch channel. Messages/frames are capped at 4KiB.

The five-second claim fallback remains active even with a socket, protecting
against lost hints, service restart and unsupported APIs. Error backoff still
cannot be bypassed by a hint. Connection attempts have a two-second deadline,
network retry cooldown, and a five-minute cooldown for disabled/old endpoints.
Endpoint and credential changes discard the old socket. Cancellation aborts the
socket task. Fixed `event_connect` and `event_connection` records contain no
credentials, endpoint, account, command text or raw network error.

The corresponding backend candidate uses an account-scoped Durable Object with
the WebSocket Hibernation API and at most four sockets per account. D1 remains
the queue authority. A committed enqueue publishes a bounded best-effort hint
before the existing host wake step. Failed publication cannot lose or invalidate
the command. The backend logs static `mobile_command_events` notify durations
and outcomes without payloads. No queue data is stored in the socket object.
Actual production hibernation/eviction and billing have not been measured.

**Rollout is gated.** The backend's `MOBILE_COMMAND_EVENTS_ENABLED` remains false.
Backend commits `f3f1dd8` and `6e87e1d` are on the pre-existing
`feat/browser-code-subscribe` branch in `solosystems-backend`, not a production
deployment or permission to merge unrelated branch changes. Integrate only
those reviewed commits, deploy the binding/migration with the flag disabled,
validate account isolation, then explicitly enable it and monitor queue wait.
Update the managed daemon only when idle and after resolving its existing
pending runtime. Do not replace a separately staged package or force a busy
daemon restart for a benchmark.

On November 8, 2026, read-only rollout checks found one active cloud turn and
an already-pending runtime, so neither was modified. No Apple USB device was
present and usbmuxd was inactive, preventing real-device install/paint timing.
The configured account's read-only command history request returned HTTP 403,
so no historical queue percentile is claimed. These are rollout/measurement
limitations, not evidence that the new event path is active in production.

The separate `jcode-mobile` repository's phone transcript transport now pools
HTTP connections too (commit `a98b496`). Eight actual list calls in a loopback
fixture with 30ms connection setup and 8ms body delay improved from median
315.039ms/eight connections to 97.462ms/one connection. All 85 phone-core tests
passed. See that repository's `docs/MOBILE_SYNC_LATENCY.md` for reproduction,
account/origin isolation and timing boundaries. A new mobile app build and
installation are required before this source change affects a device. No live
phone, push service, or production API was used for these benchmarks.
