/**
 * Opt-in acceptance check using the real SDK, private daemon, bridge, provider,
 * and bash tool. Requires a working provider login. No mocks or shared daemon.
 *
 * Usage: node sdk/typescript/test/live-text-framing.mjs ./target/selfdev/jcode
 * Optional: JCODE_SDK_TEST_MODEL (defaults to the daemon's chosen route).
 */
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { JcodeClient } from "../dist/index.js";

const binary = path.resolve(process.argv[2] ?? "target/selfdev/jcode");
const workingDir = fs.mkdtempSync(path.join(process.env.JCODE_SCRATCH_DIR ?? os.tmpdir(), "guy-live-"));
const client = await JcodeClient.launch({
  binary,
  workingDir,
  startupTimeoutMs: 60_000,
  inheritStderr: true,
  wakeMode: "external",
  env: {
    JCODE_NO_TELEMETRY: "1",
    JCODE_MEMORY_ENABLED: "0",
    JCODE_MEMORY_SIDECAR_ENABLED: "0",
    JCODE_ACTIVE_PROVIDER: "",
  },
});
const home = client.instanceHome;
console.log(`Private instance launched with ${binary}`);
client.on("harness_error", (event) => console.error("Harness error:", event.code, event.message));
const deadline = setTimeout(() => {
  console.error("Live acceptance exceeded 180 seconds");
  void client.close().finally(() => process.exit(1));
}, 180_000);
try {
  assert.ok(client.capabilities.includes("text_framing"), "new bridge must advertise text framing");
  assert.equal((await client.listSessions()).length, 0, "instance must be isolated");
  const session = await client.createSession(workingDir);
  if (process.env.JCODE_SDK_TEST_MODEL) {
    await client.setModel(session.session_id, process.env.JCODE_SDK_TEST_MODEL);
  }
  console.log(`Session created, model ${process.env.JCODE_SDK_TEST_MODEL ?? "default"}`);
  const probes = [];
  const metrics = [];
  let terminalAt;
  let sawTool = false;
  let toolFinishedAt;
  const probe = (phase) => {
    const start = performance.now();
    const promise = client.getHistory(session.session_id).then((history) => {
      const end = performance.now();
      const row = { phase, durationMs: Math.round(end - start), completedAt: end, messages: history.length };
      metrics.push(row);
      console.log(`history ${phase}: ${row.durationMs}ms, ${row.messages} messages`);
      return row;
    });
    // Attach a handler immediately while still surfacing the error via all().
    promise.catch(() => {});
    probes.push(promise);
  };
  // Repeated fresh-session reads also race model-catalog prefetch, which can
  // briefly own the agent lock before the first transcript has been saved.
  for (let batch = 0; batch < 5; batch++) {
    for (let i = 0; i < 3; i++) probe("idle");
    await Promise.all(probes);
  }
  const turnStarted = performance.now();
  const events = [];
  const turn = await client.run(session.session_id,
    "This is an isolated SDK regression check. First emit exactly CHECKING_HISTORY as an assistant commentary message. Then call bash once with command `sleep 3; printf 'TOOL_OK\\n'`, run_in_background=false, accept_large_output=false, notify=false, wake=false, timeout=10000, and intent='Verify SDK history'. Boolean arguments must be false, never null. Do not use batch. Wait for the tool result. Finally reply exactly FRAMED_OK. Do not edit files or call other tools.",
    {
      autoApprove: true,
      onEvent(event) {
        events.push(event.ev);
        if (["message_accepted", "tool_exec", "tool_done", "text_done", "turn_done"].includes(event.ev)) {
          console.log(`event ${event.ev}`);
        }
        if (event.ev === "message_accepted") {
          for (let i = 0; i < 3; i++) probe("message-accepted");
        }
        if (event.ev === "tool_exec") {
          sawTool = true;
          for (let i = 0; i < 5; i++) probe("tool-running");
        }
        if (event.ev === "tool_done") toolFinishedAt = performance.now();
        if (event.ev === "turn_done") terminalAt = performance.now();
      },
    });
  await Promise.all(probes);
  assert.ok(sawTool, "real provider must execute the requested bash tool");
  assert.ok(turn.toolCalls.some((call) => call.name === "bash" && call.output.includes("TOOL_OK") && !call.error), "real bash result missing");
  assert.ok(turn.messages.length >= 2, "narration and final answer must be separate framed messages");
  assert.ok(turn.messages[0].text.includes("CHECKING_HISTORY"), "narration missing from first message");
  assert.equal(turn.finalText.trim(), "FRAMED_OK", "final answer must exclude narration");
  assert.ok(turn.text.includes("CHECKING_HISTORY") && turn.text.includes("FRAMED_OK"), "legacy aggregate text must retain both messages");
  assert.ok(turn.messages.every((message) => message.messageId), "all live messages need correlation ids");
  assert.equal(new Set(turn.messages.map((message) => message.messageId)).size, turn.messages.length);
  assert.ok(metrics.filter((row) => row.phase === "tool-running").length >= 5);
  for (const row of metrics) {
    assert.ok(row.durationMs < 1500, `${row.phase} history stalled for ${row.durationMs}ms`);
    if (row.phase === "tool-running") {
      assert.ok(row.completedAt < toolFinishedAt, "history must return while the real tool still holds the turn lock");
      assert.ok(row.completedAt < terminalAt, "history must arrive before turn_done");
    }
  }
  const history = await client.getHistory(session.session_id);
  assert.ok(history.some((message) => JSON.stringify(message).includes("FRAMED_OK")), "final answer missing from persisted history");
  console.log(JSON.stringify({
    accepted: true,
    binary,
    turnDurationMs: Math.round(terminalAt - turnStarted),
    framedMessages: turn.messages,
    finalText: turn.finalText,
    historyProbes: metrics.map(({ completedAt, ...row }) => row),
    events: [...new Set(events)],
    finalHistoryMessages: history.length,
  }, null, 2));
} catch (error) {
  console.error("Acceptance failure before cleanup:", error);
  const logs = path.join(home, "logs");
  for (const name of fs.existsSync(logs) ? fs.readdirSync(logs) : []) {
    if (!name.endsWith(".log")) continue;
    const lines = fs.readFileSync(path.join(logs, name), "utf8").split("\n");
    console.error(`Private daemon ${name} tail:\n${lines.slice(-60).join("\n")}`);
  }
  throw error;
} finally {
  clearTimeout(deadline);
  await client.close();
  assert.ok(!fs.existsSync(home), "private instance must be cleaned up");
  fs.rmSync(workingDir, { recursive: true, force: true });
}
