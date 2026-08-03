import { test } from "node:test";
import assert from "node:assert/strict";
import { JcodeClient, StructuredOutputError } from "../dist/index.js";
import { startMockHarness } from "./mock-harness.ts";

const schema = {
  type: "object",
  additionalProperties: false,
  required: ["summary", "count"],
  properties: {
    summary: { type: "string" },
    count: { type: "integer", minimum: 0 },
  },
} as const;

type Summary = { summary: string; count: number };

function sendTurn(send: (frame: any) => void, sessionId: string, text: string): void {
  send({ v: 1, ev: "message_accepted", session_id: sessionId });
  send({ v: 1, ev: "text_delta", session_id: sessionId, text });
  send({ v: 1, ev: "turn_done", session_id: sessionId });
}

test("runStructured validates JSON Schema and returns parsed data", async () => {
  const prompts: string[] = [];
  const server = await startMockHarness({
    onRequest(request, send) {
      if (request.req !== "send_message") return;
      prompts.push(request.content);
      sendTurn(send, "s1", '```json\n{"summary":"done","count":2}\n```');
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });

  const result = await client.runStructured<Summary>("s1", "Summarize the work", { schema });

  assert.deepEqual(result.data, { summary: "done", count: 2 });
  assert.equal(result.text, '```json\n{"summary":"done","count":2}\n```');
  assert.equal(result.attempts.length, 1);
  assert.deepEqual(result.attempts[0].errors, []);
  assert.match(prompts[0], /Return the answer as JSON only/);
  assert.match(prompts[0], /"additionalProperties": false/);

  await client.close();
  await server.close();
});

test("runStructured sends a corrective retry after schema validation fails", async () => {
  const prompts: string[] = [];
  const responses = ['{"summary":42,"count":2}', '{"summary":"fixed","count":2}'];
  const server = await startMockHarness({
    onRequest(request, send) {
      if (request.req !== "send_message") return;
      prompts.push(request.content);
      sendTurn(send, "s1", responses[prompts.length - 1]);
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });

  const result = await client.runStructured<Summary>("s1", "Return a summary", {
    schema,
    maxRetries: 1,
  });

  assert.deepEqual(result.data, { summary: "fixed", count: 2 });
  assert.equal(prompts.length, 2);
  assert.match(prompts[1], /Validation errors:/);
  assert.match(prompts[1], /summary/);
  assert.match(prompts[1], /must be string/);
  assert.match(prompts[1], /"summary":42/);
  assert.equal(result.attempts.length, 2);
  assert.equal(result.attempts[0].errors[0].keyword, "type");
  assert.deepEqual(result.attempts[1].errors, []);

  await client.close();
  await server.close();
});

test("runStructured rejects with validation details after bounded retries are exhausted", async () => {
  const prompts: string[] = [];
  const server = await startMockHarness({
    onRequest(request, send) {
      if (request.req !== "send_message") return;
      prompts.push(request.content);
      sendTurn(send, "s1", "not json");
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });

  await assert.rejects(
    () => client.runStructured<Summary>("s1", "Return a summary", { schema, maxRetries: 1 }),
    (error: unknown) => {
      assert.ok(error instanceof StructuredOutputError);
      assert.equal(error.code, "structured_output_invalid");
      assert.equal(error.attempts.length, 2);
      assert.equal(error.validationErrors[0].keyword, "parse");
      assert.match(error.message, /after 2 attempts/);
      return true;
    },
  );
  assert.equal(prompts.length, 2);

  await client.close();
  await server.close();
});
