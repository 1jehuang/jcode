import { test } from "node:test";
import assert from "node:assert/strict";
import { JcodeClient } from "../dist/index.js";
import { startMockHarness } from "./mock-harness.ts";

type Event = { ev: string; [key: string]: unknown };

async function runFrames(events: Event[]) {
  const server = await startMockHarness({
    onRequest(request, send) {
      if (request.req !== "send_message") return;
      send({ v: 1, ev: "message_accepted", session_id: "s1" });
      for (const event of events) send({ v: 1, session_id: "s1", ...event });
      send({ v: 1, ev: "turn_done", session_id: "s1" });
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  try {
    return await client.run("s1", "investigate");
  } finally {
    await client.close();
    await server.close();
  }
}

test("run separates narration and final answer without splitting interleaved reasoning", async () => {
  const turn = await runFrames([
    { ev: "text_delta", message_id: "m1", text: "Checking the logs." },
    { ev: "text_done", message_id: "m1" },
    { ev: "tool_start", call_id: "c1", name: "read" },
    { ev: "tool_done", call_id: "c1", name: "read", output: "logs" },
    { ev: "text_delta", message_id: "m2", text: "The root cause is " },
    { ev: "reasoning_delta", text: "verify this" },
    { ev: "reasoning_done" },
    { ev: "text_delta", message_id: "m2", text: "the retry loop." },
    { ev: "text_done", message_id: "m2" },
  ]);
  assert.equal(turn.text, "Checking the logs.The root cause is the retry loop.");
  assert.equal(turn.finalText, "The root cause is the retry loop.");
  assert.equal(turn.reasoning, "verify this");
  assert.deepEqual(turn.messages, [
    { messageId: "m1", text: "Checking the logs." },
    { messageId: "m2", text: "The root cause is the retry loop." },
  ]);
});

test("message ids correlate interleaved chunks and out-of-order completion", async () => {
  const turn = await runFrames([
    { ev: "text_delta", message_id: "a", text: "First " },
    { ev: "text_delta", message_id: "b", text: "Second." },
    { ev: "text_delta", message_id: "a", text: "message." },
    { ev: "text_done", message_id: "b" },
    { ev: "text_done", message_id: "a" },
    { ev: "text_done", message_id: "a" },
  ]);
  assert.deepEqual(turn.messages, [
    { messageId: "a", text: "First message." },
    { messageId: "b", text: "Second." },
  ]);
  assert.equal(turn.finalText, "Second.");
});

test("text_done supports sequential messages without optional ids", async () => {
  const turn = await runFrames([
    { ev: "text_delta", text: "First." },
    { ev: "text_done" },
    { ev: "text_delta", text: "Last." },
    { ev: "text_done" },
  ]);
  assert.deepEqual(turn.messages, [{ text: "First." }, { text: "Last." }]);
  assert.equal(turn.finalText, "Last.");
});

test("older bridges retain whole-turn text without inventing boundaries", async () => {
  const turn = await runFrames([
    { ev: "text_delta", text: "The root cause is " },
    { ev: "reasoning_delta", text: "thinking" },
    { ev: "text_delta", text: "the retry loop." },
  ]);
  assert.equal(turn.finalText, "The root cause is the retry loop.");
  assert.equal(turn.text, turn.finalText);
  assert.deepEqual(turn.messages, []);
});

test("text corrections remove discarded output, including already completed messages", async () => {
  const turn = await runFrames([
    { ev: "text_delta", message_id: "old", text: "Discarded." },
    { ev: "text_done", message_id: "old" },
    { ev: "text_replace", message_id: "old", text: "" },
    { ev: "text_delta", message_id: "new", text: "Answer with leaked tool wrapper" },
    { ev: "text_replace", message_id: "new", text: "Answer." },
    { ev: "text_done", message_id: "new" },
  ]);
  assert.equal(turn.text, "Answer.");
  assert.equal(turn.finalText, "Answer.");
  assert.deepEqual(turn.messages, [{ messageId: "new", text: "Answer." }]);
});

test("reasoning-only turns have no phantom text messages", async () => {
  const turn = await runFrames([
    { ev: "reasoning_delta", text: "thinking" },
    { ev: "reasoning_done" },
    { ev: "text_done" },
  ]);
  assert.equal(turn.text, "");
  assert.equal(turn.finalText, "");
  assert.deepEqual(turn.messages, []);
});

test("runStructured validates the final framed message rather than narration", async () => {
  const server = await startMockHarness({
    onRequest(request, send) {
      if (request.req !== "send_message") return;
      send({ v: 1, ev: "message_accepted", session_id: "s1" });
      send({ v: 1, ev: "text_delta", session_id: "s1", message_id: "narration", text: "Let me check." });
      send({ v: 1, ev: "text_done", session_id: "s1", message_id: "narration" });
      send({ v: 1, ev: "text_delta", session_id: "s1", message_id: "answer", text: '{"ok":true}' });
      send({ v: 1, ev: "text_done", session_id: "s1", message_id: "answer" });
      send({ v: 1, ev: "turn_done", session_id: "s1" });
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  try {
    const result = await client.runStructured("s1", "Check", {
      schema: { type: "object", properties: { ok: { type: "boolean" } }, required: ["ok"] },
      maxRetries: 0,
    });
    assert.deepEqual(result.data, { ok: true });
    assert.equal(result.text, 'Let me check.{"ok":true}');
    assert.equal(result.attempts[0].text, '{"ok":true}');
  } finally {
    await client.close();
    await server.close();
  }
});
