/**
 * Live turn check, driven by `scripts/test_sdk_e2e.sh`.
 *
 * Asserts the SDK's real behaviour against a running harness: handshake,
 * session creation, a full streamed turn, and history readback. Kept out of
 * `npm test` because it needs a bridge, a daemon, and a configured provider.
 */

import assert from "node:assert/strict";
import { JcodeClient } from "../dist/index.js";

const client = await JcodeClient.connect({
  clientName: "sdk-e2e/0.1",
  requestTimeoutMs: 30_000,
});
assert.ok(client.server.length > 0, "server identity missing from handshake");
console.log(`handshake: ${client.server} [${client.capabilities.join(", ")}]`);

await client.ping();
console.log("ping ok");

const session = await client.createSession(process.cwd());
assert.ok(session.session_id, "created session has no id");
console.log(`created ${session.session_id}`);

const sessions = await client.listSessions();
assert.ok(
  sessions.some((entry) => entry.session_id === session.session_id),
  "created session missing from list_sessions",
);
console.log(`list_sessions sees ${sessions.length}`);

const kinds = new Set();
const turn = await client.run(session.session_id, "Reply with exactly: PONG42", {
  autoApprove: true,
  onEvent: (event) => kinds.add(event.ev),
});
console.log(`turn text: ${JSON.stringify(turn.text)}`);
console.log(`turn events: ${[...kinds].sort().join(", ")}`);

assert.ok(turn.text.includes("PONG42"), `unexpected reply: ${turn.text}`);
assert.ok(kinds.has("message_accepted"), "no message_accepted acknowledgement");
assert.ok(turn.usage && turn.usage.output > 0, "no token usage reported");

const history = await client.getHistory(session.session_id);
assert.ok(history.length >= 2, `history too short: ${history.length}`);
console.log(`history: ${history.length} messages`);

const peeked = await client.peekSession(session.session_id, 5);
assert.ok(Array.isArray(peeked), "peek_session did not return messages");
console.log(`peek: ${peeked.length} messages`);

client.close();
console.log("live turn ok");
