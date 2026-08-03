# @1jehuang/jcode-sdk

TypeScript SDK for the **jcode harness API** (protocol v1) — the stable,
versioned boundary between the jcode agent runtime and any client.

It mirrors `crates/jcode-harness-api` and talks NDJSON over the harness API
Unix socket. Schema drift is guarded from both sides: a Rust test fails if a
variant is added without mirroring it here, and a Node test fails if the tag
sets diverge.

Full documentation: **[jcode.sh/sdk](https://jcode.sh/sdk)**

## Install

```bash
npm install @1jehuang/jcode-sdk
```

From a source checkout:

```bash
cd sdk/typescript
npm install
npm run build
```

## Requirements

jcode must be installed, and Node 20 or newer.

macOS and Linux are exercised end to end in CI. Windows builds and is wired up
(the bridge listens on a named pipe rather than a Unix socket, and the SDK
resolves the same pipe name), but it has no live end-to-end coverage yet, so
treat it as untested rather than unsupported and please report what breaks.

`launch()` needs nothing else: it starts its own daemon and bridge. `connect()`
needs a bridge already running, which the user starts once and leaves running.
The bridge ships in the released binary, so no Rust toolchain is needed:

```bash
jcode api-bridge
```

It starts the jcode server if one is not already up, then exposes the API
socket (`$XDG_RUNTIME_DIR/jcode-api.sock`) and translates onto the internal
daemon socket. The socket is owner-only, matching the daemon socket it fronts.

Use `--api-socket <path>` to listen elsewhere, and set `JCODE_API_SOCKET` to
the same path in your client. (The global `--socket` selects the *internal
daemon* socket, which is a different thing.)

## Two ways to use jcode

**Embed jcode as an agent engine** (`launch`). Starts a private instance with
its own state, sessions, and sockets. It cannot see or disturb the jcode the
user runs in their terminal, and `close()` shuts it down. This is the default
for applications.

```ts
const client = await JcodeClient.launch({ workingDir: process.cwd() });
const session = await client.createSession();
console.log((await client.run(session.session_id, "hello")).text);
await client.close();  // stops the instance
```

Provider logins are inherited from the user by default, since an instance with
no credentials cannot reach a model. Pass `inheritLogins: false` to start empty
and supply your own. Pass `jcodeHome` to keep sessions across runs instead of
using a temporary directory.

Inheritance shares only recognized credential **files**, never whole config or
tool directories. This keeps rotating OAuth tokens coherent without exposing
unrelated transcripts and state, and instance cleanup cannot recurse into the
user's credential directories. Temporary homes are owner-only and cleanup is
restricted to SDK-created temp paths. The launched process still runs as the
current OS user and can spend those accounts' quota, so disable inheritance
when running untrusted application code (`inheritLogins: false`).

**Automate the user's own jcode** (`connect`). Attaches to the jcode already
running on the machine, sharing its live sessions. This is what an editor
plugin or a status dashboard wants. Anything it does is visible in the user's
terminal, and it needs a bridge already running (`jcode api-bridge`).

## Quick start

Swap `launch` for `connect` to drive the user's own jcode instead of a private
instance; everything after that line is identical.

A complete runnable application is available in
[`examples/demo-app`](./examples/demo-app).

```ts
import { JcodeClient } from "@1jehuang/jcode-sdk";

const client = await JcodeClient.launch({ workingDir: process.cwd() });

const session = await client.createSession(process.cwd());
const turn = await client.run(session.session_id, "What files are in src/?", {
  autoApprove: true,
  onEvent: (event) => {
    if (event.ev === "text_delta") process.stdout.write(event.text);
  },
});

console.log("\ntools:", turn.toolCalls.map((call) => call.name));
console.log("tokens:", turn.usage);
client.close();
```

## Structured output

`runStructured()` asks the model for JSON, validates the response with Ajv, and
sends bounded corrective retries when the response is not valid JSON or does not
match your JSON Schema. It returns the normal turn metadata plus validated
`data` and an `attempts` audit trail.

```ts
const result = await client.runStructured<{ summary: string; count: number }>(
  session.session_id,
  "Summarize the current changes",
  {
    schema: {
      type: "object",
      additionalProperties: false,
      required: ["summary", "count"],
      properties: {
        summary: { type: "string" },
        count: { type: "integer", minimum: 0 },
      },
    },
    maxRetries: 2, // default
  },
);

console.log(result.data.summary);
```

If all attempts fail validation, the promise rejects with
`StructuredOutputError`. Its `validationErrors`, `lastText`, and `attempts`
fields are stable for logging or user-facing diagnostics.

## Streaming

`run()` is the batch convenience path. For live UIs, iterate events directly:

```ts
const session = await client.createSession();
await client.sendMessage(session.session_id, "hello");

for await (const event of client.events(session.session_id)) {
  switch (event.ev) {
    case "text_delta":
      process.stdout.write(event.text);
      break;
    case "tool_start":
      console.log("\n[tool]", event.name);
      break;
    case "permission_request":
      await client.respondToPermission(session.session_id, event.request_id, "allow");
      break;
    case "turn_done":
      return;
  }
}
```

Per-kind listeners work too: `client.on("token_usage", handler)`.

Protocol `error` frames arrive on the `harness_error` channel, not `error`.
Node treats an unlistened `error` event as a fatal throw, so the plain channel
is reserved for transport faults.

### All-session events

`globalEvents()` is the process-wide stream for dashboards and integrations. The
bridge attaches one session per connection, so the SDK discovers every persisted
session, opens one child connection for each, and fans their streams into one
bounded iterator. Discovery repeats to include sessions created later.

```ts
const stop = new AbortController();
for await (const event of client.globalEvents({ signal: stop.signal })) {
  if ("session_id" in event) console.log(event.session_id, event.ev);
}
```

Delivery is **at-least-from-attach**, not historical replay. Protocol v1 cannot
recover events emitted before a child attaches or during an unexpected
disconnect and reattach. Per-session order is preserved, but there is no total
ordering across sessions. `return()`, aborting the signal, or closing the parent
closes all children. The iterator fails with `event_buffer_overflow` rather than
silently dropping events if its bounded queue fills. A custom `Transport` is
rejected with `unsupported_transport` because it cannot be cloned safely into
independent child connections. Set `discoveryIntervalMs: 0` for one initial
discovery pass only.

## API surface

| Method | Purpose |
| --- | --- |
| `JcodeClient.launch(options)` | Start a private instance and connect to it |
| `JcodeClient.connect(options)` | Attach to the jcode already running on this machine |
| `listSessions({ includeArchived? })` | Every persisted session, optionally including archived sessions |
| `archiveSession(id)` / `restoreSession(id)` | Reversibly hide or restore a session |
| `setRetentionPolicy(days?)` | Auto-archive inactive sessions, or disable retention |
| `createSession(workingDir?)` | Create and attach |
| `attachSession(id)` / `detachSession(id)` | Subscribe / unsubscribe |
| `sendMessage(id, content, images?)` | Send a user message (awaits `message_accepted`) |
| `run(id, content, options?)` | Send and collect one full turn |
| `runStructured(id, content, options)` | Send, validate JSON Schema output, and retry corrections |
| `events(sessionId?)` | Async iterator over stream events |
| `globalEvents(options?)` | Bounded fan-in stream over all persisted and newly created sessions |
| `cancel(id)` / `softInterrupt(id, content, urgent?)` | Interrupt a turn |
| `getHistory(id)` / `peekSession(id, limit?)` | Read a transcript (peek works unattached) |
| `clear(id)` / `rewind(id, index)` | Edit history |
| `respondToPermission(id, requestId, decision)` | Answer a permission prompt |
| `listModels(id)` / `setModel(id, model)` | List and choose the session's model |
| `getRuntimeInfo(id)` | Provider, model route, protocol, capability, and health metadata |
| `setApiKey(provider, key)` / `clearApiKey(provider)` | Atomically provision or remove owner-only API-key files |
| `readFile(id, path, maxBytes?)` | Read bounded UTF-8 text under the session root |
| `findFiles(id, query, limit?)` | Find rooted files by path substring |
| `searchText(id, query, options?)` | Bounded rooted literal text search |
| `fileStatus(id, path)` | Read safe rooted file metadata |
| `setReasoningEffort(id, effort)` | Set the cost/quality dial |
| `compact(id)` | Schedule transcript compaction to free context |
| `renameSession(id, title?)` | Set a session title, or clear it |
| `rewindUndo(id)` | Restore what the last `rewind` removed |
| `cancelSoftInterrupts(id)` | Retract queued soft interrupts |
| `ping()` | Liveness |

## Models

A client that cannot enumerate models cannot offer a picker, so the catalog is
first-class. It is served from the push the daemon sends on attach, meaning
opening a picker costs no round trip:

```ts
const { models, current } = await client.listModels(session.session_id);
await client.setModel(session.session_id, "claude-opus-5");
```

An unknown model, or one the provider refuses, rejects with `invalid_request`
rather than silently leaving the session where it was. When the model changes,
every client attached to that session receives a `model_info` event, so a UI
that did not make the change still updates.

`setReasoningEffort(id, effort)` sets how much the model deliberates. The
accepted values are per-provider (typically `minimal` through `max`), so this
takes a string and reports what the provider says instead of guessing at a
union that would go stale.

`getRuntimeInfo(id)` adds the active provider/model, every available model route,
the negotiated protocol version, advertised capability strings, and a live ping.
API-key provisioning accepts the supported provider aliases, normalizes Gemini
aliases to `gemini`, supports the jcode subscription key, writes owner-only files
atomically, and asks the daemon to reload credentials. OAuth tokens are not part
of this API.

File methods are rooted at the persisted session working directory. Absolute
paths, `..`, and symlink escapes are rejected. Directory walks do not follow
symlinks and both file count and byte scanning are bounded. `readFile()` accepts
UTF-8 text only and reports when its byte limit truncated the result.

## Session archive and retention

Archiving never deletes a transcript. It removes the session from the default
`listSessions()` result and records an archive timestamp in owner-only state.
Pass `includeArchived: true` to display and restore archived sessions.
`setRetentionPolicy(days)` applies the same reversible archive operation to
inactive persisted sessions when sessions are listed. Omit `days` to disable
automatic retention.

## Long sessions

`compact(id)` summarizes the transcript so far, freeing context. It is
asynchronous: the daemon summarizes at the next safe point rather than
interrupting a turn, so it resolving means the request was accepted, not that
the transcript has already shrunk. Read the history afterwards for the result.

It is refused below about 10% context usage, on the grounds that there is
nothing worth compacting yet, and the rejection carries the current usage. So
treat `invalid_request` here as information for the user rather than an error
to retry.

```ts
await client.renameSession(id, "nightly refactor");  // omit the title to clear it
await client.rewind(id, 4);
await client.rewindUndo(id);                          // rewind is reversible
await client.cancelSoftInterrupts(id);                // retract what is queued
```

## Instance lifecycle

A launched instance owns a daemon and a state directory, and both are cleaned
up for you:

- `close()` stops the daemon and removes an ephemeral home. It waits for the
  process to actually be gone, so the directory cannot be recreated behind the
  delete. Expect it to take a few seconds.
- If your process exits without calling `close()`, including after an uncaught
  exception, the instance is still reaped. Without this a server that restarts
  would accumulate one daemon and one temp directory per restart.
- `SIGKILL` is the one case nothing can cover, since no handler runs.

### `launch()` options

| Option | Effect |
| --- | --- |
| `workingDir` | Working directory for sessions. Defaults to `process.cwd()`. |
| `jcodeHome` | Keep state at a fixed path across runs. Defaults to a temporary directory that is removed on `close()`. See the note below. |
| `inheritLogins` | Inherit the user's provider logins. Defaults to `true`. |
| `binary` | Path to the jcode binary. Defaults to `jcode` on `PATH`. |
| `env` | Extra environment variables for the instance. |
| `startupTimeoutMs` | How long to wait for the instance to come up. Defaults to 30000. |
| `cleanupTimeoutMs` | How long `close()` spends removing an ephemeral home. Defaults to 30000. |
| `inheritStderr` | Forward the instance's stderr to your process. Defaults to `false`. |

A fixed `jcodeHome` persists transcripts on disk. `listSessions()` discovers
those records even on a fresh, unattached connection, so a restarted process can
rebuild its complete session index without keeping a separate id registry.

## Configuration

| Env var | Effect |
| --- | --- |
| `JCODE_API_SOCKET` | Override the API socket path |
| `JCODE_RUNTIME_DIR` | Override the runtime directory |
| `XDG_RUNTIME_DIR` | Default runtime directory on Linux |

Or pass `socketPath` to `connect()`.

## Errors

Every failure is a `HarnessError` with a `code`:

| Code | Meaning |
| --- | --- |
| `jcode_not_found` | `launch()` could not run jcode: not installed, or not on `PATH`. Pass `binary` with a full path. |
| `startup_failed` | The instance exited while starting. The message carries its stderr. |
| `startup_timeout` | The instance never opened its socket within `startupTimeoutMs`. |
| `connect_failed` | The bridge is not running, or the socket path is wrong. The message names the path and the command to start it. |
| `disconnected` | The connection dropped mid-request. |
| `timeout` | No reply within `requestTimeoutMs` (30s by default). |
| `structured_schema_invalid` | `runStructured()` received an invalid JSON Schema. |
| `structured_output_invalid` | The model did not produce valid structured output within the retry budget. |
| `unsupported_transport` | `globalEvents()` was requested on a custom transport that cannot be cloned safely. |
| `event_buffer_overflow` | A `globalEvents()` consumer fell behind its bounded queue. |
| `unknown_session`, `invalid_request`, ... | Protocol errors relayed from the harness. |

## Stability

This package is generally available and follows semver against the protocol
it speaks.

- **Protocol v1 is stable.** The handshake negotiates a major version, and a
  server that cannot speak v1 is rejected with `unsupported_version` rather
  than half-working. A breaking protocol change bumps to v2 and to a new SDK
  major.
- **Additive changes are minor releases.** New events, new request fields, and
  new methods arrive in minors. Existing frames keep their shape.
- **The API covers what real clients need.** A test diffs the API against every
  request the terminal app makes and fails on an untriaged gap, so the surface
  cannot quietly fall behind the app it mirrors.
- **Drift is checked mechanically, in both directions.** A Rust test reads
  `src/protocol.ts` and fails if a variant *or a field* is missing here; a Node
  test reads the Rust enums and fails if the tag sets diverge. Neither side can
  land a schema change alone.
- **The tarball is tested as a tarball.** `scripts/test_sdk_package.sh` packs
  it, installs it into a throwaway project, and imports it as ESM, as CJS, and
  through `tsc`.

Compatibility: Node 20+, ESM and CJS. Linux and macOS are covered end to end in
CI; Windows builds and is wired up but is not yet exercised live.

## Forward compatibility

The harness may add events at any time within protocol v1. `events()` and
`run()` are typed as `ApiEvent`, the union of kinds this SDK knows, so
`switch (event.ev)` narrows each case; always keep a `default` branch for kinds
added after your version. A frame of unknown kind is delivered as
`UnknownApiEvent` (`{ ev: string; [key: string]: unknown }`); use
`isKnownEvent(frame)` to narrow `AnyApiEvent` when you handle raw frames.

`UnknownApiEvent` is deliberately not a member of `ApiEvent`: a member with
`ev: string` widens the discriminant, and TypeScript then refuses to narrow any
case, typing every field as `unknown`.

## Releasing

See [RELEASING.md](RELEASING.md). `bash scripts/sdk_publish_preflight.sh` runs
every gate and reports what is left.

## Development

```bash
npm run check   # typecheck + build + tests (mock harness, no daemon needed)
```

`test/schema-parity.test.ts` reads the Rust enums directly, and
`crates/jcode-harness-api`'s `typescript_sdk_lists_every_variant` test reads
this package. Adding a variant on either side without the other fails CI.
