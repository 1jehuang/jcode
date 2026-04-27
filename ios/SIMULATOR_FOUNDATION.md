# jcode Mobile App Simulator Foundation

This document describes the first simulation slice now checked into the repo.

For the full target architecture and milestone plan, see
[`docs/MOBILE_AGENT_SIMULATOR.md`](../docs/MOBILE_AGENT_SIMULATOR.md).

For the current day-to-day agent workflow, see
[`docs/MOBILE_SIMULATOR_WORKFLOW.md`](../docs/MOBILE_SIMULATOR_WORKFLOW.md).

## Product direction

The simulator is intended to be a **Linux-native simulator for the jcode mobile
application itself**. It is not Apple iOS Simulator, not an iPhone mirror, and
not a substitute for final on-device validation. Its purpose is to let humans
and AI agents build, run, inspect, test, and iterate on the mobile app without a
MacBook, Xcode, or a live iPhone.

The mobile app should be **Rust-first**. Shared behavior should live in Rust and
be exercised by both the Linux simulator and the eventual iOS host. The iOS app
should become a thin platform shell for OS-specific capabilities such as
window/view hosting, secure storage, push notifications, camera/photo picker,
microphone, and haptics.

## What exists now

The simulator foundation is currently **headless-first** and focused on
automation, logging, and deterministic state transitions. This is the seed of
the larger app simulator. It should evolve from a mocked flow into the real
shared mobile application core plus an agent-native automation surface.

### Workspace crates

- `crates/jcode-mobile-core`
  - shared simulator state
  - typed actions
  - reducer/store
  - semantic UI tree generation
  - transition/effect logging
  - baseline scenarios
- `crates/jcode-mobile-sim`
  - headless simulator daemon
  - Unix socket automation protocol
  - CLI for starting, inspecting, and driving the simulator

## Current scope

This first slice intentionally does **not** include a GUI renderer yet.

Instead, it gives us a solid automation and state foundation so agents can:

- start the simulator
- query state snapshots
- query the semantic UI tree
- dispatch typed actions
- tap semantic node IDs
- load scenarios
- inspect transition/effect logs
- reset and shut down the simulator

The long-term simulator must also support human-like interaction and visual
inspection:

- deterministic layout export
- hit testing by coordinates
- screenshots
- image/layout diffs
- replay bundles
- high-level assertions
- fake backend scenarios
- integration with jcode debug/tester tooling

The goal is for an agent to test autonomously in every way a human would, while
also having richer semantic APIs than a human has.

## Rust-owned gateway protocol helpers

`jcode-mobile-core::protocol` owns the gateway-facing mobile protocol shapes and
transport helpers that the future iOS shell can call through FFI:

- `MobileRequest` and `MobileServerEvent` for typed request/event JSON
- `MobileGatewayConfig` and `MobileGatewayEndpoints` for HTTP/WebSocket URL derivation
- `MobilePairingConfig` to build pair requests without Swift-owned request logic
- `serialize_mobile_request` to produce gateway JSON envelopes with stable IDs
- `decode_mobile_server_event_lossy` to preserve unknown future gateway events

This keeps pairing, health, WebSocket URL construction, request serialization,
and event decoding in Rust while Swift remains a thin platform shell.

## Default transport

The simulator listens on a **Unix socket** by default.

Default path:

- `$JCODE_RUNTIME_DIR/jcode-mobile-sim.sock` if `JCODE_RUNTIME_DIR` is set
- otherwise `$XDG_RUNTIME_DIR/jcode-mobile-sim.sock`
- otherwise a private temp dir fallback

You can always override the path with `--socket`.

## Scenarios

Supported baseline scenarios:

- `onboarding`
- `pairing_ready`
- `connected_chat`
- `pairing_invalid_code`
- `server_unreachable`
- `connected_empty_chat`
- `chat_streaming`
- `tool_approval_required`
- `tool_failed`
- `network_reconnect`
- `offline_queued_message`
- `long_running_task`

## Fake backend model

The simulator includes a deterministic in-process fake jcode backend for effects
emitted by the mobile core.

Current fake backend behavior:

- pairing succeeds when the host is reachable and the pairing code is `123456`
- pairing fails with `Invalid or expired pairing code.` for any other code
- pairing fails with an unreachable-server error when the host contains
  `offline` or `unreachable`
- message sends append `Simulated response to: <message>` and finish the turn

This lets agents validate pairing and chat behavior without a real jcode server,
MacBook, Xcode, Apple iOS Simulator, or iPhone.

## CLI usage

### Start a simulator in the background

```bash
cargo run -p jcode-mobile-sim -- start --scenario onboarding
```

This prints the socket path when the simulator is ready.

### Serve in the foreground

```bash
cargo run -p jcode-mobile-sim -- serve --scenario pairing_ready
```

### Query status

```bash
cargo run -p jcode-mobile-sim -- status
```

### Dump full state

```bash
cargo run -p jcode-mobile-sim -- state
```

### Dump semantic UI tree

```bash
cargo run -p jcode-mobile-sim -- tree
```

### Render a Linux text preview

The `render` command prints a deterministic human-readable shell view generated
from the same Rust semantic UI tree used by agents. It is useful on Linux hosts
without a graphical simulator.

```bash
cargo run -p jcode-mobile-sim -- render
cargo run -p jcode-mobile-sim -- render --output /tmp/mobile-render.txt
```

### Find and assert semantic UI nodes

```bash
cargo run -p jcode-mobile-sim -- find-node pair.submit
cargo run -p jcode-mobile-sim -- assert-screen onboarding
cargo run -p jcode-mobile-sim -- assert-node pair.submit --enabled true --role button
cargo run -p jcode-mobile-sim -- assert-text "Ready to pair"
cargo run -p jcode-mobile-sim -- assert-no-error
```

Assertions are the preferred agent workflow because they return structured
success/failure instead of requiring ad-hoc JSON parsing.

### Dump transition/effect logs

```bash
cargo run -p jcode-mobile-sim -- log
cargo run -p jcode-mobile-sim -- log --limit 10
```

### Export and assert replay traces

Replay traces capture the initial app state, top-level agent actions,
transition log, effect log, and final state in a deterministic JSON bundle.
They can be replayed without a live simulator process or compared against a
running simulator.

```bash
cargo run -p jcode-mobile-sim -- export-replay --name pairing-ready-chat-send --output crates/jcode-mobile-core/tests/golden/pairing_ready_chat_send.json
cargo run -p jcode-mobile-sim -- assert-replay crates/jcode-mobile-core/tests/golden/pairing_ready_chat_send.json
cargo run -p jcode-mobile-sim -- assert-live-replay crates/jcode-mobile-core/tests/golden/pairing_ready_chat_send.json
```

The checked-in golden trace `crates/jcode-mobile-core/tests/golden/pairing_ready_chat_send.json`
locks the current pairing-to-chat-send behavior for regression tests.

### Export and assert deterministic screenshots

The screenshot pipeline exports deterministic SVG-based snapshots with viewport
dimensions, theme, stable hash, SVG markup, and semantic layout metadata. This
keeps screenshot regression tests Linux-native and dependency-free.

```bash
cargo run -p jcode-mobile-sim -- screenshot --output /tmp/mobile-screenshot.json
cargo run -p jcode-mobile-sim -- screenshot --format svg --output /tmp/mobile-screenshot.svg
cargo run -p jcode-mobile-sim -- assert-screenshot /tmp/mobile-screenshot.json
```

`assert-screenshot` compares stable hashes and reports a structured diff with
lengths and first differing byte offset when snapshots diverge.

### Set fields

```bash
cargo run -p jcode-mobile-sim -- set-field host devbox.tailnet.ts.net
cargo run -p jcode-mobile-sim -- set-field pair_code 123456
cargo run -p jcode-mobile-sim -- set-field draft "hello simulator"
```

Supported fields right now:

- `host`
- `port`
- `pair_code`
- `device_name`
- `draft`

### Tap semantic nodes

```bash
cargo run -p jcode-mobile-sim -- tap pair.submit
cargo run -p jcode-mobile-sim -- tap chat.send
cargo run -p jcode-mobile-sim -- tap chat.interrupt
```

### Hit-test and tap by coordinates

The semantic tree includes deterministic default viewport bounds for Linux
headless tests. Agents can inspect the node under a point, assert expected hit
targets, or tap spatially like a human.

```bash
cargo run -p jcode-mobile-sim -- hit-test 195 354
cargo run -p jcode-mobile-sim -- assert-hit 195 354 pair.submit
cargo run -p jcode-mobile-sim -- tap-at 195 354
```

The default viewport is `390x844` logical pixels. Semantic node IDs remain the
preferred stable automation surface, while coordinate taps validate layout and
hit-testing behavior.

### Type, keypress, wait, scroll, gesture, and fault injection

The automation socket also supports higher-level agent operations beyond direct
state dispatch:

```bash
cargo run -p jcode-mobile-sim -- type-text chat.draft "hello from typing"
cargo run -p jcode-mobile-sim -- keypress Enter --node-id chat.draft
cargo run -p jcode-mobile-sim -- wait --screen chat --contains "Simulated response"
cargo run -p jcode-mobile-sim -- scroll chat.messages 120
cargo run -p jcode-mobile-sim -- gesture swipe_up
cargo run -p jcode-mobile-sim -- inject-fault tool_failed
```

Text and keypress operations map onto the same reducer actions as semantic
field setting and tapping. Scroll and gesture currently validate and acknowledge
agent input against the semantic tree, ready for a richer renderer. Fault
injection drives deterministic error/offline scenarios.

### Load a scenario

```bash
cargo run -p jcode-mobile-sim -- load-scenario connected_chat
```

### Reset to default onboarding state

```bash
cargo run -p jcode-mobile-sim -- reset
```

### Dispatch an action directly as JSON

```bash
cargo run -p jcode-mobile-sim -- dispatch-json '{"type":"set_host","value":"devbox.tailnet.ts.net"}'
```

### Shut down the simulator

```bash
cargo run -p jcode-mobile-sim -- shutdown
```

## Semantic node IDs

Examples exposed by the current semantic tree:

### Pairing/onboarding

- `pair.host`
- `pair.port`
- `pair.code`
- `pair.device_name`
- `pair.submit`

### Chat

- `chat.messages`
- `chat.draft`
- `chat.send`
- `chat.interrupt`

## Logging model

Every dispatched action produces a transition record containing:

- sequence number
- timestamp
- action
- state before
- state after
- emitted effects

Effects are also recorded separately.

This is the foundation for future:

- replay bundles
- simulator-driven regression tests
- renderer debugging
- fidelity comparisons against the eventual iPhone app

## Current limitations

This is an initial foundation only.

Not included yet:

- interactive desktop renderer beyond deterministic text/SVG rendering
- raster screenshot export in addition to deterministic SVG snapshots
- richer replay DSL beyond deterministic JSON action bundles
- live render inspector
- iOS host integration
- shared custom renderer backend
- fake jcode backend that exercises real pairing/WebSocket/protocol flows
- physical gesture physics beyond deterministic acknowledgement
- Rust-owned mobile protocol adapters equivalent to the current Swift SDK

## Recommended first workflow

A good current loop is:

1. start the simulator
2. inspect `state`
3. inspect `tree`
4. drive it with `set-field` and `tap`
5. assert expected behavior with `assert-screen`, `assert-node`, `assert-text`, and `assert-no-error`
6. inspect `log` on failure
7. iterate on the shared simulator core

Example:

```bash
cargo run -p jcode-mobile-sim -- start --scenario pairing_ready
cargo run -p jcode-mobile-sim -- state
cargo run -p jcode-mobile-sim -- tap pair.submit
cargo run -p jcode-mobile-sim -- assert-screen chat
cargo run -p jcode-mobile-sim -- set-field draft "hello simulator"
cargo run -p jcode-mobile-sim -- tap chat.send
cargo run -p jcode-mobile-sim -- assert-text "Simulated response to: hello simulator"
cargo run -p jcode-mobile-sim -- assert-no-error
cargo run -p jcode-mobile-sim -- log --limit 10
cargo run -p jcode-mobile-sim -- shutdown
```
