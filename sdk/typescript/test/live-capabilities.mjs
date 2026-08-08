/**
 * Live coverage for the seven capabilities added to close the API gaps.
 *
 * A mock server proves the SDK sends the right frames; it cannot prove the
 * daemon answers them. Every one of these crosses a distinct translation path,
 * and three of them (set_model, set_reasoning_effort, compact) report failure
 * in-band on a success-shaped event, which is exactly the sort of thing that
 * looks fine in a unit test and silently no-ops in production.
 */

import { JcodeClient, HarnessError } from "../dist/index.js";

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
  clientName: "sdk-capabilities-e2e/0.1",
  requestTimeoutMs: 60_000,
});
client.on("harness_error", (frame) => console.log(`  (harness_error: ${frame.message})`));

const session = await client.createSession(process.cwd());
const id = session.session_id;
console.log(`session ${id}`);

let firstModel;

await step("listModels returns a catalog with a current model", async () => {
  const { models, current } = await client.listModels(id);
  if (!Array.isArray(models) || models.length === 0) {
    throw new Error(`expected a non-empty catalog, got ${JSON.stringify(models)}`);
  }
  if (!current) throw new Error("expected a current model");
  if (!models.includes(current)) {
    throw new Error(`current ${current} is not in the catalog`);
  }
  firstModel = models.find((m) => m !== current) ?? current;
  console.log(`     ${models.length} models, current=${current}`);
});

await step("setModel switches the session and broadcasts model_info", async () => {
  const seen = new Promise((resolve) => {
    const onEvent = (frame) => {
      if (frame.ev === "model_info" && frame.model === firstModel) {
        client.off("event", onEvent);
        resolve(frame);
      }
    };
    client.on("event", onEvent);
    setTimeout(() => {
      client.off("event", onEvent);
      resolve(undefined);
    }, 15_000);
  });
  await client.setModel(id, firstModel);
  const broadcast = await seen;
  if (!broadcast) throw new Error("no model_info broadcast followed the switch");
  const { current } = await client.listModels(id);
  if (current !== firstModel) {
    throw new Error(`catalog still reports ${current}, expected ${firstModel}`);
  }
  console.log(`     switched to ${firstModel}`);
});

await step("setModel rejects an unknown model instead of silently ignoring it", async () => {
  try {
    await client.setModel(id, "definitely-not-a-real-model-xyz");
  } catch (error) {
    if (!(error instanceof HarnessError)) throw new Error(`wrong type: ${error}`);
    console.log(`     rejected: ${error.code}`);
    return;
  }
  // A daemon that accepts anything is a finding, not a pass: the client would
  // believe it switched.
  const { current } = await client.listModels(id);
  throw new Error(`accepted a bogus model; catalog now reports ${current}`);
});

await step("setReasoningEffort is accepted or reports why not", async () => {
  try {
    await client.setReasoningEffort(id, "high");
    console.log("     effort=high accepted");
  } catch (error) {
    if (!(error instanceof HarnessError)) throw error;
    console.log(`     provider declined: ${error.message.slice(0, 70)}`);
  }
});

await step("renameSession sets a title and broadcasts session_renamed", async () => {
  const seen = new Promise((resolve) => {
    const onEvent = (frame) => {
      if (frame.ev === "session_renamed") {
        client.off("event", onEvent);
        resolve(frame);
      }
    };
    client.on("event", onEvent);
    setTimeout(() => {
      client.off("event", onEvent);
      resolve(undefined);
    }, 10_000);
  });
  await client.renameSession(id, "sdk gap coverage");
  const renamed = await seen;
  if (!renamed) throw new Error("no session_renamed event");
  if (renamed.display_title !== "sdk gap coverage") {
    throw new Error(`display_title was ${renamed.display_title}`);
  }
});

await step("cancelSoftInterrupts is accepted", async () => {
  await client.softInterrupt(id, "queued follow-up that should be retracted");
  await client.cancelSoftInterrupts(id);
});

await step("rewind then rewindUndo restores the transcript", async () => {
  await client.run(id, "Reply with exactly: ONE");
  await client.run(id, "Reply with exactly: TWO");
  const before = await client.getHistory(id);
  if (before.length < 4) throw new Error(`expected 4+ messages, got ${before.length}`);

  await client.rewind(id, 2);
  const trimmed = await client.getHistory(id);
  if (trimmed.length >= before.length) {
    throw new Error(`rewind did not trim: ${before.length} -> ${trimmed.length}`);
  }

  await client.rewindUndo(id);
  const restored = await client.getHistory(id);
  if (restored.length !== before.length) {
    throw new Error(
      `undo did not restore: ${before.length} -> ${trimmed.length} -> ${restored.length}`,
    );
  }
  console.log(`     ${before.length} -> ${trimmed.length} -> ${restored.length}`);
});

await step("compact is scheduled, or explains why it was refused", async () => {
  try {
    const message = await client.compact(id);
    console.log(`     accepted: ${message.slice(0, 70)}`);
  } catch (error) {
    if (!(error instanceof HarnessError)) throw error;
    if (error.code !== "invalid_request") throw error;
    console.log(`     refused: ${error.message.slice(0, 70)}`);
  }
});

await step("these requests need an attached session", async () => {
  const other = await JcodeClient.connect({
    clientName: "sdk-capabilities-unattached/0.1",
    requestTimeoutMs: 20_000,
  });
  try {
    await other.setModel(id, firstModel);
    throw new Error("expected a rejection while unattached");
  } catch (error) {
    if (!(error instanceof HarnessError)) throw error;
    if (error.code !== "unknown_session") {
      throw new Error(`expected unknown_session, got ${error.code}`);
    }
  } finally {
    other.close();
  }
});

client.close();
console.log(failures.length ? `\nFAILURES:\n${failures.join("\n")}` : "\ncapabilities ok");
process.exit(failures.length ? 1 : 0);
