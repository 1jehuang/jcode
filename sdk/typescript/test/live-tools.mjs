/** Opt-in real daemon/provider test: JCODE_SDK_TEST_BINARY=/absolute/jcode node test/live-tools.mjs */
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import os from "node:os";
import { JcodeClient } from "../dist/index.js";

const binary = process.env.JCODE_SDK_TEST_BINARY;
assert.ok(binary, "Set JCODE_SDK_TEST_BINARY to the freshly built jcode binary");
const root = await fs.mkdtemp(path.join(process.env.JCODE_SCRATCH_DIR ?? os.tmpdir(), "sdk-tools-live-"));
const client = await JcodeClient.launch({
  binary, jcodeHome: path.join(root, "home"), workingDir: root,
  startupTimeoutMs: 60_000, requestTimeoutMs: 30_000,
  env: { JCODE_DISABLE_TELEMETRY: "1" },
});
const deadline = setTimeout(() => { console.error("live tool test timed out"); void client.close(); }, 180_000);
try {
  assert.ok(client.supports("session_tools"));
  let calls = 0;
  const token = `SDK_TOOL_${Date.now()}`;
  const session = await client.createSession({ workingDir: root, tools: {
    enabled: [],
    custom: [{ name: "sdk_lookup", description: "Return the requested test token",
      parameters: { type: "object", properties: { key: { type: "string" } }, required: ["key"], additionalProperties: false },
      execute(input, context) {
        assert.equal(input.key, "test");
        assert.equal(context.sessionId, session.session_id);
        calls++;
        return token;
      },
    }],
  } });
  const id = session.session_id;
  assert.deepEqual((await client.listTools(id)).map((t) => t.name), ["sdk_lookup"]);
  const turn = await client.run(id, 'Call sdk_lookup with key "test" exactly once. Then reply with its exact output. Do not invent the output.');
  assert.equal(calls, 1);
  assert.ok(turn.text.includes(token), turn.text);
  assert.ok(turn.toolCalls.some((t) => t.name === "sdk_lookup" && t.output.includes(token)));
  console.log("custom callback round trip passed");

  await client.configureTools(id, { enabled: [] });
  assert.deepEqual(await client.listTools(id), []);
  await client.configureTools(id, { enabled: ["read", "bash"], disabled: ["bash"] });
  assert.deepEqual((await client.listTools(id)).map((t) => t.name), ["read"]);

  const replacement = `${token}_OVERRIDE`;
  let replacements = 0;
  await client.configureTools(id, { enabled: [], custom: [{ name: "read", description: "Read through the SDK virtual filesystem",
    parameters: { type: "object", properties: { file_path: { type: "string" } }, required: ["file_path"], additionalProperties: false },
    execute(input) { assert.equal(input.file_path, "virtual.txt"); replacements++; return replacement; },
  }] });
  const second = await client.run(id, 'Call read with file_path "virtual.txt" exactly once and reply with its exact output.');
  assert.equal(replacements, 1);
  assert.ok(second.text.includes(replacement), second.text);
  console.log("built-in replacement round trip passed");

  await client.configureTools(id, { disabled: ["read"], custom: [{ name: "read", description: "Disabled override",
    parameters: { type: "object", properties: {} }, execute() { assert.fail("disabled tool executed"); },
  }], enabled: [] });
  assert.deepEqual(await client.listTools(id), []);
  await client.configureTools(id, {});
  assert.ok((await client.listTools(id)).some((t) => t.name === "bash"));
  console.log("empty allow-list, deny precedence, and defaults restoration passed");
} finally {
  clearTimeout(deadline);
  await client.close();
  await fs.rm(root, { recursive: true, force: true });
}
