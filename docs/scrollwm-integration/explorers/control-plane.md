# ScrollWM x jcode - Control Plane (explorer: control-plane)

How jcode drives ScrollWM and how ScrollWM launches jcode. The link is
ScrollWM's Unix-domain control socket + the `scrollwm <verb>` CLI shim.

Anchors (ScrollWM, `/Users/jeremy/scrollwm`):
- `Sources/WindowLab/ControlServer.swift` - socket bind/listen, `ControlSocket.path()`,
  `ControlClient.send` (the reference Swift client).
- `Sources/WindowLab/ControlCommands.swift` - `handleControlCommand`, `controlStatusJSON`.
- `Sources/WindowLab/ControlCLI.swift` - `runControlCLI`, launch-on-`notRunning`.
- `Sources/WindowLab/main.swift` - `controlVerbs` set, `scrollwm` help text.
- `Sources/WindowLab/ScrollWMApp.swift` - `arrange(pidFilter:)`, `focus(index:)`,
  `controlColumns()`, `floatingWindows`, `tileFloating`, `startControlServer`.
- `Sources/WindowLab/Config.swift` - `spawn` map (commented `ctrl+opt+j` jcode launcher),
  `KeyAction.spawnTerminal`.

Anchors (jcode, `/Users/jeremy/jcode`):
- `crates/jcode-terminal-launch/src/lib.rs` - terminal spawn (`open -na Ghostty ...`).
- `crates/jcode-app-core/src/session_launch.rs` - `resumed_window_title`, `spawn_resume_in_new_terminal_with_provider`.
- `crates/jcode-app-core/src/server/comm_session.rs` - `spawn_visible_session_window`, swarm spawn flow.
- `crates/jcode-config-types/src/lib.rs` - `SwarmSpawnMode { Visible, Headless, Auto }`.
- `crates/jcode-mobile-sim/src/lib.rs` - reference Rust Unix-socket line client (`send_request`).

Ground-truth probe (this machine, read-only): socket exists at
`~/Library/Application Support/ScrollWM/control.sock` (`srw-------`, mode 0600,
owner `jeremy`); `scrollwm` is on PATH at `/opt/homebrew/bin/scrollwm` (cask
symlink to `ScrollWM.app/Contents/MacOS/ScrollWM`); installed bundle
`CFBundleShortVersionString = 0.1.6`; ScrollWM not currently running (`ping`
returned the "isn't running" hint). Cask version pin is `0.1.5`.

---

## 1. Full verb inventory

Source of truth: `controlVerbs` in `main.swift` + the `switch` in
`ControlCommands.handleControlCommand`. Replies are one human line except
`status` (JSON). Lines beginning `error:` map to CLI exit code 2;
`notRunning` maps to exit 3 (`ControlCLI.printReply`).

| Verb (aliases) | Args | Reply (success) | Needs managing? | jcode use |
|---|---|---|---|---|
| `ping` | - | `pong` | no | YES - liveness/handshake |
| `status` | - | JSON snapshot | no | YES - detect + read strip/columns |
| `arrange` | - | `ok: arranged N windows` / `error: nothing to arrange` | no (starts it; idempotent resync if already) | YES - adopt after spawning agents |
| `release` | - | `ok: released, all windows restored` | no | maybe - "un-manage" teardown |
| `toggle` | - | `ok: arranged N` / `ok: released` | no | no (non-deterministic) |
| `focus` | `next\|prev\|left\|right\|N` | `ok: focused column K (title)` | yes | YES - focus active agent (by index today) |
| `move` | `left\|right\|up\|down` | `ok: moved to column K` | yes | maybe - reorder agent columns |
| `workspace` (`ws`) | `up\|down\|N` (or none=query) | `ok: on workspace K of M` | yes | YES - put a swarm on its own workspace |
| `width` | `25\|50\|75\|100\|0.0-1.0` | `ok: set focused width to P%` | yes | maybe - widen focused agent |
| `close` | - | `ok: closed <title>` | yes | no (jcode owns its own lifecycle; risky) |
| `display` | `next\|main\|primary\|largest\|N` (or none=list) | `ok: displays: ...` | no | maybe - pin swarm strip to a monitor |
| `focus-mode` (`focusmode`) | `fit\|centered` (or none=query) | `ok: focus-mode set to X` | no | no |
| `reload` (`reload-config`) | - | `ok: config reloaded` | no | YES (after onboarding writes a `spawn` binding) |
| `tutorial` | - | `ok: opened tutorial` | no | no |
| `update` (`update-check`) | `[--install]` | update status line | no | no |
| `quit` | - | `ok: quitting (windows restored)` | no | no (never kill the user's WM) |

`status` JSON fields (`controlStatusJSON`): `managing` (bool), `focusMode`,
`windowCount`; and when managing: `focusedColumn` (1-based), `workspace`,
`workspaceCount`, `floatingCount`, `floating[]` (`app`/`title`/`canTile`), and
`columns[]` from `controlColumns()`: `index` (1-based), `app`, `title`, `width`
(px int), `focused` (bool), `healthy` (bool).

Gap that matters for jcode: **`status` carries no `version` and no PID per
column.** Columns are identified only by `app`+`title`. jcode's spawned terminal
windows do carry a unique title (`resumed_window_title`, e.g. `🛰 jcode/<name>
<label>`), so title is a usable key today; PID is not exposed.

### jcode subset (what we'd actually call)
- Detect/handshake: `ping`, `status` (+ a new `version`/`hello`, see below).
- Arrange after a headed spawn batch: `arrange` (idempotent: also resyncs while
  managing, so it doubles as "re-adopt the new windows").
- Focus the active agent: `focus N` today; `focus title <...>` proposed.
- Place a swarm on its own surface: `workspace N`, optionally `display N`.
- Post-onboarding: `reload` after writing a `spawn` binding into ScrollWM config.

---

## 2. Chosen transport: socket-direct from Rust, CLI only as a launch fallback

Two options:

(a) **Shell out to `scrollwm <verb>`** - spawn `/opt/homebrew/bin/scrollwm
arrange`, parse stdout/exit code.

(b) **Speak the socket protocol directly from Rust** - `UnixStream::connect`
the control.sock, write `"<verb args>\n"`, half-close write side
(`shutdown(SHUT_WR)`), read the reply to EOF, trim. This is exactly what
`ControlClient.send` does in Swift and what `jcode-mobile-sim::send_request`
already does in Rust.

**Recommendation: (b) socket-direct is the primary transport; (a) CLI is a
narrow fallback used only to *launch* ScrollWM when the socket is absent.**

Rationale:
- **No dependency on PATH / install layout.** `scrollwm` may be a Homebrew
  symlink, a `~/.local/bin` link (`scripts/install.sh`), or absent while the app
  is running from `~/Applications`. The socket path is deterministic
  (`~/Library/Application Support/ScrollWM/control.sock`, override
  `SCROLLWM_CONTROL_SOCK`) and computed identically by app and client - jcode can
  reproduce it with one line and never shell out.
- **Latency + no fork.** A spawn of the universal `WindowLab` binary pays
  process startup, AppKit framework load (`ControlCLI` imports AppKit), and
  LaunchServices lookups on the `notRunning` path. A direct connect is a single
  syscall round-trip on the same machine; this matters when arranging after each
  of N headed spawns.
- **Structured errors.** Direct connect distinguishes `ENOENT`/`ECONNREFUSED`
  (= not running / stale socket) from real I/O errors, mirroring
  `ControlClient.Failure.notRunning`. Shelling out collapses everything into an
  exit code + an English string on stderr.
- **No quoting hazard.** Proposed title/path-bearing verbs (`focus title ...`,
  `spawn-strip <cmd>`) avoid a second layer of shell-arg quoting if jcode writes
  the protocol line itself.
- **Protocol is trivial + stable.** One request line in, one reply line out,
  newline-terminated, ASCII. The framing is already battle-tested by the Swift
  CLI and fuzzed (`FuzzController.swift`).

Where CLI wins, and where we keep it: **launching** a not-running ScrollWM.
`ControlCLI` already implements bundle-relative launch + 6s poll
(`launchRunningApp` / `retryAfterLaunch`). Reimplementing app launch in Rust
(`NSWorkspace.open`, bundle-id `dev.scrollwm.app`) is avoidable: if the socket is
absent and the user opted in, jcode can run `scrollwm arrange` (or
`open -a ScrollWM`) once to start it, then switch to socket-direct for everything
after. So: **socket for steady-state; `scrollwm`/`open` for cold start only.**

Transport decision is encapsulated behind one Rust client type so a future
switch (or a CLI-only mode for hardened environments) is a one-file change.

---

## 3. Proposed NEW ScrollWM verbs for jcode

Design constraints honored: every verb returns one line (or JSON for queries);
`error:`-prefixed on failure; safe to add behind the existing `switch` in
`handleControlCommand` + the `controlVerbs` set in `main.swift`; PID-filtered
arrange already exists (`arrange(pidFilter:)`), so most of this is plumbing the
filter/keys through the parser.

### 3.1 `version` (a.k.a. capability handshake) - REQUIRED
```
scrollwm version
```
Reply (JSON, single line):
```json
{"name":"ScrollWM","version":"0.1.6","protocol":1,"verbs":["ping","status","arrange","focus","workspace","display","reload","focus-pid","focus-title","arrange-pids","spawn-strip","workspace-new"]}
```
Why: `status` has no version; jcode needs feature detection that does not depend
on the installed bundle's Info.plist (the running app may differ from the
on-disk app mid-update). `protocol` is a monotonic integer jcode gates on.
Implementation: `Bundle.main.infoDictionary["CFBundleShortVersionString"]`
(already read in `Updater.swift`) + a static verb list. Trivial, no managing
required. Alternatively fold these three keys (`version`, `protocol`, `verbs`)
into `status` and keep `version` as a thin alias.

### 3.2 `arrange-pids <pid...>` - adopt specific processes
```
scrollwm arrange-pids 4123 4147 4190
```
Reply: `ok: arranged 3 of 3 windows (2 new)` or
`error: no manageable windows for pids 4123` (none had an on-screen std window).
Why: jcode wants to adopt exactly the agent terminals it just spawned, not
"every window on the Space." The controller **already** supports this:
`func arrange(pidFilter: Set<pid_t>? = nil)` enumerates `AXSource.windows(forPID:)`
per pid. New verb just parses ints and calls `arrange(pidFilter:)`. Note the
plumbing caveat: when **already managing**, today's `arrange` early-returns into a
resync that ignores `pidFilter` - so for "adopt these new pids while already
managing" the verb should route to the same per-strip resync/insert path the
auto-adopt uses, or force a scoped re-adopt of the given pids.
Caveat for jcode: the terminal *window* belongs to the terminal app process
(Ghostty/Terminal), not to the `jcode` child; see 4.3 for how jcode gets pids.

### 3.3 `focus-pid <pid>` and `focus-title <substring>`
```
scrollwm focus-pid 4147
scrollwm focus-title "jcode/aqua"
```
Reply: `ok: focused column 3 (🛰 jcode/aqua main)` or
`error: no managed column matches pid 4147` / `error: no managed column title contains "jcode/aqua"`.
Why: today `focus` is index-only, but jcode tracks agents by session
title/identity, not by volatile column index (which shifts as columns are
added/moved/closed). Implementation: search `engine.slots` for the matching
`window.pid` or `window.title.contains(substring)` (case-insensitive), then
`focus(index:)`. `controlColumns()` already exposes title; pid would need to be
threaded through (small).
**`focus-title` is the pragmatic MVP** because jcode controls the window title
deterministically (`resumed_window_title`) and does not reliably have the
terminal-window pid (4.3). Prefer an exact match on a jcode-issued token (e.g. the
session id embedded in the label) over a fuzzy contains.

### 3.4 `workspace-new` - create/switch to a fresh vertical workspace
```
scrollwm workspace-new
```
Reply: `ok: on workspace 4 of 4 (new)`.
Why: a jcode swarm should land on its own niri-style workspace so it does not
shuffle the user's existing columns. Engine semantics already support this
("going down past the last workspace makes a new empty one" -
`Config.swift` doc + `switchWorkspace`). Verb = "switch down until a new empty
workspace exists, return its index," giving jcode a deterministic target without
guessing counts from `status`.

### 3.5 `spawn-strip <command...>` - reverse direction, run a command into the strip
```
scrollwm spawn-strip /Users/jeremy/.local/bin/jcode --resume <id>
```
Reply: `ok: spawning into strip (will adopt on appear)`.
Why: symmetry with ScrollWM's own `cmd+return` "new terminal into strip"
(`KeyAction.spawnTerminal` / `Terminals.swift`). Lets ScrollWM be the one that
opens the terminal *and* guarantees adoption via its AX-observer fast path
(`SpawnLatencyTest`), instead of jcode spawning blind and then asking for an
arrange. Lower priority than 3.1-3.4: jcode already owns terminal spawning
(`jcode-terminal-launch`), and routing through ScrollWM adds a trust surface
(arbitrary command execution over the socket). If added, it should be **opt-in**
and ideally restricted (e.g. only argv whose program basename is `jcode`, or
gated by a config flag), because the socket is the app's RCE boundary.

### Exact CLI syntax + reply summary
```
scrollwm version                      -> {"name":...,"version":...,"protocol":1,"verbs":[...]}
scrollwm arrange-pids <pid> [pid...]  -> ok: arranged A of B windows (N new) | error: ...
scrollwm focus-pid <pid>              -> ok: focused column K (title) | error: no managed column matches pid <pid>
scrollwm focus-title <substr>         -> ok: focused column K (title) | error: no managed column title contains "<substr>"
scrollwm workspace-new                -> ok: on workspace K of M (new)
scrollwm spawn-strip <argv...>        -> ok: spawning into strip (will adopt on appear) | error: ...
```
All additive; each is ~5-15 lines in `ControlCommands.handleControlCommand` plus
one entry in `controlVerbs`. None changes existing verb behavior.

---

## 4. Rust client API sketch

### 4.1 Where it lives
New crate **`jcode-scrollwm`** (`crates/jcode-scrollwm`), workspace member,
`publish = false`, deps `serde`/`serde_json`/`anyhow`/`dirs` (+ `libc` only if we
want `SHUT_WR`; `std::os::unix::net::UnixStream::shutdown(Write)` suffices, no
libc). Rationale for a dedicated crate (not folding into
`jcode-terminal-launch`): it is consumed by `jcode-app-core`
(`comm_session.rs` spawn path) and by onboarding (`jcode-tui` /
`jcode-setup-hints`), and it owns a platform integration with its own
detection/version logic; keeping it standalone keeps `jcode-terminal-launch`
focused on terminal emulators and keeps the macOS-only surface isolable behind
`#[cfg(target_os = "macos")]` with a no-op stub elsewhere.

The whole client is **synchronous + blocking** (one tiny round-trip; mirrors the
Swift CLI and is callable from non-async contexts), with an optional
`tokio::task::spawn_blocking` wrapper for the async spawn path.

### 4.2 API
```rust
// crates/jcode-scrollwm/src/lib.rs

/// Resolved like ScrollWM's ControlSocket.path(): $SCROLLWM_CONTROL_SOCK or
/// ~/Library/Application Support/ScrollWM/control.sock.
pub fn control_socket_path() -> PathBuf;

#[derive(Debug)]
pub enum ScrollWmError {
    NotRunning,                 // ENOENT / ECONNREFUSED on connect
    Io(std::io::Error),
    Protocol(String),           // reply began with "error:"
    Unsupported { verb: &'static str, have: u32, need: u32 }, // version gate
}

/// One short-lived connect/send/recv round-trip. Verb line in, trimmed reply out.
/// Never auto-launches. Times out fast (default ~750ms connect+read).
pub fn send_raw(line: &str) -> Result<String, ScrollWmError>;

#[derive(Clone, Debug)]
pub struct ScrollWm { socket: PathBuf, timeout: Duration }

impl ScrollWm {
    pub fn discover() -> Self;                       // default socket + timeout
    pub fn with_socket(path: PathBuf) -> Self;       // for SCROLLWM_CONTROL_SOCK / sandbox

    // --- detection / handshake ---
    pub fn is_running(&self) -> bool;                // ping == "pong"
    pub fn hello(&self) -> Result<ScrollWmInfo, ScrollWmError>; // `version`, falls back to status+bundle
    pub fn status(&self) -> Result<StripStatus, ScrollWmError>;

    // --- actions jcode uses ---
    pub fn arrange(&self) -> Result<ArrangeOutcome, ScrollWmError>;
    pub fn arrange_pids(&self, pids: &[u32]) -> Result<ArrangeOutcome, ScrollWmError>; // needs protocol>=1
    pub fn focus_index(&self, one_based: usize) -> Result<(), ScrollWmError>;
    pub fn focus_title(&self, needle: &str) -> Result<(), ScrollWmError>;             // needs protocol>=1
    pub fn focus_pid(&self, pid: u32) -> Result<(), ScrollWmError>;                   // needs protocol>=1
    pub fn workspace(&self, target: WorkspaceTarget) -> Result<WorkspaceState, ScrollWmError>;
    pub fn workspace_new(&self) -> Result<WorkspaceState, ScrollWmError>;             // needs protocol>=1
    pub fn display(&self, sel: &str) -> Result<String, ScrollWmError>;
    pub fn reload_config(&self) -> Result<(), ScrollWmError>;

    // --- cold start (CLI/`open` fallback, opt-in) ---
    pub fn ensure_running(&self, launch: LaunchPolicy) -> Result<(), ScrollWmError>;
}

#[derive(serde::Deserialize, Debug)]
pub struct ScrollWmInfo { pub name: String, pub version: String,
    #[serde(default)] pub protocol: u32, #[serde(default)] pub verbs: Vec<String> }

#[derive(serde::Deserialize, Debug)]
pub struct StripStatus { pub managing: bool, pub focus_mode: String,
    pub window_count: u32, pub focused_column: Option<u32>,
    pub workspace: Option<u32>, pub workspace_count: Option<u32>,
    #[serde(default)] pub columns: Vec<Column> }

#[derive(serde::Deserialize, Debug)]
pub struct Column { pub index: u32, pub app: String, pub title: String,
    pub width: u32, pub focused: bool, pub healthy: bool }

pub enum WorkspaceTarget { Up, Down, Index(u32) }
pub enum LaunchPolicy { Never, ViaCli, ViaOpen } // Never = pure socket; others shell out once
pub struct ArrangeOutcome { pub arranged: u32, pub new: u32, pub already_managing: bool }
```
Wire details (matching `ControlServer`): connect `UnixStream`; write
`format!("{line}\n")`; `stream.shutdown(Shutdown::Write)`; read to EOF; trim; if
it starts with `error:` -> `Protocol`. Replies for non-JSON verbs are parsed
leniently (e.g. extract the trailing `column K`), but jcode should prefer
re-reading `status` for authoritative state rather than scraping prose.

### 4.3 Getting pids (the hard part) - how `arrange_pids` / `focus_pid` are fed
jcode's headed spawn (`spawn_visible_session_window` ->
`spawn_command_in_new_terminal`) runs `open -na Ghostty ...` /
`osascript ... do script` and **detaches**, so the returned child pid is the
short-lived `open`/`osascript`, not the terminal window's process. The adoptable
AX window belongs to the terminal app (e.g. `com.mitchellh.ghostty`), shared
across all its windows -> a single pid maps to many windows, so `focus_pid` on the
terminal pid is ambiguous.
Consequence: **prefer title-keyed control.** jcode already sets a unique terminal
title per session (`resumed_window_title`), so `focus_title`/title-matched adopt
are the reliable primitives. `arrange` (whole-Space) + `focus_title` covers the
core flow without any pid at all. `arrange_pids`/`focus_pid` stay useful for
non-terminal/native windows and future direct-window spawns, but are not on the
critical path for the swarm-terminal use case.

---

## 5. Handshake / version negotiation

Sequence jcode runs before driving anything (cached for the session, re-checked
on `NotRunning`):
1. `ping` -> `"pong"`? If connect fails with `NotRunning`, ScrollWM is absent;
   surface the onboarding/opt-in path, do not error hard.
2. `hello()`:
   - Send `version`. If recognized, parse `{version, protocol, verbs}`.
   - If `version` is unknown (older ScrollWM replies
     `error: unknown command 'version'...`), fall back: treat `protocol = 0`,
     read `status` for capability presence, and read the bundle's
     `CFBundleShortVersionString` only as a display string.
3. Gate optional verbs on `protocol`/`verbs`: call `arrange_pids` only if
   `protocol >= 1` (or `verbs.contains("arrange-pids")`); otherwise degrade to
   plain `arrange` + `focus_title`.

This makes jcode forward/backward compatible: a new jcode against an old
ScrollWM degrades to the v0 verb set; an old jcode against a new ScrollWM just
ignores extra `verbs`. The negotiated `protocol` integer (not the marketing
version) is the contract; bump it in ScrollWM whenever a verb's wire shape
changes.

---

## 6. Failure modes & handling

| Condition | Detection | jcode behavior |
|---|---|---|
| ScrollWM not installed | `control_socket_path()` parent dir/socket absent; `which scrollwm` empty | Treat as opt-out; only offer install during onboarding; never block spawning. |
| Installed but not running | connect -> `ENOENT`/`ECONNREFUSED` -> `NotRunning` (matches `ControlClient.Failure.notRunning`) | If user opted in, `ensure_running(ViaCli)` once (run `scrollwm arrange` or `open -a ScrollWM`, poll ~6s like `retryAfterLaunch`); else skip arranging, log once. |
| Stale socket file (crash) | connect -> `ECONNREFUSED` | Same as not-running; the app `unlink`s + rebinds on next start (`ControlServer.start`). |
| Older ScrollWM (no new verbs) | `version` unknown / `verbs` lacks the verb | Degrade to v0 verbs; never send an unsupported verb (avoids `error: unknown command`). |
| Not AX-trusted / session locked | `arrange*` returns `error: ...` (controller refuses via `LifecycleMonitor.sessionIsActive()`) | Surface as a one-time hint ("grant ScrollWM Accessibility"); do not retry-loop. |
| `focus*` while dormant | `error: not managing; run scrollwm arrange first` | Send `arrange` (or `arrange_pids`) first, then retry focus. |
| `arrange` with nothing to adopt | `error: nothing to arrange ...` | Benign; agents may not have opened windows yet -> brief retry/backoff, then give up quietly. |
| Slow/hung app (main-thread blocked) | read exceeds client timeout | Time out (~750ms), return `Io`; never block the spawn path; retry at most once. |
| Wrong-owner socket / perms | connect -> `EPERM`/`EACCES` | `Io`; do not escalate. Socket is `chmod 0600` + per-user `Application Support`, so cross-user contact should not happen. |
| Sandbox/dev instance | honor `SCROLLWM_CONTROL_SOCK` via `with_socket` | Tests/dev never touch the real session (same override ScrollWM's sandbox uses). |

Cross-cutting rule: **ScrollWM control is best-effort and must never gate jcode
functionality.** Every call is fire-and-log on failure; the swarm spawn succeeds
whether or not the WM cooperates.

---

## 7. Should jcode bundle/own a `scrollwm` integration helper?

**No bundling of the ScrollWM binary; yes to owning a thin client + an opt-in
installer hook.**
- Do NOT vendor or ship `scrollwm`/`ScrollWM.app` inside jcode. It needs the
  Accessibility TCC grant, ships its own signed/notarized bundle + in-app
  updater (`Updater.swift`, cask `auto_updates true`), and lives on its own
  release cadence. jcode embedding it would fork the trust + update story.
- DO own `jcode-scrollwm` (the Rust client above) - small, no extra runtime
  deps, macOS-gated.
- DO add an **opt-in onboarding action** that installs ScrollWM via its
  published channel (Homebrew cask `1jehuang/scrollwm/scrollwm`, or
  `scripts/web-install.sh` curl|bash) and links the `scrollwm` CLI - reusing
  ScrollWM's own installer rather than reimplementing it. This belongs to the
  onboarding explorer; control-plane just exposes `is_running()`/`hello()` so
  onboarding can show live status.
- Detection over assumption: jcode resolves the socket path itself and never
  assumes `scrollwm` is on PATH for steady-state calls.

---

## 8. Reverse direction: ScrollWM's `spawn` keybind launching jcode

`Config.swift` already ships the recipe, commented out, in `defaultFileContents`:
```jsonc
"spawn": {
  "ctrl+opt+j": "open -na Ghostty --args --working-directory=$HOME/scrollwm --command=$HOME/.local/bin/jcode",
  "ctrl+opt+return": "open -na Ghostty"
}
```
`spawn` maps a chord -> `/bin/sh -c <cmd>` global hotkey (`spawnBindings()`),
adopted into the strip by ScrollWM's normal new-window fast path. This is the
clean reverse hook: a user opts into "ctrl+opt+j opens jcode in the strip."

jcode's onboarding (opt-in) can offer to write this binding:
- Compute jcode's launch command (resolve the real `jcode` path like
  `client_update_candidate` / `current_exe`), pick the user's best terminal
  (jcode already knows this via `jcode-terminal-launch`), and write a `spawn`
  entry into `~/Library/Application Support/ScrollWM/config.json`.
- Because ScrollWM's config is JSONC with comment-stripping and is the single
  source of truth (`ScrollWMConfig.load/parse`), jcode should do a minimal,
  idempotent merge (add the key only if absent; never rewrite the user's file
  destructively), then call **`reload`** over the socket so it takes effect live
  (no relaunch).
- Use Ghostty's `--command=` not `-e` (the default file's comment calls out that
  `-e` triggers Ghostty's per-launch security prompt).

Net: jcode -> ScrollWM uses the socket; ScrollWM -> jcode uses one `spawn`
binding. Two thin, independent hooks, each opt-in.

---

## 9. Security model

- **Transport scope:** Unix socket under per-user `~/Library/Application
  Support/ScrollWM/`, `chmod 0600` (`ControlServer.start`), no network, no
  entitlement. Reaching it already implies same-user filesystem access. Verified
  live: `srw-------` owned by the user. jcode (same user) is the intended caller.
- **No new surface from jcode's side:** the client only connects out; it never
  binds or listens.
- **`spawn-strip` is the one risky proposed verb** (arbitrary command over the
  socket). Recommendation: keep it opt-in / restricted (program allowlist or
  config flag) or omit it for the first cut, since jcode can spawn terminals
  itself.
- **Config writes** (reverse hook) are an additive, idempotent JSONC merge of a
  single `spawn` key, never a clobber, then a `reload` - the user can read/revert
  the one line.
- **Never destructive to the user's session:** jcode must not send `close`,
  `release` of the user's real arrangement, or `quit`; control is limited to
  arrange/focus/workspace for surfaces jcode itself created.

---

## 10. Minimal first PR

Scope: pure jcode-side, additive, macOS-gated, no behavior change unless
ScrollWM is present and the user opts in. Lands the transport + handshake; new
ScrollWM verbs are a separate ScrollWM-side PR.

1. **New crate `jcode-scrollwm`** (`crates/jcode-scrollwm`):
   - `control_socket_path()` (env override + Application Support default).
   - Blocking `send_raw()` (connect / write+`shutdown(Write)` / read-to-EOF /
     trim; `ENOENT|ECONNREFUSED -> NotRunning`; `error:` -> `Protocol`), modeled
     on `jcode-mobile-sim::send_request` + Swift `ControlClient.send`.
   - `ScrollWm { is_running(), hello() (version with status+bundle fallback),
     status(), arrange(), focus_index(), focus_title()-via best-effort,
     workspace(), reload_config() }`.
   - `#[cfg(not(target_os = "macos"))]` no-op stub returning `NotRunning`.
   - Unit tests: parse a captured `status` JSON + `version` JSON; assert
     `error:`-prefixed reply maps to `Protocol`; assert socket-path resolution
     honors `SCROLLWM_CONTROL_SOCK`. (A loopback test can bind a temp
     `UnixListener` that echoes canned replies - no ScrollWM needed.)
2. **Add to workspace** `members` in root `Cargo.toml`; depend from
   `jcode-app-core`.
3. **Wire one call** in the visible swarm-spawn path
   (`comm_session.rs`, after `spawn_visible_session_window` succeeds): if
   `ScrollWm::discover().is_running()` and a config flag
   (`agents.scrollwm_integration`, default off) is set, fire a best-effort
   `arrange()` (and later `focus_title(<session label>)`). Strictly fire-and-log;
   never affects spawn success.
4. **Config flag** in `jcode-config-types` (`AgentsConfig`):
   `scrollwm_integration: bool` (default `false`), so this is inert until a user
   (or onboarding) enables it.

Explicitly out of scope for PR 1 (follow-ups): the new ScrollWM verbs
(`version`, `arrange-pids`, `focus-pid/title`, `workspace-new`, `spawn-strip`) as
a ScrollWM repo PR; the onboarding install + `spawn`-binding writer; the
`ensure_running` cold-start launcher.

Validation for PR 1: `cargo test -p jcode-scrollwm` (offline, loopback);
manual live check against the running app limited to `ping`/`status`/`version`
plus a `scrollwm sandbox`-spawned disposable strip for any `arrange`/`focus`
exercise - **never** `arrange` the real desktop in a test (ScrollWM safety
contract; use `SCROLLWM_CONTROL_SOCK` + sandbox).
