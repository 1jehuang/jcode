# jcode Mobile Simulator Foundation

This document describes the first simulation slice now checked into the repo.

## What exists now

The simulator foundation is currently **headless-first** and focused on automation, logging, and deterministic state transitions.

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

Instead, it gives us a solid automation and state foundation so the agent can:

- start the simulator
- query state snapshots
- query the semantic UI tree
- dispatch typed actions
- tap semantic node IDs
- load scenarios
- inspect transition/effect logs
- reset and shut down the simulator

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

### Dump transition/effect logs

```bash
cargo run -p jcode-mobile-sim -- log
cargo run -p jcode-mobile-sim -- log --limit 10
```

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

- visible desktop renderer
- layout geometry export
- screenshot export
- richer fixtures/replay DSL
- live render inspector
- iOS host integration
- shared custom renderer backend

## Recommended first workflow

A good current loop is:

1. start the simulator
2. inspect `state`
3. inspect `tree`
4. drive it with `set-field` and `tap`
5. inspect `log`
6. iterate on the shared simulator core

Example:

```bash
cargo run -p jcode-mobile-sim -- start --scenario pairing_ready
cargo run -p jcode-mobile-sim -- state
cargo run -p jcode-mobile-sim -- tap pair.submit
cargo run -p jcode-mobile-sim -- tree
cargo run -p jcode-mobile-sim -- set-field draft "hello simulator"
cargo run -p jcode-mobile-sim -- tap chat.send
cargo run -p jcode-mobile-sim -- log --limit 10
cargo run -p jcode-mobile-sim -- shutdown
```
