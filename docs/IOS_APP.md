# jcode iOS App

> Status: v2 rebuild. Pure Swift. This replaces the earlier prototype and the
> Rust-mobile-core/simulator direction, both removed in the `ios-app-restart`
> branch history.

## Product definition

A native iOS remote control for jcode servers running on your own machines.
The phone renders conversations and drives sessions; all heavy lifting (LLM
calls, tools, git, files, MCP) stays on the server. Reachability is assumed to
be Tailscale (or LAN); the app never talks to LLM providers directly.

Design identity: shared with Jcode Desktop. The palette mirrors Desktop's
default "Warm neutral" theme (`jcode-desktop-ui/src/theme.rs`): charcoal and
stone surfaces, ivory type, a restrained sandstone accent (`#B6A08A`), and
Desktop's OK/WARN/ERROR semantic colors. The conversation is a raised page
(`PANEL_BG #25221F`) on the canvas tone (`BG #1C1A18`).

Transcript presentation follows Desktop, adapted for touch:

- User prompts are numbered cards tinted by the CLI prompt rainbow in reverse
  hue order (newest violet), fading exponentially into card paper.
- Assistant replies sit on the page with no role caption or bubble.
- Reasoning is inline, smaller, and muted with no label or border (clamped to
  three lines, tap to expand).
- Tool calls are inline rows, not cards: a monochrome glyph whose color carries
  status (warn running, ok done, error failed), then intent/name/subject.
  Expanding shows output on the code background.
- Activity uses Desktop's eight-dot spinner with Thinking / Running tools /
  Responding labels. Reduce Motion gets a static marker.
- The brand mark is the canonical halftone donut from Desktop/website.

## Architecture decision: pure Swift

The app is a thin client over an existing, server-owned protocol. Behavior
worth sharing lives server-side already. A shared Rust core would duplicate
the server protocol a third time and require an FFI bridge, custom renderer
work, and a parallel simulator to stay honest. Instead:

- **Swift owns the client.** SwiftUI for UI, Swift 6 concurrency, `@Observable`.
- **The server protocol is the single source of truth.** The Swift codec is
  validated by fixture tests against real wire JSON; drift fails tests.
- **Testability without devices** comes from layering, not a simulator:
  everything below the view layer builds and tests on macOS via `swift test`.

## Layering

```
ios/
  Package.swift              SPM package, builds on macOS + iOS
  Sources/JCodeKit/          platform-free client core (no UIKit)
    Gateway.swift            endpoints: /health, /pair, /ws
    Pairing.swift            pairing code -> auth token
    Wire.swift               Request/ServerEvent codecs (NDJSON over WS)
    Transport.swift          WebSocketTransport protocol + URLSession impl
    Connection.swift         actor: connect/auth/reconnect, AsyncStream<ServerEvent>
    SessionReducer.swift     pure state machine: events -> transcript/app state
    CredentialStore.swift    Keychain-backed server credentials (protocol + impl)
  Sources/JCodeMobile/       SwiftUI app shell (iOS only)
    JCodeMobileApp.swift     entry
    AppModel.swift           @Observable glue: Connection + Reducer -> views
    Views/                   Pairing, Chat, Transcript, Sessions, Settings
    QRScannerView.swift      camera pairing
    Theme.swift              colors/typography tokens
  Tests/JCodeKitTests/       swift test on macOS: codec fixtures, reducer, pairing
  project.yml                XcodeGen spec for the app target
```

Rules:

- `JCodeKit` never imports UIKit/SwiftUI. It must compile for macOS so the
  whole behavior layer is testable headlessly by agents on this machine.
- Views contain no protocol or state-transition logic. `AppModel` only
  forwards actions and publishes reducer output.
- `SessionReducer` is a pure function `(State, ServerEvent) -> State`
  (plus local user intents). All streaming/tool/session edge cases are unit
  tested there, replacing the old Rust simulator's role.

## Protocol

Server side (already shipped, unchanged):

- `jcode pair` CLI generates a 6-digit code (5 min TTL) and QR with
  `jcode://pair?host=H&port=P&code=C`.
- `POST http://host:7643/pair` with `{code, device_id, device_name}` returns
  `{token, server_name, server_version}`. Token is stored hashed server-side.
- `GET /health` for reachability checks.
- `ws://host:7643/ws?token=...` upgrades to a WebSocket carrying the same
  newline-delimited JSON protocol as Unix-socket TUI clients
  (`crates/jcode-protocol/src/wire.rs`, `#[serde(tag = "type")]`).

Client v1 requests: `subscribe`, `message`, `cancel`, `soft_interrupt`,
`ping`, `get_history`, `resume_session`, `set_model`, `rename_session`,
`clear`.

Client v1 events consumed: `ack`, `text_delta`, `reasoning_delta`,
`reasoning_done`, `text_replace`, `tool_start`, `tool_input`, `tool_exec`,
`tool_done`, `message_end`, `done`, `error`, `pong`, `state`, `session`,
`session_renamed`, `history`, `model_changed`, `available_models_updated`,
`tokens`, `interrupted`, `status_detail`, `notification`, `compaction`.
Unknown event types are ignored by design (forward compatibility).

## Feature scope

v1 (this rebuild):

- Pair via QR scan or manual host/port/code; multiple saved servers in Keychain
- Connect/disconnect lifecycle with automatic reconnect + backoff
- Chat: send, streamed assistant text, markdown rendering, reasoning indicator
- Tool calls rendered as collapsible cards with live status
- Interrupt (cancel) and soft-interrupt (queue a message mid-run)
- Session list (from history payload), switch via `resume_session`, rename
- Model picker from `available_models`
- Token usage + connection status surfaces
- Error and disconnect banners that never silently drop user input

Later (explicitly out of v1): push notifications (APNs), Live Activities,
voice input, image attachments, tool approval UX, ambient/swarm dashboards,
widgets, Mac Catalyst polish.

## Testing strategy

1. `swift test` (macOS, no Xcode project needed):
   - codec round-trips against fixture JSON captured from the real server
   - `SessionReducer` streaming scenarios (deltas, tool lifecycle, history
     replay, interrupts, errors, reconnect resubscribe)
   - pairing client against a stubbed URLProtocol
   - connection actor against a fake `WebSocketTransport`
2. `xcodebuild build` of the app target (XcodeGen) keeps the UI compiling.
3. Manual/automated device pass via `xcrun simctl` once a simulator runtime is
   installed; not a CI gate.

## CI

A macOS job runs `swift test` in `ios/` plus `xcodegen` + `xcodebuild build`
for the app target. TestFlight delivery can be re-added later (the previous
Codemagic pipeline was removed with the prototype).
