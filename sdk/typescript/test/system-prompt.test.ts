import { test } from "node:test";
import assert from "node:assert/strict";
import { JcodeClient, type CreateSessionOptions } from "../dist/index.js";
import { startMockHarness } from "./mock-harness.ts";

const cases: { name: string; options?: string | CreateSessionOptions; prompt?: string; workingDir?: string }[] = [
  { name: "provided", options: { workingDir: "/project", systemPrompt: "Review precisely.\nKeep Unicode: λ" }, prompt: "Review precisely.\nKeep Unicode: λ", workingDir: "/project" },
  { name: "empty", options: { systemPrompt: "" }, prompt: "" },
  { name: "omitted", options: { workingDir: "/project" }, workingDir: "/project" },
  { name: "undefined", options: { systemPrompt: undefined } },
  { name: "default options", options: {} },
  { name: "no arguments" },
  { name: "legacy working directory", options: "/legacy", workingDir: "/legacy" },
];

for (const { name, options, prompt, workingDir } of cases) {
  test(`createSession system prompt wire behavior: ${name}`, async (t) => {
    const requests: any[] = [];
    const server = await startMockHarness({
      onRequest(req, send) {
        requests.push(req);
        send({ v: 1, reply_to: req.id, ev: "attached", session: { session_id: "created" } });
      },
    });
    t.after(() => server.close());
    const client = await JcodeClient.connect({ socketPath: server.socketPath });
    t.after(() => client.close());
    assert.equal((await client.createSession(options)).session_id, "created");
    assert.equal(requests.length, 1);
    const wire = requests[0];
    assert.equal(wire.req, "create_session");
    assert.equal(wire.working_dir, workingDir);
    assert.equal(wire.system_prompt, prompt);
    assert.equal(Object.hasOwn(wire, "system_prompt"), prompt !== undefined);
    assert.equal(Object.hasOwn(wire, "systemPrompt"), false);
  });
}
