/**
 * End-to-end check for observable, cancellable mid-turn messages (#1778).
 *
 * Drives a private instance through a real turn that runs a slow tool, then
 * sends follow-ups while it works:
 *  - `client_id` comes back in `soft_interrupt_injected`, between the tool's
 *    `tool_done` and the model's answer to the follow-up;
 *  - a queued follow-up can be cancelled by id before delivery, and cancelling
 *    one that was already delivered reports `cancelled: false`;
 *  - `background_tool` reports whether a tool was actually moved.
 *
 * Usage: node test/live-soft-interrupt-ids.mjs [path-to-jcode-binary]
 */

import assert from "node:assert/strict";
import { JcodeClient, launchInstance } from "../dist/index.js";

const binary = process.argv[2] ?? "jcode";
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

const instance = await launchInstance({
  binary,
  workingDir: process.cwd(),
  startupTimeoutMs: 60_000,
});
const client = await JcodeClient.connect({
  socketPath: instance.socketPath,
  requestTimeoutMs: 30_000,
});
const session = await client.createSession(process.cwd());
const id = session.session_id;
console.log(`session ${id}`);

function record() {
  const frames = [];
  const onEvent = (frame) => {
    if (frame.session_id === id || frame.session_id === undefined) frames.push(frame);
  };
  client.on("event", onEvent);
  return { frames, stop: () => client.off("event", onEvent) };
}

function waitFor(frames, predicate, timeoutMs, label) {
  return new Promise((resolve, reject) => {
    const started = Date.now();
    const tick = () => {
      const hit = frames.find(predicate);
      if (hit) return resolve(hit);
      if (Date.now() - started > timeoutMs) return reject(new Error(`timed out waiting for ${label}`));
      setTimeout(tick, 50);
    };
    tick();
  });
}

const SLOW_TOOL =
  "Run exactly this bash command and nothing else: `sleep 8 && echo slept`. " +
  "After it finishes, reply with the single word DONE.";

await step("bridge advertises the capabilities", () => {
  assert.ok(client.supports("soft_interrupt_ids"), `capabilities: ${client.capabilities}`);
  assert.ok(client.supports("background_tool_result"));
});

await step("injected event carries ids, ordered after tool_done and before the reply", async () => {
  const rec = record();
  try {
    await client.sendMessage(id, SLOW_TOOL);
    await waitFor(rec.frames, (f) => f.ev === "tool_exec", 60_000, "the slow tool to start");
    await client.softInterruptWithId(id, "Also: reply with the word PINEAPPLE at the end.", { clientId: "steer-1" });
    const injected = await waitFor(
      rec.frames, (f) => f.ev === "soft_interrupt_injected", 60_000, "soft_interrupt_injected");
    await waitFor(rec.frames, (f) => f.ev === "turn_done", 120_000, "turn_done");

    assert.deepEqual(injected.client_ids, ["steer-1"]);
    assert.ok(["B", "D"].includes(injected.point), `point ${injected.point}`);
    const at = rec.frames.indexOf(injected);
    const lastToolDone = rec.frames.map((f) => f.ev).lastIndexOf("tool_done", at);
    assert.ok(lastToolDone !== -1 && lastToolDone < at, "injection should follow the tool's tool_done");
    const after = rec.frames.slice(at + 1);
    const replyText = after.filter((f) => f.ev === "text_delta").map((f) => f.text).join("");
    assert.match(replyText, /PINEAPPLE/i, "the model's answer to the steer should follow the event");
    console.log(`     point=${injected.point}, ${after.filter((f) => f.ev === "text_delta").length} deltas after`);

    assert.equal(await client.cancelSoftInterrupt(id, "steer-1"), false,
      "a delivered message cannot be cancelled");
  } finally {
    rec.stop();
  }
});

await step("a queued message cancelled by id never reaches the model", async () => {
  const rec = record();
  try {
    await client.sendMessage(id, SLOW_TOOL);
    await waitFor(rec.frames, (f) => f.ev === "tool_exec", 60_000, "the slow tool to start");
    await client.softInterruptWithId(id, "Reply with the word KUMQUAT.", { clientId: "take-back" });
    await client.softInterruptWithId(id, "Reply with the word MANGO.", { clientId: "keep" });
    const receipt = await client.cancelSoftInterruptsById(id, ["take-back", "unknown"]);
    assert.deepEqual(receipt, { cancelled: ["take-back"], notQueued: ["unknown"] });

    await waitFor(rec.frames, (f) => f.ev === "turn_done", 120_000, "turn_done");
    const injected = rec.frames.filter((f) => f.ev === "soft_interrupt_injected");
    assert.deepEqual(injected.flatMap((f) => f.client_ids), ["keep"]);
    const history = await client.getHistory(id);
    const text = JSON.stringify(history);
    assert.ok(!text.includes("KUMQUAT"), "cancelled message must not be in history");
    assert.ok(text.includes("MANGO"), "kept message should be in history");
  } finally {
    rec.stop();
  }
});

await step("background_tool reports whether anything moved", async () => {
  const idle = await client.backgroundToolResult(id);
  assert.deepEqual(idle, { moved: false });

  const rec = record();
  try {
    await client.sendMessage(id, "Run exactly this bash command: `sleep 20 && echo late`. Then say OK.");
    const exec = await waitFor(rec.frames, (f) => f.ev === "tool_exec", 60_000, "the tool to start");
    await new Promise((r) => setTimeout(r, 500));
    const moved = await client.backgroundToolResult(id);
    assert.equal(moved.moved, true, JSON.stringify(moved));
    assert.equal(moved.toolCallId, exec.call_id);
    assert.equal(moved.toolName, "bash");
    await waitFor(rec.frames, (f) => f.ev === "turn_done", 120_000, "turn_done");
  } finally {
    rec.stop();
  }
});

await step("session_status after attach reports the pending count", async () => {
  const other = await JcodeClient.connect({ socketPath: instance.socketPath, requestTimeoutMs: 30_000 });
  try {
    const statuses = [];
    other.on("event", (f) => { if (f.ev === "session_status") statuses.push(f); });
    await other.attachSession(id);
    await new Promise((r) => setTimeout(r, 500));
    const withCount = statuses.find((s) => typeof s.pending_soft_interrupts === "number");
    assert.ok(withCount, `no session_status with a count: ${JSON.stringify(statuses)}`);
    assert.equal(withCount.pending_soft_interrupts, 0);
  } finally {
    other.close();
  }
});

client.close();
await instance.shutdown();
if (failures.length) {
  console.log(`\n${failures.length} failure(s):\n  ${failures.join("\n  ")}`);
  process.exit(1);
}
console.log("\nall soft interrupt id checks passed");
