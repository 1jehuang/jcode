# KCode build identity, shared sessions, and reload

Status: proposed
Date: 2026-11-17
Companion to: `2026-11-17-kcode-freeze-and-churn.md`
Upstream gap this depends on: issue #2207 (jcode/kcode session delineation)

## 1. Build identity

Rules, in order of importance:

1. **Our artifacts are named `kcode*`, never `jcode*`.** No prefix, no suffix, no `-next`, no `-fixed-<sha>` under a jcode name. A jcode-named file that is actually our build is what caused a locked-file install failure and a confusing bin directory.
2. **No name may collide with the upstream sync process.** Anything the sync-up machinery writes or reads keeps upstream naming; ours sits beside it under `kcode`.
3. Install location: `%LOCALAPPDATA%\kcode\bin\kcode.exe` (jcode keeps `%LOCALAPPDATA%\jcode\bin\jcode.exe`). Two binaries, two names, no overwrite, so a running chat can never block an install again.
4. Scratch/build artifacts: `kcode-<line>-<sha>.exe`. Nothing inside jcode's directories.

## 2. The session-store question, and why copies happen

Today a kcode build that uses its own home directory writes its own session store, so a chat opened in kcode is effectively **copied/forked into `.kcode`**. Consequences, all observed:

- reusing a change in jcode does not appear in kcode, because they are separate stores;
- reconciling the two means forking again;
- the picker cannot say which client a session belongs to (issue #2207).

**Least-complexity decision: one store, one server, two clients.**

- kcode defaults to the **same** state directory as jcode (`KCODE_HOME` if set, else `JCODE_HOME`, else the shared default). No copy step, no re-fork, no divergence to reconcile.
- Both clients attach to the **same server over the same socket**. A session opened in jcode is the same session kcode sees, in real time, because it is the same server-side object stream.
- Divergence is confined to the client presentation layer: kcode's own features translate what it reads, and where a jcode-side change is newer than kcode's fork, kcode runs in a **back-compat mode with a reduced feature surface** rather than forking the session.

That is the smallest design that satisfies "kcode reflects jcode" without a sync project: no duplication of state, no migration, no merge protocol.

## 3. Translation layer, explicitly bounded

kcode is allowed to differ from jcode in:

- which commands/hotkeys it exposes;
- how it renders a session (extra panels, denser layout);
- what it does with events jcode added that kcode does not model yet.

kcode is **not** allowed to differ in:

- the wire protocol on the socket (it must speak jcode's, or fail loudly rather than silently degrade);
- the on-disk session format (write only fields jcode already understands; extra fields must be additive and ignorable);
- session identity (same ids, so either client can resume what the other created).

Back-compat mode is a runtime state, not a build: when kcode sees a session feature it does not model, it drops the feature and says so once, instead of refusing to open the session.

## 4. Reload contract

jcode already has the mechanism; use it instead of asking a human to restart:

| Mechanism | What it does | When we use it |
|---|---|---|
| `jcode server reload --force` (CLI) | reloads the server process | after a server-side build change |
| `/client-reload` (in a remote TUI) | forces the client binary to reload | after installing a new client build |
| `display.auto_server_reload` | detects a newer server binary and reloads automatically | default path; the manual commands are the escape hatch |

Rules for our builds:

1. `kcode server reload --force` and `/client-reload` must exist and behave identically, because they are the same code.
2. Installing a kcode build must be: build → place at the kcode path → reload. Never "close your chats and copy over jcode.exe".
3. A reload that would change the wire protocol must refuse and say so, rather than half-attach.

## 5. Delineation (depends on #2207)

Until #2207 lands, kcode cannot label its sessions as kcode. The workaround is explicit, not inferred: kcode sets its own origin marker in session metadata as an **additive** field, and reads jcode's field when present. Once #2207 adds a harness/origin field, kcode adopts it and drops the workaround.

## 6. Acceptance

- `kcode.exe` exists at the kcode path, `jcode.exe` untouched.
- A session opened in jcode is visible and resumable in kcode with the same id, with no copy on disk.
- A kcode build change is picked up by `server reload` / `/client-reload`, not by restarting chats.
- No file under jcode's directories is named after our build.
