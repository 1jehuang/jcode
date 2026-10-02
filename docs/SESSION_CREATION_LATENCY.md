# Desktop session creation: synchronous telemetry in Agent construction

Measured 2026-09-06 on the XPS 13 Linux development host. No active daemon was
restarted or replaced, and no compositor state was changed.

## Production observation

One fresh connection to the running harness API, `hello`, and `create_session`
for the desktop workspace, followed by closing only that new connection:

| Stage | Time |
| --- | ---: |
| Unix socket connect | 0.021 ms |
| API hello | 1.658 ms |
| API create_session reply | 1804.314 ms |
| Daemon registry initialization | 2 ms |
| Daemon Agent construction | 1358 ms |
| Daemon setup total | 1512 ms |
| Subscribe setup before completion events | 10 ms |

The bridge replies to `hello` after dialing the legacy socket. Creation sends
Subscribe, State, and GetModelCatalog. State produces the API Attached reply.
SDK connection/handshake is not the dominant delay in this observation.

Daemon timestamps put normal Agent setup at approximately 20 ms. The remaining
constructor interval starts at `begin telemetry session`. This call replaces the
process-global telemetry accumulator. If the prior session emitted its start,
it closes that session with `SessionEndReason::Superseded`. That path previously
sent turn_end, session_end, and todo_session synchronously, each with an 800 ms
HTTP timeout. The turn_end send also held SESSION_STATE, blocking other telemetry
calls. An empty isolated daemon does not reproduce this history-dependent delay.

## Change

Use the existing bounded background worker for **Superseded** lifecycle delivery.
Keep all payloads, their submission order, the old session identity, and the new
session reset. Retain bounded blocking delivery for actual shutdown/crash paths.
No protocol, provider initialization, desktop, or user telemetry-setting changes.

New timing logs split Agent local setup from telemetry and daemon provider fork
from idle prewarm, so future startup regressions are directly attributable.

## Reproducible transport regression

`crates/jcode-telemetry-core/tests/session_creation_latency.rs` calls the real
public telemetry API in a subprocess with a temporary JCODE_HOME. It seeds a
session and turn, then times the same begin_session replacement used by Agent
construction. A loopback HTTP proxy accepts CONNECT and never responds. All HTTPS
telemetry is trapped locally, so this test sends no telemetry to the service and
uses no model request. Unlike unit tests, it exercises production HTTP delivery,
not the cfg(test) payload sink.

```sh
cargo test -p jcode-telemetry-core --test session_creation_latency -- --nocapture
cargo test -p jcode-telemetry-core --lib
```

Before/after binaries were compiled from this checkout's actual telemetry crate
using the same Cargo-produced selfdev dependencies. They were linked and run in
scratch while unrelated builds held the host-wide Cargo gate. The old library
failed the regression at 2410.400 ms. The first fixed run passed at 9.421 ms.

Five further runs per version while the host was heavily compiling:

| | Samples (ms) | Median |
| --- | --- | ---: |
| Before | 2722.908, 2644.553, 2691.321, 2819.976, 2538.386 | 2691.321 ms |
| After | 79.753, 202.837, 155.207, 82.312, 117.014 | 117.014 ms |

This is a **95.65% median reduction in the measured telemetry component**, not a
claim of measured full desktop or full API improvement. Scheduling and filesystem
contention remain visible in the fixed samples. All 62 telemetry unit tests
passed, including event order/identity, queued replacement delivery, and blocking
shutdown regression checks.

## Full isolated API verification

The complete `cargo build --profile selfdev` subsequently passed. Both official
Cargo test commands above also passed: 62 unit tests and the production HTTP
integration test (5.029 ms for its measured replacement).

A real daemon and the same standalone API bridge were run against private
HOME, JCODE_HOME, JCODE_RUNTIME_DIR, and sockets. Both versions used the Jcode
provider, a synthetic credential, and the loopback stalled HTTPS proxy. Selecting
the current model through the API seeded meaningful telemetry without submitting
any prompt or requesting model inference. Each following create_session then
superseded the prior telemetry session. With no turn to finalize, this case
exercises two 800 ms lifecycle sends rather than three.

| API create_session | Samples (ms) | Median |
| --- | --- | ---: |
| Before, activity seeded | 1654.824, 1670.033, 1675.223 | 1670.033 ms |
| After, activity seeded | 13.258, 17.427, 28.968 | 17.427 ms |

This is a **98.96% median reduction in isolated full API creation latency**. The
first unseeded create was 179.284 ms before and 117.357 ms after and is excluded
from the activity-seeded comparison. The old Agent constructor took 1622-1625 ms.
The new logs show telemetry below the 1 ms log resolution and Agent construction
of 3-6 ms for those activity-seeded sessions.

## Deployment boundary

The first full baseline build was terminated by SIGTERM, but the retry succeeded.
The verified new binary was copied to the immutable store and published to the
current channel without touching the stable channel, shared-server channel, or
running daemon:

- Binary: `~/.jcode/builds/versions/82a93e6fb-dirty-e293f2a72492/jcode`
- SHA-256: `e293f2a7249263a664aceb413d1df0d96ed53cf12f15119a4ca31ebd8fb5a330`
- Version: `v0.82.5-dev (82a93e6fb, dirty)`
- The build stamp predates commit `8e9f40498`, but the build includes the tested fix.
- `~/.local/bin/jcode` resolves through current to that new binary.
- Shared-server symlink and active daemon PID 20700 still resolve to the prior
  `a495fb059-dirty-40e6123ec268` binary, verified unchanged after publication.

The desktop's existing shared-daemon connection therefore still uses the old
backend. Its live post-change latency has **not** been measured. A supported
checkpoint reload signals active generations, so it was deliberately deferred
until a safe idle window. No user daemon or compositor was restarted.
