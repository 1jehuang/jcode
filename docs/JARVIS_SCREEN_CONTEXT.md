# Jarvis session prompting and proposed screen awareness

## Session instructions

Jarvis is an opt-in interaction mode, not a replacement for Jcode's base prompt.
The client registers its voice tool and a session-owned `system_prompt_append`
with the daemon. That addition belongs in the stable system prompt, once per
assembled prompt, rather than in a new transcript message on every turn or retry.
Normal sessions do not register it. Clearing the registration or disconnecting
the temporary Jarvis client removes the addition. Existing approval rules remain
in force. Tool queue acceptance does not establish that speech was heard.

## Screen context: implemented read-only metadata stage

Enable with `--jarvis-screen`, which explicitly opts in to Jarvis window metadata.
Voice-only and ordinary sessions never start
the observer. This requires a local Niri graphical session (`NIRI_SOCKET`).
The TUI reuses the shared `jcode-companion::screen` observer locally even when
its daemon is remote. No screenshot, OCR, input monitoring, focus change, or
computer action is performed.

The wire `screen_context` request carries its immutable session attachment and
an explicit enable/update/disable action. The daemon accepts updates only from
the live Jarvis overlay owner. Transport snapshots are capped at 16 KiB,
32 windows, 16 outputs, 512-byte titles and 128-byte app IDs. A replaceable watch
slot and socket-write throttling prevent a slow daemon from growing a history.
Compositor events debounce for 500 ms and publish no faster than 2.05 seconds,
with final-state delivery and 15-second metadata renewal. The lease and
observation freshness expire after 30 seconds. Out-of-order, stale, oversized,
cross-client and wrong-session updates are rejected without changing state.

Only a compact, focus-first summary of at most 2 KiB enters dynamic provider
context at existing request/tool boundaries. It never enters the stable prefix
or persisted transcript. Idle updates only replace state and never start a turn.
The read-only `screen_context` client tool returns the latest bounded local
metadata on demand, not pixels. Unknown visibility and activity stay unknown.
Disconnect, disable, and session-transfer paths revoke collection and ownership.
The read path also removes expired leases before exposing metadata.

The remaining sections document the original design and later visual/action
work, not permissions granted by this first metadata stage.

Screen awareness should be a separate explicit opt-in capability. Enabling voice
must not silently enable screenshots, OCR, input monitoring, or desktop control.
The companion owns local desktop observation. A remote/headless daemon receives
bounded observations only through the attached client, never by assuming that
its own display is the user's display.

### Lightweight observations

For the first implementation, use the local Niri IPC event stream and refresh
its authoritative window/workspace/output snapshot after relevant changes:

- Foreground window ID, application, bounded title, and workspace.
- Visible-window IDs and available layout geometry.
- Output logical size, scale, transform, and coordinate-space information.
- Observation timestamp, sequence, source client, and expiry.
- A conservative interaction state: active, idle, or unknown.

Window focus is not eye tracking. A scrolling-layout tile size is not necessarily
a global on-screen rectangle. Missing geometry, visibility, or activity signals
must remain unknown, not be fabricated. Niri focus timestamps alone cannot tell
whether the user is typing, dragging, reading, or watching a video.

### Event delivery, not new user turns

Do not send raw compositor events as ordinary `SoftInterrupt` requests. The
current idle soft-interrupt path starts a new user turn, which would turn normal
window changes into unsolicited model requests and speech.

Introduce a distinct session-scoped passive-context update:

1. Subscribe only while the explicitly opted-in client owns a live lease.
2. Debounce short bursts, initially around 500 ms. Publish at most once every
   two seconds while changing, with a trailing update for the final state.
3. Keep one latest snapshot, replacing pending older snapshots. Bound the
   window count, title length, total bytes, and update rate.
4. Idle updates only replace state. They do not start inference or a response.
5. During a running turn, make the latest changed snapshot available at an
   existing safe boundary. Do not cancel generation, skip tools, or override
   queued user instructions. Deliver no more than one latest update per boundary.
6. Put the bounded observation in dynamic context, not the stable system prefix.
   Do not retain an unbounded stream of historical titles in conversation history.
7. Clear observations on lease expiry, disable, disconnect, or session transfer.
   Reject updates and callbacks from another client/session owner.

The Jarvis system instructions tell the model these are untrusted observations,
not requests or instructions, and usually require no reaction. A separate
read-only refresh tool should return the current snapshot on demand.

### Visual detail and computer actions

Window metadata cannot establish what is drawn inside a window. Provide a
separately permissioned screenshot tool for a selected window or output, with
freshness and coordinate metadata. Do not continuously attach pixels or OCR by
default. Respect the session model's image capability. Titles, screenshots, web
pages, and OCR are untrusted content, not authority to issue commands.

Showing a visual should not steal focus during user interaction. A model's
intent is not a sufficient guard: check interaction/focus state locally just
before the action, since it may have changed after the model's observation.
When activity is unknown, use a non-focus-stealing presentation or ask before
changing the user's foreground window. Recent user input takes precedence.
All existing action permissions and irreversible-action approvals still apply.

### Acceptance checks for the follow-up

- A burst of 100 focus/layout events produces bounded, latest-state updates.
- Idle updates produce zero model requests, user turns, or speech calls.
- A running turn receives changes only at safe boundaries, without cancellation.
- Stale, out-of-order, oversized, cross-client, and expired-lease updates fail closed.
- Unknown visibility or activity never becomes a claim of certainty.
- Input arriving between observation and presentation prevents focus stealing.
- Screenshots remain off without explicit permission and are never substituted
  by captures from a remote server's screen.
- Disabling Jarvis leaves default prompting and ordinary sessions unchanged.
