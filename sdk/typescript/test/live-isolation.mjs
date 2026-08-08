/**
 * Isolation and path-safety checks for a launched instance.
 *
 * `launch()` promises an instance that cannot see the user's work. That is a
 * property of the bridge's session-record lookups, not of the SDK, and it was
 * broken: `peek_session` read `~/.jcode/sessions` directly, so an "isolated"
 * instance returned the real transcripts of the jcode the user runs
 * interactively.
 *
 * Usage: node test/live-isolation.mjs [path-to-jcode-binary]
 */

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { JcodeClient, userJcodeHome } from "../dist/index.js";

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

const client = await JcodeClient.launch({
  binary,
  workingDir: process.cwd(),
  startupTimeoutMs: 60_000,
});

await step("an instance cannot read the user's session transcripts", async () => {
  const dir = path.join(userJcodeHome(), "sessions");
  if (!fs.existsSync(dir)) {
    console.log("     (no user sessions on this machine; skipping)");
    return;
  }
  const records = fs
    .readdirSync(dir)
    .filter((name) => name.startsWith("session_") && name.endsWith(".json"));
  if (records.length === 0) {
    console.log("     (no user sessions on this machine; skipping)");
    return;
  }
  // Pick a record with real content, so an empty result means "blocked"
  // rather than "that session happened to be empty".
  let victim;
  for (const name of records.slice(0, 40)) {
    const raw = JSON.parse(fs.readFileSync(path.join(dir, name), "utf8"));
    const messages = raw.messages ?? [];
    if (messages.some((m) => (m.role === "user" || m.role === "assistant") && m.content)) {
      victim = name.replace(/\.json$/, "");
      break;
    }
  }
  if (!victim) {
    console.log("     (no non-empty user sessions; skipping)");
    return;
  }
  const leaked = await client.peekSession(victim, 5);
  assert.equal(
    leaked.length,
    0,
    `an isolated instance read ${leaked.length} messages from the user's session ${victim}`,
  );
});

await step("a session id cannot escape the sessions directory", async () => {
  // The id is interpolated into a path, so a traversal must be refused rather
  // than read. An empty result is the correct, boring answer.
  for (const hostile of [
    "../../../etc/passwd",
    "../../.ssh/id_rsa",
    "..%2F..%2Fetc%2Fpasswd",
    "/etc/passwd",
  ]) {
    const result = await client.peekSession(hostile, 5);
    assert.equal(result.length, 0, `traversal id ${hostile} returned ${result.length} messages`);
  }
});

await step("the instance can still peek its own sessions", async () => {
  const session = await client.createSession(process.cwd());
  await client.sendMessage(session.session_id, "peek probe");
  // Give the daemon a moment to persist the record.
  await new Promise((resolve) => setTimeout(resolve, 1500));
  const own = await client.peekSession(session.session_id, 5);
  assert.ok(own.length > 0, "an instance must still read its own transcripts");
});

await client.close();

// Confirm the instance is actually gone, rather than trusting that close()
// returned. `process.exit()` below would truncate any cleanup still running,
// so this both checks the property and gives it a moment to settle.
await new Promise((resolve) => setTimeout(resolve, 3000));
if (fs.existsSync(client.instanceHome ?? "")) {
  failures.push(`instance home leaked: ${client.instanceHome}`);
  console.log(`FAIL instance home leaked: ${client.instanceHome}`);
}

console.log(failures.length ? `\nFAILURES:\n${failures.join("\n")}` : "\nisolation ok");
process.exit(failures.length ? 1 : 0);
