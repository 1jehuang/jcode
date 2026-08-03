import { test } from "node:test";
import assert from "node:assert/strict";
import {
  JcodeClient,
  HarnessError,
  NdjsonDecoder,
  unixSocketTransport,
} from "../dist/index.js";
import { startMockHarness } from "./mock-harness.ts";
import os from "node:os";
import path from "node:path";
import fs from "node:fs";

async function waitFor(predicate: () => boolean, timeoutMs = 1_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() >= deadline) throw new Error("condition was not met before timeout");
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

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

test("sendMessage supports context-only options and waits for request completion", async () => {
  let received: any;
  const server = await startMockHarness({
    onRequest(request, send) {
      if (request.req === "send_message") {
        received = request;
        send({ v: 1, reply_to: request.id, ev: "ok" });
      }
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  await client.sendMessage("s1", "context", {
    noReply: true,
    images: [["image/png", "abc"]],
  });
  assert.equal(received.no_reply, true);
  assert.deepEqual(received.images, [["image/png", "abc"]]);
  client.close();
  await server.close();
});

test("sendMessage retains the legacy images argument", async () => {
  let received: any;
  const server = await startMockHarness({
    onRequest(request, send) {
      if (request.req === "send_message") {
        received = request;
        send({ v: 1, ev: "message_accepted", session_id: request.session_id });
      }
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  await client.sendMessage("s1", "normal", [["image/jpeg", "xyz"]]);
  assert.deepEqual(received.images, [["image/jpeg", "xyz"]]);
  assert.equal(received.no_reply, undefined);
  client.close();
  await server.close();
});

test("sendMessage noReply waits for request ok and does not wait for turn events", async () => {
  let observed: any;
  const server = await startMockHarness({
    onRequest(request, send) {
      if (request.req === "send_message") {
        observed = request;
        send({ v: 1, reply_to: request.id, ev: "ok" });
      }
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  const seenTurn = new Promise<boolean>((resolve) => {
    const timer = setTimeout(() => resolve(false), 20);
    timer.unref?.();
    client.once("turn_done", () => {
      clearTimeout(timer);
      resolve(true);
    });
  });

  await client.sendMessage("s1", "context only", { noReply: true });

  assert.equal(observed.req, "send_message");
  assert.equal(observed.session_id, "s1");
  assert.equal(observed.content, "context only");
  assert.equal(observed.no_reply, true);
  assert.equal(await seenTurn, false);
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

test("events() return settles a pending next call", async () => {
  const server = await startMockHarness();
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  const stream = client.events("s1");
  const pending = stream.next();
  await stream.return();
  assert.deepEqual(await pending, { value: undefined, done: true });
  await client.close();
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
      assert.match(error.message, /jcode api-bridge/);
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

test("GA methods send stable request shapes and map typed replies", async () => {
  const requests: any[] = [];
  const server = await startMockHarness({
    onRequest(request, send) {
      requests.push(request);
      const reply = (frame: any) => send({ v: 1, reply_to: request.id, ...frame });
      switch (request.req) {
        case "list_sessions":
          reply({
            ev: "sessions",
            sessions: [{ session_id: "s1", status: "idle", archived: true }],
          });
          break;
        case "archive_session":
        case "restore_session":
        case "set_retention_policy":
          reply({ ev: "ok" });
          break;
        case "ping":
          reply({ ev: "pong" });
          break;
        case "get_runtime_info":
          reply({
            ev: "runtime_info",
            session_id: "s1",
            provider: "anthropic",
            model: "claude",
            routes: [
              {
                model: "claude",
                provider: "anthropic",
                api_method: "messages",
                available: true,
                detail: "ready",
              },
              {
                model: "claude-fast",
                provider: "anthropic",
                api_method: "messages",
                available: true,
                detail: "ready",
              },
            ],
          });
          break;
        case "set_api_key":
          reply({ ev: "credential_updated", provider: "gemini", configured: true });
          break;
        case "clear_api_key":
          reply({ ev: "credential_updated", provider: "jcode", configured: false });
          break;
        case "read_file":
          reply({
            ev: "file_content",
            session_id: "s1",
            path: "src/a.ts",
            content: "hello",
            size: 8,
            truncated: true,
          });
          break;
        case "find_files":
          reply({ ev: "files", session_id: "s1", paths: ["src/a.ts"] });
          break;
        case "search_text":
          reply({
            ev: "text_matches",
            session_id: "s1",
            matches: [{ path: "src/a.ts", line: 2, column: 3, preview: "  hello" }],
          });
          break;
        case "file_status":
          reply({
            ev: "file_status",
            session_id: "s1",
            path: "src/a.ts",
            exists: true,
            kind: "file",
            size: 8,
            modified_ms: 123,
          });
          break;
      }
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  try {
    assert.deepEqual(await client.listSessions({ includeArchived: true }), [
      { session_id: "s1", status: "idle", archived: true },
    ]);
    await client.archiveSession("s1");
    await client.restoreSession("s1");
    await client.setRetentionPolicy(30);

    const runtime = await client.getRuntimeInfo("s1");
    assert.equal(runtime.healthy, true);
    assert.deepEqual(runtime.providers, ["anthropic"]);
    assert.equal(runtime.routes.length, 2);

    await client.setApiKey("gemini-api", "secret");
    await client.clearApiKey("jcode");
    assert.deepEqual(await client.readFile("s1", "src/a.ts", 5), {
      path: "src/a.ts",
      content: "hello",
      size: 8,
      truncated: true,
    });
    assert.deepEqual(await client.findFiles("s1", "a.ts", 4), ["src/a.ts"]);
    assert.deepEqual(await client.searchText("s1", "hello", { path: "src", limit: 2 }), [
      { path: "src/a.ts", line: 2, column: 3, preview: "  hello" },
    ]);
    assert.deepEqual(await client.fileStatus("s1", "src/a.ts"), {
      path: "src/a.ts",
      exists: true,
      kind: "file",
      size: 8,
      modifiedMs: 123,
    });

    const byKind = (kind: string) => requests.find((request) => request.req === kind);
    assert.equal(byKind("list_sessions").include_archived, true);
    assert.equal(byKind("archive_session").session_id, "s1");
    assert.equal(byKind("set_retention_policy").archive_after_days, 30);
    assert.deepEqual(
      {
        provider: byKind("set_api_key").provider,
        api_key: byKind("set_api_key").api_key,
      },
      { provider: "gemini-api", api_key: "secret" },
    );
    assert.equal(byKind("read_file").max_bytes, 5);
    assert.deepEqual(
      { path: byKind("search_text").path, limit: byKind("search_text").limit },
      { path: "src", limit: 2 },
    );
  } finally {
    await client.close();
    await server.close();
  }
});

test("globalEvents discovers persisted and newly-created sessions and cleans up children", async () => {
  const sessions = ["persisted-1", "persisted-2"];
  const listRequests: any[] = [];
  const server = await startMockHarness({
    onRequest(request, send) {
      if (request.req === "list_sessions") {
        listRequests.push(request);
        send({
          v: 1,
          reply_to: request.id,
          ev: "sessions",
          sessions: sessions.map((session_id) => ({ session_id, status: "idle" })),
        });
      } else if (request.req === "attach_session") {
        send({
          v: 1,
          reply_to: request.id,
          ev: "attached",
          session: { session_id: request.session_id, status: "attached" },
        });
        queueMicrotask(() =>
          send({
            v: 1,
            ev: "text_delta",
            session_id: request.session_id,
            text: request.session_id,
          }),
        );
      }
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  const stream = client.globalEvents({ discoveryIntervalMs: 10 });
  try {
    const first = await stream.next();
    const second = await stream.next();
    assert.deepEqual(
      new Set([first.value.session_id, second.value.session_id]),
      new Set(["persisted-1", "persisted-2"]),
    );
    assert.equal(listRequests[0].include_archived, true);

    sessions.push("new-3");
    const third = await stream.next();
    assert.equal(third.value.session_id, "new-3");
    await waitFor(() => server.clientCount() === 4);

    await stream.return();
    await waitFor(() => server.clientCount() === 1);
  } finally {
    await stream.return();
    await client.close();
    await server.close();
  }
});

test("globalEvents aborts a pending consumer and closes every child", async () => {
  const server = await startMockHarness({
    onRequest(request, send) {
      if (request.req === "list_sessions") {
        send({
          v: 1,
          reply_to: request.id,
          ev: "sessions",
          sessions: [{ session_id: "s1", status: "idle" }],
        });
      } else if (request.req === "attach_session") {
        send({
          v: 1,
          reply_to: request.id,
          ev: "attached",
          session: { session_id: "s1", status: "attached" },
        });
      }
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  const abort = new AbortController();
  const stream = client.globalEvents({ signal: abort.signal, discoveryIntervalMs: 0 });
  try {
    await waitFor(() => server.clientCount() === 2);
    const pending = stream.next();
    abort.abort();
    assert.deepEqual(await pending, { value: undefined, done: true });
    await waitFor(() => server.clientCount() === 1);
  } finally {
    await stream.return();
    await client.close();
    await server.close();
  }
});

test("globalEvents fails loudly when its bounded event queue overflows", async () => {
  const server = await startMockHarness({
    onRequest(request, send) {
      if (request.req === "list_sessions") {
        send({
          v: 1,
          reply_to: request.id,
          ev: "sessions",
          sessions: [{ session_id: "s1", status: "idle" }],
        });
      } else if (request.req === "attach_session") {
        send({
          v: 1,
          reply_to: request.id,
          ev: "attached",
          session: { session_id: "s1", status: "attached" },
        });
        setTimeout(() => {
          for (const text of ["one", "two"]) {
            send({ v: 1, ev: "text_delta", session_id: "s1", text });
          }
        }, 10);
      }
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  const stream = client.globalEvents({ discoveryIntervalMs: 0, maxBufferedEvents: 1 });
  try {
    await new Promise((resolve) => setTimeout(resolve, 30));
    await assert.rejects(
      () => stream.next(),
      (error: HarnessError) => error.code === "event_buffer_overflow",
    );
  } finally {
    await stream.return();
    await client.close();
    await server.close();
  }
});

test("globalEvents explicitly rejects custom transports", async () => {
  const server = await startMockHarness();
  const transport = await unixSocketTransport(server.socketPath);
  const client = await JcodeClient.connect({ transport });
  try {
    assert.throws(
      () => client.globalEvents(),
      (error: HarnessError) => error.code === "unsupported_transport",
    );
  } finally {
    await client.close();
    await server.close();
  }
});
