# Live justrust status in the terminal harness

The terminal harness displays a single live justrust row beneath pinned todos.
It also appears near the top of the transcript when there are no todos.

Example:

```text
justrust [remote/hosted] test justrust-clean-room · 51s · 7.8/96 cores · compiling 2/10
```

The row includes execution location, operation and repository, elapsed time,
measured CPU cores when available, and the current build/test phase. Current
crate, unit progress, estimated time left and the number of additional active
builds are included when space permits. Terminal-cell-aware truncation keeps
this chrome to one visual row, including narrow windows and Unicode names.

## Source and attribution

The existing `justrust status --json` command is the data API. The harness does
not parse human-readable build output, infer CPU usage from process counts,
start builds, or change compiler/cache settings.

Selection prefers an exact agent-session match, then a build within the current
repository, then another active build with its own repository label. A recent
completion with scope metadata is shown only for a matching session/repository.
The row disappears when idle. Successful and failed completions have different
colors and expire after eight seconds.

The additive justrust status API supplies `agent_session`, `ncpu`, `cpu_cores`
and `sampled_at`. Older versions remain usable, but unavailable CPU metrics are
not fabricated. With a known machine size and no valid CPU sample, the row says
`?/96 cores`, for example.

## Responsiveness and failure behavior

Queries are asynchronous and have a two-second deadline, a 512 KiB response
limit, and at most one in-flight query per client. Normal refresh is once per
second. Missing executables, failed queries and invalid responses clear the
cached row and use a ten-second retry interval. Cached active results expire
after five seconds. No query runs in the render path.

For an SSH-attached harness, the row is suppressed rather than showing local
laptop telemetry as remote-machine telemetry. Local-daemon clients still show
justrust's local, private-remote and hosted builds. An active row keeps the
otherwise-idle UI from freezing its live telemetry in a hibernated frame.

## Checks

```sh
justrust check -p jcode-tui
justrust test -p jcode-tui --lib justrust
justrust test -p jcode-tui --lib
justrust clippy -p jcode-tui --all-targets --no-deps
```

Tests cover old API compatibility, session/repository selection, path-component
boundaries, stale completion expiry, unknown CPU usage, missing commands,
nonzero exits, invalid/oversized output, command timeout, placement beneath
todos, absence while idle, operation without todos and Unicode/narrow widths.
