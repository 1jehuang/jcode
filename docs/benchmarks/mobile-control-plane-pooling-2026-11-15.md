# Control-plane HTTP connection reuse, November 15, 2026 UTC

Mobile account-scoped diagnostics from the preceding day contained multi-second
connection waits. Those cumulative milestones do not identify the cost of each
HTTP request, and this change does not claim to eliminate cloud boot time or a
particular device's observed 20-second connection.

## Proven unnecessary work

`CloudClient::send_raw` constructed a fresh `ureq::Agent` for every request,
including consecutive host readiness polls. Dropping the agent discarded its
socket pool. A real loopback regression constructed fresh `CloudClient` values
as the mobile path does and observed four accepted TCP connections for four
requests. Hosted RED: `20261114-215005895-3963010`.

The SDK now retains up to eight transport agents, LRU by client name and request
timeout. Configuration changes select a distinct agent. ureq still pools sockets
by origin. The cache contains no account authorization headers or response data.
Each request constructs its own headers. Request timeout budgets, HTTP status
handling, host discovery, TLS validation and SSH pinning are unchanged. The cache
mutex is held only while selecting/building an agent, never over network I/O.

## Controlled measurement

A loopback server independently serves connections and injects **100 ms per
accepted TCP connection**. Both paths perform eight actual SDK HTTP requests.
The fresh path varies client configuration to force fresh agents, matching the
old per-request transport construction. This is artificial connection overhead,
not measured WAN/TLS latency, iPhone startup or server/cloud readiness.

| Path | TCP connections | Eight requests elapsed |
| --- | ---: | ---: |
| Fresh transport, first pass | 8 | 809.943 ms |
| Pooled transport, first pass | 1 | 102.934 ms |
| Pooled transport, second pass | 1 | 102.746 ms |
| Fresh transport, second pass | 8 | 810.253 ms |

Hosted benchmark receipt: `20261114-215109463-3970128`. Timings exclude server
thread cleanup. No machine-dependent timing threshold is asserted. Socket counts
are strict assertions.

## Validation

- Hosted check: `20261114-215059010-3968403`.
- Cloud-focused suite: 25 passed, one opt-in benchmark ignored,
  `20261114-215103524-3969211`.
- Full SDK library suite after adding timeout-budget regression: **120 passed,
  zero failed, four ignored**, `20261114-215219801-3986471`.
- Tests cover credential changes on one retained connection, distinct origins,
  changed user-agent/timeout configuration and an actual delayed reply rejected
  under the caller's new shorter timeout.

```sh
justrust check -p jcode-sdk --features cloud --tests
justrust test -p jcode-sdk --features cloud --lib -- --test-threads=1
justrust test -p jcode-sdk --features cloud --lib benchmark_control_plane_connection_reuse -- --ignored --nocapture --test-threads=1
```

No production requests, host wakeups, session creation, credential registration
or deployments were performed for these tests. The SDK change must be compiled
into each client to affect that client. A new production/device timeline is
required before claiming any reduction in observed device startup latency.
