import { test } from "node:test";
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import { JcodeClient, type SdkTool } from "../dist/index.js";
import { startMockHarness } from "./mock-harness.ts";

const schema = { type: "object", properties: { value: { type: "string" } }, required: ["value"], additionalProperties: false };
const definition = { name: "lookup", description: "Look up a value", parameters: schema };
const call = (id = "c1", input: unknown = { value: "hello" }, session = "s1", name = "lookup") =>
  ({ v: 1, ev: "tool_call", session_id: session, call_id: id, name, input });

async function fixture(t: any, extra?: (req: any, send: (frame: any) => void) => boolean) {
  const requests: any[] = [];
  const server = await startMockHarness({
    capabilities: ["session_tools"],
    onRequest(req, send) {
      requests.push(req);
      if (extra?.(req, send)) return;
      if (req.req === "create_session") send({ v: 1, reply_to: req.id, ev: "attached", session: { session_id: "s1" } });
      else if (req.req === "list_tools") send({ v: 1, reply_to: req.id, ev: "tools", session_id: "s1", tools: [definition] });
      else send({ v: 1, reply_to: req.id, ev: "ok" });
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  t.after(async () => { await client.close(); await server.close(); });
  const result = async (id = "c1") => {
    for (let i = 0; i < 100; i++) {
      const found = requests.find((r) => r.req === "tool_result" && r.call_id === id);
      if (found) return found;
      await delay(5);
    }
    assert.fail(`no result for ${id}`);
  };
  return { server, client, requests, result };
}

test("createSession configures tools before returning and strips local callbacks from the wire", async (t) => {
  const { client, requests } = await fixture(t);
  const session = await client.createSession({ workingDir: "/project", systemPrompt: "Use only configured tools.", tools: { enabled: [], custom: [{ ...definition, execute: () => "ok" }] } });
  assert.equal(session.session_id, "s1");
  assert.equal(requests[0].working_dir, "/project");
  assert.equal(requests[0].system_prompt, "Use only configured tools.");
  assert.deepEqual(requests[1].tools, { enabled: [], disabled: [], custom: [definition] });
  assert.deepEqual(await client.listTools("s1"), [definition]);
});

test("callbacks execute once and submit output automatically, including immediate post-ack events", async (t) => {
  let executions = 0;
  const { client, result } = await fixture(t, (req, send) => {
    if (req.req !== "configure_tools") return false;
    send({ v: 1, reply_to: req.id, ev: "ok" });
    send(call());
    send(call());
    return true;
  });
  await client.configureTools("s1", { enabled: [], custom: [{ ...definition, execute: async (input, ctx) => {
    executions++;
    assert.equal(ctx.sessionId, "s1");
    assert.equal(ctx.callId, "c1");
    assert.equal(ctx.signal.aborted, false);
    return `found ${input.value}`;
  } }] });
  assert.equal((await result()).output, "found hello");
  assert.equal(executions, 1);
});

test("callbacks validate input and convert throws to tool errors", async (t) => {
  let executions = 0;
  const { client, server, result } = await fixture(t);
  await client.configureTools("s1", { custom: [{ ...definition, execute: () => { executions++; throw new Error("lookup failed"); } }] });
  server.broadcast(call("bad", { value: 5 }));
  assert.match((await result("bad")).error, /Invalid tool input/);
  assert.equal(executions, 0);
  server.broadcast(call("throws"));
  assert.equal((await result("throws")).error, "lookup failed");
  assert.equal(executions, 1);
});

test("callbacks are session scoped and missing registered names fail promptly", async (t) => {
  let executions = 0;
  const { client, server, requests, result } = await fixture(t);
  await client.configureTools("s1", { custom: [{ ...definition, execute: () => { executions++; return "ok"; } }] });
  server.broadcast(call("other", {}, "s2"));
  server.broadcast(call("missing", {}, "s1", "unknown"));
  assert.match((await result("missing")).error, /not registered/);
  assert.equal(executions, 0);
  assert.ok(!requests.some((r) => r.call_id === "other"));
});

test("callback timeout aborts signal and returns an error without waiting for application code", async (t) => {
  let signal: AbortSignal | undefined;
  const { client, server, result } = await fixture(t);
  await client.configureTools("s1", { custom: [{ ...definition, timeoutMs: 10, execute: (_, ctx) => {
    signal = ctx.signal;
    return new Promise(() => {});
  } }] });
  server.broadcast(call());
  assert.match((await result()).error, /timed out/);
  assert.equal(signal?.aborted, true);
});

for (const stop of ["cancel", "close", "turn_done"] as const) {
  test(`${stop} aborts callbacks and suppresses late results`, async (t) => {
    let signal: AbortSignal | undefined;
    let finish: (value: string) => void = () => {};
    const { client, server, requests } = await fixture(t);
    await client.configureTools("s1", { custom: [{ ...definition, execute: (_, ctx) => {
      signal = ctx.signal;
      return new Promise((resolve) => { finish = resolve; });
    } }] });
    server.broadcast(call());
    for (let i = 0; i < 100 && !signal; i++) await delay(5);
    assert.ok(signal);
    if (stop === "cancel") await client.cancel("s1");
    else if (stop === "close") await client.close();
    else server.broadcast({ v: 1, ev: "turn_done", session_id: "s1" });
    await delay(20);
    assert.equal(signal.aborted, true);
    finish("late");
    await delay(20);
    assert.ok(!requests.some((r) => r.req === "tool_result"));
  });
}

test("failed reconfiguration keeps existing callback and schema", async (t) => {
  let configurations = 0;
  const { client, server, result } = await fixture(t, (req, send) => {
    if (req.req !== "configure_tools" || ++configurations === 1) return false;
    send(call("during"));
    send({ v: 1, reply_to: req.id, ev: "error", code: "busy", message: "busy" });
    return true;
  });
  await client.configureTools("s1", { custom: [{ ...definition, execute: () => "old" }] });
  await assert.rejects(client.configureTools("s1", { custom: [{ ...definition, execute: () => "new" }] }), /busy/);
  assert.equal((await result("during")).output, "old");
  server.broadcast(call("after"));
  assert.equal((await result("after")).output, "old");
});

test("invalid tool schemas, duplicate names and deadlines are rejected before a request", async (t) => {
  const { client, requests } = await fixture(t);
  const tool: SdkTool = { ...definition, execute: () => "ok" };
  for (const custom of [[tool, tool], [{ ...tool, timeoutMs: 0 }], [{ ...tool, parameters: { type: "array" } }],
    [{ ...tool, parameters: { type: "object", $async: true } }]]) {
    await assert.rejects(client.configureTools("s1", { custom }));
  }
  assert.equal(requests.length, 0);
});

test("older runtimes fail before creating an unrestricted session", async (t) => {
  const requests: any[] = [];
  const server = await startMockHarness({ onRequest: (req) => requests.push(req) });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  t.after(async () => { await client.close(); await server.close(); });
  await assert.rejects(client.createSession({ tools: { enabled: [] } }), /session_tools/);
  assert.equal(requests.length, 0);
});

test("concurrent configuration is rejected without changing the callback table", async (t) => {
  let release: (() => void) | undefined;
  const { client, requests, server, result } = await fixture(t, (req, send) => {
    if (req.req !== "configure_tools") return false;
    release = () => send({ v: 1, reply_to: req.id, ev: "ok" });
    return true;
  });
  const first = client.configureTools("s1", { custom: [{ ...definition, execute: () => "first" }] });
  for (let i = 0; i < 100 && !release; i++) await delay(5);
  assert.ok(release);
  await assert.rejects(client.configureTools("s1", { custom: [{ ...definition, execute: () => "second" }] }), /already in flight/);
  release();
  await first;
  assert.equal(requests.filter((r) => r.req === "configure_tools").length, 1);
  server.broadcast(call());
  assert.equal((await result()).output, "first");
});

test("ambiguous configuration timeout closes the connection instead of dispatching stale callbacks", async (t) => {
  const server = await startMockHarness({ capabilities: ["session_tools"] });
  const client = await JcodeClient.connect({ socketPath: server.socketPath, requestTimeoutMs: 30 });
  t.after(async () => { await client.close(); await server.close(); });
  let closed = false;
  client.on("close", () => { closed = true; });
  await assert.rejects(client.configureTools("s1", { custom: [{ ...definition, execute: () => "unexpected" }] }), /no reply/);
  for (let i = 0; i < 100 && !closed; i++) await delay(5);
  assert.equal(closed, true);
});

test("switching attachments drops old callbacks before processing subsequent events", async (t) => {
  let executions = 0;
  const { client, server } = await fixture(t, (req, send) => {
    if (req.req !== "attach_session") return false;
    send({ v: 1, reply_to: req.id, ev: "attached", session: { session_id: "s2" } });
    send(call("stale", { value: "old" }, "s1"));
    return true;
  });
  await client.configureTools("s1", { custom: [{ ...definition, execute: () => { executions++; return "old"; } }] });
  await client.attachSession("s2");
  server.broadcast(call("another", { value: "old" }, "s1"));
  await delay(20);
  assert.equal(executions, 0);
});
