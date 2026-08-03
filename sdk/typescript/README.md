# @jcode/sdk

TypeScript SDK for the **jcode harness API** (protocol v1) — the stable,
versioned boundary between the jcode agent runtime and any client.

It mirrors `crates/jcode-harness-api` and talks NDJSON over the harness API
Unix socket. Schema drift is guarded from both sides: a Rust test fails if a
variant is added without mirroring it here, and a Node test fails if the tag
sets diverge.

## Install

```bash
cd sdk/typescript
npm install
npm run build
```

## Requirements

The harness API bridge must be running. It exposes the API socket
(`$XDG_RUNTIME_DIR/jcode-api.sock`) and translates onto the internal daemon
socket:

```bash
cargo run -p jcode-harness-api-server --bin jcode-harness-api-bridge
```

## Quick start

```ts
import { JcodeClient } from "@jcode/sdk";

const client = await JcodeClient.connect({ clientName: "my-app/1.0" });

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

## API surface

| Method | Purpose |
| --- | --- |
| `JcodeClient.connect(options)` | Dial and complete the version handshake |
| `listSessions()` | Sessions visible to this client |
| `createSession(workingDir?)` | Create and attach |
| `attachSession(id)` / `detachSession(id)` | Subscribe / unsubscribe |
| `sendMessage(id, content, images?)` | Send a user message (awaits `message_accepted`) |
| `run(id, content, options?)` | Send and collect one full turn |
| `events(sessionId?)` | Async iterator over stream events |
| `cancel(id)` / `softInterrupt(id, content, urgent?)` | Interrupt a turn |
| `getHistory(id)` / `peekSession(id, limit?)` | Read a transcript (peek works unattached) |
| `clear(id)` / `rewind(id, index)` | Edit history |
| `respondToPermission(id, requestId, decision)` | Answer a permission prompt |
| `ping()` | Liveness |

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
| `connect_failed` | The bridge is not running, or the socket path is wrong. The message names the path and the command to start it. |
| `disconnected` | The connection dropped mid-request. |
| `timeout` | No reply within `requestTimeoutMs` (30s by default). |
| `unknown_session`, `invalid_request`, ... | Protocol errors relayed from the harness. |

## Forward compatibility

The harness may add events at any time within protocol v1. Unknown kinds are
delivered on the generic `event` channel and are typed as
`{ ev: string; [key: string]: unknown }`, so clients should switch on `ev` and
ignore what they do not recognize. Breaking changes bump the major version and
are rejected in the handshake.

## Development

```bash
npm run check   # typecheck + build + tests (mock harness, no daemon needed)
```

`test/schema-parity.test.ts` reads the Rust enums directly, and
`crates/jcode-harness-api`'s `typescript_sdk_lists_every_variant` test reads
this package. Adding a variant on either side without the other fails CI.
