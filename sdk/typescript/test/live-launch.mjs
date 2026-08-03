/**
 * End-to-end check for `JcodeClient.launch()`: a private instance.
 *
 * The isolation claim is the whole point of `launch()`, and it is exactly the
 * kind of claim that is easy to assert and hard to actually hold. So this
 * drives a real instance and checks the properties that matter: it starts, it
 * cannot see the user's sessions, it inherits credentials well enough to reach
 * a model, and it leaves nothing behind.
 *
 * Usage: node test/live-launch.mjs [path-to-jcode-binary]
 */

import assert from "node:assert/strict";
import fs from "node:fs";
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

console.log(`launching a private instance with ${binary}`);
const started = Date.now();
const client = await JcodeClient.launch({
  binary,
  workingDir: process.cwd(),
  startupTimeoutMs: 60_000,
});
const home = client.instanceHome;
console.log(`launched in ${Date.now() - started}ms, home ${home}`);

await step("a fresh instance starts with no sessions", async () => {
  const sessions = await client.listSessions();
  assert.equal(sessions.length, 0, `expected an empty instance, saw ${sessions.length}`);
});

await step("the instance home is separate from the user's", () => {
  assert.notEqual(home, userJcodeHome());
  assert.ok(fs.existsSync(home), "instance home should exist while running");
});

await step("inherited credentials are owner-only", () => {
  const auth = path.join(home, "auth.json");
  if (!fs.existsSync(auth)) {
    console.log("     (no user credentials to inherit; skipping)");
    return;
  }
  const mode = fs.statSync(auth).mode & 0o777;
  assert.equal(mode, 0o600, `credentials must be 0600, saw 0${mode.toString(8)}`);
});

await step("the instance can reach a model", async () => {
  const session = await client.createSession(process.cwd());
  // CI and developer machines often retain a stale default provider alongside
  // another valid login. Let the validation choose a known-good route so this
  // tests inherited credentials rather than unrelated historical selection.
  if (process.env.JCODE_SDK_TEST_MODEL) {
    await client.setModel(session.session_id, process.env.JCODE_SDK_TEST_MODEL);
  }
  const turn = await client.run(session.session_id, "Reply with exactly: ISOLATED", {
    autoApprove: true,
  });
  assert.match(turn.text, /ISOLATED/, `unexpected reply: ${JSON.stringify(turn.text)}`);
});

await step("its sessions stay inside the instance", async () => {
  const sessions = await client.listSessions();
  assert.equal(sessions.length, 1, "the instance should see only its own session");
});

await step("close() stops the instance and removes its home", async () => {
  await client.close();
  assert.ok(!fs.existsSync(home), "an ephemeral instance home must be cleaned up");

  // Gone once is not gone. Background work started before shutdown (the
  // session-search indexer writes a multi-megabyte file) can finish seconds
  // later and recreate the directory behind a delete that already succeeded.
  // That is exactly how this leaked in the first place, so wait and re-check
  // rather than trusting the instant after close().
  await new Promise((resolve) => setTimeout(resolve, 5000));
  assert.ok(
    !fs.existsSync(home),
    `the instance home came back after cleanup: ${
      fs.existsSync(home) ? fs.readdirSync(home).join(", ") : ""
    }`,
  );
});

// A launched instance must never be visible to the user's own jcode. Check
// from the other side: connect to the shared socket, if one exists, and
// confirm the instance's session is not there.
await step("the user's jcode never saw the instance", async () => {
  let shared;
  try {
    shared = await JcodeClient.connect({ clientName: "launch-isolation-check/1.0" });
  } catch {
    console.log("     (no user jcode running; skipping)");
    return;
  }
  const sessions = await shared.listSessions();
  await shared.close();
  assert.ok(
    !sessions.some((session) => session.working_dir === home),
    "a launched instance's sessions leaked into the user's jcode",
  );
});

// A consumer whose process dies without calling close() must not leave the
// instance daemon running. The daemon calls setsid(), so it survives anything
// aimed at the process that started it: a server embedding jcode would
// otherwise accumulate one daemon per restart, each holding a temp directory
// and a model connection open indefinitely.
await step("an exit without close() does not leak the daemon", async () => {
  const { execFileSync, execSync } = await import("node:child_process");
  const countDaemons = () =>
    Number(execSync("pgrep -cf 'jcode --provider auto serve' || true", { encoding: "utf8" }).trim());

  const before = countDaemons();
  const script = `
    import { JcodeClient } from ${JSON.stringify(new URL("../dist/index.js", import.meta.url).href)};
    const client = await JcodeClient.launch({
      binary: ${JSON.stringify(binary)},
      workingDir: process.cwd(),
      startupTimeoutMs: 60000,
    });
    console.log(client.instanceHome);
    process.exit(0);  // no close(), like a crash
  `;
  const abandonedHome = execFileSync(
    process.execPath,
    ["--input-type=module", "-e", script],
    { encoding: "utf8", timeout: 120_000 },
  ).trim();

  await new Promise((resolve) => setTimeout(resolve, 7000));
  const after = countDaemons();
  assert.equal(after, before, `a crashed consumer leaked ${after - before} daemon(s)`);

  // Reaping the daemon is only half of it: a temp directory per crash is the
  // same unbounded growth in a different resource.
  assert.ok(
    !fs.existsSync(abandonedHome),
    `a crashed consumer leaked its instance home: ${abandonedHome}`,
  );
});

console.log(failures.length ? `\nFAILURES:\n${failures.join("\n")}` : "\nlaunch ok");
process.exit(failures.length ? 1 : 0);
