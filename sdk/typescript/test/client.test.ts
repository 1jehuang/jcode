import { test } from "node:test";
import assert from "node:assert/strict";
import { JcodeClient, HarnessError, NdjsonDecoder } from "../dist/index.js";
import { startMockHarness } from "./mock-harness.ts";
import os from "node:os";
import path from "node:path";
import fs from "node:fs";

test("ndjson decoder reassembles frames split across chunks", () => {
  const decoder = new NdjsonDecoder();
  assert.deepEqual(decoder.push('{"v":1,"ev":"p'), []);
  assert.deepEqual(decoder.push('ong"}\n\n{"v":1,"ev":"ok"}\n'), [
    { v: 1, ev: "pong" },
    { v: 1, ev: "ok" },
  ]);
});

test("handshake records server identity and capabilities", async () => {
  const server = await startMockHarness();
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  assert.equal(client.server, "mock/0.1");
  assert.deepEqual(client.capabilities, ["sessions", "streaming"]);
  client.close();
  await server.close();
});

test("replies are correlated by id even when out of order", async () => {
  const server = await startMockHarness({
    onRequest(request, send) {
      if (request.req === "ping") {
        // Answer late, after the list_sessions that followed it.
        setTimeout(() => send({ v: 1, reply_to: request.id, ev: "pong" }), 30);
      }
      if (request.req === "list_sessions") {
        send({
          v: 1,
          reply_to: request.id,
          ev: "sessions",
          sessions: [{ session_id: "s1", status: "idle" }],
        });
      }
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  const [, sessions] = await Promise.all([client.ping(), client.listSessions()]);
  assert.deepEqual(sessions, [{ session_id: "s1", status: "idle" }]);
  client.close();
  await server.close();
});

test("error frames reject as HarnessError", async () => {
  const server = await startMockHarness({
    onRequest(request, send) {
      send({
        v: 1,
        reply_to: request.id,
        ev: "error",
        code: "unknown_session",
        message: "no such session",
      });
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  await assert.rejects(() => client.attachSession("nope"), (error: unknown) => {
    assert.ok(error instanceof HarnessError);
    assert.equal((error as HarnessError).code, "unknown_session");
    return true;
  });
  client.close();
  await server.close();
});

test("run() collects a full turn and auto-approves permissions", async () => {
  const server = await startMockHarness({
    onRequest(request, send) {
      if (request.req === "send_message") {
        send({ v: 1, reply_to: request.id, ev: "ok" });
        const s = "s1";
        send({ v: 1, ev: "message_accepted", session_id: s });
        send({ v: 1, ev: "reasoning_delta", session_id: s, text: "think" });
        send({
          v: 1,
          ev: "permission_request",
          session_id: s,
          request_id: "p1",
          tool_name: "bash",
          description: "ls",
        });
        send({ v: 1, ev: "text_delta", session_id: s, text: "hello " });
        send({ v: 1, ev: "text_delta", session_id: s, text: "world" });
        send({
          v: 1,
          ev: "tool_done",
          session_id: s,
          call_id: "c1",
          name: "bash",
          output: "ok",
        });
        send({ v: 1, ev: "token_usage", session_id: s, input: 10, output: 4 });
        // A different session must not leak into this turn.
        send({ v: 1, ev: "text_delta", session_id: "other", text: "IGNORE" });
        send({ v: 1, ev: "turn_done", session_id: s });
      }
      if (request.req === "permission_response") {
        send({ v: 1, reply_to: request.id, ev: "ok" });
      }
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  const turn = await client.run("s1", "hi", { autoApprove: true });
  assert.equal(turn.text, "hello world");
  assert.equal(turn.reasoning, "think");
  assert.deepEqual(turn.toolCalls, [
    { callId: "c1", name: "bash", output: "ok", error: undefined },
  ]);
  assert.deepEqual(turn.usage, { input: 10, output: 4, cacheReadInput: undefined });
  client.close();
  await server.close();
});

test("events() buffers while the consumer is busy and filters by session", async () => {
  const server = await startMockHarness();
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  const stream = client.events("s1");
  const collected: string[] = [];
  const consumer = (async () => {
    for await (const event of stream) {
      if (event.ev === "text_delta") {
        collected.push((event as { text: string }).text);
        await new Promise((r) => setTimeout(r, 10));
      }
      if (event.ev === "turn_done") break;
    }
  })();
  for (const text of ["a", "b", "c"]) {
    server.broadcast({ v: 1, ev: "text_delta", session_id: "s1", text });
  }
  server.broadcast({ v: 1, ev: "text_delta", session_id: "s2", text: "x" });
  server.broadcast({ v: 1, ev: "turn_done", session_id: "s1" });
  await consumer;
  assert.deepEqual(collected, ["a", "b", "c"]);
  client.close();
  await server.close();
});

test("unknown event kinds still surface on the generic channel", async () => {
  const server = await startMockHarness();
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  const seen = new Promise<any>((resolve) => client.once("event", resolve));
  server.broadcast({ v: 1, ev: "some_future_event", payload: 1 });
  const frame = await seen;
  assert.equal(frame.ev, "some_future_event");
  client.close();
  await server.close();
});

test("pending requests reject when the connection drops", async () => {
  const server = await startMockHarness();
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  const pending = client.ping();
  await server.close();
  await assert.rejects(() => pending);
});

test("a missing bridge socket explains how to start it", async () => {
  const missing = path.join(os.tmpdir(), `jcode-sdk-absent-${process.pid}.sock`);
  await assert.rejects(
    () => JcodeClient.connect({ socketPath: missing }),
    (error: HarnessError) => {
      assert.equal(error.name, "HarnessError");
      assert.equal(error.code, "connect_failed");
      assert.match(error.message, /jcode-harness-api-bridge/);
      assert.match(error.message, new RegExp(missing.replace(/[/\\]/g, "\\$&")));
      return true;
    },
  );
});

test("a stale socket file reports a dead bridge, not a missing one", async () => {
  // A bridge killed with SIGKILL leaves its socket file behind, so the path
  // exists and dialling gets ECONNREFUSED. "Not found" would send the user
  // looking for a config problem that is not there.
  const stale = path.join(os.tmpdir(), `jcode-sdk-stale-${process.pid}.sock`);
  fs.writeFileSync(stale, "");
  try {
    await assert.rejects(
      () => JcodeClient.connect({ socketPath: stale }),
      (error: HarnessError) => {
        assert.equal(error.code, "connect_failed");
        assert.match(error.message, /stale socket file|not a socket|could not connect/);
        return true;
      },
    );
  } finally {
    fs.rmSync(stale, { force: true });
  }
});
