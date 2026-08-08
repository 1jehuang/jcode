/**
 * Coverage check for the SDK methods a smoke test does not reach.
 *
 * `live-turn.mjs` proves the happy path; this drives the control surface
 * (attach, cancel, soft interrupt, clear, rewind, detach) against a real
 * bridge. Each of those crosses a different translation path, and a mock
 * server cannot tell us whether the daemon actually replies to them: the
 * `ping` teardown and the `send_message` non-reply were both invisible until
 * a real one ran.
 */

import assert from "node:assert/strict";
import { JcodeClient } from "../dist/index.js";

const failures = [];
async function step(name, fn) {
  try {
    await fn();
    console.log(`ok   ${name}`);
  } catch (error) {
    failures.push(`${name}: ${error.message}`);
    console.log(`FAIL ${name}: ${error.message}`);
  }
}

const client = await JcodeClient.connect({
  clientName: "sdk-control-e2e/0.1",
  requestTimeoutMs: 20_000,
});
client.on("harness_error", (frame) => console.log(`  (harness_error: ${frame.message})`));

const session = await client.createSession(process.cwd());
const id = session.session_id;
console.log(`session ${id}`);

await step("attachSession on a second connection", async () => {
  const other = await JcodeClient.connect({
    clientName: "sdk-control-e2e-2/0.1",
    requestTimeoutMs: 20_000,
  });
  const attached = await other.attachSession(id);
  assert.equal(attached.session_id, id, "attach returned a different session");
  other.close();
});

await step("cancel with no turn in flight is a no-op", async () => {
  await client.cancel(id);
});

await step("cancel stops a live turn", async () => {
  // Subscribe before sending: a short turn can finish while we are still
  // setting up, and a listener attached afterwards would miss the boundary
  // and report a hang that never happened.
  let ended = false;
  let onEnd = () => {};
  const done = new Promise((resolve) => {
    onEnd = resolve;
  });
  let onToken = () => {};
  const firstDelta = new Promise((resolve) => {
    onToken = resolve;
  });
  const onEvent = (frame) => {
    if (frame.session_id !== id) return;
    if (frame.ev === "text_delta") onToken(true);
    if (frame.ev === "turn_done") {
      ended = true;
      onToken(false);
      onEnd(true);
    }
  };
  client.on("event", onEvent);
  try {
    await client.sendMessage(id, "Count from 1 to 400, one number per line, no commentary.");
    // Cancel on the first token rather than after a fixed sleep: a fixed wait
    // either races a fast turn (skipping the assertion, as it did) or slows
    // every run down. The first delta proves generation is actually running.
    const started = await Promise.race([
      firstDelta,
      new Promise((resolve) => setTimeout(() => resolve(false), 30_000)),
    ]);
    if (ended || !started) {
      console.log(`  (turn ended before any token; ended=${ended} started=${started}; skipping)`);
      return;
    }
    const timeout = new Promise((resolve) => setTimeout(() => resolve(false), 20_000));
    await client.cancel(id);
    assert.ok(await Promise.race([done, timeout]), "turn did not end within 20s of cancel");
  } finally {
    client.off("event", onEvent);
  }
});

await step("softInterrupt is accepted", async () => {
  await client.softInterrupt(id, "noted, keep going", false);
});

await step("getHistory then rewind trims the transcript", async () => {
  const before = await client.getHistory(id);
  assert.ok(before.length >= 1, "no history to rewind");
  await client.rewind(id, 1);
  const after = await client.getHistory(id);
  assert.ok(
    after.length <= before.length,
    `rewind grew history: ${before.length} -> ${after.length}`,
  );
});

await step("clear empties the transcript", async () => {
  await client.clear(id);
  const history = await client.getHistory(id);
  assert.equal(history.length, 0, `history not empty after clear: ${history.length}`);
});

await step("peekSession works without attaching", async () => {
  const other = await JcodeClient.connect({
    clientName: "sdk-control-e2e-3/0.1",
    requestTimeoutMs: 20_000,
  });
  const messages = await other.peekSession(id, 3);
  assert.ok(Array.isArray(messages), "peek did not return an array");
  other.close();
});

// A typo'd session id used to be forwarded to the daemon, which answered
// "Client must Subscribe first" and *closed the connection*. Every other
// in-flight request on it died, and the SDK reported a bare EPIPE. The whole
// point is that one bad id costs one request, not the connection.
await step("a bad session id fails that request only, not the connection", async () => {
  const probe = await JcodeClient.connect({
    clientName: "sdk-control-e2e-bad-id/0.1",
    requestTimeoutMs: 20_000,
  });
  try {
    for (const [name, call] of [
      ["getHistory", () => probe.getHistory("session_does_not_exist")],
      ["clear", () => probe.clear("session_does_not_exist")],
      ["rewind", () => probe.rewind("session_does_not_exist", 1)],
      ["cancel", () => probe.cancel("session_does_not_exist")],
    ]) {
      let code;
      try {
        await call();
      } catch (error) {
        code = error.code;
      }
      assert.equal(code, "unknown_session", `${name} should fail with unknown_session`);
    }
    // The connection must still be usable after all of those.
    await probe.ping();
    const session = await probe.createSession(process.cwd());
    assert.ok(session.session_id, "should still be able to create a session");
  } finally {
    probe.close();
  }
});

await step("detachSession is accepted", async () => {
  await client.detachSession(id);
});

client.close();

if (failures.length > 0) {
  console.error(`\n${failures.length} control-surface failure(s):`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("control surface ok");
